//! Statement splitting and the limit/sort wrapper shared by the UI and the API.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SortSpec {
    pub column: String,
    pub dir: SortDir,
}

/// A statement's position in the source text, in UTF-16 code units so the
/// frontend can use the offsets directly.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StatementRange {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// Split SQL into statements on top-level semicolons, respecting quotes,
/// comments and dollar-quoted strings. Empty statements are dropped.
pub fn split_with_ranges(sql: &str) -> Vec<StatementRange> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let n = bytes.len();
    let push = |out: &mut Vec<StatementRange>, s: usize, e: usize| {
        let raw = &sql[s..e];
        let trimmed_start = s + (raw.len() - raw.trim_start().len());
        let trimmed_end = e - (raw.len() - raw.trim_end().len());
        if trimmed_end > trimmed_start {
            let text = sql[trimmed_start..trimmed_end].to_string();
            if !only_comments(&text) {
                out.push(StatementRange {
                    start: utf16_len(&sql[..trimmed_start]),
                    end: utf16_len(&sql[..trimmed_end]),
                    text,
                });
            }
        }
    };
    while i < n {
        let c = bytes[i];
        match c {
            b'\'' | b'"' => {
                let q = c;
                i += 1;
                while i < n {
                    if bytes[i] == q {
                        if i + 1 < n && bytes[i + 1] == q {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'-' if i + 1 < n && bytes[i + 1] == b'-' => {
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b'$' => {
                // $tag$ ... $tag$
                let mut j = i + 1;
                while j < n && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                if j < n && bytes[j] == b'$' {
                    let tag = &sql[i..=j];
                    if let Some(pos) = sql[j + 1..].find(tag) {
                        i = j + 1 + pos + tag.len();
                    } else {
                        i = n;
                    }
                } else {
                    i += 1;
                }
            }
            b';' => {
                push(&mut out, start, i);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < n {
        push(&mut out, start, n);
    }
    out
}

pub fn split(sql: &str) -> Vec<String> {
    split_with_ranges(sql).into_iter().map(|r| r.text).collect()
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// True when the text has nothing but whitespace and comments.
fn only_comments(s: &str) -> bool {
    let mut rest = s.trim_start();
    loop {
        if rest.is_empty() {
            return true;
        }
        if let Some(r) = rest.strip_prefix("--") {
            rest = match r.find('\n') {
                Some(p) => r[p + 1..].trim_start(),
                None => "",
            };
        } else if let Some(r) = rest.strip_prefix("/*") {
            rest = match r.find("*/") {
                Some(p) => r[p + 2..].trim_start(),
                None => "",
            };
        } else {
            return false;
        }
    }
}

pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Keyword fallback used when DuckDB's parser cannot answer.
pub fn looks_like_query(stmt: &str) -> bool {
    let first = first_keyword(stmt);
    matches!(
        first.as_str(),
        "SELECT" | "WITH" | "FROM" | "VALUES" | "TABLE" | "SUMMARIZE" | "DESCRIBE" | "PIVOT" | "UNPIVOT" | "SHOW" | "("
    )
}

fn first_keyword(stmt: &str) -> String {
    let mut s = stmt.trim_start();
    loop {
        if let Some(r) = s.strip_prefix("--") {
            s = r.find('\n').map(|p| r[p + 1..].trim_start()).unwrap_or("");
        } else if let Some(r) = s.strip_prefix("/*") {
            s = r.find("*/").map(|p| r[p + 2..].trim_start()).unwrap_or("");
        } else {
            break;
        }
    }
    if s.starts_with('(') {
        return "(".into();
    }
    s.split(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or("")
        .to_ascii_uppercase()
}

/// The user-visible wrapper: `SELECT * FROM (<query>) AS a ORDER BY ... LIMIT n`.
/// `fetch` is the number of rows to fetch (limit + 1), or None for no limit.
/// Newlines around the query keep a trailing `--` comment from swallowing the
/// closing parenthesis.
pub fn wrap(query: &str, sort: &[SortSpec], fetch: Option<usize>) -> String {
    wrap_with_projection(query, "*", sort, fetch)
}

pub fn wrap_with_projection(query: &str, projection: &str, sort: &[SortSpec], fetch: Option<usize>) -> String {
    let mut sql = format!("SELECT {projection} FROM (\n{}\n) AS a", strip_query(query));
    if !sort.is_empty() {
        let parts: Vec<String> = sort
            .iter()
            .map(|s| {
                format!(
                    "a.{} {}",
                    quote_ident(&s.column),
                    match s.dir {
                        SortDir::Asc => "ASC",
                        SortDir::Desc => "DESC",
                    }
                )
            })
            .collect();
        sql.push_str(" ORDER BY ");
        sql.push_str(&parts.join(", "));
    }
    if let Some(n) = fetch {
        sql.push_str(&format!(" LIMIT {n}"));
    }
    sql
}

/// Remove trailing semicolons and whitespace.
pub fn strip_query(q: &str) -> &str {
    let mut s = q.trim();
    while let Some(r) = s.strip_suffix(';') {
        s = r.trim_end();
    }
    s
}

pub fn count_sql(query: &str) -> String {
    format!("SELECT count(*) FROM (\n{}\n) AS a", strip_query(query))
}

pub fn summarize_sql(query: &str) -> String {
    format!("SUMMARIZE SELECT * FROM (\n{}\n) AS a", strip_query(query))
}

pub fn describe_sql(query: &str) -> String {
    format!("DESCRIBE SELECT * FROM (\n{}\n) AS a", strip_query(query))
}

/// DuckDB types sent to the client as native JSON numbers or booleans. All
/// other types are rendered by DuckDB as text, or as JSON for nested types, so
/// 64-bit integers and decimals keep full precision in JavaScript.
pub fn is_native_type(t: &str) -> bool {
    matches!(
        t,
        "BOOLEAN" | "TINYINT" | "SMALLINT" | "INTEGER" | "UTINYINT" | "USMALLINT" | "UINTEGER" | "FLOAT" | "DOUBLE"
    )
}

pub fn is_nested_type(t: &str) -> bool {
    t == "JSON"
        || t.starts_with("STRUCT")
        || t.starts_with("MAP")
        || t.starts_with("UNION")
        || t.ends_with(']')
}

/// Projection that renders each column for display while keeping the source
/// column available to ORDER BY through the `a.` qualifier.
pub fn display_projection(columns: &[(String, String)]) -> String {
    columns
        .iter()
        .map(|(name, ty)| {
            let q = quote_ident(name);
            if is_native_type(ty) {
                format!("a.{q} AS {q}")
            } else if is_nested_type(ty) {
                format!("CAST(to_json(a.{q}) AS VARCHAR) AS {q}")
            } else {
                format!("CAST(a.{q} AS VARCHAR) AS {q}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_top_level_semicolons() {
        let s = split("select 1; select ';' as x; -- c;\nselect \"a;b\" from t;");
        assert_eq!(s, vec!["select 1", "select ';' as x", "-- c;\nselect \"a;b\" from t"]);
    }

    #[test]
    fn split_handles_block_comments_and_dollar_quotes() {
        let s = split("select /* ; */ 1; select $$a;b$$; select $t$x;$t$");
        assert_eq!(s, vec!["select /* ; */ 1", "select $$a;b$$", "select $t$x;$t$"]);
    }

    #[test]
    fn split_drops_empty_and_comment_only() {
        assert_eq!(split(" ; ;\n-- only\n"), Vec::<String>::new());
        assert_eq!(split("select 1;;"), vec!["select 1"]);
    }

    #[test]
    fn split_escaped_quotes() {
        assert_eq!(split("select 'it''s;'; select 2"), vec!["select 'it''s;'", "select 2"]);
    }

    #[test]
    fn ranges_are_utf16() {
        let r = split_with_ranges("select 'é😀'; select 2");
        assert_eq!(r[0].start, 0);
        assert_eq!(r[0].end, "select 'é😀'".encode_utf16().count());
        assert_eq!(r[1].text, "select 2");
        assert_eq!(r[1].start, "select 'é😀'; ".encode_utf16().count());
    }

    #[test]
    fn wrap_plain_and_sorted() {
        assert_eq!(wrap("select 1;", &[], Some(1001)), "SELECT * FROM (\nselect 1\n) AS a LIMIT 1001");
        let sort = vec![
            SortSpec { column: "amount".into(), dir: SortDir::Desc },
            SortSpec { column: "we\"ird name".into(), dir: SortDir::Asc },
        ];
        assert_eq!(
            wrap("from t ;; ", &sort, Some(11)),
            "SELECT * FROM (\nfrom t\n) AS a ORDER BY a.\"amount\" DESC, a.\"we\"\"ird name\" ASC LIMIT 11"
        );
        assert_eq!(wrap("from t", &[], None), "SELECT * FROM (\nfrom t\n) AS a");
    }

    #[test]
    fn wrap_survives_trailing_line_comment() {
        let w = wrap("select 1 -- note", &[], Some(2));
        assert!(w.contains("-- note\n) AS a"));
    }

    #[test]
    fn keyword_detection() {
        assert!(looks_like_query("  -- x\n select 1"));
        assert!(looks_like_query("FROM t"));
        assert!(looks_like_query("with a as (select 1) select * from a"));
        assert!(looks_like_query("(select 1)"));
        assert!(!looks_like_query("create table t(a int)"));
        assert!(!looks_like_query("/* c */ INSERT INTO t VALUES (1)"));
        assert!(!looks_like_query("set threads = 4"));
    }

    #[test]
    fn projection() {
        let p = display_projection(&[
            ("a".into(), "INTEGER".into()),
            ("b".into(), "STRUCT(x INTEGER)".into()),
            ("c".into(), "BIGINT".into()),
            ("d".into(), "INTEGER[]".into()),
        ]);
        assert_eq!(
            p,
            "a.\"a\" AS \"a\", CAST(to_json(a.\"b\") AS VARCHAR) AS \"b\", CAST(a.\"c\" AS VARCHAR) AS \"c\", CAST(to_json(a.\"d\") AS VARCHAR) AS \"d\""
        );
    }
}

/// Number of bins for numeric and temporal histograms, and of distinct
/// values listed before the rest are grouped.
pub const HISTOGRAM_BINS: usize = 20;

/// How a column's histogram is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HistogramKind {
    /// Equal-width bins over the value range. Rows: bin, lo, hi, n.
    Numeric,
    /// Equal-width bins over epoch seconds. Rows: bin, lo, hi, n.
    Date,
    Timestamp,
    /// Most frequent values, the rest grouped in one row. Rows: rk, label, n, other.
    Values,
}

pub fn histogram_kind(ty: &str) -> HistogramKind {
    let t = ty.trim().to_ascii_uppercase();
    let base = t.split('(').next().unwrap_or("").trim();
    match base {
        "TINYINT" | "SMALLINT" | "INTEGER" | "BIGINT" | "HUGEINT" | "UTINYINT" | "USMALLINT" | "UINTEGER" | "UBIGINT"
        | "UHUGEINT" | "FLOAT" | "DOUBLE" | "DECIMAL" => HistogramKind::Numeric,
        "DATE" => HistogramKind::Date,
        _ if base.starts_with("TIMESTAMP") => HistogramKind::Timestamp,
        _ => HistogramKind::Values,
    }
}

/// SQL computing a histogram of `column` over the result of `query`.
pub fn histogram_sql(query: &str, column: &str, kind: HistogramKind) -> String {
    let q = strip_query(query);
    let col = quote_ident(column);
    let n = HISTOGRAM_BINS;
    match kind {
        HistogramKind::Values => format!(
            "WITH c AS (SELECT CAST({col} AS VARCHAR) AS v, count(*) AS n FROM (\n{q}\n) AS a GROUP BY 1),\n\
             t AS (SELECT v, n, row_number() OVER (ORDER BY n DESC, v NULLS LAST) AS rk FROM c)\n\
             SELECT CAST(rk AS INTEGER) AS rk, v AS label, CAST(n AS DOUBLE) AS n, false AS other FROM t WHERE rk <= {n}\n\
             UNION ALL SELECT {k}, CAST(count(*) AS VARCHAR), CAST(sum(n) AS DOUBLE), true FROM t WHERE rk > {n} HAVING count(*) > 0\n\
             ORDER BY rk",
            k = n + 1
        ),
        _ => {
            let v = if kind == HistogramKind::Numeric {
                format!("CAST({col} AS DOUBLE)")
            } else {
                format!("CAST(epoch({col}) AS DOUBLE)")
            };
            format!(
                "WITH src AS (SELECT {v} AS v FROM (\n{q}\n) AS a WHERE {col} IS NOT NULL),\n\
                 r AS (SELECT min(v) AS lo, max(v) AS hi FROM src WHERE isfinite(v)),\n\
                 b AS (SELECT CASE WHEN r.hi = r.lo THEN 0 ELSE least(CAST(floor((v - r.lo) / (r.hi - r.lo) * {n}) AS INTEGER), {last}) END AS bin\n\
                 FROM src, r WHERE isfinite(v)),\n\
                 c AS (SELECT bin, count(*) AS n FROM b GROUP BY bin)\n\
                 SELECT CAST(g.bin AS INTEGER) AS bin,\n\
                 CASE WHEN r.hi = r.lo THEN r.lo ELSE r.lo + (r.hi - r.lo) * g.bin / {n} END AS lo,\n\
                 CASE WHEN r.hi = r.lo THEN r.hi ELSE r.lo + (r.hi - r.lo) * (g.bin + 1) / {n} END AS hi,\n\
                 CAST(coalesce(c.n, 0) AS DOUBLE) AS n\n\
                 FROM r, range(CASE WHEN r.lo IS NULL THEN 0 WHEN r.hi = r.lo THEN 1 ELSE {n} END) AS g(bin)\n\
                 LEFT JOIN c ON c.bin = g.bin ORDER BY g.bin",
                last = n - 1
            )
        }
    }
}
