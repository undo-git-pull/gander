//! Watches config files and `query.sql` and applies changes live.

use crate::core::Core;
use crate::paths;
use notify_debouncer_full::notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub type Handle = notify_debouncer_full::Debouncer<notify_debouncer_full::notify::RecommendedWatcher, notify_debouncer_full::RecommendedCache>;

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

pub fn start(core: Arc<Core>) -> anyhow::Result<Handle> {
    let config_dir = paths::config_dir();
    let ws_dir = paths::workspace_dot_dir(core.workspace().as_deref());
    std::fs::create_dir_all(&config_dir)?;
    std::fs::create_dir_all(&ws_dir)?;
    let config_c = canon(&config_dir);
    let ws_c = canon(&ws_dir);
    let query_c = canon(&ws_dir).join("query.sql");

    let handler_core = core.clone();
    let mut debouncer = new_debouncer(Duration::from_millis(150), None, move |res: DebounceEventResult| {
        let Ok(events) = res else { return };
        let mut config_changed = false;
        let mut query_changed = false;
        for ev in events {
            for p in &ev.paths {
                let parent = p.parent().map(canon).unwrap_or_default();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "config.toml" && (parent == config_c || parent == ws_c) {
                    config_changed = true;
                }
                if name == "query.sql" && parent.join("query.sql") == query_c {
                    query_changed = true;
                }
            }
        }
        if config_changed {
            handler_core.reload_config();
        }
        if query_changed {
            handler_core.on_query_file_changed();
        }
    })?;
    debouncer.watch(&config_dir, RecursiveMode::NonRecursive)?;
    if canon(&ws_dir) != canon(&config_dir) {
        debouncer.watch(&ws_dir, RecursiveMode::NonRecursive)?;
    }
    Ok(debouncer)
}
