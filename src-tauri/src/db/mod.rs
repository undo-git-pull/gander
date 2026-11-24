//! DuckDB access: opening sources, executing queries, catalog, value conversion.

pub mod wrap;

use crate::config::{Config, ConnectionConfig};
use crate::paths;
use duckdb::types::{TimeUnit, Value};
use duckdb::{AccessMode, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;
use wrap::SortSpec;

/// What the user picked in the source dropdown.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SourceSpec {
    /// Files-only mode: in-memory DuckDB.
    Memory,
    /// A `[[connections]]` entry from config.
    Configured { name: String },
    /// A `.duckdb` file opened ad hoc.
    File { path: String },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SourceInfo {
    #[serde(flatten)]
    pub spec: SourceSpec,
    pub label: String,
    /// Resolved database file, if any.
    pub path: Option<String>,
    pub read_only: bool,
    /// True when there is a catalog to browse (a database file is open).
    pub has_catalog: bool,
    /// Non-fatal problems while opening (extension failures etc.).
    pub warnings: Vec<String>,
}

pub struct OpenedSource {
    pub info: SourceInfo,
    pub conn: Connection,
}

fn open_path(path: &Path, read_only: bool) -> duckdb::Result<Connection> {
    let mut cfg = duckdb::Config::default();
    if read_only {
        cfg = cfg.access_mode(AccessMode::ReadOnly)?;
    }
    Connection::open_with_flags(path, cfg)
}

pub fn open_source(
    spec: &SourceSpec,
    config: &Config,
    workspace: Option<&Path>,
) -> anyhow::Result<OpenedSource> {
    match spec {
        SourceSpec::Memory => Ok(OpenedSource {
            info: SourceInfo {
                spec: spec.clone(),
                label: "Files only".into(),
                path: None,
                read_only: false,
                has_catalog: false,
                warnings: vec![],
            },
            conn: Connection::open_in_memory()?,
        }),
        SourceSpec::File { path } => {
            let p = paths::expand(path, workspace);
            if !p.exists() {
                anyhow::bail!("file not found: {}", p.display());
            }
            // Prefer read-write; fall back to read-only if another process holds the lock.
            let (conn, ro) = match open_path(&p, false) {
                Ok(c) => (c, false),
                Err(_) => (open_path(&p, true)?, true),
            };
            Ok(OpenedSource {
                info: SourceInfo {
                    spec: SourceSpec::File {
                        path: p.display().to_string(),
                    },
                    label: p
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    path: Some(p.display().to_string()),
                    read_only: ro,
                    has_catalog: true,
                    warnings: vec![],
                },
                conn,
            })
        }
        SourceSpec::Configured { name } => {
            let cc = config
                .connections
                .iter()
                .find(|c| &c.name == name)
                .ok_or_else(|| anyhow::anyhow!("no connection named \"{name}\" in config"))?;
            open_configured(cc, workspace)
        }
    }
}

fn open_configured(
    cc: &ConnectionConfig,
    workspace: Option<&Path>,
) -> anyhow::Result<OpenedSource> {
    let memory = cc.path == ":memory:" || cc.path.is_empty();
    let resolved: Option<PathBuf> = if memory {
        None
    } else {
        Some(paths::expand(&cc.path, workspace))
    };
    let conn = match &resolved {
        None => Connection::open_in_memory()?,
        Some(p) => open_path(p, cc.read_only)
            .map_err(|e| anyhow::anyhow!("opening {}: {e}", p.display()))?,
    };
    let mut warnings = Vec::new();
    for ext in &cc.extensions {
        let q = ext.replace('\'', "''");
        if let Err(e) = conn.execute_batch(&format!("INSTALL '{q}'; LOAD '{q}';")) {
            warnings.push(format!("extension {ext}: {e}"));
        }
    }
    for a in &cc.attach {
        let p = paths::expand(&a.path, workspace)
            .display()
            .to_string()
            .replace('\'', "''");
        let ro = if a.read_only { " (READ_ONLY)" } else { "" };
        if let Err(e) = conn.execute_batch(&format!(
            "ATTACH '{p}' AS {}{ro};",
            wrap::quote_ident(&a.alias)
        )) {
            warnings.push(format!("attach {}: {e}", a.alias));
        }
    }
    for s in &cc.init_sql {
        if let Err(e) = conn.execute_batch(s) {
            warnings.push(format!("init_sql \"{s}\": {e}"));
        }
    }
    Ok(OpenedSource {
        info: SourceInfo {
            spec: SourceSpec::Configured {
                name: cc.name.clone(),
            },
            label: cc.name.clone(),
            path: resolved.as_ref().map(|p| p.display().to_string()),
            read_only: cc.read_only,
            has_catalog: !memory || !cc.attach.is_empty(),
            warnings,
        },
        conn,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<serde_json::Value>>,
    /// More rows existed beyond the limit.
    pub truncated: bool,
    /// SQL of the final statement as it actually ran.
    pub executed_sql: String,
    /// The final statement before wrapping (used for sort, count, stats).
    pub statement: String,
    /// The final statement was a query and was wrapped.
    pub wrapped: bool,
    /// Sort that was applied (columns that no longer exist are dropped).
    pub sort: Vec<SortSpec>,
    pub limit: usize,
    pub elapsed_ms: f64,
}

#[derive(Debug)]
pub struct QueryError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<duckdb::Error> for QueryError {
    fn from(e: duckdb::Error) -> Self {
        let message = e.to_string();
        let code = if message.contains("INTERRUPT")
            || message.to_ascii_lowercase().contains("interrupted")
        {
            "interrupted"
        } else {
            "query_error"
        };
        QueryError { code, message }
    }
}

pub const UNLIMITED_CONFIRM_ROWS: u64 = 1_000_000;

/// Decide whether a statement is a query, using DuckDB's parser.
/// `json_serialize_sql` only accepts SELECT statements.
pub fn is_query(conn: &Connection, stmt: &str) -> bool {
    let r: duckdb::Result<String> = conn.query_row(
        "SELECT json_serialize_sql(?::VARCHAR)::VARCHAR",
        [stmt],
        |row| row.get(0),
    );
    match r {
        Ok(json) => match serde_json::from_str::<serde_json::Value>(&json) {
            Ok(v) => !v.get("error").and_then(|e| e.as_bool()).unwrap_or(false),
            Err(_) => wrap::looks_like_query(stmt),
        },
        Err(_) => wrap::looks_like_query(stmt),
    }
}

pub struct RunOptions<'a> {
    /// Full text: every statement runs; the last one is shown.
    pub sql: &'a str,
    /// Only run the final statement (used for re-sorting, re-limiting).
    pub last_only: bool,
    pub sort: &'a [SortSpec],
    /// 0 means no limit.
    pub limit: usize,
    /// Skip the large-result check for limit 0.
    pub force: bool,
}

pub fn run(conn: &Connection, opts: RunOptions) -> Result<QueryResult, QueryError> {
    let started = Instant::now();
    let stmts = wrap::split(opts.sql);
    let Some(last) = stmts.last().cloned() else {
        return Err(QueryError {
            code: "empty",
            message: "Nothing to run".into(),
        });
    };
    if !opts.last_only {
        for s in &stmts[..stmts.len() - 1] {
            conn.execute_batch(s)?;
        }
    }
    if !is_query(conn, &last) {
        let (columns, rows) = fetch_generic(conn, &last)?;
        return Ok(QueryResult {
            columns,
            rows,
            truncated: false,
            executed_sql: last.clone(),
            statement: last,
            wrapped: false,
            sort: vec![],
            limit: opts.limit,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        });
    }
    let described = describe(conn, &last)?;
    let names: Vec<&str> = described.iter().map(|(n, _)| n.as_str()).collect();
    let sort: Vec<SortSpec> = opts
        .sort
        .iter()
        .filter(|s| names.contains(&s.column.as_str()))
        .cloned()
        .collect();
    if opts.limit == 0 && !opts.force {
        let n: i64 = conn.query_row(&wrap::count_sql(&last), [], |r| r.get(0))?;
        if n as u64 > UNLIMITED_CONFIRM_ROWS {
            return Err(QueryError {
                code: "too_many_rows",
                message: format!(
                    "Query returns {n} rows with no row limit. Run anyway to load them all."
                ),
            });
        }
    }
    let fetch = if opts.limit == 0 {
        None
    } else {
        Some(opts.limit + 1)
    };
    let unique = {
        let mut seen = std::collections::HashSet::new();
        names.iter().all(|n| seen.insert(*n))
    };
    let executed_sql = wrap::wrap(&last, &sort, fetch);
    let (columns, mut rows) = if unique {
        let proj = wrap::display_projection(&described);
        let sql = wrap::wrap_with_projection(&last, &proj, &sort, fetch);
        let rows = fetch_rows(conn, &sql, &described)?;
        let cols = described
            .iter()
            .map(|(n, t)| Column {
                name: n.clone(),
                ty: t.clone(),
            })
            .collect();
        (cols, rows)
    } else {
        // Duplicate column names cannot be referenced individually; fetch as-is.
        let (_, rows) = fetch_generic(conn, &executed_sql)?;
        let cols = described
            .iter()
            .map(|(n, t)| Column {
                name: n.clone(),
                ty: t.clone(),
            })
            .collect();
        (cols, rows)
    };
    let truncated = opts.limit > 0 && rows.len() > opts.limit;
    if truncated {
        rows.truncate(opts.limit);
    }
    Ok(QueryResult {
        columns,
        rows,
        truncated,
        executed_sql,
        statement: last,
        wrapped: true,
        sort,
        limit: opts.limit,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

pub fn describe(conn: &Connection, query: &str) -> Result<Vec<(String, String)>, QueryError> {
    let mut stmt = conn.prepare(&wrap::describe_sql(query))?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push((r.get::<_, String>(0)?, r.get::<_, String>(1)?));
    }
    Ok(out)
}

fn fetch_rows(
    conn: &Connection,
    sql: &str,
    cols: &[(String, String)],
) -> Result<Vec<Vec<serde_json::Value>>, QueryError> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut row = Vec::with_capacity(cols.len());
        for (i, (_, ty)) in cols.iter().enumerate() {
            let v: Value = r.get(i)?;
            let j = match v {
                Value::Text(s) if wrap::is_nested_type(ty) => {
                    serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s))
                }
                other => to_json(other),
            };
            row.push(j);
        }
        out.push(row);
    }
    Ok(out)
}

