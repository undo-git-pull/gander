//! Localhost HTTP API. Every endpoint acts on the active tab only.

use crate::core::{Core, CoreError, Event, Origin, RunRequest, RunStatus, TabRef};
use crate::db::{QueryResult, wrap::SortSpec};
use crate::paths;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::sync::watch;

pub struct ServerHandle {
    shutdown: watch::Sender<bool>,
    pub port: u16,
    configured_port: u16,
}

#[derive(Clone)]
struct AppCtx {
    core: Arc<Core>,
    port: u16,
}

pub struct ApiError(StatusCode, String, String);

impl ApiError {
    fn new(status: StatusCode, code: &str, msg: impl Into<String>) -> Self {
        Self(status, code.to_string(), msg.into())
    }
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        let status = match e.code {
            "not_found" => StatusCode::NOT_FOUND,
            "empty" | "no_result" | "bad_request" => StatusCode::BAD_REQUEST,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        ApiError(status, e.code.to_string(), e.message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(json!({ "error": { "code": self.1, "message": self.2 } })),
        )
            .into_response()
    }
}

type ApiResult = Result<Response, ApiError>;

/// Start, stop or restart the server so it matches the current config.
pub fn apply_config(core: &Arc<Core>) {
    let cfg = core.config().server;
    let mut slot = core.server.lock();
    if let Some(h) = slot.as_ref() {
        if cfg.enabled && h.configured_port == cfg.port {
            // The key may have changed; keep server.json in step.
            write_server_file(h.port, core);
            return;
        }
    }
    if let Some(h) = slot.take() {
        let _ = h.shutdown.send(true);
        let _ = std::fs::remove_file(paths::server_file());
    }
    let mut status = core.api_status();
    status.enabled = cfg.enabled;
    status.running = false;
    status.port = None;
    status.error = None;
    status.auth_required = true;
    if !cfg.enabled {
        drop(slot);
        core.set_api_status(status);
        return;
    }
    match bind(cfg.port) {
        Ok((v4, v6)) => {
            let port = v4.local_addr().map(|a| a.port()).unwrap_or(cfg.port);
            let (tx, rx) = watch::channel(false);
            *slot = Some(ServerHandle {
                shutdown: tx,
                port,
                configured_port: cfg.port,
            });
            drop(slot);
            serve(core.clone(), port, v4, v6, rx);
            status.running = true;
            status.port = Some(port);
            write_server_file(port, core);
            core.set_api_status(status);
        }
        Err(e) => {
            drop(slot);
            status.error = Some(format!("cannot bind 127.0.0.1:{}: {e}", cfg.port));
            core.set_api_status(status);
        }
    }
}

pub fn shutdown(core: &Arc<Core>) {
    if let Some(h) = core.server.lock().take() {
        let _ = h.shutdown.send(true);
    }
    let _ = std::fs::remove_file(paths::server_file());
}

/// Written with mode 0600. Holds the API key only when it is the random
/// per-launch one; a key from the config stays out of the file.
fn write_server_file(port: u16, core: &Core) {
    let mut body = json!({
        "port": port,
        "url": format!("http://127.0.0.1:{port}"),
        "pid": std::process::id(),
        "auth_required": true,
        "version": env!("CARGO_PKG_VERSION"),
    });
    if core.api_key_is_generated() {
        body["api_key"] = json!(core.api_key());
    }
    let _ = paths::write_private(
        &paths::server_file(),
        serde_json::to_string_pretty(&body)
            .unwrap_or_default()
            .as_bytes(),
    );
}

type Listeners = (std::net::TcpListener, Option<std::net::TcpListener>);

fn bind(port: u16) -> std::io::Result<Listeners> {
    let v4 = std::net::TcpListener::bind(("127.0.0.1", port))?;
    let actual = v4.local_addr()?.port();
    // IPv6 loopback is optional; some systems have it disabled.
    let v6 = std::net::TcpListener::bind(("::1", actual)).ok();
    v4.set_nonblocking(true)?;
    if let Some(l) = &v6 {
        l.set_nonblocking(true)?;
    }
    Ok((v4, v6))
}

