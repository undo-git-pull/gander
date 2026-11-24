pub mod api;
pub mod commands;
pub mod config;
pub mod core;
pub mod db;
pub mod fuzzy;
pub mod housekeeping;
pub mod paths;
pub mod watcher;

use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager};

struct TauriSink(tauri::AppHandle);

impl core::UiSink for TauriSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.0.emit(event, payload);
    }
}

/// Workspace: first CLI argument that is a directory, else the last one used.
fn workspace_from_args() -> Option<PathBuf> {
    std::env::args()
        .skip(1)
        .map(PathBuf::from)
        .find(|p| p.is_dir())
        .and_then(|p| std::fs::canonicalize(p).ok())
        .or_else(core::last_workspace)
}

/// App menu without "Close Window" so Cmd+W reaches the webview and closes a tab.
/// The Edit items keep clipboard and undo shortcuts working in the webview.
fn build_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, PredefinedMenuItem as P, Submenu};
    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                "Gander",
                true,
                &[
                    &P::about(app, None, None)?,
                    &P::separator(app)?,
                    &P::hide(app, None)?,
                    &P::hide_others(app, None)?,
                    &P::show_all(app, None)?,
                    &P::separator(app)?,
                    &P::quit(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &P::undo(app, None)?,
                    &P::redo(app, None)?,
                    &P::separator(app)?,
                    &P::cut(app, None)?,
                    &P::copy(app, None)?,
                    &P::paste(app, None)?,
                    &P::select_all(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "Window",
                true,
                &[
                    &P::minimize(app, None)?,
                    &P::maximize(app, None)?,
                    &P::fullscreen(app, None)?,
                ],
            )?,
        ],
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.set_menu(build_menu(app.handle())?)?;
            let workspace = workspace_from_args();
            if let Some(ws) = &workspace {
                // Relative file paths in queries resolve against the workspace.
                let _ = std::env::set_current_dir(ws);
            }
            let core = core::Core::new(core::CoreOptions {
                workspace,
                ui: Box::new(TauriSink(app.handle().clone())),
                rt: tauri::async_runtime::handle().inner().clone(),
                ephemeral: false,
            });
            core.spawn_background();
            {
                let core = core.clone();
                tauri::async_runtime::spawn(async move { api::apply_config(&core) });
            }
            match watcher::start(core.clone()) {
                Ok(w) => {
                    app.manage(std::sync::Mutex::new(Some(w)));
                }
                Err(e) => eprintln!("gander: file watching disabled: {e}"),
            }
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::new_tab,
            commands::close_tab,
            commands::activate_tab,
            commands::move_tab,
            commands::rename_tab,
            commands::set_sql,
            commands::run_tab,
            commands::rerun_tab,
            commands::stop_tab,
            commands::set_sort,
            commands::set_limit,
            commands::count_rows,
            commands::get_result,
            commands::get_stats,
            commands::set_stats_open,
            commands::column_histogram,
            commands::set_selection,
            commands::get_catalog,
            commands::open_source,
            commands::split_statements,
            commands::get_history,
            commands::get_layout,
            commands::save_layout,
            commands::get_workspace_config,
            commands::trust_workspace_config,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(core) = app.try_state::<Arc<core::Core>>() {
                    core.flush(true);
                    api::shutdown(&core);
                }
            }
        });
}
