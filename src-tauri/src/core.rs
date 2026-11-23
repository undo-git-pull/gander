//! Application core: the single source of truth for tabs, results, the data
//! source and config. The Tauri commands and the HTTP API both call into it.

use crate::config::{self, Config, ConfigError, WorkspaceTrust};
use crate::db::{self, wrap, wrap::SortSpec, QueryResult, SourceInfo, SourceSpec};
use crate::paths;
use duckdb::Connection;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    User,
    Agent,
    File,
    Restore,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Idle,
    Running,
    Done,
    Error,
}

/// Receives events for the webview.
pub trait UiSink: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

pub struct NoUi;
impl UiSink for NoUi {
    fn emit(&self, _: &str, _: serde_json::Value) {}
}

#[derive(Debug, Clone, Serialize)]
pub struct TabRef {
    pub id: String,
    pub title: String,
}

/// Events for API subscribers (SSE) and for waiting on runs.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    TabChanged { tab: TabRef },
    SourceChanged { source: SourceInfo },
    QueryChanged { tab: TabRef, origin: Origin },
    RunStarted { tab: TabRef, seq: u64, origin: Origin },
    RunFinished {
        tab: TabRef,
        seq: u64,
        status: String,
        error: Option<String>,
        row_count: Option<usize>,
        truncated: bool,
    },
    StatsFinished { tab: TabRef, seq: u64, status: String, error: Option<String> },
    ConfigReloaded,
    ConfigError { errors: Vec<ConfigError> },
}

#[derive(Debug, Clone)]
pub struct CoreError {
    pub code: &'static str,
    pub message: String,
}

impl CoreError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<CoreError> for String {
    fn from(e: CoreError) -> Self {
        e.message
    }
}

pub struct TabConn {
    conn: Mutex<Connection>,
    interrupt: Arc<duckdb::InterruptHandle>,
    generation: u64,
}