fn serve(
    core: Arc<Core>,
    port: u16,
    v4: std::net::TcpListener,
    v6: Option<std::net::TcpListener>,
    rx: watch::Receiver<bool>,
) {
    let router = router(core.clone(), port);
    let rt = core.rt_handle();
    for l in std::iter::once(v4).chain(v6) {
        let router = router.clone();
        let mut rx = rx.clone();
        rt.spawn(async move {
            let Ok(listener) = tokio::net::TcpListener::from_std(l) else {
                return;
            };
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = rx.wait_for(|v| *v).await;
                })
                .await;
        });
    }
}

pub fn router(core: Arc<Core>, port: u16) -> Router {
    let ctx = AppCtx { core, port };
    Router::new()
        .route("/health", get(health))
        .route("/state", get(state))
        .route("/query", get(get_query).put(put_query))
        .route("/run", post(run))
        .route("/sort", put(put_sort))
        .route("/limit", put(put_limit))
        .route("/stop", post(stop))
        .route("/result", get(result))
        .route("/result/schema", get(result_schema))
        .route("/selection", get(selection))
        .route("/stats", get(stats))
        .route("/catalog", get(catalog))
        .route("/events", get(events))
        .fallback(|| async {
            ApiError::new(StatusCode::NOT_FOUND, "not_found", "unknown endpoint")
        })
        .layer(middleware::from_fn_with_state(ctx.clone(), guard))
        .with_state(ctx)
}

pub fn host_allowed(host: Option<&str>, port: u16) -> bool {
    let Some(h) = host else { return false };
    let h = h.to_ascii_lowercase();
    [
        format!("localhost:{port}"),
        format!("127.0.0.1:{port}"),
        format!("[::1]:{port}"),
    ]
    .contains(&h)
}

pub fn key_matches(expected: &str, header: Option<&str>) -> bool {
    let Some(h) = header else { return false };
    let Some(token) = h
        .strip_prefix("Bearer ")
        .or_else(|| h.strip_prefix("bearer "))
    else {
        return false;
    };
    let a = token.trim().as_bytes();
    let b = expected.as_bytes();
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

async fn guard(State(ctx): State<AppCtx>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    if !host_allowed(host, ctx.port) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden_host",
            "Host header must be localhost with the API port",
        )
        .into_response();
    }
    if headers.contains_key(header::ORIGIN) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden_origin",
            "browser requests are not accepted",
        )
        .into_response();
    }
    if req.uri().path() != "/health" {
        let auth = headers
            .get(header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok());
        if !key_matches(&ctx.core.api_key(), auth) {
            return ApiError::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing or wrong API key",
            )
            .into_response();
        }
    }
    next.run(req).await
}

async fn health() -> Response {
    Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") })).into_response()
}

fn tab_json(t: &TabRef) -> Value {
    json!({ "id": t.id, "title": t.title })
}

async fn state(State(ctx): State<AppCtx>) -> ApiResult {
    let core = &ctx.core;
    let id = core.active_id();
    let s = core.tab_summary(&id)?;
    Ok(Json(json!({
        "tab": { "id": s.id, "title": s.title },
        "source": core.source_info(),
        "query": s.sql,
        "dirty": s.dirty,
        "executed_sql": s.executed_sql,
        "limit": s.limit,
        "sort": s.sort,
        "status": s.status,
        "error": s.error,
        "row_count": s.row_count,
        "truncated": s.truncated,
        "total_count": s.total_count,
        "elapsed_ms": s.elapsed_ms,
        "stats_open": s.stats_open,
        "last_change": s.last_change,
        "config_errors": core.config_errors(),
        "workspace": core.workspace().map(|p| p.display().to_string()),
        "query_file": core.query_file().display().to_string(),
        "config_file": core.global_config_file().display().to_string(),
        "workspace_config_file": core.workspace_config_file().map(|p| p.display().to_string()),
    }))
    .into_response())
}

