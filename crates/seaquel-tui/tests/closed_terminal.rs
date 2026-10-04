//! Probe F5: the terminal itself goes away (its window closed, an SSH
//! session dropped). The TUI runs on a pty of its own, as the session
//! leader with the pty as its controlling terminal, its stderr on the pty
//! too; the test closes the master end. The kernel then hangs the session
//! up (SIGHUP) and every write to the pty fails with EIO. The TUI must end
//! cleanly: no panic or abort, exit 0, Core's connections closed (a
//! statement streaming on Postgres stops on the server) and the state file
//! written.
//!
//! Every run has a hard timeout and is killed if it overstays.

#![cfg(unix)]

use std::os::unix::process::ExitStatusExt;
use std::time::{Duration, Instant};

mod pty;
use pty::{Pty, ANSWER, QUERY, WAIT};

fn origin() -> seaquel_core::WriteOrigin {
    seaquel_core::WriteOrigin::new(Some("app-window"))
}

/// A current `seaquel.db`, made as the app makes it, with a project `P`
/// and, given a connection string, a saved Postgres connection `pg`
/// (its password in the test secrets file).
fn seed(dir: &std::path::Path, postgres: Option<&str>) {
    use seaquel_core::domain::library::{ConnectionDraft, ProjectDraft, SecretChanges};
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let core = seaquel_terminal::core_builder(seaquel_terminal::CoreOptions::default()).build();
        let ws = core
            .open_workspace(seaquel_core::WorkspaceSpec::new(dir))
            .await
            .unwrap();
        if let Some(url) = postgres {
            let project: ProjectDraft =
                serde_json::from_value(serde_json::json!({"name": "P"})).unwrap();
            let project = ws
                .create_project(&core, &origin(), project)
                .await
                .unwrap()
                .value
                .id;
            let draft: ConnectionDraft = serde_json::from_value(serde_json::json!({
                "projectId": project, "name": "pg", "type": "postgres",
                "connectionString": url, "host": "", "port": 0, "databaseName": "",
                "username": "", "savePassword": true, "saveSshPassword": false,
                "saveSshKeyPassphrase": false, "labelIds": [],
            }))
            .unwrap();
            let id = ws
                .create_connection(&core, &origin(), draft, SecretChanges::default())
                .await
                .unwrap()
                .value
                .id;
            // Trust auth: any password connects.
            std::fs::write(
                dir.join("test-secrets.json"),
                serde_json::json!({ format!("db:{id}"): "unused" }).to_string(),
            )
            .unwrap();
        }
        ws.close().await;
    });
}

fn assert_clean_exit(status: std::process::ExitStatus, out: &[u8]) {
    let tail = &out[out.len().saturating_sub(600)..];
    assert_eq!(
        (status.code(), status.signal()),
        (Some(0), None),
        "{status:?}; the screen ended with:\n{}",
        String::from_utf8_lossy(tail)
    );
}

#[test]
fn closing_an_idle_terminal_ends_the_tui_cleanly() {
    let data = tempfile::tempdir().unwrap();
    seed(data.path(), None);
    let mut pty = Pty::start(data.path(), &[]);
    pty.wait_for(QUERY);
    pty.send(ANSWER);
    pty.wait_for(b"Command Log");
    pty.close_terminal();
    let status = pty.wait_exit();
    assert_clean_exit(status, &pty.output());
}

/// Behind `SEAQUEL_TEST_POSTGRES` (a ConnectConfig JSON with a
/// `connection_string`, trust auth).
fn postgres() -> Option<serde_json::Value> {
    match std::env::var("SEAQUEL_TEST_POSTGRES") {
        Ok(raw) => Some(serde_json::from_str(&raw).expect("SEAQUEL_TEST_POSTGRES is JSON")),
        Err(_) if std::env::var("SEAQUEL_TEST_REQUIRE_ENGINES").as_deref() == Ok("1") => {
            panic!("SEAQUEL_TEST_POSTGRES is not set, and SEAQUEL_TEST_REQUIRE_ENGINES requires it")
        }
        Err(_) => {
            eprintln!("skipping: SEAQUEL_TEST_POSTGRES is not set");
            None
        }
    }
}

/// The backends whose query names `marker`, other than the asking one.
async fn backends(config: &serde_json::Value, marker: &str) -> usize {
    let core = seaquel_terminal::core_builder(seaquel_terminal::CoreOptions::default()).build();
    let config: seaquel_types::ConnectConfig = serde_json::from_value(config.clone()).unwrap();
    let id = core.connect(&config).await.unwrap().connection_id;
    let rows = core
        .query(
            &id,
            &format!(
                "SELECT pid FROM pg_stat_activity WHERE query LIKE '%{marker}%' \
                 AND pid <> pg_backend_pid()"
            ),
            Vec::new(),
        )
        .await
        .unwrap()
        .rows
        .len();
    let _ = core.disconnect(&id).await;
    rows
}

