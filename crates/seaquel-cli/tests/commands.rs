//! Runs the built `seaquel-cli`'s read commands (`conn`, `schema`, `saved`,
//! `query`) against a sandboxed data dir.
//!
//! As in `stdio.rs`, every run goes through the binary's debug-build test
//! hooks: `SEAQUEL_DATA_DIR` is a temp dir, `SEAQUEL_CLI_TEST_SECRETS` a JSON
//! file loaded into a `MemoryStore` instead of the keychain, and
//! `SEAQUEL_CLI_TEST_KNOWN_HOSTS` a temp file instead of `~/.ssh`. stdin is
//! `/dev/null`, so no run is interactive and nothing is ever asked.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use seaquel_core::storage::{connections, projects, saved_queries};
use seaquel_core::WorkspaceSpec;
use seaquel_types::storage::{PersistedConnection, PersistedProject, PersistedSavedQuery};
use seaquel_types::ConnectConfig;
use serde_json::{json, Value as Json};

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    /// `seaquel-cli <args>` with the sandbox's environment, stdin closed.
    fn run(&self, args: &[&str]) -> Output {
        self.command(args)
            .stdin(Stdio::null())
            .output()
            .expect("run seaquel-cli")
    }

    /// [`Sandbox::run`] with `input` piped to stdin.
    fn run_with_stdin(&self, args: &[&str], input: &str) -> Output {
        use std::io::Write;
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start seaquel-cli");
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(input.as_bytes()).expect("write stdin");
        drop(stdin);
        child.wait_with_output().expect("run seaquel-cli")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_seaquel-cli"));
        command
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
            // No DuckDB helper unless a test puts one there.
            .env_remove("SEAQUEL_CLI_TEST_DUCKDB_HELPER")
            .env_remove("SEAQUEL_CLI_TEST_DUCKDB_RELEASES");
        command
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("utf-8 stdout")
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("utf-8 stderr")
}

const TIME: &str = "2026-01-02T03:04:05.000Z";

