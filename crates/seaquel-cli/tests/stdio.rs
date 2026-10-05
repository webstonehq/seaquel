//! Runs the built `seaquel-cli mcp` over stdio.
//!
//! Every run is sandboxed through the binary's debug-build test hooks (see
//! `src/mcp.rs`): `SEAQUEL_DATA_DIR` is a temp dir, `SEAQUEL_CLI_TEST_SECRETS`
//! a JSON file loaded into a `MemoryStore` instead of the keychain, and
//! `SEAQUEL_CLI_TEST_KNOWN_HOSTS` a temp file instead of `~/.ssh`.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::ServiceExt;
use seaquel_core::storage::{connections, projects, StorageOptions};
use seaquel_core::WorkspaceSpec;
use seaquel_types::storage::{PersistedConnection, PersistedProject};
use seaquel_types::ConnectConfig;
use serde_json::{json, Value as Json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Saved for `pg refused`; must appear in no output, logs included.
const SECRET: &str = "S3cret-Hunter2-pw";

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    /// `seaquel-cli mcp <args>` with the sandbox's environment.
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_seaquel-cli"));
        cmd.arg("mcp")
            .args(args)
            .env("SEAQUEL_DATA_DIR", self.data())
            .env(
                "SEAQUEL_CLI_TEST_SECRETS",
                self.dir.path().join("secrets.json"),
            )
            .env(
                "SEAQUEL_CLI_TEST_KNOWN_HOSTS",
                self.dir.path().join("known_hosts"),
            )
            .kill_on_drop(true);
        cmd
    }
}

fn row(id: &str, name: &str, fields: Json) -> PersistedConnection {
    let mut row = json!({
        "id": id, "projectId": "p1", "name": name,
        "type": "sqlite", "host": "localhost", "port": 0, "databaseName": "",
        "username": "", "savePassword": false, "saveSshPassword": false,
        "saveSshKeyPassphrase": false, "labelIds": [],
        "aiShareSchema": true, "aiShareData": true,
    });
    for (k, v) in fields.as_object().unwrap() {
        row[k] = v.clone();
    }
    serde_json::from_value(row).unwrap()
}

/// A data dir with one project, a SQLite connection (`lite`), a Postgres one
/// whose saved password goes through the test secrets file (`pg refused`),
/// and two connections named `twin`.
async fn sandbox() -> Sandbox {
    // Under the target dir, so a DuckDB helper the hook links in here is
    // found by `pgrep -f` on the target path.
    let dir = tempfile::Builder::new()
        .prefix("cli-stdio-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let lite = dir.path().join("app.sqlite");

    let core = seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    let config: ConnectConfig = serde_json::from_value(json!({
        "driver": "sqlite",
        "connection_string": format!("sqlite://{}", lite.display()),
        "create_if_missing": true,
    }))
    .unwrap();
    let id = core.connect(&config).await.unwrap().connection_id;
    core.execute(&id, "CREATE TABLE t (id INTEGER, s TEXT)", vec![])
        .await
        .unwrap();
    core.execute(
        &id,
        "INSERT INTO t VALUES (1, 'line one\nline two \"quoted\"'), (2, NULL)",
        vec![],
    )
    .await
    .unwrap();
    core.disconnect(&id).await.unwrap();

    let ws = core
        .open_workspace(WorkspaceSpec::new(&data))
        .await
        .unwrap();
    let st = ws.storage();
    let project: PersistedProject = serde_json::from_value(json!({
        "id": "p1", "name": "Main",
        "createdAt": "2026-01-02T03:04:05.000Z", "updatedAt": "2026-01-02T03:04:05.000Z",
        "customLabels": [],
    }))
    .unwrap();
    projects::save(st, &project).await.unwrap();
    let lite_fields = json!({
        "databaseName": lite.to_str().unwrap(),
        "connectionString": format!("sqlite://{}", lite.display()),
    });
    for r in [
        row("c-lite", "lite", lite_fields.clone()),
        row("c-twin-a", "twin", lite_fields.clone()),
        row("c-twin-b", "twin", lite_fields),
        row(
            "c-duck",
            "duck",
            json!({ "type": "duckdb", "databaseName": ":memory:" }),
        ),
        row(
            "c-pg-refused",
            "pg refused",
            json!({ "type": "postgres", "host": "127.0.0.1", "port": 99999,
                    "databaseName": "seaquel_test", "username": "alice", "savePassword": true,
                    "connectionString": "postgresql://alice@127.0.0.1:99999/seaquel_test" }),
        ),
    ] {
        connections::save(st, &r).await.unwrap();
    }
    ws.close().await;
    std::fs::write(
        dir.path().join("secrets.json"),
        json!({ "db:c-pg-refused": SECRET }).to_string(),
    )
    .unwrap();
    Sandbox { dir }
}

fn text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect()
}

