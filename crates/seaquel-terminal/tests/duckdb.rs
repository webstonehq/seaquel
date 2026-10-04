//! The terminal binaries' DuckDB (the DuckDB helper plan, Task 6): their
//! Core registers the remote engine under `duckdb`, once, even in a build
//! where Cargo unified the native driver in (the workspace line, or this
//! crate's tests next to `seaquel-mcp`'s); the helper is looked for under
//! the data-local dir; the debug hooks link a built helper into that
//! layout and point the download at a loopback release server.
//!
//! Every folder is a temp one under `CARGO_TARGET_TMPDIR`, never the real
//! data dir. The test that starts a helper needs
//! `SEAQUEL_TEST_DUCKDB_HELPER` (a built `seaquel-duckdb`); without it it
//! is skipped, and with `SEAQUEL_TEST_REQUIRE_ENGINES` it fails.

use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(debug_assertions)]
use seaquel_core::SuppliedSecrets;
use seaquel_core::{ConnectRequest, ConnectionForm, Core, DuckdbHelperStatus, WorkspaceSpec};
#[cfg(debug_assertions)]
use seaquel_terminal::TestHooks;
use seaquel_terminal::{core_builder, CoreOptions, APP_IDENTIFIER, VERSION};
use serde_json::json;

#[cfg(debug_assertions)]
const LIMIT: Duration = Duration::from_secs(60);

fn temp() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("terminal-duckdb-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap()
}

fn helper_dir(data: &Path) -> PathBuf {
    data.join("bin").join("duckdb")
}

fn core_in(data: &Path, options: CoreOptions) -> Core {
    core_builder(CoreOptions {
        duckdb_helper_dir: Some(helper_dir(data)),
        ..options
    })
    .build()
}

fn duckdb_form() -> ConnectionForm {
    serde_json::from_value(json!({"name": "d", "type": "duckdb", "databaseName": ":memory:"}))
        .unwrap()
}

#[cfg(debug_assertions)]
fn built_helper() -> Option<PathBuf> {
    match std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER") {
        Some(path) => Some(PathBuf::from(path)),
        None if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("SEAQUEL_TEST_DUCKDB_HELPER is not set")
        }
        None => {
            eprintln!("skipping: SEAQUEL_TEST_DUCKDB_HELPER is not set");
            None
        }
    }
}

#[test]
fn duckdb_is_registered_once_and_runs_in_the_helper() {
    let data = temp();
    let core = core_in(data.path(), CoreOptions::default());
    let mut ids = core.engine_ids();
    ids.sort_unstable();
    assert_eq!(ids, ["duckdb", "mssql", "mysql", "postgres", "sqlite"]);
    let helper = core.duckdb_helper().expect("the remote engine's locator");
    assert_eq!(helper.version, VERSION);
    assert_eq!(helper.dir, helper_dir(data.path()));
}

/// Without a locator given, the helper is looked for beside the app's
/// command line tool: `<data_local_dir>/<identifier>/bin/duckdb`, or under
/// `SEAQUEL_DATA_DIR`. Nothing is created by looking.
#[test]
fn the_default_helper_dir_is_the_data_local_dir_s() {
    let dir = seaquel_terminal::duckdb_helper_dir().unwrap();
    let root = seaquel_core::storage::data_local_dir(APP_IDENTIFIER).unwrap();
    assert_eq!(dir, root.join("bin").join("duckdb"));
    if std::env::var_os("SEAQUEL_DATA_DIR").is_none_or(|v| v.is_empty()) {
        assert_eq!(
            dir.parent().unwrap().parent().unwrap().file_name().unwrap(),
            APP_IDENTIFIER
        );
    }
    let core = core_builder(CoreOptions::default()).build();
    assert_eq!(core.duckdb_helper().unwrap().dir, dir);
}

/// No helper: a DuckDB connect fails at once with the code the TUI's
/// install dialog (Task 7) and the CLI (Task 8) look for, not after the
/// connect's timeout.
#[tokio::test]
async fn a_duckdb_connect_without_the_helper_is_not_installed_at_once() {
    let data = temp();
    let core = core_in(data.path(), CoreOptions::default());
    let ws = core
        .open_workspace(WorkspaceSpec::new(data.path()))
        .await
        .unwrap();
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );
    let started = tokio::time::Instant::now();
    let e = ws
        .connect(&core, ConnectRequest::form(duckdb_form()))
        .await
        .unwrap_err();
    let took = started.elapsed();
    assert_eq!(e.code, "ENGINE_NOT_INSTALLED", "{e:?}");
    assert!(took < Duration::from_millis(100), "{took:?}");
    assert!(
        !helper_dir(data.path()).exists(),
        "a connect creates nothing"
    );
    ws.close().await;
}