#[test]
fn closing_the_terminal_while_postgres_streams_stops_the_statement() {
    let Some(config) = postgres() else {
        return;
    };
    let url = config["connection_string"].as_str().unwrap().to_string();
    let data = tempfile::tempdir().unwrap();
    seed(data.path(), Some(&url));
    let marker = format!("tui_closed_{}", std::process::id());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut pty = Pty::start(data.path(), &["--project", "P", "--connection", "pg"]);
    pty.wait_for(QUERY);
    pty.send(ANSWER);
    pty.wait_for(b"connected pg");
    // A new query tab, a slow statement streamed with `:all`.
    pty.send(b"Q");
    pty.wait_for(b"untitled-1");
    pty.type_text(&format!(
        "SELECT g AS {marker}, pg_sleep(0.01) FROM generate_series(1, 3000) g"
    ));
    std::thread::sleep(Duration::from_millis(200));
    pty.send(b"\x1b");
    std::thread::sleep(Duration::from_millis(200));
    pty.type_text(":all\r");
    let start = Instant::now();
    while runtime.block_on(backends(&config, &marker)) == 0 {
        assert!(
            start.elapsed() < WAIT,
            "the statement never started:\n{}",
            String::from_utf8_lossy(&pty.output())
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    pty.close_terminal();
    let status = pty.wait_exit();
    assert_clean_exit(status, &pty.output());
    // `close_all` ran: the backend is gone at once, not when the server
    // next notices a dead socket.
    let start = Instant::now();
    while runtime.block_on(backends(&config, &marker)) > 0 {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the statement is still on the server"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // And the state file was written on the way out, with the tab's text.
    let state = std::fs::read_to_string(data.path().join("tui/state.json"))
        .expect("the state file was written");
    assert!(state.contains(&marker), "{state}");
}

/// How the TUI is ended while a long `EXPLAIN ANALYZE` runs.
enum End {
    /// The terminal closes.
    HangUp,
    /// `q` (and `y` if it asks).
    Quit,
}

/// Review I1: an `EXPLAIN ANALYZE` of `pg_sleep(20)` (a Core call, not a
/// stream) is in flight when the TUI ends. The exit is bounded (the
/// tasks are aborted, `close_all` has a limit) and the statement doesn't
/// stay on the server.
fn analyze_case(end: End, tag: &str) {
    let Some(config) = postgres() else {
        return;
    };
    let url = config["connection_string"].as_str().unwrap().to_string();
    let data = tempfile::tempdir().unwrap();
    seed(data.path(), Some(&url));
    let marker = format!("tui_analyze_{tag}_{}", std::process::id());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut pty = Pty::start(data.path(), &["--project", "P", "--connection", "pg"]);
    pty.wait_for(QUERY);
    pty.send(ANSWER);
    pty.wait_for(b"connected pg");
    pty.send(b"Q");
    pty.wait_for(b"untitled-1");
    pty.type_text(&format!("SELECT pg_sleep(20) AS {marker}"));
    std::thread::sleep(Duration::from_millis(200));
    pty.send(b"\x1b");
    std::thread::sleep(Duration::from_millis(200));
    pty.type_text(":analyze\r");
    let start = Instant::now();
    while runtime.block_on(backends(&config, &marker)) == 0 {
        assert!(
            start.elapsed() < WAIT,
            "the statement never started:\n{}",
            String::from_utf8_lossy(&pty.output())
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let ended = Instant::now();
    match end {
        End::HangUp => pty.close_terminal(),
        End::Quit => {
            pty.send(b"q");
            std::thread::sleep(Duration::from_millis(200));
            pty.send(b"y");
        }
    }
    let status = pty.wait_exit();
    let took = ended.elapsed();
    assert_clean_exit(status, &pty.output());
    assert!(took < Duration::from_secs(8), "the exit took {took:?}");
    let start = Instant::now();
    while runtime.block_on(backends(&config, &marker)) > 0 {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the statement is still on the server"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_hang_up_during_analyze_exits_within_the_bound() {
    analyze_case(End::HangUp, "hup");
}

#[test]
fn q_during_analyze_exits_within_the_bound() {
    analyze_case(End::Quit, "q");
}
