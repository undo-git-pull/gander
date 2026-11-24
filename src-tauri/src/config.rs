//! Configuration: TOML files, merged global then workspace, validated with
//! serde (unknown keys are errors) and described by a generated JSON Schema.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Name of the connection opened on start. Omit to start in files-only mode.
    /// Must appear before any `[[connections]]` table in the file.
    pub default_connection: Option<String>,
    pub server: ServerConfig,
    pub ui: UiConfig,
    pub editor: EditorConfig,
    pub query: QueryConfig,
    pub keybindings: Keybindings,
    pub connections: Vec<ConnectionConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_connection: None,
            server: ServerConfig::default(),
            ui: UiConfig::default(),
            editor: EditorConfig::default(),
            query: QueryConfig::default(),
            keybindings: Keybindings::default(),
            connections: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Run the localhost HTTP API.
    pub enabled: bool,
    /// Port on 127.0.0.1 and [::1]. 0 picks a free port; the real one is written to server.json.
    pub port: u16,
    /// Literal API key. Takes precedence over `api_key_env`.
    pub api_key: Option<String>,
    /// Name of an environment variable holding the API key.
    pub api_key_env: Option<String>,
    /// Rows returned per API response when the request omits `rows`.
    pub default_rows: usize,
    /// Upper bound a request can ask for with `rows`.
    pub max_rows: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 7420,
            api_key: None,
            api_key_env: None,
            default_rows: 50,
            max_rows: 10_000,
        }
    }
}