async fn get_query(State(ctx): State<AppCtx>) -> ApiResult {
    let t = ctx.core.active_ref();
    let sql = ctx.core.get_sql(&t.id)?;
    Ok(Json(json!({ "tab": tab_json(&t), "sql": sql })).into_response())
}

#[derive(Deserialize)]
struct PutQuery {
    sql: String,
    #[serde(default)]
    run: bool,
    rows: Option<usize>,
    wait_ms: Option<u64>,
}

async fn put_query(State(ctx): State<AppCtx>, Json(body): Json<PutQuery>) -> ApiResult {
    let t = ctx.core.active_ref();
    ctx.core.set_sql(&t.id, body.sql.clone(), Origin::Agent)?;
    let t = TabRef {
        title: ctx.core.tab_summary(&t.id)?.title,
        ..t
    };
    if body.run {
        return run_and_wait(&ctx, &t, None, body.rows, body.wait_ms, false).await;
    }
    Ok(Json(json!({ "tab": tab_json(&t), "sql": body.sql })).into_response())
}

#[derive(Deserialize, Default)]
struct RunBody {
    sql: Option<String>,
    limit: Option<usize>,
    rows: Option<usize>,
    wait_ms: Option<u64>,
    #[serde(default)]
    force: bool,
}

async fn run(State(ctx): State<AppCtx>, body: Option<Json<RunBody>>) -> ApiResult {
    let body = body.map(|b| b.0).unwrap_or_default();
    let t = ctx.core.active_ref();
    if let Some(sql) = &body.sql {
        ctx.core.set_sql(&t.id, sql.clone(), Origin::Agent)?;
    }
    // The title may follow the new text.
    let t = TabRef {
        title: ctx.core.tab_summary(&t.id)?.title,
        ..t
    };
    if let Some(l) = body.limit {
        ctx.core.set_tab_limit(&t.id, Some(l))?;
    }
    run_and_wait(&ctx, &t, None, body.rows, body.wait_ms, body.force).await
}

/// Start a run on the tab (or use `seq` from a run already started) and wait.
async fn run_and_wait(
    ctx: &AppCtx,
    t: &TabRef,
    started: Option<(u64, tokio::sync::broadcast::Receiver<Event>)>,
    rows: Option<usize>,
    wait_ms: Option<u64>,
    force: bool,
) -> ApiResult {
    let (seq, mut rx) = match started {
        Some(s) => s,
        None => {
            let rx = ctx.core.events.subscribe();
            let seq = ctx.core.run(
                &t.id,
                RunRequest {
                    text: None,
                    origin: Origin::Agent,
                    force,
                    last_only: false,
                },
            )?;
            (seq, rx)
        }
    };
    let wait = Duration::from_millis(wait_ms.unwrap_or(30_000));
    let finished = Core::wait_run(&mut rx, &t.id, seq, wait).await;
    run_response(ctx, t, finished, rows)
}

fn run_response(ctx: &AppCtx, t: &TabRef, finished: bool, rows: Option<usize>) -> ApiResult {
    let s = ctx.core.tab_summary(&t.id)?;
    if !finished || s.status == RunStatus::Running {
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "tab": tab_json(t), "status": "running" })),
        )
            .into_response());
    }
    if s.status == RunStatus::Error {
        let code = s.error_code.clone().unwrap_or_else(|| "query_error".into());
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "tab": tab_json(t), "status": "error", "error": { "code": code, "message": s.error } })),
        )
            .into_response());
    }
    let cfg = ctx.core.config().server;
    let n = rows.unwrap_or(cfg.default_rows).min(cfg.max_rows);
    let res = ctx.core.get_result(&t.id)?;
    let mut body = json!({
        "tab": tab_json(t),
        "status": "done",
        "executed_sql": s.executed_sql,
        "limit": s.limit,
        "sort": s.sort,
        "elapsed_ms": s.elapsed_ms,
    });
    if let Some(r) = res {
        merge(&mut body, result_json(&r, 0, n, None));
    }
    Ok(Json(body).into_response())
}

fn merge(a: &mut Value, b: Value) {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
}