fn fetch_generic(
    conn: &Connection,
    sql: &str,
) -> Result<(Vec<Column>, Vec<Vec<serde_json::Value>>), QueryError> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    let mut columns: Option<Vec<Column>> = None;
    while let Some(r) = rows.next()? {
        let st = r.as_ref();
        let n = st.column_count();
        if columns.is_none() {
            columns = Some(column_meta(st));
        }
        let mut row = Vec::with_capacity(n);
        for i in 0..n {
            let v: Value = r.get(i)?;
            row.push(to_json(v));
        }
        out.push(row);
    }
    let columns = match columns {
        Some(c) => c,
        None => rows.as_ref().map(column_meta).unwrap_or_default(),
    };
    Ok((columns, out))
}

fn column_meta(st: &duckdb::Statement) -> Vec<Column> {
    let names = st.column_names();
    names
        .into_iter()
        .enumerate()
        .map(|(i, name)| Column {
            name,
            ty: arrow_type_name(&st.column_type(i)),
        })
        .collect()
}

fn arrow_type_name(t: &duckdb::arrow::datatypes::DataType) -> String {
    use duckdb::arrow::datatypes::DataType as D;
    match t {
        D::Boolean => "BOOLEAN",
        D::Int8 => "TINYINT",
        D::Int16 => "SMALLINT",
        D::Int32 => "INTEGER",
        D::Int64 => "BIGINT",
        D::UInt8 => "UTINYINT",
        D::UInt16 => "USMALLINT",
        D::UInt32 => "UINTEGER",
        D::UInt64 => "UBIGINT",
        D::Float32 => "FLOAT",
        D::Float64 => "DOUBLE",
        D::Utf8 | D::LargeUtf8 | D::Utf8View => "VARCHAR",
        D::Date32 | D::Date64 => "DATE",
        D::Timestamp(_, _) => "TIMESTAMP",
        D::Time32(_) | D::Time64(_) => "TIME",
        D::Decimal128(_, _) | D::Decimal256(_, _) => "DECIMAL",
        D::Binary | D::LargeBinary => "BLOB",
        D::List(_) | D::LargeList(_) | D::FixedSizeList(_, _) => "LIST",
        D::Struct(_) => "STRUCT",
        D::Map(_, _) => "MAP",
        D::Interval(_) => "INTERVAL",
        other => return format!("{other:?}").to_ascii_uppercase(),
    }
    .to_string()
}