impl ServerConfig {
    pub fn resolved_api_key(&self) -> Option<String> {
        if let Some(k) = &self.api_key {
            if !k.is_empty() {
                return Some(k.clone());
            }
        }
        if let Some(var) = &self.api_key_env {
            if let Ok(k) = std::env::var(var) {
                if !k.is_empty() {
                    return Some(k);
                }
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    Compact,
    Cozy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RowFormat {
    Json,
    Tsv,
    Csv,
    Markdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    pub theme: Theme,
    pub density: Density,
    /// Base font size in px for grid, editor and UI text.
    pub font_size: u32,
    /// Characters shown per grid cell before cutting.
    pub max_cell_chars: usize,
    /// Format used by the row copy button.
    pub copy_row_format: RowFormat,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            density: Density::Compact,
            font_size: 12,
            max_cell_chars: 200,
            copy_row_format: RowFormat::Json,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct EditorConfig {
    /// Vim keybindings in the SQL editor.
    pub vim: bool,
    pub tab_size: u32,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            vim: false,
            tab_size: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct QueryConfig {
    /// Maximum rows held per result, applied by wrapping the query. 0 disables the limit.
    pub limit: usize,
    /// Interrupt queries after this many milliseconds. 0 means no timeout.
    pub timeout_ms: u64,
}

impl Default for QueryConfig {
    fn default() -> Self {
        Self {
            limit: 1000,
            timeout_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Keybindings {
    /// CodeMirror key name, e.g. "Mod-Enter". Mod is Cmd on macOS, Ctrl elsewhere.
    pub run: String,
    pub run_statement: String,
    pub palette: String,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            run: "Mod-Enter".into(),
            run_statement: "Shift-Mod-Enter".into(),
            palette: "Mod-k".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfig {
    pub name: String,
    /// Path to a .duckdb file, or ":memory:". `~` and relative paths (against the workspace) are expanded.
    pub path: String,
    #[serde(default)]
    pub read_only: bool,
    /// Extensions to INSTALL and LOAD after opening.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// SQL statements run after opening and attaching.
    #[serde(default)]
    pub init_sql: Vec<String>,
    #[serde(default)]
    pub attach: Vec<AttachConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AttachConfig {
    pub alias: String,
    pub path: String,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConfigError {
    pub file: String,
    pub line: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(l) => write!(f, "{}:{}: {}", self.file, l, self.message),
            None => write!(f, "{}: {}", self.file, self.message),
        }
    }
}

pub fn json_schema() -> String {
    let schema = schemars::schema_for!(Config);
    serde_json::to_string_pretty(&schema).unwrap_or_default()
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

fn read_file(path: &Path) -> Result<Option<String>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ConfigError {
            file: path.display().to_string(),
            line: None,
            message: e.to_string(),
        }),
    }
}

/// Keys a workspace config can only set once the user trusts that exact file,
/// because opening a connection runs its `init_sql`, extensions and attaches.
pub const TRUST_KEYS: &[&str] = &["connections"];
/// Keys read only from the global config. A workspace must not be able to
/// turn off the API key or move the server.
pub const GLOBAL_ONLY_KEYS: &[&str] = &["server"];

/// How much of the workspace config was applied.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct WorkspaceTrust {
    /// SHA-256 (hex) of the workspace config file, when it exists.
    pub hash: Option<String>,
    /// The file's current contents were approved by the user.
    pub trusted: bool,
    /// The file sets keys that only apply once it is trusted.
    pub needs_trust: bool,
    /// Top-level keys in the workspace file that were not applied.
    pub ignored: Vec<String>,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Remove keys the workspace file is not allowed to set. Returns the removed keys.
fn restrict_workspace(table: &mut toml::Table, trusted: bool) -> Vec<String> {
    let mut removed = Vec::new();
    for k in GLOBAL_ONLY_KEYS.iter().chain(if trusted {
        [].iter()
    } else {
        TRUST_KEYS.iter()
    }) {
        if let Some(v) = table.remove(*k) {
            removed.push(k.to_string());
            // A default pointing at a connection that was just dropped would
            // only show up as a confusing config error; drop it too.
            if *k == "connections" {
                let names: Vec<&str> = v
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c.get("name")?.as_str())
                    .collect();
                let points_at_dropped = table
                    .get("default_connection")
                    .and_then(|d| d.as_str())
                    .is_some_and(|d| names.contains(&d));
                if points_at_dropped {
                    table.remove("default_connection");
                    removed.push("default_connection".into());
                }
            }
        }
    }
    removed
}

pub fn parse_text(text: &str, file: &str) -> Result<toml::Table, ConfigError> {
    let mk = |e: toml::de::Error| ConfigError {
        file: file.to_string(),
        line: e.span().map(|s| line_of(text, s.start)),
        message: e.message().trim().to_string(),
    };
    // Validate the file on its own first so errors carry its line numbers.
    toml::from_str::<Config>(text).map_err(mk)?;
    toml::from_str::<toml::Table>(text).map_err(mk)
}

/// Deep merge `over` into `base`. Tables merge recursively; `connections`
/// merge by `name`; other arrays and scalars are replaced.
pub fn merge(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (Some(toml::Value::Array(b)), toml::Value::Array(o)) if k == "connections" => {
                for item in o {
                    let name = item.get("name").and_then(|n| n.as_str()).map(String::from);
                    let pos = name.as_ref().and_then(|n| {
                        b.iter()
                            .position(|x| x.get("name").and_then(|v| v.as_str()) == Some(n))
                    });
                    match pos {
                        Some(i) => b[i] = item,
                        None => b.push(item),
                    }
                }
            }
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

pub struct Loaded {
    pub config: Config,
    pub errors: Vec<ConfigError>,
    pub files: Vec<PathBuf>,
    pub trust: WorkspaceTrust,
}

/// Load global then workspace config. On any error the caller should keep the
/// previous good config; `config` is then the defaults merged with whichever
/// files did parse, which is only used at startup.
///
/// `trusted_hash` is the hash of the workspace file the user last approved.
/// The workspace file's `connections` apply only when its current hash
/// matches; its `server` table never applies.
pub fn load(global: &Path, workspace: Option<&Path>, trusted_hash: Option<&str>) -> Loaded {
    let mut errors = Vec::new();
    let mut merged = toml::Table::new();
    let mut files = vec![global.to_path_buf()];
    let mut trust = WorkspaceTrust::default();
    match read_file(global).and_then(|t| {
        t.map(|t| parse_text(&t, &global.display().to_string()))
            .transpose()
    }) {
        Ok(Some(t)) => merge(&mut merged, t),
        Ok(None) => {}
        Err(e) => errors.push(e),
    }
    if let Some(ws) = workspace {
        files.push(ws.to_path_buf());
        match read_file(ws) {
            Ok(Some(text)) => {
                // Hash and parse the same bytes so a file swapped after
                // approval is never applied as trusted.
                let hash = sha256_hex(text.as_bytes());
                trust.trusted = trusted_hash == Some(hash.as_str());
                trust.hash = Some(hash);
                match parse_text(&text, &ws.display().to_string()) {
                    Ok(mut t) => {
                        trust.needs_trust =
                            !trust.trusted && TRUST_KEYS.iter().any(|k| t.contains_key(*k));
                        trust.ignored = restrict_workspace(&mut t, trust.trusted);
                        merge(&mut merged, t);
                    }
                    Err(e) => errors.push(e),
                }
            }
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    let config = match Config::deserialize(toml::Value::Table(merged)) {
        Ok(c) => c,
        Err(e) => {
            errors.push(ConfigError {
                file: "merged config".into(),
                line: None,
                message: e.message().trim().to_string(),
            });
            Config::default()
        }
    };
    if let Some(name) = &config.default_connection {
        if !config.connections.iter().any(|c| &c.name == name) {
            errors.push(ConfigError {
                file: "merged config".into(),
                line: None,
                message: format!(
                    "default_connection \"{name}\" does not match any [[connections]] name"
                ),
            });
        }
    }
    let mut seen = std::collections::HashSet::new();
    for c in &config.connections {
        if !seen.insert(c.name.clone()) {
            errors.push(ConfigError {
                file: "merged config".into(),
                line: None,
                message: format!("duplicate connection name \"{}\"", c.name),
            });
        }
    }
    Loaded {
        config,
        errors,
        files,
        trust,
    }
}

/// Approved workspace config hashes, keyed by config file path.
fn read_trust_store() -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(crate::paths::trust_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn trust_key(ws_config: &Path) -> String {
    std::fs::canonicalize(ws_config)
        .unwrap_or_else(|_| ws_config.to_path_buf())
        .display()
        .to_string()
}

/// Hash of the workspace config contents the user last approved.
pub fn trusted_hash(ws_config: &Path) -> Option<String> {
    read_trust_store().remove(&trust_key(ws_config))
}

/// Record that the user approved `ws_config` with contents hashing to `hash`.
pub fn set_trusted(ws_config: &Path, hash: &str) -> std::io::Result<()> {
    let mut store = read_trust_store();
    store.insert(trust_key(ws_config), hash.to_string());
    let body = serde_json::to_string_pretty(&store).unwrap_or_default();
    crate::paths::write_private(&crate::paths::trust_file(), body.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty() {
        let t = parse_text("", "x").unwrap();
        let c = Config::deserialize(toml::Value::Table(t)).unwrap();
        assert_eq!(c, Config::default());
        assert_eq!(c.query.limit, 1000);
        assert_eq!(c.server.default_rows, 50);
    }

    #[test]
    fn unknown_key_reports_line() {
        let err = parse_text("[ui]\ntheme = \"dark\"\nbogus = 1\n", "cfg.toml").unwrap_err();
        assert_eq!(err.line, Some(3));
        assert!(err.message.contains("bogus"), "{}", err.message);
    }

    #[test]
    fn bad_enum_value() {
        let err = parse_text("[ui]\ntheme = \"purple\"\n", "cfg.toml").unwrap_err();
        assert_eq!(err.line, Some(2));
    }

    #[test]
    fn merge_overrides_and_connections_by_name() {
        let mut base = parse_text(
            "[ui]\ntheme = \"dark\"\nfont_size = 13\n[[connections]]\nname = \"a\"\npath = \"a.duckdb\"\n[[connections]]\nname = \"b\"\npath = \"b.duckdb\"\n",
            "g",
        )
        .unwrap();
        let over = parse_text(
            "[ui]\nfont_size = 11\n[[connections]]\nname = \"b\"\npath = \"b2.duckdb\"\n[[connections]]\nname = \"c\"\npath = \":memory:\"\n",
            "w",
        )
        .unwrap();
        merge(&mut base, over);
        let c = Config::deserialize(toml::Value::Table(base)).unwrap();
        assert_eq!(c.ui.theme, Theme::Dark);
        assert_eq!(c.ui.font_size, 11);
        let names: Vec<_> = c
            .connections
            .iter()
            .map(|c| (c.name.as_str(), c.path.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![("a", "a.duckdb"), ("b", "b2.duckdb"), ("c", ":memory:")]
        );
    }

    #[test]
    fn load_reports_missing_default_connection() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path().join("config.toml");
        std::fs::write(&g, "default_connection = \"nope\"\n").unwrap();
        let l = load(&g, None, None);
        assert_eq!(l.errors.len(), 1);
        assert!(l.errors[0].message.contains("nope"));
    }

    #[test]
    fn workspace_connections_need_trust_and_server_is_global_only() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path().join("global.toml");
        let w = dir.path().join("ws.toml");
        std::fs::write(
            &g,
            "[server]\napi_key = \"global\"\n[[connections]]\nname = \"g\"\npath = \":memory:\"\n",
        )
        .unwrap();
        let ws_text = "default_connection = \"x\"\n[server]\napi_key = \"\"\nport = 1\n[ui]\nfont_size = 14\n[[connections]]\nname = \"x\"\npath = \":memory:\"\ninit_sql = [\"select 1\"]\n";
        std::fs::write(&w, ws_text).unwrap();

        let l = load(&g, Some(&w), None);
        assert_eq!(l.config.server.api_key.as_deref(), Some("global"));
        assert_eq!(l.config.server.port, 7420);
        assert_eq!(l.config.ui.font_size, 14);
        assert_eq!(
            l.config
                .connections
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["g"]
        );
        assert!(l.trust.needs_trust && !l.trust.trusted);
        assert_eq!(
            l.trust.ignored,
            vec!["server", "connections", "default_connection"]
        );
        assert_eq!(l.config.default_connection, None);
        assert!(l.errors.is_empty(), "{:?}", l.errors);

        let hash = sha256_hex(ws_text.as_bytes());
        let l = load(&g, Some(&w), Some(&hash));
        assert!(l.trust.trusted && !l.trust.needs_trust);
        assert_eq!(l.config.default_connection.as_deref(), Some("x"));
        assert_eq!(l.config.server.api_key.as_deref(), Some("global"));
        assert_eq!(l.config.connections.len(), 2);
        assert!(l.errors.is_empty(), "{:?}", l.errors);

        // Any edit after approval drops the trust.
        std::fs::write(&w, format!("{ws_text}# changed\n")).unwrap();
        let l = load(&g, Some(&w), Some(&hash));
        assert!(!l.trust.trusted && l.trust.needs_trust);
        assert_eq!(l.config.connections.len(), 1);
    }

    #[test]
    fn api_key_resolution() {
        let mut s = ServerConfig::default();
        assert_eq!(s.resolved_api_key(), None);
        s.api_key = Some("k".into());
        assert_eq!(s.resolved_api_key().as_deref(), Some("k"));
    }

    #[test]
    fn schema_is_json() {
        let s = json_schema();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v.get("properties").is_some());
    }
}