#[derive(Deserialize)]
struct SortBody {
    sort: Vec<SortSpec>,
    rows: Option<usize>,
    wait_ms: Option<u64>,
}

async fn put_sort(State(ctx): State<AppCtx>, Json(body): Json<SortBody>) -> ApiResult {
    let t = ctx.core.active_ref();
    let rx = ctx.core.events.subscribe();
    match ctx.core.set_sort(&t.id, body.sort, Origin::Agent)? {
        Some(seq) => run_and_wait(&ctx, &t, Some((seq, rx)), body.rows, body.wait_ms, false).await,
        None => Ok(Json(json!({ "tab": tab_json(&t), "status": "idle", "note": "sort saved; it applies to the next run" })).into_response()),
    }
}

#[derive(Deserialize)]
struct LimitBody {
    limit: usize,
    rows: Option<usize>,
    wait_ms: Option<u64>,
    #[serde(default)]
    force: bool,
}

async fn put_limit(State(ctx): State<AppCtx>, Json(body): Json<LimitBody>) -> ApiResult {
    let t = ctx.core.active_ref();
    let rx = ctx.core.events.subscribe();
    match ctx.core.set_limit(&t.id, Some(body.limit), Origin::Agent, body.force)? {
        Some(seq) => run_and_wait(&ctx, &t, Some((seq, rx)), body.rows, body.wait_ms, body.force).await,
        None => Ok(Json(json!({ "tab": tab_json(&t), "status": "idle", "note": "limit saved; it applies to the next run" })).into_response()),
    }
}

async fn stop(State(ctx): State<AppCtx>) -> ApiResult {
    let t = ctx.core.active_ref();
    ctx.core.stop(&t.id)?;
    Ok(Json(json!({ "tab": tab_json(&t), "ok": true })).into_response())
}

#[derive(Deserialize, Default)]
struct ResultQuery {
    offset: Option<usize>,
    rows: Option<usize>,
    format: Option<String>,
    columns: Option<String>,
}

fn result_json(r: &QueryResult, offset: usize, n: usize, cols: Option<&[usize]>) -> Value {
    let all: Vec<usize> = (0..r.columns.len()).collect();
    let cols = cols.unwrap_or(&all);
    let end = (offset + n).min(r.rows.len());
    let start = offset.min(end);
    let rows: Vec<Vec<Value>> = r.rows[start..end]
        .iter()
        .map(|row| cols.iter().map(|&c| row[c].clone()).collect())
        .collect();
    json!({
        "columns": cols.iter().map(|&c| &r.columns[c]).collect::<Vec<_>>(),
        "rows": rows,
        "offset": start,
        "returned_rows": end - start,
        "result_rows": r.rows.len(),
        "truncated": r.truncated,
        "limit": r.limit,
        "wrapped": r.wrapped,
    })
}

