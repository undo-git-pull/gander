//! Startup cleanup for files under the state directory that would otherwise
//! grow without bound: the query history log and per-workspace tab folders.

use crate::paths;
use serde::Deserialize;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Entries kept in `history.jsonl` after compaction.
pub const HISTORY_KEEP: usize = 5000;
/// Compaction only runs once the log exceeds this many lines, so it is not
/// rewritten on every launch while hovering around the cap.
pub const HISTORY_COMPACT_AT: usize = HISTORY_KEEP + HISTORY_KEEP / 4;

/// A workspace whose recorded path no longer exists is removed once its tabs
/// have been untouched this long. The grace period covers unmounted volumes.
pub const MISSING_PATH_GRACE: Duration = Duration::from_secs(30 * 24 * 3600);
/// Folders written before the workspace path was recorded are removed once
/// untouched this long, since there is no way to check their project.
pub const LEGACY_MAX_AGE: Duration = Duration::from_secs(90 * 24 * 3600);

/// Run all cleanup against the real state directory.
pub fn run(current_workspace: Option<&Path>) {
    compact_history(&paths::history_file(), HISTORY_KEEP, HISTORY_COMPACT_AT);
    let current = paths::tabs_file(current_workspace);
    let current_dir = current.parent().map(Path::to_path_buf);
    prune_workspaces(&paths::state_dir().join("workspaces"), current_dir.as_deref(), SystemTime::now());
}

/// Keep only the newest `keep` lines of the history log once it holds more
/// than `threshold` lines. Returns true when the file was rewritten.
pub fn compact_history(path: &Path, keep: usize, threshold: usize) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() <= threshold {
        return false;
    }
    let mut out = lines[lines.len() - keep..].join("\n");
    out.push('\n');
    paths::write_atomic(path, out.as_bytes()).is_ok()
}

#[derive(Deserialize, Default)]
struct WorkspaceMarker {
    #[serde(default)]
    workspace: Option<String>,
}

/// Remove tab folders for workspaces that are gone or long abandoned.
/// `default` (no workspace) and `current` are never removed. Returns the
/// number of folders deleted.
pub fn prune_workspaces(root: &Path, current: Option<&Path>, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() || entry.file_name() == "default" || current == Some(dir.as_path()) {
            continue;
        }
        let tabs = dir.join("tabs.json");
        let age = std::fs::metadata(&tabs)
            .or_else(|_| std::fs::metadata(&dir))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .unwrap_or_default();
        let marker: WorkspaceMarker = std::fs::read_to_string(&tabs)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let stale = match marker.workspace {
            Some(ws) => !Path::new(&ws).is_dir() && age >= MISSING_PATH_GRACE,
            None => age >= LEGACY_MAX_AGE,
        };
        if stale && std::fs::remove_dir_all(&dir).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, s: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, s).unwrap();
    }

    #[test]
    fn history_compacts_only_past_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("history.jsonl");
        let body: String = (0..12).map(|i| format!("{{\"sql\":\"q{i}\"}}\n")).collect();
        write(&p, &body);
        assert!(!compact_history(&p, 5, 12));
        assert!(compact_history(&p, 5, 10));
        let text = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "{\"sql\":\"q7\"}");
        assert_eq!(lines[4], "{\"sql\":\"q11\"}");
    }

    #[test]
    fn history_missing_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!compact_history(&dir.path().join("nope.jsonl"), 5, 10));
    }

    #[test]
    fn prunes_missing_and_legacy_workspaces() {
        let root = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let r = root.path();
        let live = format!("{{\"workspace\":{:?},\"tabs\":[]}}", project.path().display().to_string());
        write(&r.join("live/tabs.json"), &live);
        write(&r.join("gone/tabs.json"), "{\"workspace\":\"/definitely/not/here\",\"tabs\":[]}");
        write(&r.join("legacy/tabs.json"), "{\"tabs\":[]}");
        write(&r.join("default/tabs.json"), "{\"tabs\":[]}");
        write(&r.join("current/tabs.json"), "{\"workspace\":\"/also/not/here\",\"tabs\":[]}");

        // Fresh files: nothing is old enough yet.
        let now = SystemTime::now();
        assert_eq!(prune_workspaces(r, Some(&r.join("current")), now), 0);

        // 45 days later: the missing-path folder goes, legacy is still in grace.
        let later = now + Duration::from_secs(45 * 24 * 3600);
        assert_eq!(prune_workspaces(r, Some(&r.join("current")), later), 1);
        assert!(!r.join("gone").exists());
        assert!(r.join("legacy").exists());

        // 100 days later: legacy goes too; live, default and current stay.
        let much_later = now + Duration::from_secs(100 * 24 * 3600);
        assert_eq!(prune_workspaces(r, Some(&r.join("current")), much_later), 1);
        assert!(!r.join("legacy").exists());
        assert!(r.join("live").exists());
        assert!(r.join("default").exists());
        assert!(r.join("current").exists());
    }
}