async fn read_all(mut stderr: impl tokio::io::AsyncRead + Unpin) -> String {
    let mut out = String::new();
    stderr.read_to_string(&mut out).await.unwrap();
    out
}

#[tokio::test]
async fn an_mcp_client_talks_to_the_binary() {
    let sb = sandbox().await;
    let (transport, stderr) = TokioChildProcess::builder(sb.command(&[]).configure(|c| {
        c.args(["--connection", "lite", "--connection", "c-pg-refused"])
            .args(["--log-level", "trace"]);
    }))
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    let stderr = tokio::spawn(read_all(stderr.unwrap()));
    let client = ().serve(transport).await.unwrap();

    let info = client.peer_info().unwrap();
    let server = info.server_info.clone().unwrap();
    assert_eq!(server.name, "seaquel");
    assert_eq!(server.version, seaquel_cli::VERSION);
    assert_eq!(client.list_all_tools().await.unwrap().len(), 8);

    let call = |name: &str, args: Json| {
        CallToolRequestParams::new(name.to_string())
            .with_arguments(args.as_object().unwrap().clone())
    };
    let result = client
        .call_tool(call(
            "run_query",
            json!({ "connection": "lite", "sql": "SELECT id, s FROM t ORDER BY id" }),
        ))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let out: Json = serde_json::from_str(&text(&result)).unwrap();
    assert_eq!(
        out["rows"],
        json!([[1, "line one\nline two \"quoted\""], [2, null]])
    );

    let result = client
        .call_tool(call(
            "run_query",
            json!({ "connection": "pg refused", "sql": "SELECT 1" }),
        ))
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(!text(&result).contains(SECRET));

    let result = client
        .call_tool(call(
            "run_query",
            json!({ "connection": "lite", "sql": "DELETE FROM t" }),
        ))
        .await
        .unwrap();
    assert!(
        text(&result).starts_with("READ_ONLY: "),
        "{}",
        text(&result)
    );

    client.cancel().await.unwrap();
    let stderr = tokio::time::timeout(Duration::from_secs(10), stderr)
        .await
        .expect("the binary exits once stdin closes")
        .unwrap();
    assert!(stderr.contains("Serving MCP on stdio"), "{stderr}");
    assert!(stderr.contains("Connecting \"lite\""), "{stderr}");
    // No DuckDB connection exposed: no word about its helper.
    assert!(!stderr.contains("DuckDB support"), "{stderr}");
    assert!(!stderr.contains(SECRET), "a secret in the logs:\n{stderr}");
}

/// Runs `sql` on `connection` through the binary and returns the tool
/// result's text and whether it is an error.
async fn one_query(cmd: Command, connection: &str, sql: &str) -> (String, bool) {
    let (text, is_error, _) = one_query_logged(cmd, connection, sql).await;
    (text, is_error)
}

/// [`one_query`], with the binary's stderr.
async fn one_query_logged(cmd: Command, connection: &str, sql: &str) -> (String, bool, String) {
    let (transport, stderr) = TokioChildProcess::builder(cmd.configure(|c| {
        c.args(["--connection", connection]);
    }))
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    let stderr = tokio::spawn(read_all(stderr.unwrap()));
    let client = ().serve(transport).await.unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        client.call_tool(
            CallToolRequestParams::new("run_query".to_string()).with_arguments(
                json!({ "connection": connection, "sql": sql })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        ),
    )
    .await
    .expect("the call answers in time")
    .unwrap();
    client.cancel().await.unwrap();
    let stderr = tokio::time::timeout(Duration::from_secs(10), stderr)
        .await
        .expect("the binary exits once stdin closes")
        .unwrap();
    (text(&result), result.is_error == Some(true), stderr)
}

/// The MCP server's words for a DuckDB connection without the helper,
/// as the tool error's text and the startup line.
fn not_installed_text() -> String {
    format!(
        "DuckDB support isn't installed for seaquel-cli {}. Run \"seaquel-cli duckdb install\", \
         or use Install Command Line Tool in the Seaquel app.",
        seaquel_cli::VERSION
    )
}