/// Generic conversion used for unwrapped statements and native columns.
pub fn to_json(v: Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Null => J::Null,
        Value::Boolean(b) => J::Bool(b),
        Value::TinyInt(i) => J::from(i),
        Value::SmallInt(i) => J::from(i),
        Value::Int(i) => J::from(i),
        Value::UTinyInt(i) => J::from(i),
        Value::USmallInt(i) => J::from(i),
        Value::UInt(i) => J::from(i),
        // Beyond 2^53 JavaScript loses precision, so wide integers go as text.
        Value::BigInt(i) => J::String(i.to_string()),
        Value::UBigInt(i) => J::String(i.to_string()),
        Value::HugeInt(i) => J::String(i.to_string()),
        Value::UHugeInt(i) => J::String(i.to_string()),
        Value::Float(f) => serde_json::Number::from_f64(f as f64).map(J::Number).unwrap_or(J::String(f.to_string())),
        Value::Double(f) => serde_json::Number::from_f64(f).map(J::Number).unwrap_or(J::String(f.to_string())),
        Value::Decimal(d) => J::String(d.to_string()),
        Value::Text(s) | Value::Enum(s) => J::String(s),
        Value::Blob(b) | Value::Geometry(b) => J::String(blob_to_string(&b)),
        Value::Date32(d) => J::String(format_date(d as i64)),
        Value::Timestamp(unit, t) => J::String(format_timestamp(unit, t)),
        Value::Time64(unit, t) => J::String(format_time(to_micros(unit, t))),
        Value::Interval { months, days, nanos } => J::String(format!("{months} months {days} days {} us", nanos / 1000)),
        Value::List(items) | Value::Array(items) => J::Array(items.into_iter().map(to_json).collect()),
        Value::Struct(map) => {
            let mut o = serde_json::Map::new();
            for (k, v) in map.iter() {
                o.insert(k.clone(), to_json(v.clone()));
            }
            J::Object(o)
        }
        Value::Map(map) => J::Array(
            map.iter()
                .map(|(k, v)| serde_json::json!({ "key": to_json(k.clone()), "value": to_json(v.clone()) }))
                .collect(),
        ),
        Value::Union(inner) => to_json(*inner),
        other => J::String(format!("{other:?}")),
    }
}