fn cell_text(v: &Value, null: &str) -> String {
    match v {
        Value::Null => null.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn render_text(body: &Value, format: &str) -> String {
    let cols: Vec<String> = body["columns"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| c["name"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    let rows = body["rows"].as_array().cloned().unwrap_or_default();
    let mut out = String::new();
    if format == "csv" {
        out.push_str(
            &cols
                .iter()
                .map(|c| csv_field(c))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
        for r in rows {
            let cells: Vec<String> = r
                .as_array()
                .map(|a| a.iter().map(|v| csv_field(&cell_text(v, ""))).collect())
                .unwrap_or_default();
            out.push_str(&cells.join(","));
            out.push('\n');
        }
    } else {
        let esc = |s: String| s.replace('|', "\\|").replace('\n', " ");
        out.push_str(&format!(
            "| {} |\n",
            cols.iter()
                .map(|c| esc(c.clone()))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
        out.push_str(&format!(
            "|{}|\n",
            cols.iter().map(|_| "---").collect::<Vec<_>>().join("|")
        ));
        for r in rows {
            let cells: Vec<String> = r
                .as_array()
                .map(|a| a.iter().map(|v| esc(cell_text(v, "NULL"))).collect())
                .unwrap_or_default();
            out.push_str(&format!("| {} |\n", cells.join(" | ")));
        }
    }
    out
}

fn respond_rows(body: Value, format: Option<&str>) -> ApiResult {
    match format.unwrap_or("json") {
        "json" => Ok(Json(body).into_response()),
        f @ ("csv" | "markdown") => {
            let text = render_text(&body, f);
            let ct = if f == "csv" {
                "text/csv; charset=utf-8"
            } else {
                "text/markdown; charset=utf-8"
            };
            let mut resp = Response::new(Body::from(text));
            let h: &mut HeaderMap = resp.headers_mut();
            h.insert(header::CONTENT_TYPE, ct.parse().unwrap());
            for (k, key) in [
                ("x-gander-result-rows", "result_rows"),
                ("x-gander-returned-rows", "returned_rows"),
                ("x-gander-offset", "offset"),
                ("x-gander-truncated", "truncated"),
                ("x-gander-limit", "limit"),
            ] {
                if let Ok(v) = body[key].to_string().parse() {
                    h.insert(k, v);
                }
            }
            Ok(resp)
        }
        other => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "bad_request",
            format!("unknown format {other}; use json, csv or markdown"),
        )),
    }
}

fn column_indexes(r: &QueryResult, spec: Option<&str>) -> Result<Option<Vec<usize>>, ApiError> {
    let Some(spec) = spec.filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    let mut out = Vec::new();
    for name in spec.split(',').map(str::trim) {
        match r.columns.iter().position(|c| c.name == name) {
            Some(i) => out.push(i),
            None => {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    format!("no column named {name}"),
                ));
            }
        }
    }
    Ok(Some(out))
}

async fn result(State(ctx): State<AppCtx>, Query(q): Query<ResultQuery>) -> ApiResult {
    let t = ctx.core.active_ref();
    let Some(r) = ctx.core.get_result(&t.id)? else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no_result",
            "the active tab has no result yet",
        ));
    };
    let cfg = ctx.core.config().server;
    let n = q.rows.unwrap_or(cfg.default_rows).min(cfg.max_rows);
    let cols = column_indexes(&r, q.columns.as_deref())?;
    let mut body = result_json(&r, q.offset.unwrap_or(0), n, cols.as_deref());
    merge(
        &mut body,
        json!({ "tab": tab_json(&t), "executed_sql": r.executed_sql }),
    );
    respond_rows(body, q.format.as_deref())
}

async fn result_schema(State(ctx): State<AppCtx>) -> ApiResult {
    let t = ctx.core.active_ref();
    let Some(r) = ctx.core.get_result(&t.id)? else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no_result",
            "the active tab has no result yet",
        ));
    };
    Ok(Json(json!({
        "tab": tab_json(&t),
        "columns": r.columns,
        "result_rows": r.rows.len(),
        "truncated": r.truncated,
        "limit": r.limit,
        "executed_sql": r.executed_sql,
    }))
    .into_response())
}

async fn selection(State(ctx): State<AppCtx>, Query(q): Query<ResultQuery>) -> ApiResult {
    let t = ctx.core.active_ref();
    let Some(r) = ctx.core.get_result(&t.id)? else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no_result",
            "the active tab has no result yet",
        ));
    };
    let Some(sel) = ctx.core.get_selection(&t.id)? else {
        return Ok(Json(json!({ "tab": tab_json(&t), "selection": null, "rows": [], "columns": [], "returned_rows": 0 })).into_response());
    };
    let cfg = ctx.core.config().server;
    let n = q.rows.unwrap_or(cfg.default_rows).min(cfg.max_rows);
    let row_end = sel.row_end.min(r.rows.len().saturating_sub(1));
    let col_end = sel.col_end.min(r.columns.len().saturating_sub(1));
    let cols: Vec<usize> = (sel.col_start.min(col_end)..=col_end).collect();
    let count = row_end + 1 - sel.row_start.min(row_end + 1);
    let mut body = result_json(&r, sel.row_start, n.min(count), Some(&cols));
    merge(
        &mut body,
        json!({ "tab": tab_json(&t), "selection": sel, "selected_rows": count }),
    );
    respond_rows(body, q.format.as_deref())
}

