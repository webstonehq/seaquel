//! The DuckDB helper's install dialog in the built binary:
//! `seaquel-tui` on a pty, with `SEAQUEL_DATA_DIR`
//! and `SEAQUEL_TUI_TEST_DUCKDB_RELEASES` pointing at a release server on
//! 127.0.0.1 (`MockReleases`), connects to a DuckDB connection, asks,
//! downloads into the data dir and connects. With
//! `SEAQUEL_TEST_DUCKDB_HELPER` the real helper is served and the connect
//! succeeds; without it a stand-in that exits at once is served and the
//! connect after the install says the helper isn't usable (no second
//! download). Either way no helper is left running after the TUI quits.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use seaquel_http::release_asset::target_triple;
use seaquel_http::release_asset::testing::{asset_path, gzip, MockReleases};

mod pty;
use pty::{Pty, ANSWER, QUERY};

const VERSION: &str = seaquel_terminal::VERSION;

fn origin() -> seaquel_core::WriteOrigin {
    seaquel_core::WriteOrigin::new(Some("app-window"))
}

fn asset_name() -> String {
    let triple = target_triple(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    format!("seaquel-duckdb-{triple}{}.gz", std::env::consts::EXE_SUFFIX)
}

/// A data dir with a project `Warehouse` and a DuckDB connection
/// `warehouse` (in memory).
fn seed(runtime: &tokio::runtime::Runtime, dir: &Path) {
    use seaquel_core::domain::library::{ConnectionDraft, ProjectDraft, SecretChanges};
    runtime.block_on(async {
        let core = seaquel_terminal::core_builder(seaquel_terminal::CoreOptions {
            duckdb_helper_dir: Some(dir.join("bin").join("duckdb")),
            ..seaquel_terminal::CoreOptions::default()
        })
        .build();
        let ws = core
            .open_workspace(seaquel_core::WorkspaceSpec::new(dir))
            .await
            .unwrap();
        let project: ProjectDraft =
            serde_json::from_value(serde_json::json!({"name": "Warehouse"})).unwrap();
        let project = ws
            .create_project(&core, &origin(), project)
            .await
            .unwrap()
            .value
            .id;
        let draft: ConnectionDraft = serde_json::from_value(serde_json::json!({
            "projectId": project, "name": "warehouse", "type": "duckdb",
            "databaseName": ":memory:", "host": "", "port": 0, "username": "",
            "savePassword": false, "saveSshPassword": false,
            "saveSshKeyPassphrase": false, "labelIds": [],
        }))
        .unwrap();
        ws.create_connection(&core, &origin(), draft, SecretChanges::default())
            .await
            .unwrap();
        ws.close().await;
    });
}

/// The helper to serve: the built one, or a stand-in that exits at once.
fn helper() -> (Vec<u8>, bool) {
    match std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER") {
        Some(path) => (std::fs::read(PathBuf::from(path)).unwrap(), true),
        None => (b"#!/bin/sh\nexit 0\n".to_vec(), false),
    }
}

fn running(file: &Path) -> bool {
    std::process::Command::new("pgrep")
        .arg("-f")
        .arg(file)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn the_first_duckdb_connect_asks_downloads_and_connects() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let data = tempfile::Builder::new()
        .prefix("tui-duckdb-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    seed(&runtime, data.path());
    let (bytes, real) = helper();
    let mock = runtime.block_on(MockReleases::start());
    mock.publish(VERSION, &asset_name(), gzip(&bytes));

    let mut pty = Pty::start_with(
        data.path(),
        &["--project", "Warehouse", "--connection", "warehouse"],
        (100, 30),
        &[("SEAQUEL_TUI_TEST_DUCKDB_RELEASES", mock.url())],
    );
    pty.wait_for(QUERY);
    pty.send(ANSWER);
    pty.wait_for_screen("Download now?");
    assert_eq!(mock.hits(&asset_path(VERSION, &asset_name())), 0);
    pty.send(b"\r");
    let file = data
        .path()
        .join("bin")
        .join("duckdb")
        .join(VERSION)
        .join("seaquel-duckdb");
    // The waits read the rendered screen: ratatui redraws only
    // changed cells, so `warehouse (duckdb)` needn't be in the bytes whole.
    pty.wait_for_screen("installed DuckDB support");
    if real {
        pty.wait_for_screen("warehouse (duckdb)");
        assert!(running(&file), "the helper runs the connection");
    } else {
        pty.wait_for_screen("The DuckDB helper can't be used");
    }
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    assert_eq!(mock.hits(&asset_path(VERSION, &asset_name())), 1);

    // Quit: nothing staged, so `q` quits at once (Esc first closes a
    // problem dialog).
    pty.send(b"\x1b");
    std::thread::sleep(Duration::from_millis(100));
    pty.send(b"q");
    let status = pty.wait_exit();
    assert!(status.success(), "{status:?}");
    let start = Instant::now();
    while running(&file) {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "a helper outlived the TUI"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // The log names no path.
    let log = std::fs::read_to_string(data.path().join("logs").join("tui.log")).unwrap_or_default();
    assert!(
        !log.contains(&*data.path().to_string_lossy()),
        "the log names the data dir"
    );
}