fn blob_to_string(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len());
    for &c in b {
        if (0x20..0x7f).contains(&c) && c != b'\\' {
            s.push(c as char);
        } else {
            s.push_str(&format!("\\x{c:02X}"));
        }
    }
    s
}

fn to_micros(unit: TimeUnit, t: i64) -> i64 {
    match unit {
        TimeUnit::Second => t * 1_000_000,
        TimeUnit::Millisecond => t * 1000,
        TimeUnit::Microsecond => t,
        TimeUnit::Nanosecond => t / 1000,
    }
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn format_date(days: i64) -> String {
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn format_time(us: i64) -> String {
    let us = us.rem_euclid(86_400_000_000);
    let (h, rem) = (us / 3_600_000_000, us % 3_600_000_000);
    let (mi, rem) = (rem / 60_000_000, rem % 60_000_000);
    let (s, frac) = (rem / 1_000_000, rem % 1_000_000);
    if frac == 0 {
        format!("{h:02}:{mi:02}:{s:02}")
    } else {
        format!("{h:02}:{mi:02}:{s:02}.{frac:06}")
    }
}

fn format_timestamp(unit: TimeUnit, t: i64) -> String {
    let us = to_micros(unit, t);
    let days = us.div_euclid(86_400_000_000);
    format!(
        "{} {}",
        format_date(days),
        format_time(us.rem_euclid(86_400_000_000))
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogTable {
    pub database: String,
    pub schema: String,
    pub name: String,
    /// "table" or "view"
    pub kind: String,
    pub columns: Vec<CatalogColumn>,
}

pub fn catalog(conn: &Connection) -> Result<Vec<CatalogTable>, QueryError> {
    let mut tables: Vec<CatalogTable> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT database_name, schema_name, table_name, 'table' FROM duckdb_tables() WHERE NOT internal AND NOT temporary
             UNION ALL
             SELECT database_name, schema_name, view_name, 'view' FROM duckdb_views() WHERE NOT internal AND NOT temporary
             ORDER BY 1, 2, 3",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            tables.push(CatalogTable {
                database: r.get(0)?,
                schema: r.get(1)?,
                name: r.get(2)?,
                kind: r.get(3)?,
                columns: vec![],
            });
        }
    }
    let mut index = std::collections::HashMap::new();
    for (i, t) in tables.iter().enumerate() {
        index.insert((t.database.clone(), t.schema.clone(), t.name.clone()), i);
    }
    let mut stmt = conn.prepare(
        "SELECT database_name, schema_name, table_name, column_name, data_type FROM duckdb_columns()
         WHERE NOT internal ORDER BY database_name, schema_name, table_name, column_index",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let key = (
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        );
        if let Some(&i) = index.get(&key) {
            tables[i].columns.push(CatalogColumn {
                name: r.get(3)?,
                ty: r.get(4)?,
            });
        }
    }
    Ok(tables)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wrap::SortDir;

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE t AS SELECT range AS id, range * 1.5 AS amount, {'a': range, 'b': [1,2]} AS meta,
             '{\"x\": 1}' AS js, 9007199254740993::BIGINT AS big, DATE '2026-11-02' AS d FROM range(25)",
        )
        .unwrap();
        c
    }

    fn opts<'a>(sql: &'a str, sort: &'a [SortSpec], limit: usize) -> RunOptions<'a> {
        RunOptions {
            sql,
            last_only: false,
            sort,
            limit,
            force: false,
        }
    }

    #[test]
    fn limit_and_truncation() {
        let c = mem();
        let r = run(&c, opts("from t", &[], 10)).unwrap();
        assert!(r.wrapped);
        assert!(r.truncated);
        assert_eq!(r.rows.len(), 10);
        assert_eq!(r.executed_sql, "SELECT * FROM (\nfrom t\n) AS a LIMIT 11");
        let r = run(&c, opts("from t", &[], 25)).unwrap();
        assert!(!r.truncated);
        assert_eq!(r.rows.len(), 25);
    }

    #[test]
    fn sort_desc_and_unknown_column_dropped() {
        let c = mem();
        let sort = vec![
            SortSpec {
                column: "id".into(),
                dir: SortDir::Desc,
            },
            SortSpec {
                column: "missing".into(),
                dir: SortDir::Asc,
            },
        ];
        let r = run(&c, opts("from t", &sort, 3)).unwrap();
        assert_eq!(r.sort.len(), 1);
        assert_eq!(r.rows[0][0], serde_json::json!("24"));
    }

    #[test]
    fn value_rendering() {
        let c = mem();
        let r = run(
            &c,
            opts(
                "select id::INTEGER as i, amount, meta, js, big, d from t order by id limit 2",
                &[],
                100,
            ),
        )
        .unwrap();
        let row = &r.rows[1];
        assert_eq!(row[0], serde_json::json!(1));
        assert_eq!(row[1], serde_json::json!("1.5"));
        assert_eq!(row[2], serde_json::json!({"a": 1, "b": [1, 2]}));
        assert_eq!(row[3], serde_json::json!("{\"x\": 1}"));
        assert_eq!(row[4], serde_json::json!("9007199254740993"));
        assert_eq!(row[5], serde_json::json!("2026-11-02"));
        assert!(r.columns[2].ty.starts_with("STRUCT"));
    }

    #[test]
    fn multi_statement_and_ddl() {
        let c = Connection::open_in_memory().unwrap();
        let r = run(
            &c,
            opts(
                "create table x(a int); insert into x values (1),(2); select * from x;",
                &[],
                10,
            ),
        )
        .unwrap();
        assert_eq!(r.rows.len(), 2);
        let r = run(&c, opts("create table y(a int)", &[], 10)).unwrap();
        assert!(!r.wrapped);
    }

    #[test]
    fn unwrapped_statement_returns_rows() {
        let c = Connection::open_in_memory().unwrap();
        let r = run(&c, opts("PRAGMA version", &[], 10)).unwrap();
        assert!(!r.wrapped);
        assert!(!r.rows.is_empty());
    }

    #[test]
    fn query_error_message() {
        let c = Connection::open_in_memory().unwrap();
        let e = run(&c, opts("select * from nope", &[], 10)).unwrap_err();
        assert_eq!(e.code, "query_error");
        assert!(e.message.contains("nope"));
    }

    #[test]
    fn unlimited_guard() {
        let c = Connection::open_in_memory().unwrap();
        let e = run(&c, opts("from range(2000000)", &[], 0)).unwrap_err();
        assert_eq!(e.code, "too_many_rows");
        let ok = run(&c, opts("from range(10)", &[], 0)).unwrap();
        assert_eq!(ok.rows.len(), 10);
    }

    #[test]
    fn summarize_and_count() {
        let c = mem();
        let r = run(&c, opts(&wrap::summarize_sql("from t"), &[], 0)).unwrap();
        assert_eq!(r.columns[0].name, "column_name");
        assert_eq!(r.rows.len(), 6);
        let n: i64 = c
            .query_row(&wrap::count_sql("from t"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 25);
    }

    #[test]
    fn catalog_lists_tables_and_columns() {
        let c = mem();
        c.execute_batch("create view v as select id from t")
            .unwrap();
        let cat = catalog(&c).unwrap();
        let t = cat.iter().find(|t| t.name == "t").unwrap();
        assert_eq!(t.kind, "table");
        assert_eq!(t.columns[0].name, "id");
        assert!(cat.iter().any(|t| t.name == "v" && t.kind == "view"));
    }

    #[test]
    fn dates() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(20_394), "2025-11-02");
        assert_eq!(
            format_timestamp(TimeUnit::Microsecond, 1_500_000),
            "1970-01-01 00:00:01.500000"
        );
    }

    #[test]
    fn histograms() {
        use wrap::{HistogramKind, histogram_kind, histogram_sql};
        let c = mem();
        let h = |q: &str, col: &str, ty: &str| {
            let sql = histogram_sql(q, col, histogram_kind(ty));
            run(
                &c,
                RunOptions {
                    sql: &sql,
                    last_only: false,
                    sort: &[],
                    limit: 0,
                    force: true,
                },
            )
            .unwrap()
        };
        assert_eq!(histogram_kind("DECIMAL(18,3)"), HistogramKind::Numeric);
        assert_eq!(
            histogram_kind("TIMESTAMP WITH TIME ZONE"),
            HistogramKind::Timestamp
        );
        assert_eq!(histogram_kind("VARCHAR"), HistogramKind::Values);

        let r = h("from t;", "amount", "DECIMAL(18,1)");
        assert_eq!(r.rows.len(), wrap::HISTOGRAM_BINS);
        let total: f64 = r.rows.iter().map(|row| row[3].as_f64().unwrap()).sum();
        assert_eq!(total, 25.0);

        // Constant column: one bin. Empty input: no bins.
        assert_eq!(h("from t", "d", "DATE").rows.len(), 1);
        assert_eq!(h("from t where id < 0", "id", "BIGINT").rows.len(), 0);

        let r = h(
            "select (id % 30)::VARCHAR as s from t union all select null",
            "s",
            "VARCHAR",
        );
        assert_eq!(r.rows.len(), wrap::HISTOGRAM_BINS + 1);
        let last = r.rows.last().unwrap();
        assert_eq!(last[3], serde_json::Value::Bool(true));
        assert_eq!(last[2].as_f64().unwrap(), 6.0);
    }
}