#[derive(Deserialize, Default)]
struct StatsQuery {
    wait_ms: Option<u64>,
    format: Option<String>,
}

async fn stats(State(ctx): State<AppCtx>, Query(q): Query<StatsQuery>) -> ApiResult {
    let t = ctx.core.active_ref();
    // Opening the panel makes the SUMMARIZE run visible to the user.
    ctx.core.set_stats_open(&t.id, true)?;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(q.wait_ms.unwrap_or(30_000));
    while ctx.core.stats_state(&t.id)?.0 == RunStatus::Running
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (status, res, err) = ctx.core.stats_state(&t.id)?;
    match (status, res) {
        (_, Some(r)) => {
            let mut body = result_json(&r, 0, r.rows.len(), None);
            merge(&mut body, json!({ "tab": tab_json(&t), "status": "done" }));
            respond_rows(body, q.format.as_deref())
        }
        (RunStatus::Running, None) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "tab": tab_json(&t), "status": "running" })),
        )
            .into_response()),
        (_, None) => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_stats",
            err.unwrap_or_else(|| "no stats: run a query first".into()),
        )),
    }
}

#[derive(Deserialize, Default)]
struct CatalogQuery {
    q: Option<String>,
    table: Option<String>,
}

async fn catalog(State(ctx): State<AppCtx>, Query(q): Query<CatalogQuery>) -> ApiResult {
    let core = ctx.core.clone();
    let tables = tokio::task::spawn_blocking(move || core.catalog())
        .await
        .map_err(|e| {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
        })??;
    let mut tables = tables;
    if let Some(name) = q.table.as_deref().filter(|s| !s.is_empty()) {
        tables.retain(|t| {
            t.name == name
                || format!("{}.{}", t.schema, t.name) == name
                || format!("{}.{}.{}", t.database, t.schema, t.name) == name
        });
    }
    if let Some(query) = q.q.as_deref().filter(|s| !s.trim().is_empty()) {
        tables = crate::fuzzy::filter_catalog(tables, query);
    }
    Ok(Json(json!({ "source": ctx.core.source_info(), "tables": tables })).into_response())
}

async fn events(State(ctx): State<AppCtx>) -> Response {
    let rx = ctx.core.events.subscribe();
    let stream = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(|e| async move {
        let e = e.ok()?;
        let v = serde_json::to_value(&e).ok()?;
        let name = v
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("event")
            .to_string();
        Some(Ok::<_, std::convert::Infallible>(
            axum::response::sse::Event::default()
                .event(name)
                .data(v.to_string()),
        ))
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert!(host_allowed(Some("localhost:7420"), 7420));
        assert!(host_allowed(Some("127.0.0.1:7420"), 7420));
        assert!(host_allowed(Some("[::1]:7420"), 7420));
        assert!(!host_allowed(Some("localhost:7421"), 7420));
        assert!(!host_allowed(Some("evil.com:7420"), 7420));
        assert!(!host_allowed(Some("localhost"), 7420));
        assert!(!host_allowed(None, 7420));
    }

    #[test]
    fn keys() {
        assert!(key_matches("abc", Some("Bearer abc")));
        assert!(!key_matches("abc", Some("Bearer abd")));
        assert!(!key_matches("abc", Some("Bearer ab")));
        assert!(!key_matches("abc", Some("abc")));
        assert!(!key_matches("abc", None));
    }

    #[test]
    fn markdown_and_csv() {
        let body = json!({ "columns": [{"name": "a"}, {"name": "b"}], "rows": [[1, "x,y"], [null, "p|q"]] });
        assert_eq!(render_text(&body, "csv"), "a,b\n1,\"x,y\"\n,p|q\n");
        assert_eq!(
            render_text(&body, "markdown"),
            "| a | b |\n|---|---|\n| 1 | x,y |\n| NULL | p\\|q |\n"
        );
    }
}