/// The CLI links no DuckDB: with no
/// helper under the data dir, a DuckDB query is `ENGINE_NOT_INSTALLED` (a
/// native driver would have answered), and nothing is downloaded or made.
/// The error says how to install it, and so does
/// one stderr line at startup; the server starts anyway.
#[tokio::test]
async fn duckdb_without_the_helper_is_not_installed() {
    let sb = sandbox().await;
    let (text, is_error, stderr) =
        one_query_logged(sb.command(&[]), "duck", "SELECT 40 + 2 AS n").await;
    assert!(is_error, "{text}");
    assert_eq!(
        text,
        format!("ENGINE_NOT_INSTALLED: {}", not_installed_text())
    );
    assert_eq!(
        stderr.matches(&not_installed_text()).count(),
        1,
        "one startup line: {stderr}"
    );
    assert!(!stderr.contains("duck\""), "no connection name: {stderr}");
    assert!(!sb.data().join("bin").exists());
}

/// A helper that is installed (it passes the start's check) but still
/// refused, here a stand-in that exits at once: installing again wouldn't
/// help (`install` finds it intact), so the error points at `duckdb
/// status` instead, and there is no startup line.
#[cfg(unix)]
#[tokio::test]
async fn an_installed_helper_that_is_refused_points_at_status() {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let sb = sandbox().await;
    let folder = sb
        .data()
        .join("bin")
        .join("duckdb")
        .join(seaquel_cli::VERSION);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(folder.join("seaquel-duckdb"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"#!/bin/sh\nexit 0\n"))
        .unwrap();
    let (text, is_error, stderr) =
        one_query_logged(sb.command(&[]), "duck", "SELECT 40 + 2 AS n").await;
    assert!(is_error, "{text}");
    assert!(text.starts_with("ENGINE_NOT_INSTALLED: "), "{text}");
    assert!(text.contains("seaquel-cli duckdb status"), "{text}");
    assert!(!text.contains("seaquel-cli duckdb install"), "{text}");
    assert!(!stderr.contains(&not_installed_text()), "{stderr}");
}

/// `SEAQUEL_CLI_TEST_DUCKDB_HELPER` (debug builds) links a built helper
/// into `<data dir>/bin/duckdb/<version>/`, and the MCP server's DuckDB
/// query runs in it. Needs `SEAQUEL_TEST_DUCKDB_HELPER`; skipped without
/// it, failing with `SEAQUEL_TEST_REQUIRE_ENGINES`.
#[tokio::test]
async fn duckdb_runs_in_the_hook_s_helper() {
    let built = match std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER") {
        Some(path) => PathBuf::from(path),
        None if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("SEAQUEL_TEST_DUCKDB_HELPER is not set")
        }
        None => {
            eprintln!("skipping: SEAQUEL_TEST_DUCKDB_HELPER is not set");
            return;
        }
    };
    let sb = sandbox().await;
    let mut cmd = sb.command(&[]);
    cmd.env("SEAQUEL_CLI_TEST_DUCKDB_HELPER", &built);
    let (text, is_error) = one_query(cmd, "duck", "SELECT 40 + 2 AS n").await;
    assert!(!is_error, "{text}");
    let out: Json = serde_json::from_str(&text).unwrap();
    assert_eq!(out["rows"], json!([[42]]));
    let installed = sb
        .data()
        .join("bin")
        .join("duckdb")
        .join(seaquel_cli::VERSION)
        .join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX));
    assert!(installed.is_file(), "{installed:?}");
}

/// The MCP server's DuckDB is restricted through the helper
/// too. A query reading a file in the sandbox through `read_csv` is a
/// tool error, and the file's content never comes back. Needs
/// `SEAQUEL_TEST_DUCKDB_HELPER`, as above.
#[tokio::test]
async fn the_hook_s_helper_runs_duckdb_restricted() {
    let built = match std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER") {
        Some(path) => PathBuf::from(path),
        None if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("SEAQUEL_TEST_DUCKDB_HELPER is not set")
        }
        None => {
            eprintln!("skipping: SEAQUEL_TEST_DUCKDB_HELPER is not set");
            return;
        }
    };
    let sb = sandbox().await;
    let csv = sb.dir.path().join("escape.csv");
    std::fs::write(&csv, "marker\nRestricted-Marker-7731\n").unwrap();
    let mut cmd = sb.command(&[]);
    cmd.env("SEAQUEL_CLI_TEST_DUCKDB_HELPER", &built);
    let sql = format!("SELECT * FROM read_csv('{}')", csv.display());
    let (text, is_error) = one_query(cmd, "duck", &sql).await;
    assert!(is_error, "{text}");
    assert!(!text.contains("Restricted-Marker-7731"), "{text}");
    assert!(
        text.contains("file system operations are disabled"),
        "refused by DuckDB's restriction: {text}"
    );
}

