//! Tauri commands for the frontend. Thin wrappers over `Core`.

use crate::core::{Core, Origin, RunRequest, Selection, Snapshot};
use crate::db::{SourceSpec, wrap, wrap::SortSpec};
use serde_json::Value;
use std::sync::Arc;
use tauri::State;

type C<'a> = State<'a, Arc<Core>>;
type R<T> = Result<T, String>;

#[tauri::command]
pub fn get_snapshot(core: C) -> Snapshot {
    core.snapshot()
}

#[tauri::command]
pub fn new_tab(core: C, sql: Option<String>, run: Option<bool>) -> R<String> {
    let id = core.new_tab(sql, true);
    if run.unwrap_or(false) {
        core.run(
            &id,
            RunRequest {
                text: None,
                origin: Origin::User,
                force: false,
                last_only: false,
            },
        )?;
    }
    Ok(id)
}

#[tauri::command]
pub fn close_tab(core: C, id: String) -> R<()> {
    Ok(core.close_tab(&id)?)
}

#[tauri::command]
pub fn activate_tab(core: C, id: String) -> R<()> {
    Ok(core.activate_tab(&id)?)
}

#[tauri::command]
pub fn move_tab(core: C, id: String, to: usize) -> R<()> {
    Ok(core.move_tab(&id, to)?)
}

#[tauri::command]
pub fn rename_tab(core: C, id: String, title: String) -> R<()> {
    Ok(core.rename_tab(&id, &title)?)
}

#[tauri::command]
pub fn set_sql(core: C, id: String, sql: String) -> R<()> {
    Ok(core.set_sql(&id, sql, Origin::User)?)
}

#[tauri::command]
pub fn run_tab(core: C, id: String, text: Option<String>, force: Option<bool>) -> R<u64> {
    Ok(core.run(
        &id,
        RunRequest {
            text,
            origin: Origin::User,
            force: force.unwrap_or(false),
            last_only: false,
        },
    )?)
}

#[tauri::command]
pub fn rerun_tab(core: C, id: String, force: Option<bool>) -> R<u64> {
    Ok(core.run(
        &id,
        RunRequest {
            text: None,
            origin: Origin::User,
            force: force.unwrap_or(false),
            last_only: true,
        },
    )?)
}

#[tauri::command]
pub fn stop_tab(core: C, id: String) -> R<()> {
    Ok(core.stop(&id)?)
}

#[tauri::command]
pub fn set_sort(core: C, id: String, sort: Vec<SortSpec>) -> R<()> {
    core.set_sort(&id, sort, Origin::User)?;
    Ok(())
}

#[tauri::command]
pub fn set_limit(core: C, id: String, limit: Option<usize>, force: Option<bool>) -> R<()> {
    core.set_limit(&id, limit, Origin::User, force.unwrap_or(false))?;
    Ok(())
}

#[tauri::command]
pub fn count_rows(core: C, id: String) -> R<()> {
    Ok(core.count_rows(&id)?)
}

#[tauri::command]
pub fn get_result(core: C, id: String) -> R<Value> {
    match core.get_result(&id)? {
        Some(r) => serde_json::to_value(&*r).map_err(|e| e.to_string()),
        None => Ok(Value::Null),
    }
}

#[tauri::command]
pub fn get_stats(core: C, id: String) -> R<Value> {
    match core.get_stats(&id)? {
        Some(r) => serde_json::to_value(&*r).map_err(|e| e.to_string()),
        None => Ok(Value::Null),
    }
}

#[tauri::command]
pub async fn column_histogram(
    core: State<'_, Arc<Core>>,
    id: String,
    column: String,
    ty: String,
) -> R<Value> {
    let core = core.inner().clone();
    let (kind, r) = tokio::task::spawn_blocking(move || core.histogram(&id, &column, &ty))
        .await
        .map_err(|e| e.to_string())??;
    Ok(serde_json::json!({ "kind": kind, "result": r }))
}

#[tauri::command]
pub fn set_stats_open(core: C, id: String, open: bool) -> R<()> {
    Ok(core.set_stats_open(&id, open)?)
}

#[tauri::command]
pub fn set_selection(core: C, id: String, selection: Option<Selection>) -> R<()> {
    Ok(core.set_selection(&id, selection)?)
}

#[tauri::command]
pub async fn get_catalog(core: State<'_, Arc<Core>>) -> R<Value> {
    let core = core.inner().clone();
    let tables = tokio::task::spawn_blocking(move || core.catalog())
        .await
        .map_err(|e| e.to_string())??;
    serde_json::to_value(tables).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_source(core: State<'_, Arc<Core>>, spec: SourceSpec) -> R<Value> {
    let core = core.inner().clone();
    let info = tokio::task::spawn_blocking(move || core.open_source(spec))
        .await
        .map_err(|e| e.to_string())??;
    serde_json::to_value(info).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn split_statements(sql: String) -> Vec<wrap::StatementRange> {
    wrap::split_with_ranges(&sql)
}

#[tauri::command]
pub fn get_history(core: C, limit: Option<usize>) -> Vec<Value> {
    core.history(limit.unwrap_or(300))
}

#[tauri::command]
pub fn get_layout(core: C) -> Value {
    core.layout()
}

#[tauri::command]
pub fn save_layout(core: C, layout: Value) {
    core.save_layout(layout)
}

#[tauri::command]
pub fn get_workspace_config(core: C) -> R<Value> {
    let (path, text, hash) = core.workspace_config()?;
    Ok(serde_json::json!({ "path": path, "text": text, "hash": hash }))
}

/// Approve the workspace config the user just reviewed. `hash` must match the
/// file's current contents. Only the webview can call this; the HTTP API has
/// no equivalent, so an agent cannot approve its own edits.
#[tauri::command]
pub async fn trust_workspace_config(core: State<'_, Arc<Core>>, hash: String) -> R<()> {
    let core = core.inner().clone();
    tokio::task::spawn_blocking(move || core.trust_workspace_config(&hash))
        .await
        .map_err(|e| e.to_string())??;
    Ok(())
}
