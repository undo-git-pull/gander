//! File locations. XDG style paths are used on every platform so agents can
//! find them without platform logic.

use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("GANDER_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("gander");
    }
    home().join(".config").join("gander")
}

pub fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("GANDER_STATE_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(dir).join("gander");
    }
    home().join(".local").join("state").join("gander")
}

pub fn global_config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn schema_file() -> PathBuf {
    config_dir().join("schema.json")
}

pub fn state_file() -> PathBuf {
    state_dir().join("state.json")
}

pub fn server_file() -> PathBuf {
    state_dir().join("server.json")
}

/// Hashes of workspace config files the user approved.
pub fn trust_file() -> PathBuf {
    state_dir().join("trusted_workspaces.json")
}

pub fn history_file() -> PathBuf {
    state_dir().join("history.jsonl")
}

/// Directory holding workspace-level config and `query.sql`.
pub fn workspace_dot_dir(workspace: Option<&Path>) -> PathBuf {
    match workspace {
        Some(ws) => ws.join(".gander"),
        None => config_dir(),
    }
}

pub fn workspace_config_file(workspace: Option<&Path>) -> Option<PathBuf> {
    workspace.map(|ws| ws.join(".gander").join("config.toml"))
}

pub fn query_file(workspace: Option<&Path>) -> PathBuf {
    workspace_dot_dir(workspace).join("query.sql")
}

/// Per-workspace tab storage, keyed by a hash of the workspace path.
pub fn tabs_file(workspace: Option<&Path>) -> PathBuf {
    let key = match workspace {
        Some(ws) => format!("{:016x}", fnv1a(ws.to_string_lossy().as_bytes())),
        None => "default".to_string(),
    };
    state_dir().join("workspaces").join(key).join("tabs.json")
}

/// Expand a leading `~` and resolve relative paths against `base`.
pub fn expand(path: &str, base: Option<&Path>) -> PathBuf {
    let p = if path == "~" {
        home()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(path)
    };
    if p.is_relative() {
        if let Some(b) = base {
            return b.join(p);
        }
    }
    p
}

/// Small stable hash (FNV-1a 64). Used for workspace keys and content hashes.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Write a file atomically (write to a temp file in the same dir, then rename).
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp{}",
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        std::process::id()
    ));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

/// Write a file readable only by the current user (mode 0600 on unix).
pub fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