/// Sends raw JSON-RPC lines and checks every stdout line is a JSON-RPC
/// message, while the logs go to stderr.
#[tokio::test]
async fn stdout_carries_only_json_rpc() {
    let sb = sandbox().await;
    let mut child = sb
        .command(&[
            "--connection",
            "lite",
            "--connection",
            "duck",
            "--log-level",
            "debug",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stderr = tokio::spawn(read_all(child.stderr.take().unwrap()));
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();

    let send = |msg: Json| format!("{msg}\n");
    stdin
        .write_all(
            send(
                json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": { "name": "stdio-test", "version": "1" } } }),
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let first = stdout.next_line().await.unwrap().unwrap();
    let first: Json = serde_json::from_str(&first).unwrap();
    assert_eq!(first["id"], 1);
    assert_eq!(first["result"]["serverInfo"]["name"], "seaquel");

    for msg in [
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "run_query", "arguments": { "connection": "lite",
            "sql": "SELECT s FROM t WHERE id = 1" } } }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "list_tables", "arguments": { "connection": "nope" } } }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
            "name": "no_such_tool", "arguments": {} } }),
    ] {
        stdin.write_all(send(msg).as_bytes()).await.unwrap();
    }
    let mut seen = std::collections::BTreeMap::new();
    while seen.len() < 3 {
        let line = tokio::time::timeout(Duration::from_secs(10), stdout.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("a response line");
        let msg: Json = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(msg["jsonrpc"], "2.0", "{line}");
        if let Some(id) = msg["id"].as_i64() {
            seen.insert(id, msg);
        }
    }
    let rows: Json =
        serde_json::from_str(seen[&2]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(rows["rows"], json!([["line one\nline two \"quoted\""]]));
    assert_eq!(seen[&3]["result"]["isError"], true);
    assert!(seen[&4]["error"]["code"].is_i64(), "{}", seen[&4]);

    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("exits once stdin closes")
        .unwrap();
    assert!(status.success(), "{status}");
    // Nothing else on stdout.
    assert_eq!(stdout.next_line().await.unwrap(), None);
    let stderr = stderr.await.unwrap();
    assert!(stderr.contains("Serving MCP on stdio"), "{stderr}");
}

/// Starts the binary on raw pipes and completes the handshake.
async fn raw_session(
    sb: &Sandbox,
    args: &[&str],
) -> (
    tokio::process::Child,
    tokio::process::ChildStdin,
    tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    tokio::task::JoinHandle<String>,
) {
    let mut child = sb
        .command(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stderr = tokio::spawn(read_all(child.stderr.take().unwrap()));
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    for msg in [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": { "name": "stdio-test", "version": "1" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    ] {
        stdin
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
    }
    let first: Json = serde_json::from_str(&stdout.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(first["id"], 1);
    (child, stdin, stdout, stderr)
}

async fn next_message(
    stdout: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
) -> Json {
    let line = tokio::time::timeout(Duration::from_secs(10), stdout.next_line())
        .await
        .expect("a reply in time")
        .unwrap()
        .expect("a reply line");
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"))
}

/// A lone UTF-16 surrogate escape isn't JSON serde_json takes: the client
/// gets a parse error and stderr a warning, and the session goes on.
#[tokio::test]
async fn a_request_that_is_not_json_gets_a_parse_error() {
    let sb = sandbox().await;
    let (mut child, mut stdin, mut stdout, stderr) =
        raw_session(&sb, &["--connection", "lite"]).await;
    stdin
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"run_query\",\"arguments\":{\"connection\":\"lite\",\"sql\":\"SELECT '\\ud800'\"}}}\n",
        )
        .await
        .unwrap();
    let reply = next_message(&mut stdout).await;
    assert_eq!(reply["error"]["code"], -32700, "{reply}");
    assert_eq!(reply["jsonrpc"], "2.0");
    assert_eq!(reply.get("id"), Some(&Json::Null), "{reply}");

    stdin
        .write_all(
            format!(
                "{}\n",
                json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
                    "name": "list_connections", "arguments": {} } })
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let reply = next_message(&mut stdout).await;
    assert_eq!(reply["id"], 3, "{reply}");
    assert_ne!(reply["result"]["isError"], true, "{reply}");

    drop(stdin);
    assert!(child.wait().await.unwrap().success());
    let stderr = stderr.await.unwrap();
    assert!(
        stderr.contains("WARN") && stderr.contains("isn't valid JSON"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("SELECT"),
        "the line itself is not logged: {stderr}"
    );
}

/// SIGTERM and SIGINT end the session the way stdin EOF does: the server
/// closes its connections and the process exits cleanly.
#[cfg(unix)]
#[tokio::test]
async fn a_signal_closes_the_server() {
    for signal in ["TERM", "INT"] {
        let sb = sandbox().await;
        let (mut child, mut stdin, mut stdout, stderr) =
            raw_session(&sb, &["--connection", "lite", "--log-level", "info"]).await;
        stdin
            .write_all(
                format!(
                    "{}\n",
                    json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                        "name": "run_query",
                        "arguments": { "connection": "lite", "sql": "SELECT 1 AS one" } } })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(next_message(&mut stdout).await["id"], 2);

        let pid = child.id().unwrap().to_string();
        let killed = std::process::Command::new("kill")
            .args([format!("-{signal}").as_str(), pid.as_str()])
            .status()
            .unwrap();
        assert!(killed.success());
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("exits after the signal")
            .unwrap();
        assert!(status.success(), "SIG{signal}: {status}");
        let stderr = stderr.await.unwrap();
        assert!(stderr.contains(&format!("Got SIG{signal}")), "{stderr}");
        assert!(
            stderr.contains("Closed 1 connection(s) and the workspace"),
            "{stderr}"
        );
        drop(stdin);
    }
}

async fn fail(sb: &Sandbox, args: &[&str]) -> String {
    let out = sb
        .command(args)
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();
    assert!(!out.status.success(), "{args:?} should fail");
    assert!(
        out.stdout.is_empty(),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8(out.stderr).unwrap()
}

#[tokio::test]
async fn a_data_dir_the_app_must_upgrade_is_refused() {
    let sb = sandbox().await;
    let file = sb.data().join("seaquel.db");
    // A file the app hasn't brought up to date: its schema version is gone.
    // The DELETE runs on one plain connection that checkpoints and closes
    // before the file is read: through the storage pool, a
    // connection could still be closing, and checkpointing the WAL into the
    // file, after `before` was read.
    {
        use sqlx::{ConnectOptions, Connection};
        let storage = seaquel_core::storage::Storage::open(&file, StorageOptions::default())
            .await
            .unwrap();
        storage.close().await;
        let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&file)
            .connect()
            .await
            .unwrap();
        sqlx::query("DELETE FROM schema_version")
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&mut conn)
            .await
            .unwrap();
        conn.close().await.unwrap();
    }
    let wal = file.with_extension("db-wal");
    assert!(
        std::fs::metadata(&wal).map_or(true, |m| m.len() == 0),
        "the WAL is checkpointed before the file is read"
    );
    let before = std::fs::read(&file).unwrap();
    let stderr = fail(&sb, &["--connection", "lite"]).await;
    assert!(
        stderr.starts_with("seaquel-cli mcp: STORAGE_NEEDS_UPGRADE: "),
        "{stderr}"
    );
    assert!(stderr.contains("Open the Seaquel app once"), "{stderr}");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        before,
        "the file is left as it was"
    );
}