/// `_DUCKDB_HELPER` names one file; Core's check wants the install's
/// layout. The hook builds it: the file linked (or copied) to
/// `<dir>/<version>/seaquel-duckdb`, the folders private, so the status
/// and every start see an installed helper. Building the Core again (a
/// second binary, a restart) replaces it.
#[cfg(debug_assertions)]
#[test]
fn the_helper_hook_installs_into_the_layout() {
    let data = temp();
    let built = data.path().join("built-helper");
    std::fs::write(&built, b"#!/bin/sh\nexit 0\n").unwrap();
    let hooks = TestHooks::from_lookup("SEAQUEL_TUI_TEST", |name| {
        (name == "SEAQUEL_TUI_TEST_DUCKDB_HELPER").then(|| built.to_str().unwrap().to_string())
    });
    let options = CoreOptions::default().with_hooks(&hooks);
    assert_eq!(options.test_duckdb_helper.as_deref(), Some(built.as_path()));

    for _ in 0..2 {
        let core = core_in(data.path(), options.clone());
        let helper = core.duckdb_helper().unwrap();
        let path = helper.path();
        assert_eq!(
            path,
            helper_dir(data.path())
                .join(VERSION)
                .join(seaquel_core::DuckdbHelper::file_name())
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            std::fs::read(&built).unwrap()
        );
        assert_eq!(
            core.duckdb_helper_status().unwrap(),
            DuckdbHelperStatus::Installed { path: path.clone() }
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for folder in [
                helper_dir(data.path()).parent().unwrap().to_path_buf(),
                helper_dir(data.path()),
                helper_dir(data.path()).join(VERSION),
            ] {
                let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o700, "{folder:?}");
            }
        }
    }
}

/// The hook's helper answers: a DuckDB connect through the terminal
/// binaries' Core runs in `seaquel-duckdb`.
#[cfg(debug_assertions)]
#[tokio::test]
async fn the_hook_s_helper_answers_a_query() {
    let Some(built) = built_helper() else { return };
    let data = temp();
    let core = core_in(
        data.path(),
        CoreOptions {
            test_duckdb_helper: Some(built),
            ..CoreOptions::default()
        },
    );
    let ws = core
        .open_workspace(WorkspaceSpec::new(data.path()))
        .await
        .unwrap();
    let req = ConnectRequest::form(duckdb_form()).with_secrets(SuppliedSecrets::none());
    let id = tokio::time::timeout(LIMIT, ws.connect(&core, req))
        .await
        .expect("connect in time")
        .expect("connect");
    let result = tokio::time::timeout(LIMIT, ws.query(&core, &id, "SELECT 40 + 2 AS n", vec![]))
        .await
        .expect("query in time")
        .expect("query");
    assert_eq!(serde_json::to_value(&result.rows).unwrap(), json!([[42]]));
    ws.close_all(&core).await;
    ws.close().await;
}

/// `_DUCKDB_RELEASES` points the helper's download at a loopback server
/// (`MockReleases`), never GitHub.
#[cfg(debug_assertions)]
#[tokio::test]
async fn the_releases_hook_points_the_download_at_it() {
    use seaquel_http::release_asset::testing::{release_path, MockReleases};

    let mock = MockReleases::start().await;
    let data = temp();
    let hooks = TestHooks::from_lookup("SEAQUEL_TUI_TEST", |name| {
        (name == "SEAQUEL_TUI_TEST_DUCKDB_RELEASES").then(|| mock.url().to_string())
    });
    let core = core_in(data.path(), CoreOptions::default().with_hooks(&hooks));
    assert!(core.duckdb_helper_releases().is_some());
    assert!(core_in(data.path(), CoreOptions::default())
        .duckdb_helper_releases()
        .is_none());
    // Nothing published for this version: the server says so.
    let e = tokio::time::timeout(LIMIT, core.duckdb_helper_asset())
        .await
        .expect("in time")
        .unwrap_err();
    assert_eq!(e.code, "RELEASE_NOT_FOUND", "{e:?}");
    assert_eq!(mock.hits(&release_path(VERSION)), 1);
}

/// A release build ignores both DuckDB hooks even when the options carry
/// them (`TestHooks` already reads nothing there): no file is linked and
/// no other release source is set, so the download stays GitHub's. Run
/// with `cargo test --release`.
#[cfg(not(debug_assertions))]
#[test]
fn a_release_build_ignores_the_duckdb_hooks() {
    let data = temp();
    let built = data.path().join("built-helper");
    std::fs::write(&built, b"#!/bin/sh\nexit 0\n").unwrap();
    let core = core_in(
        data.path(),
        CoreOptions {
            test_duckdb_helper: Some(built),
            duckdb_releases: Some("http://127.0.0.1:9".to_string()),
            ..CoreOptions::default()
        },
    );
    assert!(!helper_dir(data.path()).exists());
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );
    assert!(core.duckdb_helper_releases().is_none());
}