impl TabConn {
    fn new(conn: Connection, generation: u64) -> Self {
        let interrupt = conn.interrupt_handle();
        Self { conn: Mutex::new(conn), interrupt, generation }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Selection {
    /// Inclusive row range into the result.
    pub row_start: usize,
    pub row_end: usize,
    /// Inclusive column range into the result's columns.
    pub col_start: usize,
    pub col_end: usize,
}

struct StatsState {
    status: RunStatus,
    seq: u64,
    key: Option<(String, u64)>,
    result: Option<Arc<QueryResult>>,
    error: Option<String>,
    version: u64,
}

struct Tab {
    id: String,
    title: String,
    custom_title: bool,
    sql: String,
    sql_rev: u64,
    sql_origin: Origin,
    /// Full editor text at the last run, for the dirty marker.
    ran_text: Option<String>,
    /// Final statement of the last run, re-used for sort, limit, count, stats.
    statement: Option<String>,
    sort: Vec<SortSpec>,
    limit: Option<usize>,
    status: RunStatus,
    error: Option<String>,
    error_code: Option<String>,
    result: Option<Arc<QueryResult>>,
    result_version: u64,
    last_row_count: Option<usize>,
    last_truncated: bool,
    total_count: Option<u64>,
    counting: bool,
    run_seq: u64,
    last_change: Origin,
    stats_open: bool,
    stats: StatsState,
    selection: Option<Selection>,
    conn: Option<Arc<TabConn>>,
    side_conn: Option<Arc<TabConn>>,
}

impl Tab {
    fn new(id: String, sql: String) -> Self {
        Self {
            title: default_title(&sql).unwrap_or_else(|| "query".into()),
            id,
            custom_title: false,
            sql,
            sql_rev: 0,
            sql_origin: Origin::Restore,
            ran_text: None,
            statement: None,
            sort: vec![],
            limit: None,
            status: RunStatus::Idle,
            error: None,
            error_code: None,
            result: None,
            result_version: 0,
            last_row_count: None,
            last_truncated: false,
            total_count: None,
            counting: false,
            run_seq: 0,
            last_change: Origin::Restore,
            stats_open: false,
            stats: StatsState { status: RunStatus::Idle, seq: 0, key: None, result: None, error: None, version: 0 },
            selection: None,
            conn: None,
            side_conn: None,
        }
    }
    fn tab_ref(&self) -> TabRef {
        TabRef { id: self.id.clone(), title: self.title.clone() }
    }
}

/// Guess a tab title from the first table, view or file after FROM.
pub fn default_title(sql: &str) -> Option<String> {
    let stmt = wrap::split(sql).into_iter().last()?;
    let lower = stmt.to_ascii_lowercase();
    let mut search = 0;
    while let Some(pos) = lower[search..].find("from") {
        let at = search + pos;
        let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let rest = &stmt[at + 4..];
        if before_ok && rest.starts_with(|c: char| c.is_whitespace()) {
            let tok = rest.trim_start();
            let name: String = if let Some(q) = tok.strip_prefix('\'') {
                q.split('\'').next().unwrap_or("").to_string()
            } else if let Some(q) = tok.strip_prefix('"') {
                q.split('"').next().unwrap_or("").to_string()
            } else {
                tok.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.').collect()
            };
            if !name.is_empty() {
                let ext = Path::new(&name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                let is_file = name.contains('/')
                    || matches!(ext.as_str(), "parquet" | "csv" | "tsv" | "json" | "jsonl" | "ndjson" | "duckdb" | "gz" | "zst");
                let t = if is_file {
                    Path::new(&name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(name.clone())
                } else {
                    // qualified name: keep the last part
                    name.rsplit('.').next().unwrap_or(&name).to_string()
                };
                if !t.is_empty() {
                    return Some(t.chars().take(40).collect());
                }
            }
        }
        search = at + 4;
    }
    None
}

#[derive(Debug, Clone, Serialize)]
pub struct TabSummary {
    pub id: String,
    pub title: String,
    pub custom_title: bool,
    pub sql: String,
    pub sql_rev: u64,
    pub sql_origin: Origin,
    pub dirty: bool,
    pub status: RunStatus,
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub sort: Vec<SortSpec>,
    pub limit: usize,
    pub limit_override: Option<usize>,
    pub row_count: Option<usize>,
    pub truncated: bool,
    pub total_count: Option<u64>,
    pub counting: bool,
    pub elapsed_ms: Option<f64>,
    pub executed_sql: Option<String>,
    pub wrapped: bool,
    pub has_result: bool,
    pub result_version: u64,
    pub stats_open: bool,
    pub stats_status: RunStatus,
    pub stats_error: Option<String>,
    pub stats_version: u64,
    pub last_change: Origin,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiStatus {
    pub enabled: bool,
    pub running: bool,
    pub port: Option<u16>,
    pub auth_required: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub tabs: Vec<TabSummary>,
    pub active: String,
    pub config: Config,
    pub config_errors: Vec<ConfigError>,
    pub source: SourceInfo,
    pub source_error: Option<String>,
    pub connections: Vec<String>,
    pub recents: Vec<String>,
    pub api: ApiStatus,
    pub workspace: Option<String>,
    pub workspace_trust: WorkspaceTrust,
    pub query_file: String,
    pub config_file: String,
}

#[derive(Serialize, Deserialize, Default)]
struct PersistedTabs {
    /// Workspace path these tabs belong to, so stale folders can be pruned.
    #[serde(default)]
    workspace: Option<String>,
    active: Option<String>,
    next_id: u64,
    tabs: Vec<PersistedTab>,
}

#[derive(Serialize, Deserialize)]
struct PersistedTab {
    id: String,
    title: String,
    #[serde(default)]
    custom_title: bool,
    sql: String,
    #[serde(default)]
    sort: Vec<SortSpec>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    stats_open: bool,
    #[serde(default)]
    last_row_count: Option<usize>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct AppStateFile {
    #[serde(default)]
    last_workspace: Option<String>,
    #[serde(default)]
    recents: Vec<String>,
    #[serde(default)]
    layout: serde_json::Value,
}

struct SourceState {
    info: SourceInfo,
    conn: Mutex<Connection>,
}

struct Inner {
    config: Config,
    config_errors: Vec<ConfigError>,
    trust: WorkspaceTrust,
    workspace: Option<PathBuf>,
    source: Arc<SourceState>,
    source_error: Option<String>,
    generation: u64,
    tabs: Vec<Tab>,
    active: String,
    next_id: u64,
    app_state: AppStateFile,
    tabs_dirty_since: Option<Instant>,
    app_state_dirty: bool,
    query_file_due: Option<Instant>,
    query_file_hash: Option<u64>,
    api: ApiStatus,
}

impl Inner {
    fn tab(&self, id: &str) -> Result<&Tab, CoreError> {
        self.tabs.iter().find(|t| t.id == id).ok_or_else(|| CoreError::new("not_found", format!("no tab {id}")))
    }
    fn tab_mut(&mut self, id: &str) -> Result<&mut Tab, CoreError> {
        self.tabs.iter_mut().find(|t| t.id == id).ok_or_else(|| CoreError::new("not_found", format!("no tab {id}")))
    }
    fn effective_limit(&self, tab: &Tab) -> usize {
        tab.limit.unwrap_or(self.config.query.limit)
    }
    fn summary(&self, t: &Tab) -> TabSummary {
        let r = t.result.as_ref();
        TabSummary {
            id: t.id.clone(),
            title: t.title.clone(),
            custom_title: t.custom_title,
            sql: t.sql.clone(),
            sql_rev: t.sql_rev,
            sql_origin: t.sql_origin,
            dirty: t.ran_text.as_deref().map(|r| r != t.sql).unwrap_or(!t.sql.trim().is_empty()),
            status: t.status,
            error: t.error.clone(),
            error_code: t.error_code.clone(),
            sort: t.sort.clone(),
            limit: self.effective_limit(t),
            limit_override: t.limit,
            row_count: r.map(|r| r.rows.len()).or(t.last_row_count),
            truncated: r.map(|r| r.truncated).unwrap_or(t.last_truncated),
            total_count: t.total_count,
            counting: t.counting,
            elapsed_ms: r.map(|r| r.elapsed_ms),
            executed_sql: r.map(|r| r.executed_sql.clone()),
            wrapped: r.map(|r| r.wrapped).unwrap_or(false),
            has_result: r.is_some(),
            result_version: t.result_version,
            stats_open: t.stats_open,
            stats_status: t.stats.status,
            stats_error: t.stats.error.clone(),
            stats_version: t.stats.version,
            last_change: t.last_change,
        }
    }
    fn mark_tabs_dirty(&mut self) {
        if self.tabs_dirty_since.is_none() {
            self.tabs_dirty_since = Some(Instant::now());
        }
    }
    fn schedule_query_file(&mut self) {
        self.query_file_due = Some(Instant::now() + Duration::from_millis(500));
    }
    fn conn_for(&mut self, id: &str, side: bool) -> Result<Arc<TabConn>, CoreError> {
        let generation = self.generation;
        let source = self.source.clone();
        let tab = self.tab_mut(id)?;
        let slot = if side { &mut tab.side_conn } else { &mut tab.conn };
        if let Some(c) = slot {
            if c.generation == generation {
                return Ok(c.clone());
            }
        }
        let conn = source
            .conn
            .lock()
            .try_clone()
            .map_err(|e| CoreError::new("source_error", format!("cannot open connection: {e}")))?;
        let c = Arc::new(TabConn::new(conn, generation));
        *slot = Some(c.clone());
        Ok(c)
    }
}

pub struct RunRequest {
    /// Text to run instead of the whole editor (selection or statement at cursor).
    pub text: Option<String>,
    pub origin: Origin,
    pub force: bool,
    /// Re-run only the final statement of the last run (sort, limit changes).
    pub last_only: bool,
}

pub struct Core {
    inner: Mutex<Inner>,
    pub events: broadcast::Sender<Event>,
    ui: Box<dyn UiSink>,
    rt: tokio::runtime::Handle,
    pub(crate) server: Mutex<Option<crate::api::ServerHandle>>,
    global_config: PathBuf,
    /// Random API key for this launch, used when the config sets none.
    session_key: String,
}

pub struct CoreOptions {
    pub workspace: Option<PathBuf>,
    pub ui: Box<dyn UiSink>,
    pub rt: tokio::runtime::Handle,
    /// Skip reading/writing persisted tabs and state (tests).
    pub ephemeral: bool,
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(p: &Path) -> T {
    std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn random_key() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("system random source");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn load_config(global: &Path, ws: Option<&Path>) -> config::Loaded {
    let ws_file = paths::workspace_config_file(ws);
    let trusted = ws_file.as_deref().and_then(config::trusted_hash);
    config::load(global, ws_file.as_deref(), trusted.as_deref())
}

pub fn last_workspace() -> Option<PathBuf> {
    let st: AppStateFile = read_json(&paths::state_file());
    st.last_workspace.map(PathBuf::from).filter(|p| p.is_dir())
}

impl Core {
    pub fn new(opts: CoreOptions) -> Arc<Core> {
        let ws = opts.workspace.clone();
        let global = paths::global_config_file();
        let loaded = load_config(&global, ws.as_deref());
        let config = loaded.config;
        if !opts.ephemeral {
            let _ = std::fs::create_dir_all(paths::config_dir());
            let _ = std::fs::create_dir_all(paths::workspace_dot_dir(ws.as_deref()));
            let _ = paths::write_atomic(&paths::schema_file(), config::json_schema().as_bytes());
        }

        let mut source_error = None;
        let spec = match &config.default_connection {
            Some(name) if loaded.errors.is_empty() => SourceSpec::Configured { name: name.clone() },
            _ => SourceSpec::Memory,
        };
        let opened = match db::open_source(&spec, &config, ws.as_deref()) {
            Ok(o) => o,
            Err(e) => {
                source_error = Some(e.to_string());
                db::open_source(&SourceSpec::Memory, &config, ws.as_deref()).expect("in-memory duckdb")
            }
        };

        let mut app_state: AppStateFile = if opts.ephemeral { Default::default() } else { read_json(&paths::state_file()) };
        app_state.last_workspace = ws.as_ref().map(|p| p.display().to_string());

        let persisted: PersistedTabs = if opts.ephemeral { Default::default() } else { read_json(&paths::tabs_file(ws.as_deref())) };
        let mut tabs: Vec<Tab> = persisted
            .tabs
            .into_iter()
            .map(|p| {
                let mut t = Tab::new(p.id, p.sql);
                t.title = p.title;
                t.custom_title = p.custom_title;
                t.sort = p.sort;
                t.limit = p.limit;
                t.stats_open = p.stats_open;
                t.last_row_count = p.last_row_count;
                t.last_truncated = p.truncated;
                t
            })
            .collect();
        let mut next_id = persisted.next_id.max(1);
        if tabs.is_empty() {
            tabs.push(Tab::new(format!("t{next_id}"), String::new()));
            next_id += 1;
        }
        let active = persisted.active.filter(|a| tabs.iter().any(|t| &t.id == a)).unwrap_or_else(|| tabs[0].id.clone());

        let (events, _) = broadcast::channel(256);
        let inner = Inner {
            api: ApiStatus { enabled: config.server.enabled, running: false, port: None, auth_required: true, error: None },
            config,
            config_errors: loaded.errors,
            trust: loaded.trust,
            workspace: ws.clone(),
            source: Arc::new(SourceState { info: opened.info, conn: Mutex::new(opened.conn) }),
            source_error,
            generation: 1,
            tabs,
            active,
            next_id,
            app_state,
            tabs_dirty_since: None,
            app_state_dirty: !opts.ephemeral,
            query_file_due: None,
            query_file_hash: None,
        };
        let core = Arc::new(Core {
            inner: Mutex::new(inner),
            events,
            ui: opts.ui,
            rt: opts.rt,
            server: Mutex::new(None),
            global_config: global,
            session_key: random_key(),
        });
        if !opts.ephemeral {
            core.adopt_query_file_on_start();
            let ws = ws.clone();
            std::thread::spawn(move || crate::housekeeping::run(ws.as_deref()));
        }
        core
    }

    pub fn workspace(&self) -> Option<PathBuf> {
        self.inner.lock().workspace.clone()
    }

    pub fn config(&self) -> Config {
        self.inner.lock().config.clone()
    }

    pub fn query_file(&self) -> PathBuf {
        paths::query_file(self.inner.lock().workspace.as_deref())
    }

    pub fn workspace_config_file(&self) -> Option<PathBuf> {
        paths::workspace_config_file(self.inner.lock().workspace.as_deref())
    }

    pub fn global_config_file(&self) -> PathBuf {
        self.global_config.clone()
    }

    fn emit_ui<T: Serialize>(&self, event: &str, payload: &T) {
        if let Ok(v) = serde_json::to_value(payload) {
            self.ui.emit(event, v);
        }
    }

    fn broadcast(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn emit_tab(&self, inner: &Inner, id: &str) {
        if let Ok(t) = inner.tab(id) {
            self.emit_ui("tab", &inner.summary(t));
        }
    }

    fn emit_tabs(&self, inner: &Inner) {
        let tabs: Vec<TabSummary> = inner.tabs.iter().map(|t| inner.summary(t)).collect();
        self.emit_ui("tabs", &serde_json::json!({ "tabs": tabs, "active": inner.active }));
    }

    fn emit_status(&self, inner: &Inner) {
        self.emit_ui(
            "status",
            &serde_json::json!({
                "api": inner.api,
                "config_errors": inner.config_errors,
                "source_error": inner.source_error,
                "workspace_trust": inner.trust,
            }),
        );
    }

    pub fn snapshot(&self) -> Snapshot {
        let inner = self.inner.lock();
        Snapshot {
            tabs: inner.tabs.iter().map(|t| inner.summary(t)).collect(),
            active: inner.active.clone(),
            config: inner.config.clone(),
            config_errors: inner.config_errors.clone(),
            source: inner.source.info.clone(),
            source_error: inner.source_error.clone(),
            connections: inner.config.connections.iter().map(|c| c.name.clone()).collect(),
            recents: inner.app_state.recents.clone(),
            api: inner.api.clone(),
            workspace: inner.workspace.as_ref().map(|p| p.display().to_string()),
            workspace_trust: inner.trust.clone(),
            query_file: paths::query_file(inner.workspace.as_deref()).display().to_string(),
            config_file: self.global_config.display().to_string(),
        }
    }

    pub fn active_id(&self) -> String {
        self.inner.lock().active.clone()
    }

    pub fn active_ref(&self) -> TabRef {
        let inner = self.inner.lock();
        inner.tab(&inner.active).map(|t| t.tab_ref()).unwrap_or(TabRef { id: inner.active.clone(), title: String::new() })
    }

    pub fn tab_summary(&self, id: &str) -> Result<TabSummary, CoreError> {
        let inner = self.inner.lock();
        inner.tab(id).map(|t| inner.summary(t))
    }

    pub fn source_info(&self) -> SourceInfo {
        self.inner.lock().source.info.clone()
    }

    pub fn config_errors(&self) -> Vec<ConfigError> {
        self.inner.lock().config_errors.clone()
    }

    /// The key API requests must send: the configured one, else this launch's random key.
    pub fn api_key(&self) -> String {
        self.config().server.resolved_api_key().unwrap_or_else(|| self.session_key.clone())
    }

    /// True when the API uses the random per-launch key, which is then
    /// published in `server.json` for local clients.
    pub fn api_key_is_generated(&self) -> bool {
        self.config().server.resolved_api_key().is_none()
    }

    pub fn api_status(&self) -> ApiStatus {
        self.inner.lock().api.clone()
    }

    pub(crate) fn set_api_status(&self, status: ApiStatus) {
        let mut inner = self.inner.lock();
        inner.api = status;
        self.emit_status(&inner);
    }

    // ---- tabs ----

    pub fn new_tab(&self, sql: Option<String>, activate: bool) -> String {
        let mut inner = self.inner.lock();
        let id = format!("t{}", inner.next_id);
        inner.next_id += 1;
        let mut tab = Tab::new(id.clone(), sql.unwrap_or_default());
        tab.last_change = Origin::User;
        let pos = inner.tabs.iter().position(|t| t.id == inner.active).map(|p| p + 1).unwrap_or(inner.tabs.len());
        inner.tabs.insert(pos, tab);
        if activate {
            inner.active = id.clone();
            inner.schedule_query_file();
        }
        inner.mark_tabs_dirty();
        self.emit_tabs(&inner);
        if activate {
            let r = inner.tab(&id).map(|t| t.tab_ref()).ok();
            drop(inner);
            if let Some(r) = r {
                self.broadcast(Event::TabChanged { tab: r });
            }
        }
        id
    }

    pub fn close_tab(&self, id: &str) -> Result<(), CoreError> {
        let mut inner = self.inner.lock();
        let pos = inner.tabs.iter().position(|t| t.id == id).ok_or_else(|| CoreError::new("not_found", "no such tab"))?;
        let tab = inner.tabs.remove(pos);
        for c in [&tab.conn, &tab.side_conn].into_iter().flatten() {
            c.interrupt.interrupt();
        }
        if inner.tabs.is_empty() {
            let nid = format!("t{}", inner.next_id);
            inner.next_id += 1;
            inner.tabs.push(Tab::new(nid, String::new()));
        }
        let mut changed = None;
        if inner.active == id {
            let p = pos.min(inner.tabs.len() - 1);
            inner.active = inner.tabs[p].id.clone();
            inner.schedule_query_file();
            changed = Some(inner.tabs[p].tab_ref());
        }
        inner.mark_tabs_dirty();
        self.emit_tabs(&inner);
        drop(inner);
        if let Some(r) = changed {
            self.broadcast(Event::TabChanged { tab: r });
        }
        Ok(())
    }

    pub fn activate_tab(&self, id: &str) -> Result<(), CoreError> {
        let mut inner = self.inner.lock();
        let r = inner.tab(id)?.tab_ref();
        if inner.active == id {
            return Ok(());
        }
        inner.active = id.to_string();
        inner.schedule_query_file();
        inner.mark_tabs_dirty();
        self.emit_tabs(&inner);
        drop(inner);
        self.broadcast(Event::TabChanged { tab: r });
        Ok(())
    }

    pub fn move_tab(&self, id: &str, to: usize) -> Result<(), CoreError> {
        let mut inner = self.inner.lock();
        let pos = inner.tabs.iter().position(|t| t.id == id).ok_or_else(|| CoreError::new("not_found", "no such tab"))?;
        let t = inner.tabs.remove(pos);
        let to = to.min(inner.tabs.len());
        inner.tabs.insert(to, t);
        inner.mark_tabs_dirty();
        self.emit_tabs(&inner);
        Ok(())
    }

    pub fn rename_tab(&self, id: &str, title: &str) -> Result<(), CoreError> {
        let mut inner = self.inner.lock();
        let tab = inner.tab_mut(id)?;
        let title = title.trim();
        if title.is_empty() {
            tab.custom_title = false;
            tab.title = default_title(&tab.sql).unwrap_or_else(|| "query".into());
        } else {
            tab.custom_title = true;
            tab.title = title.chars().take(60).collect();
        }
        inner.mark_tabs_dirty();
        self.emit_tab(&inner, id);
        Ok(())
    }

    /// Replace a tab's text. User edits come from the editor; agent and file
    /// edits are pushed to the editor through the `tab` event.
    pub fn set_sql(&self, id: &str, sql: String, origin: Origin) -> Result<(), CoreError> {
        let mut inner = self.inner.lock();
        let is_active = inner.active == id;
        let tab = inner.tab_mut(id)?;
        if tab.sql == sql {
            return Ok(());
        }
        if origin != Origin::User {
            // Keep the replaced text in history so nothing is lost.
            let replaced = tab.sql.clone();
            if !replaced.trim().is_empty() && tab.ran_text.as_deref() != Some(replaced.as_str()) {
                append_history(&replaced, "replaced");
            }
        }
        tab.sql = sql;
        tab.sql_rev += 1;
        tab.sql_origin = origin;
        tab.last_change = origin;
        if !tab.custom_title {
            if let Some(t) = default_title(&tab.sql) {
                tab.title = t;
            }
        }
        let r = tab.tab_ref();
        if is_active && origin != Origin::File {
            inner.schedule_query_file();
        }
        inner.mark_tabs_dirty();
        self.emit_tab(&inner, id);
        drop(inner);
        if origin != Origin::User || is_active {
            self.broadcast(Event::QueryChanged { tab: r, origin });
        }
        Ok(())
    }

    pub fn get_sql(&self, id: &str) -> Result<String, CoreError> {
        Ok(self.inner.lock().tab(id)?.sql.clone())
    }

    pub fn get_result(&self, id: &str) -> Result<Option<Arc<QueryResult>>, CoreError> {
        Ok(self.inner.lock().tab(id)?.result.clone())
    }

    pub fn get_stats(&self, id: &str) -> Result<Option<Arc<QueryResult>>, CoreError> {
        Ok(self.inner.lock().tab(id)?.stats.result.clone())
    }

    pub fn get_selection(&self, id: &str) -> Result<Option<Selection>, CoreError> {
        Ok(self.inner.lock().tab(id)?.selection)
    }

    pub fn set_selection(&self, id: &str, sel: Option<Selection>) -> Result<(), CoreError> {
        let mut inner = self.inner.lock();
        inner.tab_mut(id)?.selection = sel;
        Ok(())
    }

    // ---- running ----

    pub fn run(self: &Arc<Self>, id: &str, req: RunRequest) -> Result<u64, CoreError> {
        let (conn, sql, sort, limit, seq, tab_ref, timeout) = {
            let mut inner = self.inner.lock();
            let timeout = inner.config.query.timeout_ms;
            let limit = inner.effective_limit(inner.tab(id)?);
            let conn = inner.conn_for(id, false)?;
            let tab = inner.tab_mut(id)?;
            let sql = if req.last_only {
                match &tab.statement {
                    Some(s) => s.clone(),
                    None => return Err(CoreError::new("no_result", "Run a query first")),
                }
            } else {
                req.text.clone().unwrap_or_else(|| tab.sql.clone())
            };
            if sql.trim().is_empty() {
                return Err(CoreError::new("empty", "Nothing to run"));
            }
            if tab.status == RunStatus::Running {
                if let Some(c) = &tab.conn {
                    c.interrupt.interrupt();
                }
            }
            if let Some(c) = &tab.side_conn {
                if tab.stats.status == RunStatus::Running || tab.counting {
                    c.interrupt.interrupt();
                }
            }
            tab.run_seq += 1;
            let seq = tab.run_seq;
            tab.status = RunStatus::Running;
            tab.error = None;
            tab.error_code = None;
            tab.total_count = None;
            tab.counting = false;
            if !req.last_only {
                // Earlier statements run now; re-runs (sort, limit, "run anyway")
                // only repeat the final one.
                if let Some(last) = wrap::split(&sql).pop() {
                    tab.statement = Some(last);
                }
                tab.ran_text = Some(tab.sql.clone());
                append_history(&sql, match req.origin {
                    Origin::Agent => "agent",
                    _ => "user",
                });
            }
            if req.origin == Origin::Agent {
                tab.last_change = Origin::Agent;
            }
            let r = tab.tab_ref();
            let sort = tab.sort.clone();
            self.emit_tab(&inner, id);
            (conn, sql, sort, limit, seq, r, timeout)
        };
        self.broadcast(Event::RunStarted { tab: tab_ref.clone(), seq, origin: req.origin });

        let core = self.clone();
        let id_owned = id.to_string();
        let force = req.force;
        let last_only = req.last_only;
        let conn2 = conn.clone();
        self.rt.spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                let guard = conn2.conn.lock();
                db::run(&guard, db::RunOptions { sql: &sql, last_only, sort: &sort, limit, force })
            })
            .await;
            let res = match res {
                Ok(r) => r,
                Err(e) => Err(db::QueryError { code: "internal", message: e.to_string() }),
            };
            core.finish_run(&id_owned, seq, res);
        });
        if timeout > 0 {
            let core = self.clone();
            let id_owned = id.to_string();
            self.rt.spawn(async move {
                tokio::time::sleep(Duration::from_millis(timeout)).await;
                let inner = core.inner.lock();
                if let Ok(t) = inner.tab(&id_owned) {
                    if t.run_seq == seq && t.status == RunStatus::Running {
                        conn.interrupt.interrupt();
                    }
                }
            });
        }
        Ok(seq)
    }

    fn finish_run(self: &Arc<Self>, id: &str, seq: u64, res: Result<QueryResult, db::QueryError>) {
        let mut inner = self.inner.lock();
        let Ok(tab) = inner.tab_mut(id) else { return };
        let tab_ref = tab.tab_ref();
        if tab.run_seq != seq {
            drop(inner);
            self.broadcast(Event::RunFinished { tab: tab_ref, seq, status: "superseded".into(), error: None, row_count: None, truncated: false });
            return;
        }
        let (status, error, row_count, truncated) = match res {
            Ok(r) => {
                tab.status = RunStatus::Done;
                tab.statement = Some(r.statement.clone());
                if r.wrapped {
                    tab.sort = r.sort.clone();
                }
                tab.last_row_count = Some(r.rows.len());
                tab.last_truncated = r.truncated;
                let (rc, tr) = (r.rows.len(), r.truncated);
                tab.result = Some(Arc::new(r));
                tab.result_version += 1;
                tab.selection = None;
                // Stats cache is keyed by statement; a new run invalidates it.
                tab.stats.result = None;
                tab.stats.key = None;
                tab.stats.error = None;
                tab.stats.status = RunStatus::Idle;
                tab.stats.version += 1;
                ("done".to_string(), None, Some(rc), tr)
            }
            Err(e) => {
                tab.status = RunStatus::Error;
                let msg = if e.code == "interrupted" { "Query interrupted".to_string() } else { e.message.clone() };
                tab.error = Some(msg.clone());
                tab.error_code = Some(e.code.to_string());
                ("error".to_string(), Some(msg), None, false)
            }
        };
        let want_stats = tab.stats_open && status == "done";
        inner.mark_tabs_dirty();
        self.emit_tab(&inner, id);
        drop(inner);
        self.broadcast(Event::RunFinished { tab: tab_ref, seq, status, error, row_count, truncated });
        if want_stats {
            let _ = self.start_stats(id);
        }
    }

    pub fn stop(&self, id: &str) -> Result<(), CoreError> {
        let inner = self.inner.lock();
        let tab = inner.tab(id)?;
        if let Some(c) = &tab.conn {
            if tab.status == RunStatus::Running {
                c.interrupt.interrupt();
            }
        }
        if let Some(c) = &tab.side_conn {
            c.interrupt.interrupt();
        }
        Ok(())
    }

    pub fn set_sort(self: &Arc<Self>, id: &str, sort: Vec<SortSpec>, origin: Origin) -> Result<Option<u64>, CoreError> {
        let has_stmt = {
            let mut inner = self.inner.lock();
            let tab = inner.tab_mut(id)?;
            tab.sort = sort;
            if origin == Origin::Agent {
                tab.last_change = Origin::Agent;
            }
            let has = tab.statement.is_some() && tab.result.as_ref().map(|r| r.wrapped).unwrap_or(false);
            inner.mark_tabs_dirty();
            self.emit_tab(&inner, id);
            has
        };
        if has_stmt {
            return self.run(id, RunRequest { text: None, origin, force: false, last_only: true }).map(Some);
        }
        Ok(None)
    }

    /// Set the tab's row limit without re-running.
    pub fn set_tab_limit(&self, id: &str, limit: Option<usize>) -> Result<(), CoreError> {
        let mut inner = self.inner.lock();
        inner.tab_mut(id)?.limit = limit;
        inner.mark_tabs_dirty();
        self.emit_tab(&inner, id);
        Ok(())
    }

    pub fn rt_handle(&self) -> tokio::runtime::Handle {
        self.rt.clone()
    }

    pub fn set_limit(self: &Arc<Self>, id: &str, limit: Option<usize>, origin: Origin, force: bool) -> Result<Option<u64>, CoreError> {
        let has_stmt = {
            let mut inner = self.inner.lock();
            let tab = inner.tab_mut(id)?;
            tab.limit = limit;
            let has = tab.statement.is_some() && tab.result.as_ref().map(|r| r.wrapped).unwrap_or(false);
            inner.mark_tabs_dirty();
            self.emit_tab(&inner, id);
            has
        };
        if has_stmt {
            return self.run(id, RunRequest { text: None, origin, force, last_only: true }).map(Some);
        }
        Ok(None)
    }

    pub fn count_rows(self: &Arc<Self>, id: &str) -> Result<(), CoreError> {
        let (conn, stmt) = {
            let mut inner = self.inner.lock();
            let conn = inner.conn_for(id, true)?;
            let tab = inner.tab_mut(id)?;
            let stmt = tab.statement.clone().ok_or_else(|| CoreError::new("no_result", "Run a query first"))?;
            tab.counting = true;
            self.emit_tab(&inner, id);
            (conn, stmt)
        };
        let core = self.clone();
        let id = id.to_string();
        self.rt.spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                let g = conn.conn.lock();
                g.query_row(&wrap::count_sql(&stmt), [], |r| r.get::<_, i64>(0))
            })
            .await;
            let mut inner = core.inner.lock();
            if let Ok(tab) = inner.tab_mut(&id) {
                tab.counting = false;
                if let Ok(Ok(n)) = res {
                    tab.total_count = Some(n as u64);
                }
                core.emit_tab(&inner, &id);
            }
        });
        Ok(())
    }

    pub fn set_stats_open(self: &Arc<Self>, id: &str, open: bool) -> Result<(), CoreError> {
        {
            let mut inner = self.inner.lock();
            let tab = inner.tab_mut(id)?;
            tab.stats_open = open;
            if !open && tab.stats.status == RunStatus::Running {
                if let Some(c) = &tab.side_conn {
                    c.interrupt.interrupt();
                }
                tab.stats.seq += 1;
                tab.stats.status = RunStatus::Idle;
            }
            inner.mark_tabs_dirty();
            self.emit_tab(&inner, id);
        }
        if open {
            self.start_stats(id)?;
        }
        Ok(())
    }

    /// Run SUMMARIZE for the tab's last statement unless cached. Returns the
    /// stats sequence number, or None when no run was needed.
    pub fn start_stats(self: &Arc<Self>, id: &str) -> Result<Option<u64>, CoreError> {
        let (conn, sql, seq, tab_ref) = {
            let mut inner = self.inner.lock();
            let generation = inner.generation;
            let t = inner.tab(id)?;
            let Some(stmt) = t.statement.clone() else { return Ok(None) };
            let wrapped = t.result.as_ref().map(|r| r.wrapped).unwrap_or(false);
            if !wrapped {
                let tab = inner.tab_mut(id)?;
                tab.stats.error = Some("No stats for this statement".into());
                tab.stats.status = RunStatus::Error;
                tab.stats.version += 1;
                self.emit_tab(&inner, id);
                return Ok(None);
            }
            let key = (stmt.clone(), generation);
            if t.stats.key.as_ref() == Some(&key) && (t.stats.result.is_some() || t.stats.status == RunStatus::Running) {
                return Ok(None);
            }
            let conn = inner.conn_for(id, true)?;
            let tab = inner.tab_mut(id)?;
            tab.stats.seq += 1;
            tab.stats.status = RunStatus::Running;
            tab.stats.key = Some(key);
            tab.stats.error = None;
            tab.stats.version += 1;
            let r = (conn, wrap::summarize_sql(&stmt), tab.stats.seq, tab.tab_ref());
            self.emit_tab(&inner, id);
            r
        };
        let core = self.clone();
        let id = id.to_string();
        self.rt.spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                let g = conn.conn.lock();
                db::run(&g, db::RunOptions { sql: &sql, last_only: false, sort: &[], limit: 0, force: true })
            })
            .await;
            let mut inner = core.inner.lock();
            let Ok(tab) = inner.tab_mut(&id) else { return };
            if tab.stats.seq != seq {
                return;
            }
            let (status, error) = match res {
                Ok(Ok(r)) => {
                    tab.stats.result = Some(Arc::new(r));
                    tab.stats.status = RunStatus::Done;
                    ("done", None)
                }
                Ok(Err(e)) => {
                    let msg = if e.code == "interrupted" { "Stats interrupted".to_string() } else { e.message };
                    tab.stats.error = Some(msg.clone());
                    tab.stats.status = RunStatus::Error;
                    tab.stats.key = None;
                    ("error", Some(msg))
                }
                Err(e) => {
                    tab.stats.error = Some(e.to_string());
                    tab.stats.status = RunStatus::Error;
                    tab.stats.key = None;
                    ("error", Some(e.to_string()))
                }
            };
            tab.stats.version += 1;
            core.emit_tab(&inner, &id);
            drop(inner);
            core.broadcast(Event::StatsFinished { tab: tab_ref, seq, status: status.into(), error });
        });
        Ok(Some(seq))
    }

    /// Histogram of one column over the tab's last statement. Blocking; runs
    /// on its own connection so it does not wait behind SUMMARIZE.
    pub fn histogram(&self, id: &str, column: &str, ty: &str) -> Result<(wrap::HistogramKind, QueryResult), CoreError> {
        let (stmt, source) = {
            let inner = self.inner.lock();
            let t = inner.tab(id)?;
            let Some(stmt) = t.statement.clone() else { return Err(CoreError::new("no_result", "Run a query first")) };
            (stmt, inner.source.clone())
        };
        let conn = source.conn.lock().try_clone().map_err(|e| CoreError::new("source_error", format!("cannot open connection: {e}")))?;
        let kind = wrap::histogram_kind(ty);
        let sql = wrap::histogram_sql(&stmt, column, kind);
        let r = db::run(&conn, db::RunOptions { sql: &sql, last_only: false, sort: &[], limit: 0, force: true })
            .map_err(|e| CoreError::new("query_error", e.message))?;
        Ok((kind, r))
    }

    pub fn stats_state(&self, id: &str) -> Result<(RunStatus, Option<Arc<QueryResult>>, Option<String>), CoreError> {
        let inner = self.inner.lock();
        let t = inner.tab(id)?;
        Ok((t.stats.status, t.stats.result.clone(), t.stats.error.clone()))
    }

    // ---- source and catalog ----

    pub fn catalog(&self) -> Result<Vec<db::CatalogTable>, CoreError> {
        let source = self.inner.lock().source.clone();
        let conn = source.conn.lock().try_clone().map_err(|e| CoreError::new("source_error", e.to_string()))?;
        db::catalog(&conn).map_err(|e| CoreError::new("query_error", e.message))
    }

    /// Open a data source chosen in the UI. Blocking; call off the main thread.
    pub fn open_source(&self, spec: SourceSpec) -> Result<SourceInfo, CoreError> {
        let (config, ws) = {
            let inner = self.inner.lock();
            (inner.config.clone(), inner.workspace.clone())
        };
        let opened = db::open_source(&spec, &config, ws.as_deref()).map_err(|e| CoreError::new("source_error", e.to_string()))?;
        self.install_source(opened, None);
        Ok(self.source_info())
    }

    fn install_source(&self, opened: db::OpenedSource, error: Option<String>) {
        let mut inner = self.inner.lock();
        inner.generation += 1;
        if let SourceSpec::File { path } = &opened.info.spec {
            let recents = &mut inner.app_state.recents;
            recents.retain(|r| r != path);
            recents.insert(0, path.clone());
            recents.truncate(10);
            inner.app_state_dirty = true;
        }
        let info = opened.info.clone();
        inner.source = Arc::new(SourceState { info: opened.info, conn: Mutex::new(opened.conn) });
        inner.source_error = error;
        for t in inner.tabs.iter_mut() {
            t.conn = None;
            t.side_conn = None;
            t.stats.key = None;
            t.stats.result = None;
            t.stats.status = RunStatus::Idle;
            t.stats.version += 1;
        }
        self.emit_ui("source", &serde_json::json!({ "source": info, "recents": inner.app_state.recents }));
        self.emit_tabs(&inner);
        self.emit_status(&inner);
        drop(inner);
        self.broadcast(Event::SourceChanged { source: info });
    }

    pub fn history(&self, limit: usize) -> Vec<serde_json::Value> {
        let Ok(text) = std::fs::read_to_string(paths::history_file()) else { return vec![] };
        let mut out: Vec<serde_json::Value> = Vec::new();
        for line in text.lines().rev() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                let sql = v.get("sql").and_then(|s| s.as_str()).unwrap_or("");
                if out.iter().any(|o| o.get("sql").and_then(|s| s.as_str()) == Some(sql)) {
                    continue;
                }
                out.push(v);
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }

    pub fn layout(&self) -> serde_json::Value {
        self.inner.lock().app_state.layout.clone()
    }

    pub fn save_layout(&self, layout: serde_json::Value) {
        let mut inner = self.inner.lock();
        inner.app_state.layout = layout;
        inner.app_state_dirty = true;
    }

    // ---- config ----

    pub fn reload_config(self: &Arc<Self>) {
        let (ws, old) = {
            let inner = self.inner.lock();
            (inner.workspace.clone(), inner.config.clone())
        };
        let loaded = load_config(&self.global_config, ws.as_deref());
        if !loaded.errors.is_empty() {
            let mut inner = self.inner.lock();
            inner.config_errors = loaded.errors.clone();
            inner.trust = loaded.trust.clone();
            self.emit_status(&inner);
            drop(inner);
            self.broadcast(Event::ConfigError { errors: loaded.errors });
            return;
        }
        let new = loaded.config;
        let had_errors;
        {
            let mut inner = self.inner.lock();
            had_errors = !inner.config_errors.is_empty();
            inner.config = new.clone();
            inner.config_errors.clear();
            inner.trust = loaded.trust.clone();
            self.emit_ui("config", &serde_json::json!({
                "config": new,
                "connections": new.connections.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
            }));
            self.emit_status(&inner);
        }
        if old == new && !had_errors {
            return;
        }
        self.broadcast(Event::ConfigReloaded);
        if old.server != new.server {
            crate::api::apply_config(self);
        }
        // Approving the workspace config can make its default connection available.
        let source = self.source_info().spec;
        if matches!(source, SourceSpec::Memory) && old.default_connection != new.default_connection {
            if let Some(name) = new.default_connection.clone() {
                let core = self.clone();
                self.rt.spawn_blocking(move || {
                    let _ = core.open_source(SourceSpec::Configured { name });
                });
            }
        }
        // Reconnect when the active configured connection changed or vanished.
        let spec = self.source_info().spec;
        if let SourceSpec::Configured { name } = &spec {
            let before = old.connections.iter().find(|c| &c.name == name);
            let after = new.connections.iter().find(|c| &c.name == name);
            if before != after {
                let core = self.clone();
                let spec = if after.is_some() { spec.clone() } else { SourceSpec::Memory };
                self.rt.spawn_blocking(move || {
                    if let Err(e) = core.open_source(spec) {
                        let fallback = db::open_source(&SourceSpec::Memory, &core.config(), None).expect("memory");
                        core.install_source(fallback, Some(e.message));
                    }
                });
            }
        }
        // A changed default limit re-runs the active tab if it follows the default and was truncated.
        if old.query.limit != new.query.limit {
            let id = self.active_id();
            let rerun = {
                let inner = self.inner.lock();
                inner.tab(&id).map(|t| t.limit.is_none() && t.result.as_ref().map(|r| r.truncated && r.wrapped).unwrap_or(false)).unwrap_or(false)
            };
            if rerun {
                let _ = self.run(&id, RunRequest { text: None, origin: Origin::File, force: false, last_only: true });
            }
            let inner = self.inner.lock();
            self.emit_tabs(&inner);
        }
    }

    /// Contents and hash of the workspace config, for the approval dialog.
    pub fn workspace_config(&self) -> Result<(String, String, String), CoreError> {
        let path = self.workspace_config_file().ok_or_else(|| CoreError::new("not_found", "no workspace is open"))?;
        let text = std::fs::read_to_string(&path).map_err(|e| CoreError::new("not_found", format!("{}: {e}", path.display())))?;
        let hash = config::sha256_hex(text.as_bytes());
        Ok((path.display().to_string(), text, hash))
    }

    /// Approve the workspace config whose contents hash to `hash`. Fails if the
    /// file changed since the user reviewed it. Not reachable from the HTTP API.
    pub fn trust_workspace_config(self: &Arc<Self>, hash: &str) -> Result<(), CoreError> {
        let (path, _, current) = self.workspace_config()?;
        if current != hash {
            return Err(CoreError::new("changed", "the workspace config changed after it was shown; review it again"));
        }
        config::set_trusted(Path::new(&path), hash).map_err(|e| CoreError::new("io_error", e.to_string()))?;
        self.reload_config();
        Ok(())
    }

    // ---- query.sql sync and persistence ----

    fn adopt_query_file_on_start(&self) {
        let path = self.query_file();
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let id = self.active_id();
        let mut inner = self.inner.lock();
        inner.query_file_hash = Some(paths::fnv1a(text.as_bytes()));
        if let Ok(tab) = inner.tab_mut(&id) {
            if !text.trim().is_empty() && tab.sql != text {
                tab.sql = text;
                tab.sql_rev += 1;
                tab.sql_origin = Origin::File;
                tab.last_change = Origin::File;
                if !tab.custom_title {
                    if let Some(t) = default_title(&tab.sql) {
                        tab.title = t;
                    }
                }
            }
        }
    }

    /// Called by the watcher when `query.sql` changes on disk.
    pub fn on_query_file_changed(&self) {
        let path = self.query_file();
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let h = paths::fnv1a(text.as_bytes());
        let id = {
            let mut inner = self.inner.lock();
            if inner.query_file_hash == Some(h) {
                return; // our own write
            }
            inner.query_file_hash = Some(h);
            inner.active.clone()
        };
        let _ = self.set_sql(&id, text, Origin::File);
    }

    /// Flush pending writes. Called periodically and on exit.
    pub fn flush(&self, force: bool) {
        let now = Instant::now();
        let mut inner = self.inner.lock();
        if let Some(due) = inner.query_file_due {
            if force || now >= due {
                inner.query_file_due = None;
                let text = inner.tab(&inner.active).map(|t| t.sql.clone()).unwrap_or_default();
                let h = paths::fnv1a(text.as_bytes());
                if inner.query_file_hash != Some(h) {
                    inner.query_file_hash = Some(h);
                    let p = paths::query_file(inner.workspace.as_deref());
                    let _ = paths::write_atomic(&p, text.as_bytes());
                }
            }
        }
        if let Some(since) = inner.tabs_dirty_since {
            if force || now.duration_since(since) >= Duration::from_millis(1000) {
                inner.tabs_dirty_since = None;
                let p = PersistedTabs {
                    workspace: inner.workspace.as_ref().map(|w| w.display().to_string()),
                    active: Some(inner.active.clone()),
                    next_id: inner.next_id,
                    tabs: inner
                        .tabs
                        .iter()
                        .map(|t| PersistedTab {
                            id: t.id.clone(),
                            title: t.title.clone(),
                            custom_title: t.custom_title,
                            sql: t.sql.clone(),
                            sort: t.sort.clone(),
                            limit: t.limit,
                            stats_open: t.stats_open,
                            last_row_count: t.result.as_ref().map(|r| r.rows.len()).or(t.last_row_count),
                            truncated: t.result.as_ref().map(|r| r.truncated).unwrap_or(t.last_truncated),
                        })
                        .collect(),
                };
                let path = paths::tabs_file(inner.workspace.as_deref());
                if let Ok(s) = serde_json::to_vec_pretty(&p) {
                    let _ = paths::write_atomic(&path, &s);
                }
            }
        }
        if inner.app_state_dirty {
            inner.app_state_dirty = false;
            if let Ok(s) = serde_json::to_vec_pretty(&inner.app_state) {
                let _ = paths::write_atomic(&paths::state_file(), &s);
            }
        }
    }

    pub fn spawn_background(self: &Arc<Self>) {
        let core = self.clone();
        self.rt.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(250));
            loop {
                tick.tick().await;
                core.flush(false);
            }
        });
    }

    /// Wait until a run with `seq` on tab `id` finishes. The receiver must be
    /// subscribed before the run starts.
    pub async fn wait_run(rx: &mut broadcast::Receiver<Event>, id: &str, seq: u64, timeout: Duration) -> bool {
        let fut = async {
            loop {
                match rx.recv().await {
                    Ok(Event::RunFinished { tab, seq: s, .. }) if tab.id == id && s >= seq => return true,
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => return false,
                }
            }
        };
        tokio::time::timeout(timeout, fut).await.unwrap_or(false)
    }
}

fn append_history(sql: &str, origin: &str) {
    use std::io::Write;
    let path = paths::history_file();
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = serde_json::json!({ "ts": ts, "sql": sql, "origin": origin });
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(default_title("select * from orders o").as_deref(), Some("orders"));
        assert_eq!(default_title("FROM 'data/events.parquet'").as_deref(), Some("events"));
        assert_eq!(default_title("select 1; from main.users").as_deref(), Some("users"));
        assert_eq!(default_title("select 1"), None);
        assert_eq!(default_title("select fromage from cheese").as_deref(), Some("cheese"));
    }
}