#[tokio::test]
async fn a_missing_data_dir_is_refused() {
    let sb = sandbox().await;
    std::fs::remove_dir_all(sb.data()).unwrap();
    let stderr = fail(&sb, &[]).await;
    assert!(stderr.contains("STORAGE_NOT_FOUND"), "{stderr}");
    assert!(!sb.data().exists(), "nothing is created");
}

#[tokio::test]
async fn bad_exposure_flags_are_refused_at_startup() {
    let sb = sandbox().await;
    let stderr = fail(&sb, &["--connection", "twin"]).await;
    assert!(stderr.contains("AMBIGUOUS_CONNECTION"), "{stderr}");
    assert!(
        stderr.contains("c-twin-a") && stderr.contains("c-twin-b"),
        "{stderr}"
    );
    let stderr = fail(&sb, &["--connection", "LITE"]).await;
    assert!(stderr.contains("CONNECTION_NOT_FOUND"), "{stderr}");
    let stderr = fail(&sb, &["--project", "nope"]).await;
    assert!(stderr.contains("PROJECT_NOT_FOUND"), "{stderr}");
}

#[tokio::test]
async fn without_flags_the_server_exposes_nothing() {
    let sb = sandbox().await;
    let transport = TokioChildProcess::new(sb.command(&["--log-level", "off"])).unwrap();
    let client = ().serve(transport).await.unwrap();
    let result = client
        .call_tool(CallToolRequestParams::new("list_connections"))
        .await
        .unwrap();
    let out: Json = serde_json::from_str(&text(&result)).unwrap();
    assert_eq!(out["connections"], json!([]));
    assert!(out["message"].as_str().unwrap().contains("--connection"));
    client.cancel().await.unwrap();
}