fn project(id: &str, name: &str) -> PersistedProject {
    serde_json::from_value(json!({
        "id": id, "name": name, "createdAt": TIME, "updatedAt": TIME, "customLabels": [],
    }))
    .unwrap()
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

fn saved(id: &str, name: &str, query: &str) -> PersistedSavedQuery {
    serde_json::from_value(json!({
        "id": id, "name": name, "query": query, "projectId": "p1",
        "createdAt": TIME, "updatedAt": TIME,
    }))
    .unwrap()
}

/// A data dir with two projects, `p1` "Main" and `p2` "Other" (empty), and
/// in `p1`:
///
/// - `c-lite` "lite": a SQLite file with `t (id INTEGER, s TEXT)` holding
///   `(1, 'one')` and `(2, NULL)`;
/// - `c-twin-a` and `c-twin-b`, both "twin", on the same file;
/// - `c-pg` "pg nopass": Postgres on `127.0.0.1:99999`, no saved password;
/// - `c-duck` "duck": DuckDB in memory (no helper is installed);
/// - saved queries `sq-1` "count t" and `sq-2` "by id" (with `{{id}}`).
async fn sandbox() -> Sandbox {
    let dir = tempfile::Builder::new()
        .prefix("cli-commands-")
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
    core.execute(&id, "INSERT INTO t VALUES (1, 'one'), (2, NULL)", vec![])
        .await
        .unwrap();
    core.disconnect(&id).await.unwrap();

    let ws = core
        .open_workspace(WorkspaceSpec::new(&data))
        .await
        .unwrap();
    let st = ws.storage();
    projects::save(st, &project("p1", "Main")).await.unwrap();
    projects::save(st, &project("p2", "Other")).await.unwrap();
    let lite_fields = json!({
        "databaseName": lite.to_str().unwrap(),
        "connectionString": format!("sqlite://{}", lite.display()),
    });
    for r in [
        row("c-lite", "lite", lite_fields.clone()),
        row("c-twin-a", "twin", lite_fields.clone()),
        row("c-twin-b", "twin", lite_fields),
        row(
            "c-pg",
            "pg nopass",
            json!({ "type": "postgres", "host": "127.0.0.1", "port": 99999,
                    "databaseName": "seaquel_test", "username": "alice", "savePassword": false }),
        ),
        row(
            "c-duck",
            "duck",
            json!({ "type": "duckdb", "databaseName": ":memory:" }),
        ),
    ] {
        connections::save(st, &r).await.unwrap();
    }
    let mut tx = st.write().await.unwrap();
    for q in [
        saved("sq-1", "count t", "SELECT count(*) AS n FROM t"),
        saved("sq-2", "by id", "SELECT s FROM t WHERE id = {{id}}"),
    ] {
        saved_queries::insert(&mut tx, &q).await.unwrap();
    }
    tx.commit().await.unwrap();
    ws.close().await;
    std::fs::write(dir.path().join("secrets.json"), "{}").unwrap();
    Sandbox { dir }
}

#[tokio::test]
async fn conn_list_prints_every_connection_as_json_when_piped() {
    let sb = sandbox().await;
    let out = sb.run(&["conn", "list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let rows: Vec<Json> = serde_json::from_str(&stdout(&out)).unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert!(
        names.contains(&"lite") && names.contains(&"duck"),
        "{names:?}"
    );
    assert_eq!(rows.len(), 5, "{names:?}");
    let lite = rows.iter().find(|r| r["id"] == "c-lite").unwrap();
    assert_eq!(lite["projectName"], "Main");
    assert_eq!(lite["type"], "sqlite");
    assert_eq!(lite["port"], Json::Null);
    assert_eq!(lite["ssh"], false);
    let pg = rows.iter().find(|r| r["id"] == "c-pg").unwrap();
    assert_eq!(pg["user"], "alice");
    // 99999 isn't a port.
    assert_eq!(pg["port"], Json::Null);
    // Never the connection string.
    assert!(!stdout(&out).contains("sqlite://"), "{}", stdout(&out));

    let out = sb.run(&["conn", "list", "--project", "Other", "--format", "table"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out).lines().count(),
        2,
        "header and rule only: {}",
        stdout(&out)
    );

    let out = sb.run(&["conn", "list", "--project", "Main", "--format", "table"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let table = stdout(&out);
    assert!(table.starts_with("NAME"), "{table}");
    assert_eq!(table.lines().count(), 7, "{table}");
}

#[tokio::test]
async fn conn_list_refuses_an_unknown_project() {
    let sb = sandbox().await;
    let out = sb.run(&["conn", "list", "--project", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).is_empty());
    let err = stderr(&out);
    assert!(
        err.starts_with("seaquel-cli conn list: PROJECT_NOT_FOUND: "),
        "{err}"
    );
}

#[tokio::test]
async fn conn_test_prints_ok() {
    let sb = sandbox().await;
    let out = sb.run(&["conn", "test", "lite"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "ok\n");
}

#[tokio::test]
async fn conn_test_without_a_terminal_doesnt_ask_and_says_how() {
    let sb = sandbox().await;
    let out = sb.run(&["conn", "test", "pg nopass"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).is_empty());
    let err = stderr(&out);
    assert!(err.starts_with("seaquel-cli conn test: "), "{err}");
    assert!(!err.contains("Password for"), "{err}");
    // Core refuses a row with no saved password before dialing, so the
    // code doesn't depend on the dead port.
    assert!(
        err.starts_with("seaquel-cli conn test: CREDENTIALS_REQUIRED: ")
            && err
                .trim_end()
                .ends_with("connect once. Run this in a terminal to be asked for it."),
        "{err}"
    );
}

#[tokio::test]
async fn an_ambiguous_name_lists_both_ids() {
    let sb = sandbox().await;
    let out = sb.run(&["conn", "test", "twin"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("AMBIGUOUS_CONNECTION")
            && err.contains("c-twin-a")
            && err.contains("c-twin-b"),
        "{err}"
    );
    // `conn test` takes the connection as an argument: no flag is named.
    assert!(err.contains("Connection \"twin\": "), "{err}");
    assert!(err.trim_end().ends_with("Pass the id instead."), "{err}");
    assert!(!err.contains("--connection"), "{err}");
}

#[tokio::test]
async fn duckdb_without_the_helper_says_how_to_install_it() {
    let sb = sandbox().await;
    let out = sb.run(&["conn", "test", "duck"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("seaquel-cli duckdb install"),
        "{}",
        stderr(&out)
    );
}

#[tokio::test]
async fn schema_lists_tables_and_describes_one() {
    let sb = sandbox().await;
    let out = sb.run(&["schema", "lite"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let tables: Vec<Json> = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(
        tables
            .iter()
            .any(|t| t["name"] == "t" && t["kind"] == "table"),
        "{tables:?}"
    );

    let out = sb.run(&["schema", "lite", "t"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let table: Json = serde_json::from_str(&stdout(&out)).unwrap();
    let cols: Vec<&str> = table["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(cols, ["id", "s"]);

    // By `schema.name` too, and as a table.
    let schema = tables.iter().find(|t| t["name"] == "t").unwrap()["schema"]
        .as_str()
        .unwrap()
        .to_string();
    let qualified = format!("{schema}.t");
    let out = sb.run(&["schema", "lite", &qualified, "--format", "table"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("NAME"), "{text}");
    assert!(text.contains("\nINDEX"), "{text}");

    let out = sb.run(&["schema", "lite", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).is_empty());
    assert!(
        stderr(&out).starts_with("seaquel-cli schema: TABLE_NOT_FOUND: "),
        "{}",
        stderr(&out)
    );
}

#[tokio::test]
async fn schema_without_a_terminal_doesnt_ask() {
    let sb = sandbox().await;
    let out = sb.run(&["schema", "pg nopass"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).is_empty());
    assert!(
        stderr(&out).starts_with("seaquel-cli schema: CREDENTIALS_REQUIRED: "),
        "{}",
        stderr(&out)
    );
}

#[tokio::test]
async fn saved_lists_and_shows() {
    let sb = sandbox().await;
    let out = sb.run(&["saved", "list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let rows: Vec<Json> = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["projectName"], "Main");
    let by_id = rows.iter().find(|r| r["id"] == "sq-2").unwrap();
    assert_eq!(by_id["parameters"], json!(["id"]));

    let out = sb.run(&["saved", "list", "--project", "Other"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        serde_json::from_str::<Json>(&stdout(&out)).unwrap(),
        json!([])
    );

    let out = sb.run(&["saved", "show", "count t"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "SELECT count(*) AS n FROM t\n");

    let out = sb.run(&["saved", "show", "sq-2", "--project", "Main"]);
    assert_eq!(stdout(&out), "SELECT s FROM t WHERE id = {{id}}\n");

    // Not in that project.
    let out = sb.run(&["saved", "show", "count t", "--project", "Other"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).starts_with("seaquel-cli saved show: SAVED_QUERY_NOT_FOUND: "),
        "{}",
        stderr(&out)
    );
}

fn json_lines(out: &Output) -> Vec<Json> {
    stdout(out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
async fn query_prints_one_json_line_per_statement() {
    let sb = sandbox().await;
    let out = sb.run(&[
        "query",
        "-c",
        "lite",
        "SELECT id, s FROM t ORDER BY id; SELECT 1 AS one",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let lines = json_lines(&out);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["statement"], 0);
    assert_eq!(lines[0]["columns"], json!(["id", "s"]));
    assert_eq!(lines[0]["rows"], json!([[1, "one"], [2, null]]));
    assert_eq!(lines[0]["truncated"], false);
    assert_eq!(lines[1]["rows"], json!([[1]]));
}

#[tokio::test]
async fn limit_pages_and_says_so() {
    let sb = sandbox().await;
    let out = sb.run(&[
        "query",
        "-c",
        "lite",
        "--limit",
        "1",
        "SELECT id FROM t ORDER BY id",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let line: Json = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(line["rows"], json!([[1]]));
    assert_eq!(line["totalRows"], 2);
    assert_eq!(line["truncated"], true);
}

#[tokio::test]
async fn a_destructive_run_needs_yes_when_not_interactive() {
    let sb = sandbox().await;
    let out = sb.run(&["query", "-c", "lite", "DELETE FROM t"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--yes"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("1. DELETE without WHERE: DELETE FROM t"),
        "{}",
        stderr(&out)
    );
    assert_eq!(stdout(&out), "");
    let count = sb.run(&["query", "-c", "lite", "SELECT count(*) AS n FROM t"]);
    assert!(
        stdout(&count).contains("[[2]]"),
        "nothing ran: {}",
        stdout(&count)
    );

    let out = sb.run(&["query", "-c", "lite", "DELETE FROM t", "--yes"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let line: Json = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(line["rowsAffected"], 2);
}

#[tokio::test]
async fn a_failing_statement_fails_the_command_but_the_rest_runs() {
    let sb = sandbox().await;
    let out = sb.run(&["query", "-c", "lite", "SELECT nope FROM t; SELECT 1 AS one"]);
    assert_eq!(out.status.code(), Some(1));
    let lines = json_lines(&out);
    assert!(lines[0]["error"]["code"].is_string(), "{:?}", lines[0]);
    assert_eq!(lines[1]["rows"], json!([[1]]));
}

#[tokio::test]
async fn saved_queries_run_with_their_parameters() {
    let sb = sandbox().await;
    let out = sb.run(&["query", "-c", "lite", "--saved", "by id"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("MISSING_PARAMETERS") && stderr(&out).contains("id"),
        "{}",
        stderr(&out)
    );
    let out = sb.run(&["query", "-c", "lite", "--saved", "by id", "--param", "id=1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("\"one\""), "{}", stdout(&out));
}

/// Missing parameters are refused before anything connects: an unknown
/// connection isn't even looked up.
#[tokio::test]
async fn missing_parameters_are_refused_before_connecting() {
    let sb = sandbox().await;
    let out = sb.run(&[
        "query",
        "-c",
        "no such",
        "SELECT {{a}}, {{b}}",
        "--param",
        "b=2",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("MISSING_PARAMETERS") && err.contains("{{a}}"),
        "{err}"
    );
    assert!(
        !err.contains("{{b}}") && !err.contains("CONNECTION"),
        "{err}"
    );
}

#[tokio::test]
async fn sql_can_come_from_stdin() {
    let sb = sandbox().await;
    let out = sb.run_with_stdin(&["query", "-c", "lite"], "SELECT 7 AS n");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("[[7]]"), "{}", stdout(&out));
    let out = sb.run_with_stdin(&["query", "-c", "lite", "-f", "-"], "SELECT 8 AS n");
    assert!(stdout(&out).contains("[[8]]"), "{}", stdout(&out));
}

#[tokio::test]
async fn blank_sql_is_nothing_to_run() {
    let sb = sandbox().await;
    let out = sb.run_with_stdin(&["query", "-c", "lite"], "  \n");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert!(stderr(&out).contains("nothing to run"), "{}", stderr(&out));
    // Only a comment: Core plans no statement.
    let out = sb.run(&["query", "-c", "lite", "-- just a note"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("nothing to run"), "{}", stderr(&out));
}

/// SQL starting with a comment is the SQL, and flags after it still count.
#[tokio::test]
async fn sql_may_start_with_a_comment() {
    let sb = sandbox().await;
    let out = sb.run(&[
        "query",
        "-c",
        "lite",
        "-- note\nSELECT 1 AS one",
        "--format",
        "json",
        "--yes",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let line: Json = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(line["rows"], json!([[1]]));
}

/// A mistyped flag alone isn't taken as SQL (a `--` comment), which
/// would ignore the script piped to stdin and succeed.
#[tokio::test]
async fn a_lone_mistyped_flag_is_a_usage_error() {
    let sb = sandbox().await;
    let out = sb.run_with_stdin(&["query", "-c", "lite", "--yse"], "DELETE FROM t");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("'--yse'"), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    // `-1` isn't flag-shaped: it reaches the database (as a syntax error).
    let out = sb.run(&["query", "-c", "lite", "-1"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(json_lines(&out)[0]["sql"], "-1");
}

#[tokio::test]
async fn the_table_format_puts_footers_on_stderr() {
    let sb = sandbox().await;
    let out = sb.run(&[
        "query",
        "-c",
        "lite",
        "--format",
        "table",
        "--limit",
        "1",
        "SELECT id, s FROM t ORDER BY id; SELECT s FROM t WHERE id = 2; SELECT nope",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "id  s", "{text}");
    assert_eq!(lines[2], "1   one", "{text}");
    // A blank line between the two tables, none after the last.
    assert_eq!(lines[3], "", "{text}");
    assert_eq!(lines[4], "s", "{text}");
    assert_eq!(lines[6], "NULL", "{text}");
    assert_eq!(lines.len(), 7, "{text}");
    assert!(!text.ends_with("\n\n"), "{text}");
    let err = stderr(&out);
    assert!(
        err.contains("1 of 2 rows (") && err.contains("; --limit 0 for all"),
        "{err}"
    );
    assert!(err.contains("\n1 row ("), "{err}");
    assert!(err.contains("statement 3: "), "{err}");
}
