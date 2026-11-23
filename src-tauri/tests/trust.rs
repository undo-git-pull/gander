//! A workspace config's connections must not run until the user approves the
//! file, and any later edit must revoke that approval.

use gander_lib::{config, core};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_connections_wait_for_approval() {
    let cfg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    // Env is process-global; this is the only test in this binary.
    unsafe {
        std::env::set_var("GANDER_CONFIG_DIR", cfg.path());
        std::env::set_var("GANDER_STATE_DIR", state.path());
    }
    let marker = ws.path().join("pwned.txt");
    let marker_sql = marker.display().to_string().replace('\'', "''");
    let text = format!(
        "default_connection = \"evil\"\n[server]\napi_key = \"\"\n[[connections]]\nname = \"evil\"\npath = \":memory:\"\ninit_sql = [\"COPY (SELECT 1) TO '{marker_sql}'\"]\n"
    );
    std::fs::create_dir_all(ws.path().join(".gander")).unwrap();
    let ws_cfg = ws.path().join(".gander").join("config.toml");
    std::fs::write(&ws_cfg, &text).unwrap();

    let new_core = || {
        core::Core::new(core::CoreOptions {
            workspace: Some(ws.path().to_path_buf()),
            ui: Box::new(core::NoUi),
            rt: tokio::runtime::Handle::current(),
            ephemeral: true,
        })
    };

    // Untrusted: nothing runs, and the API still uses a generated key.
    let c = new_core();
    assert!(!marker.exists(), "init_sql ran from an untrusted workspace config");
    assert_eq!(c.source_info().label, "Files only");
    assert!(c.snapshot().workspace_trust.needs_trust);
    assert!(c.api_key_is_generated());

    // A stale hash is refused.
    assert!(c.trust_workspace_config(&config::sha256_hex(b"something else")).is_err());

    // Approving the exact contents opens the default connection.
    c.trust_workspace_config(&config::sha256_hex(text.as_bytes())).unwrap();
    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(marker.exists(), "approved connection did not open");
    assert_eq!(c.source_info().label, "evil");
    // [server] from the workspace is still ignored after approval.
    assert!(c.api_key_is_generated());
    drop(c);

    // A restart with the same file stays trusted.
    std::fs::remove_file(&marker).unwrap();
    let c = new_core();
    assert!(marker.exists());
    assert!(!c.snapshot().workspace_trust.needs_trust);
    drop(c);

    // Editing the file revokes the approval.
    std::fs::remove_file(&marker).unwrap();
    std::fs::write(&ws_cfg, format!("{text}# edited\n")).unwrap();
    let c = new_core();
    assert!(!marker.exists(), "edited workspace config ran without new approval");
    assert!(c.snapshot().workspace_trust.needs_trust);
}
