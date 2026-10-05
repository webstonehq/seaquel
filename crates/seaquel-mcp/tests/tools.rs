//! The MCP server in process: an rmcp client over a `tokio::io::duplex`
//! pair, against a temp workspace opened read-only, as `seaquel-cli mcp`
//! opens it.
//!
//! Storage is a temp dir, secrets a `MemoryStore` (wrapped so some reads
//! fail, as a denied keychain prompt does) and known_hosts a temp file:
//! nothing touches the real keychain, data dir or `~/.ssh`. The SQLite and
//! DuckDB connections are temp files. The Postgres cases use the e2e Docker
//! database through `SEAQUEL_TEST_POSTGRES` (ConnectConfig JSON) and skip
//! without it, unless `SEAQUEL_TEST_REQUIRE_ENGINES` is set.
//!
//! DuckDB runs in the `seaquel-duckdb` helper, as in the CLI:
//! `SEAQUEL_TEST_DUCKDB_HELPER`, else the one built beside the test binary
//! (`cargo test --workspace` builds it), installed into each test's temp
//! dir as a real install is laid out ([`plugins`]). With neither, the
//! tests fail naming `cargo build -p seaquel-duckdb`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use seaquel_core::secrets::{MemoryStore, SecretError, SecretOp, SecretStore};
use seaquel_core::storage::{app_state, connections, projects, saved_queries, StorageOptions};
use seaquel_core::{Core, WorkspaceSpec};
use seaquel_mcp::{McpServer, SecretWait, Selection, ServerOptions};
use seaquel_types::storage::{PersistedConnection, PersistedProject, PersistedSavedQuery};
use seaquel_types::ConnectConfig;
use serde_json::{json, Value as Json};

const P1: &str = "p1";
const P2: &str = "p2";
/// Saved for `pg-refused`; must never appear in any result, error or log.
const SECRET: &str = "S3cret-Hunter2-pw";
const ROWS: usize = 250;

// ── The workspace ──────────────────────────────────────────────────────────

/// A `MemoryStore` whose reads of some keys fail, as a denied keychain
/// prompt does.
struct FailingStore {
    inner: MemoryStore,
    fail: Vec<String>,
}

#[seaquel_runtime::async_trait]
impl SecretStore for FailingStore {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        if self.fail.iter().any(|k| k == key) {
            return Err(SecretError::Store {
                op: SecretOp::Get,
                key: key.to_string(),
                message: "the user denied access".to_string(),
            });
        }
        self.inner.get(key).await
    }
    async fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.inner.set(key, value).await
    }
    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.inner.delete(key).await
    }
}

fn project(id: &str, name: &str) -> PersistedProject {
    serde_json::from_value(json!({
        "id": id, "name": name,
        "createdAt": "2026-01-02T03:04:05.000Z", "updatedAt": "2026-01-02T03:04:05.000Z",
        "customLabels": [],
    }))
    .unwrap()
}

fn connection(id: &str, name: &str, project: &str, fields: Json) -> PersistedConnection {
    let mut row = json!({
        "id": id, "projectId": project, "name": name,
        "type": "sqlite", "host": "localhost", "port": 0, "databaseName": "",
        "username": "", "savePassword": false, "saveSshPassword": false,
        "saveSshKeyPassphrase": false, "labelIds": [],
    });
    for (k, v) in fields.as_object().unwrap() {
        row[k] = v.clone();
    }
    serde_json::from_value(row).unwrap()
}

fn saved(
    id: &str,
    name: &str,
    project: &str,
    sql: &str,
    parameters: Option<Json>,
) -> PersistedSavedQuery {
    let mut q = json!({
        "id": id, "name": name, "query": sql, "projectId": project,
        "createdAt": "2026-01-02T03:04:05.000Z", "updatedAt": "2026-01-02T03:04:05.000Z",
        "description": format!("{name} description"),
    });
    if let Some(p) = parameters {
        q["parameters"] = p;
    }
    serde_json::from_value(q).unwrap()
}

/// A live database's `ConnectConfig` JSON from `SEAQUEL_TEST_<name>`.
fn live(name: &str) -> Option<Json> {
    let var = format!("SEAQUEL_TEST_{name}");
    match std::env::var(&var) {
        Ok(raw) => Some(serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{var}: {e}"))),
        Err(_) if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("{var} is not set, and SEAQUEL_TEST_REQUIRE_ENGINES requires it")
        }
        Err(_) => {
            eprintln!("skipping: {var} is not set");
            None
        }
    }
}

/// Core's engines, DuckDB's through the helper ([`install_helper`])
/// installed under `dir` (once; a hard link, else a copy) as
/// `duckdb-helper/bin/duckdb/<version>/seaquel-duckdb`, each folder 0700,
/// so it goes with the test's temp dir.
fn plugins(dir: &Path) -> seaquel_core::CoreBuilder {
    seaquel_core::with_plugins(|id| id != "duckdb")
        .duckdb_helper(install_helper(&dir.join("duckdb-helper")))
}

/// The built helper: `SEAQUEL_TEST_DUCKDB_HELPER`, else `seaquel-duckdb`
/// beside the test binary (`target/<profile>/`).
fn built_helper() -> PathBuf {
    if let Some(path) = std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER") {
        return PathBuf::from(path);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            let profile = exe.parent()?.parent()?;
            let bin = profile.join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX));
            bin.is_file().then_some(bin)
        })
        .expect(
            "DuckDB needs its helper: set SEAQUEL_TEST_DUCKDB_HELPER or run \
             cargo build -p seaquel-duckdb",
        )
}

fn install_helper(root: &Path) -> seaquel_core::DuckdbHelper {
    let bin = built_helper();
    let out = std::process::Command::new(&bin)
        .arg("--version")
        .output()
        .unwrap();
    let version = String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .strip_prefix("seaquel-duckdb ")
        .expect("seaquel-duckdb <version>")
        .to_string();
    let duckdb = root.join("bin").join("duckdb");
    let helper = seaquel_core::DuckdbHelper {
        dir: duckdb.clone(),
        version: version.clone(),
    };
    let to = helper.path();
    if !to.exists() {
        let folder = duckdb.join(&version);
        std::fs::create_dir_all(&folder).unwrap();
        #[cfg(unix)]
        for d in [root, &root.join("bin"), &duckdb, &folder] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        // On Windows the start reads each level's DACL, up to the folder
        // above `bin`; a temp folder inherits whatever the checkout's has.
        #[cfg(windows)]
        for d in [root, &root.join("bin"), &duckdb, &folder] {
            seaquel_runtime::acl::make_private(d, true).unwrap();
        }
        // A hard link would share the built file's DACL, so Windows copies
        // it and makes the copy private.
        #[cfg(unix)]
        if std::fs::hard_link(&bin, &to).is_err() {
            std::fs::copy(&bin, &to).unwrap();
        }
        #[cfg(windows)]
        {
            std::fs::copy(&bin, &to).unwrap();
            seaquel_runtime::acl::make_private(&to, false).unwrap();
        }
    }
    helper
}

/// Create the SQLite and DuckDB databases through Core: `items` with
/// `ROWS` rows, `tags` in DuckDB's second schema, a view.
async fn seed_databases(dir: &Path) -> (PathBuf, PathBuf) {
    let core = plugins(dir)
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    let lite = dir.join("app.sqlite");
    let duck = dir.join("app.duckdb");

    let config: ConnectConfig = serde_json::from_value(json!({
        "driver": "sqlite",
        "connection_string": format!("sqlite://{}", lite.display()),
        "create_if_missing": true,
    }))
    .unwrap();
    let id = core.connect(&config).await.unwrap().connection_id;
    for sql in [
        "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL DEFAULT 'x', price REAL, data BLOB, big INTEGER)",
        "CREATE TABLE owners (id INTEGER PRIMARY KEY, item_id INTEGER REFERENCES items(id))",
        "CREATE INDEX items_name ON items(name)",
        "CREATE VIEW cheap AS SELECT id, name FROM items WHERE price < 10",
    ] {
        core.execute(&id, sql, vec![]).await.unwrap();
    }
    core.execute(
        &id,
        &format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {ROWS}) \
             INSERT INTO items (id, name, price, data, big) \
             SELECT i, 'item ' || i, i * 0.5, x'00ff', 9007199254740993 FROM n"
        ),
        vec![],
    )
    .await
    .unwrap();
    core.disconnect(&id).await.unwrap();

    let config: ConnectConfig = serde_json::from_value(json!({
        "driver": "duckdb",
        "path": duck.to_str().unwrap(),
        "create_if_missing": true,
    }))
    .unwrap();
    let id = core.connect(&config).await.unwrap().connection_id;
    for sql in [
        "CREATE TABLE items (id INTEGER PRIMARY KEY, name VARCHAR, amount DECIMAL(10,2), payload JSON)",
        "INSERT INTO items SELECT i, 'duck ' || i, i + 0.25, '{\"b\":1,\"a\":[1,2]}' FROM range(1, 21) t(i)",
        "CREATE SCHEMA extra",
        "CREATE TABLE extra.items (id INTEGER)",
    ] {
        core.execute(&id, sql, vec![]).await.unwrap();
    }
    core.disconnect(&id).await.unwrap();
    (lite, duck)
}

struct Seeded {
    dir: tempfile::TempDir,
}

/// The temp data dir: projects, saved connections, saved queries and
/// `app_state`, written through a writable workspace that is then closed.
async fn seed(ai_settings: Option<&str>) -> Seeded {
    let dir = tempfile::tempdir().unwrap();
    let (lite, duck) = seed_databases(dir.path()).await;
    let lite_url = format!("sqlite://{}", lite.display());
    let duck_path = duck.to_str().unwrap().to_string();

    let core = plugins(dir.path())
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let st = ws.storage();
    projects::save(st, &project(P1, "Main")).await.unwrap();
    projects::save(st, &project(P2, "Other")).await.unwrap();

    let lite_row = |id: &str, name: &str, project: &str, extra: Json| {
        let mut fields =
            json!({ "databaseName": lite.to_str().unwrap(), "connectionString": lite_url });
        for (k, v) in extra.as_object().unwrap() {
            fields[k] = v.clone();
        }
        connection(id, name, project, fields)
    };
    let mut rows = vec![
        lite_row(
            "c-lite",
            "lite",
            P1,
            json!({ "aiShareSchema": true, "aiShareData": true }),
        ),
        connection(
            "c-duck",
            "duck",
            P1,
            json!({ "type": "duckdb", "databaseName": duck_path,
                    "connectionString": format!("duckdb://{duck_path}"),
                    "aiShareSchema": true, "aiShareData": true }),
        ),
        lite_row(
            "c-private",
            "private",
            P1,
            json!({ "aiShareSchema": false, "aiShareData": false }),
        ),
        lite_row(
            "c-schema-only",
            "schema only",
            P1,
            json!({ "aiShareSchema": true, "aiShareData": false }),
        ),
        lite_row("c-default", "follows default", P1, json!({})),
        lite_row(
            "c-other",
            "other",
            P2,
            json!({ "aiShareSchema": true, "aiShareData": true }),
        ),
        lite_row("c-twin-a", "twin", P2, json!({})),
        lite_row("c-twin-b", "twin", P2, json!({})),
        connection(
            "c-pg-refused",
            "pg refused",
            P1,
            json!({ "type": "postgres", "host": "127.0.0.1", "port": 1,
                    "databaseName": "seaquel_test", "username": "alice", "savePassword": true,
                    "connectionString": "postgresql://alice@127.0.0.1:99999/seaquel_test",
                    "aiShareSchema": true, "aiShareData": true }),
        ),
        connection(
            "c-pg-denied",
            "pg denied",
            P1,
            json!({ "type": "postgres", "host": "127.0.0.1", "port": 1,
                    "databaseName": "seaquel_test", "username": "alice", "savePassword": true,
                    "connectionString": "postgresql://alice@127.0.0.1:1/seaquel_test",
                    "aiShareSchema": true, "aiShareData": true }),
        ),
    ];
    if let Some(env) = std::env::var("SEAQUEL_TEST_POSTGRES")
        .ok()
        .and_then(|raw| serde_json::from_str::<Json>(&raw).ok())
    {
        rows.push(connection(
            "c-pg",
            "pg",
            P1,
            json!({ "type": "postgres", "host": "127.0.0.1", "port": 5432,
                    "databaseName": "seaquel_test", "savePassword": true,
                    "connectionString": env["connection_string"],
                    "aiShareSchema": true, "aiShareData": true }),
        ));
    }
    for row in &rows {
        connections::save(st, row).await.unwrap();
    }

    saved_queries::save_all(
        st,
        P1,
        &[
            saved(
                "q-min",
                "items from",
                P1,
                "SELECT id, name FROM items WHERE id >= {{min_id}} AND name LIKE {{pattern}} ORDER BY id",
                Some(json!([
                    { "name": "min_id", "type": "number" },
                    { "name": "pattern", "type": "text", "defaultValue": "item%", "description": "LIKE pattern" },
                ])),
            ),
            saved("q-untyped", "by name", P1, "SELECT id FROM items WHERE name = {{name}}", None),
            saved("q-all", "all items", P1, "SELECT * FROM items ORDER BY id", None),
            saved("q-write", "sneaky", P1, "DELETE FROM items WHERE id = {{id}}", None),
            saved(
                "q-gap",
                "gap",
                P1,
                "SELECT id FROM items WHERE id >= {{min_id}} AND id <= {{max_id}} ORDER BY id",
                Some(json!([{ "name": "min_id", "type": "number" }])),
            ),
            saved("q-dup-1", "dup", P1, "SELECT 1 AS one", None),
            saved("q-dup-2", "dup", P1, "SELECT 2 AS two", None),
        ],
    )
    .await
    .unwrap();
    saved_queries::save_all(
        st,
        P2,
        &[saved(
            "q-other",
            "other project query",
            P2,
            "SELECT 1",
            None,
        )],
    )
    .await
    .unwrap();
    if let Some(settings) = ai_settings {
        app_state::set(st, "aiSettings", Some(settings))
            .await
            .unwrap();
    }
    ws.close().await;
    Seeded { dir }
}

// ── The server and its client ─────────────────────────────────────────────

struct Harness {
    _seeded: Seeded,
    core: Arc<Core>,
    server: McpServer,
    client: RunningService<RoleClient, ()>,
    serving: tokio::task::JoinHandle<()>,
}

/// The secrets most tests use: `pg refused`'s password, and a failing read
/// for `pg denied`.
async fn test_store() -> FailingStore {
    let store = FailingStore {
        inner: MemoryStore::new(),
        fail: vec!["db:c-pg-denied".to_string()],
    };
    store.set("db:c-pg-refused", SECRET).await.unwrap();
    store
}

async fn start_with(seeded: Seeded, selection: Selection, options: ServerOptions) -> Harness {
    let store = Arc::new(test_store().await);
    start_with_store(seeded, selection, options, store).await
}

async fn start_with_store(
    seeded: Seeded,
    selection: Selection,
    options: ServerOptions,
    store: Arc<dyn SecretStore>,
) -> Harness {
    let known_hosts = seeded.dir.path().join("known_hosts");
    let core = Arc::new(
        plugins(seeded.dir.path())
            .ssh_known_hosts(&known_hosts)
            .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
            .build(),
    );
    let spec = WorkspaceSpec::new(seeded.dir.path())
        .with_storage_options(StorageOptions {
            read_only: true,
            ..StorageOptions::default()
        })
        .with_secrets(store);
    let ws = core.open_workspace(spec).await.unwrap();
    let server = McpServer::start(core.clone(), ws, &selection, options)
        .await
        .unwrap();

    let (server_io, client_io) = tokio::io::duplex(1 << 16);
    let handler = server.clone();
    let serving = tokio::spawn(async move {
        let running = handler.serve(server_io).await.unwrap();
        running.waiting().await.unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
    Harness {
        _seeded: seeded,
        core,
        server,
        client,
        serving,
    }
}

fn expose(connections: &[&str]) -> Selection {
    Selection {
        connections: connections.iter().map(|s| s.to_string()).collect(),
        projects: Vec::new(),
    }
}

/// The connections most tests expose (the live one only when it's set).
fn standard() -> Selection {
    let mut names = vec![
        "lite",
        "duck",
        "private",
        "schema only",
        "follows default",
        "pg refused",
        "pg denied",
    ];
    if std::env::var_os("SEAQUEL_TEST_POSTGRES").is_some() {
        names.push("pg");
    }
    expose(&names)
}

async fn harness() -> Harness {
    start_with(seed(None).await, standard(), ServerOptions::default()).await
}

impl Harness {
    async fn call(&self, tool: &str, args: Json) -> CallToolResult {
        let mut params = CallToolRequestParams::new(tool.to_string());
        if let Json::Object(map) = args {
            params = params.with_arguments(map);
        }
        self.client.call_tool(params).await.unwrap()
    }

    /// A call that must succeed: its JSON.
    async fn ok(&self, tool: &str, args: Json) -> Json {
        let result = self.call(tool, args.clone()).await;
        let text = text(&result);
        assert_ne!(result.is_error, Some(true), "{tool} {args}: {text}");
        assert!(!text.contains(SECRET));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{tool}: {e}: {text}"))
    }

    /// A call that must fail: its `CODE: message` text.
    async fn err(&self, tool: &str, args: Json) -> String {
        let result = self.call(tool, args.clone()).await;
        let text = text(&result);
        assert_eq!(result.is_error, Some(true), "{tool} {args}: {text}");
        assert!(!text.contains(SECRET), "{text}");
        text
    }

    async fn stop(self) -> Arc<Core> {
        self.client.cancel().await.unwrap();
        self.serving.await.unwrap();
        self.server.close().await;
        self.core
    }
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_code(text: &str, code: &str) {
    assert!(
        text.starts_with(&format!("{code}: ")),
        "expected {code}, got {text}"
    );
}

// ── Handshake and tool list ────────────────────────────────────────────────

#[tokio::test]
async fn the_tools_are_listed_read_only_with_their_arguments() {
    let h = harness().await;
    let info = h.client.peer_info().unwrap();
    let server = info.server_info.clone().expect("server info");
    assert_eq!(server.name, "seaquel");
    assert!(info
        .instructions
        .as_deref()
        .unwrap_or_default()
        .contains("read-only"));

    let tools = h.client.list_all_tools().await.unwrap();
    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "describe_table",
            "explain_query",
            "list_connections",
            "list_saved_queries",
            "list_schemas",
            "list_tables",
            "run_query",
            "run_saved_query",
        ]
    );
    for tool in &tools {
        let annotations = tool.annotations.as_ref().expect("annotations");
        assert_eq!(annotations.read_only_hint, Some(true), "{}", tool.name);
    }
    let run_query = tools.iter().find(|t| t.name == "run_query").unwrap();
    let schema = serde_json::to_value(&run_query.input_schema).unwrap();
    assert_eq!(schema["required"], json!(["connection", "sql"]), "{schema}");
    assert!(schema["properties"]["max_rows"].is_object(), "{schema}");
    h.stop().await;
}

#[tokio::test]
async fn an_unknown_tool_is_a_protocol_error_and_bad_arguments_a_tool_error() {
    let h = harness().await;
    let err = h
        .client
        .call_tool(CallToolRequestParams::new("drop_database"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("tool not found"), "{err}");
    let text = h.err("run_query", json!({ "connection": "lite" })).await;
    assert!(text.contains("sql"), "{text}");
    h.stop().await;
}

// ── list_connections and the exposed set ───────────────────────────────────

#[tokio::test]
async fn list_connections_shows_the_exposed_set_and_its_sharing() {
    let h = start_with(
        seed(None).await,
        expose(&["lite", "c-private", "schema only", "follows default"]),
        ServerOptions::default(),
    )
    .await;
    let out = h.ok("list_connections", json!({})).await;
    assert_eq!(
        out,
        json!({ "connections": [
            { "name": "lite", "id": "c-lite", "engine": "sqlite", "project": "Main", "shareSchema": true, "shareData": true },
            { "name": "private", "id": "c-private", "engine": "sqlite", "project": "Main", "shareSchema": false, "shareData": false },
            { "name": "schema only", "id": "c-schema-only", "engine": "sqlite", "project": "Main", "shareSchema": true, "shareData": false },
            { "name": "follows default", "id": "c-default", "engine": "sqlite", "project": "Main", "shareSchema": true, "shareData": false },
        ]})
    );
    h.stop().await;
}

#[tokio::test]
async fn with_nothing_exposed_list_connections_explains_the_flags() {
    let h = start_with(
        seed(None).await,
        Selection::default(),
        ServerOptions::default(),
    )
    .await;
    let out = h.ok("list_connections", json!({})).await;
    assert_eq!(out["connections"], json!([]));
    let message = out["message"].as_str().unwrap();
    assert!(
        message.contains("--connection") && message.contains("--project"),
        "{message}"
    );
    let text = h
        .err(
            "run_query",
            json!({ "connection": "lite", "sql": "SELECT 1" }),
        )
        .await;
    assert_code(&text, "CONNECTION_NOT_FOUND");
    assert!(text.contains("--connection"), "{text}");
    assert_eq!(
        h.ok("list_saved_queries", json!({})).await,
        json!({ "savedQueries": [] })
    );
    h.stop().await;
}

#[tokio::test]
async fn a_project_exposes_its_connections() {
    let h = start_with(
        seed(None).await,
        Selection {
            connections: vec!["lite".into()],
            projects: vec!["Other".into()],
        },
        ServerOptions::default(),
    )
    .await;
    let out = h.ok("list_connections", json!({})).await;
    let ids: Vec<&str> = out["connections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["c-lite", "c-other", "c-twin-a", "c-twin-b"]);
    // Two exposed connections share a name: the name is ambiguous, the ids work.
    let text = h.err("list_schemas", json!({ "connection": "twin" })).await;
    assert_code(&text, "AMBIGUOUS_CONNECTION");
    assert!(
        text.contains("c-twin-a") && text.contains("c-twin-b"),
        "{text}"
    );
    h.ok("list_schemas", json!({ "connection": "c-twin-a" }))
        .await;
    h.stop().await;
}

#[tokio::test]
async fn unexposed_and_unknown_connections_are_not_found() {
    let h = harness().await;
    for name in [
        "other",
        "c-other",
        "LITE",
        " lite",
        "lite ",
        "nope",
        "c-lite\u{200b}",
    ] {
        for (tool, args) in [
            ("list_schemas", json!({ "connection": name })),
            ("list_tables", json!({ "connection": name })),
            (
                "describe_table",
                json!({ "connection": name, "table": "items" }),
            ),
            (
                "run_query",
                json!({ "connection": name, "sql": "SELECT 1" }),
            ),
            (
                "explain_query",
                json!({ "connection": name, "sql": "SELECT 1" }),
            ),
            (
                "run_saved_query",
                json!({ "connection": name, "saved_query": "q-all" }),
            ),
            ("list_saved_queries", json!({ "connection": name })),
        ] {
            let text = h.err(tool, args).await;
            assert_code(&text, "CONNECTION_NOT_FOUND");
        }
    }
    let core = h.stop().await;
    assert_eq!(core.connection_count(), 0);
}

#[tokio::test]
async fn startup_refuses_unknown_and_ambiguous_names() {
    let seeded = seed(None).await;
    let core = Arc::new(
        plugins(seeded.dir.path())
            .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
            .build(),
    );
    let spec = WorkspaceSpec::new(seeded.dir.path()).with_storage_options(StorageOptions {
        read_only: true,
        ..StorageOptions::default()
    });
    let ws = core.open_workspace(spec).await.unwrap();
    let start = |selection: Selection| {
        let (core, ws) = (core.clone(), ws.clone());
        async move {
            McpServer::start(core, ws, &selection, ServerOptions::default())
                .await
                .map(|_| ())
                .unwrap_err()
        }
    };

    let err = start(expose(&["twin"])).await;
    assert_eq!(err.code, "AMBIGUOUS_CONNECTION");
    assert!(
        err.message.contains("c-twin-a") && err.message.contains("c-twin-b"),
        "{}",
        err.message
    );
    let err = start(expose(&["Lite"])).await;
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");
    let err = start(Selection {
        connections: vec![],
        projects: vec!["main".into()],
    })
    .await;
    assert_eq!(err.code, "PROJECT_NOT_FOUND");
    // Ids and exact names work.
    let server = McpServer::start(
        core.clone(),
        ws.clone(),
        &Selection {
            connections: vec!["c-twin-a".into(), "lite".into()],
            projects: vec![P1.into()],
        },
        ServerOptions::default(),
    )
    .await
    .unwrap();
    assert!(server.exposed().iter().any(|c| c.id == "c-twin-a"));
    assert!(server.exposed().iter().any(|c| c.id == "c-duck"));
}

// ── Schema tools ──────────────────────────────────────────────────────────

#[tokio::test]
async fn schema_tools_on_sqlite() {
    let h = harness().await;
    assert_eq!(
        h.ok("list_schemas", json!({ "connection": "lite" })).await,
        json!({ "schemas": ["main"] })
    );
    let tables = h.ok("list_tables", json!({ "connection": "c-lite" })).await;
    let tables = tables["tables"].as_array().unwrap();
    let entry = |name: &str| tables.iter().find(|t| t["name"] == name).cloned().unwrap();
    assert_eq!(entry("items")["type"], "table");
    assert_eq!(entry("items")["schema"], "main");
    assert_eq!(entry("cheap")["type"], "view");
    assert!(h
        .ok(
            "list_tables",
            json!({ "connection": "lite", "schema": "nope" })
        )
        .await["tables"]
        .as_array()
        .unwrap()
        .is_empty());

    let items = h
        .ok(
            "describe_table",
            json!({ "connection": "lite", "table": "items" }),
        )
        .await;
    assert_eq!(items["schema"], "main");
    assert_eq!(items["type"], "table");
    let id = &items["columns"][0];
    assert_eq!(id["name"], "id");
    assert_eq!(id["primaryKey"], true);
    let name = &items["columns"][1];
    assert_eq!(
        (name["nullable"].clone(), name["default"].clone()),
        (json!(false), json!("'x'"))
    );
    assert!(items["indexes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["name"] == "items_name" && i["columns"] == json!(["name"])));
    let owners = h
        .ok(
            "describe_table",
            json!({ "connection": "lite", "schema": "main", "table": "owners" }),
        )
        .await;
    assert_eq!(
        owners["foreignKeys"],
        json!([{ "column": "item_id", "referencedSchema": "main", "referencedTable": "items", "referencedColumn": "id" }])
    );
    let text = h
        .err(
            "describe_table",
            json!({ "connection": "lite", "table": "missing" }),
        )
        .await;
    assert_code(&text, "TABLE_NOT_FOUND");
    h.stop().await;
}

#[tokio::test]
async fn schema_tools_on_duckdb() {
    let h = harness().await;
    let schemas = h.ok("list_schemas", json!({ "connection": "duck" })).await;
    let schemas = schemas["schemas"].as_array().unwrap();
    assert!(
        schemas.contains(&json!("main")) && schemas.contains(&json!("extra")),
        "{schemas:?}"
    );
    let tables = h
        .ok(
            "list_tables",
            json!({ "connection": "duck", "schema": "extra" }),
        )
        .await;
    assert_eq!(
        tables["tables"],
        json!([{ "schema": "extra", "name": "items", "type": "table" }])
    );
    // `items` is in two schemas.
    let text = h
        .err(
            "describe_table",
            json!({ "connection": "duck", "table": "items" }),
        )
        .await;
    assert_code(&text, "AMBIGUOUS_TABLE");
    let items = h
        .ok(
            "describe_table",
            json!({ "connection": "duck", "schema": "main", "table": "items" }),
        )
        .await;
    let names: Vec<&str> = items["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["id", "name", "amount", "payload"]);
    h.stop().await;
}

/// The DuckDB tools run in the helper: Core has a helper locator, and a
/// row past the helper's 16 MiB frame is refused (`tests/HELPER.md` in
/// the engine crate, "Limits").
#[tokio::test]
async fn duckdb_runs_in_the_helper() {
    let h = harness().await;
    assert!(h.core.duckdb_helper().is_some());
    let out = h
        .ok(
            "run_query",
            json!({ "connection": "duck", "sql": "SELECT 40 + 2 AS n" }),
        )
        .await;
    assert_eq!(out["rows"], json!([[42]]));
    let big = json!({ "connection": "duck", "sql": "SELECT repeat('x', 20 * 1024 * 1024) AS big" });
    let text = h.err("run_query", big).await;
    assert_code(&text, "RESULT_TOO_LARGE");
    h.stop().await;
}

/// The MCP server opens DuckDB restricted: files beyond the database are out
/// of reach even through the reads the token check lets pass (`read_csv`, a
/// path as a table), while the database's own tables still work.
#[tokio::test]
async fn duckdb_cannot_read_files_outside_its_database() {
    let seeded = seed(None).await;
    let csv = seeded.dir.path().join("outside.csv");
    std::fs::write(&csv, "secret_column\nfile-contents-42\n").unwrap();
    let csv = csv.to_str().unwrap().to_string();
    let h = start_with(seeded, standard(), ServerOptions::default()).await;
    for sql in [
        format!("SELECT * FROM read_csv('{csv}')"),
        format!("SELECT * FROM '{csv}'"),
        format!("SELECT * FROM read_text('{csv}')"),
        format!("SELECT count(*) FROM glob('{csv}')"),
    ] {
        let text = h
            .err("run_query", json!({ "connection": "duck", "sql": sql }))
            .await;
        assert!(!text.contains("file-contents-42"), "{sql}: {text}");
        assert!(
            !text.starts_with("READ_ONLY: "),
            "the database refused it: {text}"
        );
        assert!(text.to_lowercase().contains("disabled"), "{sql}: {text}");
    }
    let out = h
        .ok(
            "run_query",
            json!({ "connection": "duck", "sql": "SELECT count(*) AS n, min(name) FROM items" }),
        )
        .await;
    assert_eq!(out["rows"], json!([[20, "duck 1"]]));
    let out = h
        .ok(
            "run_query",
            json!({ "connection": "duck", "sql": "SELECT count(*) FROM extra.items" }),
        )
        .await;
    assert_eq!(out["rows"], json!([[0]]));
    h.stop().await;
}

#[tokio::test]
async fn schema_tools_are_refused_without_schema_sharing() {
    let h = harness().await;
    for (tool, args) in [
        ("list_schemas", json!({ "connection": "private" })),
        ("list_tables", json!({ "connection": "private" })),
        (
            "describe_table",
            json!({ "connection": "private", "table": "items" }),
        ),
    ] {
        let text = h.err(tool, args).await;
        assert_code(&text, "SCHEMA_SHARING_OFF");
        assert!(text.contains("\"private\""), "{text}");
    }
    // Refused before connecting.
    assert_eq!(h.server.open_connection_count(), 0);
    h.stop().await;
}

// ── run_query and explain_query ───────────────────────────────────────────

#[tokio::test]
async fn run_query_returns_compact_rows_rendered_like_the_app() {
    let h = harness().await;
    let out = h
        .ok(
            "run_query",
            json!({ "connection": "lite", "sql": "SELECT id, name, price, data, big, NULL AS n FROM items WHERE id <= 2 ORDER BY id" }),
        )
        .await;
    assert_eq!(
        out,
        json!({
            "columns": ["id", "name", "price", "data", "big", "n"],
            "rows": [
                [1, "item 1", 0.5, "\\x00ff", "9007199254740993", null],
                [2, "item 2", 1.0, "\\x00ff", "9007199254740993", null],
            ],
            "rowCount": 2,
            "truncated": false,
        })
    );
    let out = h
        .ok(
            "run_query",
            json!({ "connection": "duck", "sql": "SELECT amount, payload, 'NaN'::DOUBLE AS nan FROM items WHERE id = 1" }),
        )
        .await;
    assert_eq!(
        out["rows"],
        json!([["1.25", "{\"a\":[1,2],\"b\":1}", "NaN"]]),
        "{out}"
    );
    h.stop().await;
}

#[tokio::test]
async fn run_query_truncates_at_max_rows() {
    let h = harness().await;
    let sql = "SELECT id FROM items ORDER BY id";
    let out = h
        .ok("run_query", json!({ "connection": "lite", "sql": sql }))
        .await;
    assert_eq!(out["rowCount"], 100);
    assert_eq!(out["rows"].as_array().unwrap().len(), 100);
    assert_eq!(out["truncated"], true);
    assert!(out["message"].as_str().unwrap().contains("first 100 rows"));

    let out = h
        .ok(
            "run_query",
            json!({ "connection": "lite", "sql": sql, "max_rows": 10 }),
        )
        .await;
    assert_eq!(
        (out["rowCount"].clone(), out["truncated"].clone()),
        (json!(10), json!(true))
    );
    assert_eq!(out["rows"][9], json!([10]));

    let out = h
        .ok(
            "run_query",
            json!({ "connection": "lite", "sql": sql, "max_rows": 1000 }),
        )
        .await;
    assert_eq!(
        (out["rowCount"].clone(), out["truncated"].clone()),
        (json!(ROWS), json!(false))
    );
    assert!(out.get("message").is_none());

    let out = h
        .ok(
            "run_query",
            json!({ "connection": "lite", "sql": sql, "max_rows": ROWS }),
        )
        .await;
    assert_eq!(out["truncated"], false);

    for bad in [0, 1001] {
        let text = h
            .err(
                "run_query",
                json!({ "connection": "lite", "sql": sql, "max_rows": bad }),
            )
            .await;
        assert_code(&text, "INVALID_ARGUMENT");
    }
    h.stop().await;
}

#[tokio::test]
async fn writes_are_refused_and_change_nothing() {
    let h = harness().await;
    for conn in ["lite", "duck"] {
        for sql in [
            "DELETE FROM items",
            "UPDATE items SET name = 'x'",
            "INSERT INTO items (id, name) VALUES (9999, 'x')",
            "DROP TABLE items",
            "CREATE TABLE t (x INT)",
            "SELECT 1; DELETE FROM items",
        ] {
            let text = h
                .err("run_query", json!({ "connection": conn, "sql": sql }))
                .await;
            assert_code(&text, "READ_ONLY");
            let text = h
                .err("explain_query", json!({ "connection": conn, "sql": sql }))
                .await;
            assert_code(&text, "READ_ONLY");
        }
    }
    let count = h
        .ok(
            "run_query",
            json!({ "connection": "lite", "sql": "SELECT count(*) AS n FROM items" }),
        )
        .await;
    assert_eq!(count["rows"], json!([[ROWS]]));
    let count = h
        .ok(
            "run_query",
            json!({ "connection": "duck", "sql": "SELECT count(*) AS n FROM items" }),
        )
        .await;
    assert_eq!(count["rows"], json!([[20]]));
    h.stop().await;
}

#[tokio::test]
async fn data_tools_are_refused_without_data_sharing() {
    let h = harness().await;
    for conn in ["private", "schema only", "follows default"] {
        for (tool, args) in [
            (
                "run_query",
                json!({ "connection": conn, "sql": "SELECT 1" }),
            ),
            (
                "explain_query",
                json!({ "connection": conn, "sql": "SELECT 1" }),
            ),
            (
                "run_saved_query",
                json!({ "connection": conn, "saved_query": "all items" }),
            ),
        ] {
            let text = h.err(tool, args).await;
            assert_code(&text, "DATA_SHARING_OFF");
        }
    }
    // Schema sharing alone still allows the schema tools.
    h.ok("list_tables", json!({ "connection": "schema only" }))
        .await;
    h.ok("list_tables", json!({ "connection": "follows default" }))
        .await;
    h.stop().await;
}

#[tokio::test]
async fn the_global_default_decides_for_connections_that_follow_it() {
    let seeded = seed(Some(
        r#"{"providers":[],"shareSchemaGlobally":false,"shareDataGlobally":true}"#,
    ))
    .await;
    let h = start_with(seeded, standard(), ServerOptions::default()).await;
    let out = h.ok("list_connections", json!({})).await;
    let default = out["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "c-default")
        .cloned()
        .unwrap();
    assert_eq!(
        (default["shareSchema"].clone(), default["shareData"].clone()),
        (json!(false), json!(true))
    );
    let text = h
        .err("list_tables", json!({ "connection": "follows default" }))
        .await;
    assert_code(&text, "SCHEMA_SHARING_OFF");
    h.ok(
        "run_query",
        json!({ "connection": "follows default", "sql": "SELECT 1 AS one" }),
    )
    .await;
    // A connection's own flags win over the default.
    h.ok("list_tables", json!({ "connection": "lite" })).await;
    let text = h
        .err(
            "run_query",
            json!({ "connection": "schema only", "sql": "SELECT 1" }),
        )
        .await;
    assert_code(&text, "DATA_SHARING_OFF");
    h.stop().await;
}

/// Phase 5d-2: Core rewrites the `aiSettings` record from the
/// stored copy (`aiSettingsPatch`, the provider calls), keeping fields it
/// doesn't know. What it writes is what this reader takes: the flags the
/// patch set, over a record that held legacy provider fields and a newer
/// release's field, and after provider calls.
#[tokio::test]
async fn the_global_default_written_by_core_is_what_mcp_reads() {
    use seaquel_core::domain::state::AiSettingsPatch;
    use seaquel_core::WriteOrigin;
    use seaquel_mcp::exposed::global_sharing_from;

    let seeded = seed(Some(
        r#"{"enabled":true,"providers":[{"id":"p","name":"Old","provider":"openai-compatible","model":"m"}],"futureField":{"x":1}}"#,
    ))
    .await;
    let core = plugins(seeded.dir.path())
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    let ws = core
        .open_workspace(WorkspaceSpec::new(seeded.dir.path()))
        .await
        .unwrap();
    let o = WriteOrigin::none();
    let patch = |schema: bool, data: bool| -> AiSettingsPatch {
        serde_json::from_value(json!({"shareSchemaGlobally": schema, "shareDataGlobally": data}))
            .unwrap()
    };
    for (schema, data) in [(false, true), (true, true), (true, false), (false, false)] {
        let written = ws
            .patch_ai_settings(&core, &o, patch(schema, data))
            .await
            .unwrap();
        let stored = app_state::get(ws.storage(), "aiSettings").await.unwrap();
        assert_eq!(stored.as_deref(), Some(written.value.get()));
        let s = global_sharing_from(stored.as_deref());
        assert_eq!((s.schema, s.data), (schema, data), "{stored:?}");
    }
    // A provider call keeps the flags, and the newer field.
    ws.create_ai_provider(
        &core,
        &o,
        serde_json::from_value(json!({"name": "New", "type": "anthropic"})).unwrap(),
        None,
    )
    .await
    .unwrap();
    ws.patch_ai_settings(&core, &o, patch(false, true))
        .await
        .unwrap();
    let stored = app_state::get(ws.storage(), "aiSettings")
        .await
        .unwrap()
        .unwrap();
    assert!(stored.contains(r#""futureField":{"x":1}"#), "{stored}");
    ws.close().await;

    // The server reads it so.
    let h = start_with(seeded, standard(), ServerOptions::default()).await;
    let out = h.ok("list_connections", json!({})).await;
    let default = out["connections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "c-default")
        .cloned()
        .unwrap();
    assert_eq!(
        (default["shareSchema"].clone(), default["shareData"].clone()),
        (json!(false), json!(true))
    );
    let text = h
        .err("list_tables", json!({ "connection": "follows default" }))
        .await;
    assert_code(&text, "SCHEMA_SHARING_OFF");
    h.stop().await;
}

#[tokio::test]
async fn explain_query_returns_the_plan_text() {
    let h = harness().await;
    for conn in ["lite", "duck"] {
        let out = h
            .ok(
                "explain_query",
                json!({ "connection": conn, "sql": "SELECT * FROM items WHERE id = 3" }),
            )
            .await;
        let plan = out["plan"].as_str().unwrap();
        assert!(!plan.is_empty(), "{conn}: {out}");
    }
    // One statement only, and only a read: both refused before planning.
    for sql in [
        "SELECT 1; SELECT 2",
        "SELECT * FROM items; DELETE FROM items",
        "DELETE FROM items",
    ] {
        let text = h
            .err("explain_query", json!({ "connection": "lite", "sql": sql }))
            .await;
        assert_code(&text, "READ_ONLY");
    }
    let count = h
        .ok(
            "run_query",
            json!({ "connection": "lite", "sql": "SELECT count(*) FROM items" }),
        )
        .await;
    assert_eq!(count["rows"], json!([[ROWS]]));
    h.stop().await;
}

#[tokio::test]
async fn a_database_error_is_a_tool_error() {
    let h = harness().await;
    let text = h
        .err(
            "run_query",
            json!({ "connection": "lite", "sql": "SELECT * FROM no_such_table" }),
        )
        .await;
    assert!(text.contains("no_such_table"), "{text}");
    h.stop().await;
}

#[tokio::test]
async fn a_call_past_the_timeout_is_cancelled() {
    const TIMEOUT: Duration = Duration::from_secs(3);
    let seeded = seed(None).await;
    let h = start_with(
        seeded,
        standard(),
        ServerOptions::default().with_call_timeout(TIMEOUT),
    )
    .await;
    // Warm the connection up so the timeout covers the query only. The
    // warm-up connects (starts the helper, opens the file) under the same
    // timeout, which took about 900 ms on slow cores: hence seconds, not
    // milliseconds. The query below runs for minutes.
    h.ok(
        "run_query",
        json!({ "connection": "duck", "sql": "SELECT 1 AS one" }),
    )
    .await;
    let started = Instant::now();
    let text = h
        .err(
            "run_query",
            json!({ "connection": "duck", "sql": "SELECT count(*) FROM range(100000000000) a" }),
        )
        .await;
    assert_code(&text, "TIMEOUT");
    assert!(
        started.elapsed() < TIMEOUT + Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(h.core.running_stream_count(), 0);
    // The connection still works.
    h.ok(
        "run_query",
        json!({ "connection": "duck", "sql": "SELECT 2 AS two" }),
    )
    .await;
    h.stop().await;
}

#[tokio::test]
async fn long_cells_are_cut_and_big_results_stop_early() {
    let h = harness().await;
    // One 100,000-byte cell: cut at 64 KB and marked.
    let out = h
        .ok(
            "run_query",
            json!({ "connection": "lite",
                    "sql": "SELECT 1 AS id, replace(hex(zeroblob(50000)), '0', 'x') AS big" }),
        )
        .await;
    let big = &out["rows"][0][1];
    assert_eq!(big["truncated"], true, "{big}");
    assert_eq!(big["bytes"], 100_000);
    assert_eq!(big["text"].as_str().unwrap().len(), 64 * 1024);
    assert_eq!(out["rows"][0][0], 1);
    assert_eq!(out["truncatedCells"], 1);
    assert_eq!(out["truncated"], false);
    assert!(out["message"].as_str().unwrap().contains("64 KB"), "{out}");

    // 1000 rows of 60,000 bytes (60 MB) come back as about 4 MB.
    let result = h
        .call(
            "run_query",
            json!({ "connection": "lite", "max_rows": 1000,
                    "sql": "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n \
                            WHERE i < 1000) SELECT i, hex(zeroblob(30000)) AS pad FROM n" }),
        )
        .await;
    let text = text(&result);
    assert!(text.len() <= 4 * 1024 * 1024 + 4096, "{} bytes", text.len());
    let out: Json = serde_json::from_str(&text).unwrap();
    let rows = out["rowCount"].as_u64().unwrap();
    assert!((60..=70).contains(&rows), "{rows} rows");
    assert_eq!(out["truncated"], true);
    assert_eq!(out["rows"][0][0], 1);
    assert!(
        out["message"].as_str().unwrap().contains("4 MB limit"),
        "{}",
        out["message"]
    );
    h.stop().await;
}

/// 1,000 rows of 1 MB (1 GB) with `max_rows` 1,000: the driver stops at
/// its 8 MB byte budget instead of fetching them all (peak memory followed
/// the result's size before: 1–3.5 GB in these tests). The cells are cut to
/// 64 KB, so the 4 MB output cap isn't reached and the budget's note shows.
async fn huge_rows_stop_at_the_fetch_budget(h: &Harness, connection: &str, sql: &str) {
    let out = h
        .ok(
            "run_query",
            json!({ "connection": connection, "max_rows": 1000, "sql": sql }),
        )
        .await;
    // Each row holds 1 MB and a bit: the 8th reaches 8 MB.
    assert_eq!(out["rowCount"], 8, "{}", out["message"]);
    assert_eq!(out["truncated"], true);
    assert_eq!(out["truncatedCells"], 8);
    assert_eq!(out["rows"][7][0], 8);
    assert_eq!(out["rows"][0][1]["bytes"], 1024 * 1024);
    let message = out["message"].as_str().unwrap();
    assert!(message.contains("Only the first 8 rows"), "{message}");
    assert!(message.contains("8 MB limit"), "{message}");
}

#[tokio::test]
async fn huge_rows_stop_at_the_fetch_budget_on_sqlite() {
    let h = harness().await;
    huge_rows_stop_at_the_fetch_budget(
        &h,
        "lite",
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000) \
         SELECT i, hex(zeroblob(524288)) AS pad FROM n",
    )
    .await;
    h.stop().await;
}

#[tokio::test]
async fn huge_rows_stop_at_the_fetch_budget_on_postgres() {
    if live("POSTGRES").is_none() {
        return;
    }
    let h = harness().await;
    huge_rows_stop_at_the_fetch_budget(
        &h,
        "pg",
        "SELECT g AS i, repeat('x', 1048576) AS pad FROM generate_series(1, 1000) g",
    )
    .await;
    h.stop().await;
}

/// `test_store`, with every read first waiting `delay` (a keychain prompt).
struct SlowStore {
    inner: FailingStore,
    delay: Duration,
}

#[seaquel_runtime::async_trait]
impl SecretStore for SlowStore {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        tokio::time::sleep(self.delay).await;
        self.inner.get(key).await
    }
    async fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.inner.set(key, value).await
    }
    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.inner.delete(key).await
    }
}

async fn slow_store(delay: Duration) -> Arc<dyn SecretStore> {
    Arc::new(SlowStore {
        inner: test_store().await,
        delay,
    })
}

#[tokio::test]
async fn a_pending_keychain_read_is_left_out_of_the_timeout() {
    let timeout = Duration::from_millis(300);
    let delay = Duration::from_millis(900);
    // `pg refused` reads its password (slowly), then fails to connect at
    // once: the result is the connect error, not TIMEOUT.
    let wait = SecretWait::new();
    let h = start_with_store(
        seed(None).await,
        standard(),
        ServerOptions::default()
            .with_call_timeout(timeout)
            .with_secret_wait(wait.clone()),
        wait.watch(slow_store(delay).await),
    )
    .await;
    let started = Instant::now();
    let text = h
        .err(
            "run_query",
            json!({ "connection": "pg refused", "sql": "SELECT 1" }),
        )
        .await;
    assert_code(&text, "CONNECTION_ERROR");
    assert!(started.elapsed() >= delay);
    h.stop().await;

    // Without the SecretWait the read counts, and the call times out.
    let h = start_with_store(
        seed(None).await,
        standard(),
        ServerOptions::default().with_call_timeout(timeout),
        slow_store(delay).await,
    )
    .await;
    let text = h
        .err(
            "run_query",
            json!({ "connection": "pg refused", "sql": "SELECT 1" }),
        )
        .await;
    assert_code(&text, "TIMEOUT");
    h.stop().await;

    // A read pending past the SecretWait's own limit ends the call, with a
    // message about the prompt.
    let wait = SecretWait::with_limit(Duration::from_millis(400));
    let h = start_with_store(
        seed(None).await,
        standard(),
        ServerOptions::default()
            .with_call_timeout(timeout)
            .with_secret_wait(wait.clone()),
        wait.watch(slow_store(Duration::from_secs(30)).await),
    )
    .await;
    let started = Instant::now();
    let text = h
        .err(
            "run_query",
            json!({ "connection": "pg refused", "sql": "SELECT 1" }),
        )
        .await;
    assert_code(&text, "TIMEOUT");
    assert!(text.contains("keychain prompt"), "{text}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    h.stop().await;
}

// ── Saved queries ─────────────────────────────────────────────────────────

#[tokio::test]
async fn list_saved_queries_shows_the_exposed_projects_only() {
    let h = harness().await;
    let out = h.ok("list_saved_queries", json!({})).await;
    let queries = out["savedQueries"].as_array().unwrap();
    assert!(queries.iter().all(|q| q["project"] == "Main"), "{out}");
    let min = queries.iter().find(|q| q["id"] == "q-min").unwrap();
    // The exposed connections of Main that share their schema (`private`
    // doesn't).
    let mut sharing_schema = vec![
        "lite",
        "duck",
        "schema only",
        "follows default",
        "pg refused",
        "pg denied",
    ];
    if std::env::var_os("SEAQUEL_TEST_POSTGRES").is_some() {
        sharing_schema.push("pg");
    }
    assert_eq!(
        min,
        &json!({
            "id": "q-min", "name": "items from", "project": "Main",
            "connections": sharing_schema,
            "description": "items from description",
            "parameters": [
                { "name": "min_id", "type": "number" },
                { "name": "pattern", "type": "text", "default": "item%", "description": "LIKE pattern" },
            ],
        })
    );
    let untyped = queries.iter().find(|q| q["id"] == "q-untyped").unwrap();
    assert_eq!(
        untyped["parameters"],
        json!([{ "name": "name", "type": "text" }])
    );

    assert_eq!(
        h.ok("list_saved_queries", json!({ "connection": "duck" }))
            .await["savedQueries"]
            .as_array()
            .unwrap()
            .len(),
        queries.len()
    );
    h.ok("list_saved_queries", json!({ "project": "Main" }))
        .await;
    h.ok("list_saved_queries", json!({ "project": P1 })).await;
    let text = h
        .err("list_saved_queries", json!({ "project": "Other" }))
        .await;
    assert_code(&text, "PROJECT_NOT_FOUND");
    h.stop().await;
}

#[tokio::test]
async fn list_saved_queries_needs_a_connection_sharing_its_schema() {
    // Only `private` (schema sharing off) of project Main is exposed.
    let h = start_with(
        seed(None).await,
        expose(&["private"]),
        ServerOptions::default(),
    )
    .await;
    let out = h.ok("list_saved_queries", json!({})).await;
    assert_eq!(out["savedQueries"], json!([]), "{out}");
    let message = out["message"].as_str().unwrap();
    assert!(
        message.starts_with("7 saved queries are not listed"),
        "{message}"
    );
    for leak in ["items from", "min_id", "LIKE", "q-min"] {
        assert!(!out.to_string().contains(leak), "{leak} in {out}");
    }
    let text = h
        .err("list_saved_queries", json!({ "connection": "private" }))
        .await;
    assert_code(&text, "SCHEMA_SHARING_OFF");
    h.stop().await;

    // With a sharing connection in the project the queries are listed, for
    // that connection only; asking through `private` is still refused.
    let h = start_with(
        seed(None).await,
        expose(&["private", "lite"]),
        ServerOptions::default(),
    )
    .await;
    let out = h.ok("list_saved_queries", json!({})).await;
    let queries = out["savedQueries"].as_array().unwrap();
    assert_eq!(queries.len(), 7, "{out}");
    assert!(
        queries.iter().all(|q| q["connections"] == json!(["lite"])),
        "{out}"
    );
    assert!(out.get("message").is_none(), "{out}");
    let text = h
        .err("list_saved_queries", json!({ "connection": "private" }))
        .await;
    assert_code(&text, "SCHEMA_SHARING_OFF");
    h.stop().await;
}

#[tokio::test]
async fn a_parameter_without_a_definition_must_be_given() {
    let h = harness().await;
    // `q-gap` uses {{max_id}}, which its stored definitions leave out.
    let text = h
        .err(
            "run_saved_query",
            json!({ "connection": "lite", "saved_query": "q-gap", "params": { "min_id": 1 } }),
        )
        .await;
    assert_code(&text, "INVALID_PARAMETERS");
    assert!(text.contains("no value for max_id"), "{text}");
    let out = h
        .ok(
            "run_saved_query",
            json!({ "connection": "lite", "saved_query": "q-gap",
                    "params": { "min_id": 1, "max_id": "3" } }),
        )
        .await;
    assert_eq!(out["rows"], json!([[1], [2], [3]]));
    let listed = h.ok("list_saved_queries", json!({})).await;
    let gap = listed["savedQueries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["id"] == "q-gap")
        .unwrap()
        .clone();
    assert_eq!(
        gap["parameters"],
        json!([{ "name": "min_id", "type": "number" }, { "name": "max_id", "type": "text" }])
    );
    h.stop().await;
}

#[tokio::test]
async fn run_saved_query_fills_its_parameters() {
    let h = harness().await;
    let out = h
        .ok(
            "run_saved_query",
            json!({ "connection": "lite", "saved_query": "items from", "params": { "min_id": 248 } }),
        )
        .await;
    assert_eq!(
        out["rows"],
        json!([[248, "item 248"], [249, "item 249"], [250, "item 250"]])
    );

    let out = h
        .ok(
            "run_saved_query",
            json!({ "connection": "lite", "saved_query": "q-min",
                    "params": { "min_id": "10", "pattern": "item 1%" }, "max_rows": 2 }),
        )
        .await;
    assert_eq!(out["rows"], json!([[10, "item 10"], [11, "item 11"]]));
    assert_eq!(out["truncated"], true);

    // Quotes in a value stay inside it.
    let out = h
        .ok(
            "run_saved_query",
            json!({ "connection": "lite", "saved_query": "by name", "params": { "name": "x' OR '1'='1" } }),
        )
        .await;
    assert_eq!(out["rows"], json!([]));

    // DuckDB inlines the values.
    let out = h
        .ok(
            "run_saved_query",
            json!({ "connection": "duck", "saved_query": "by name", "params": { "name": "duck 3" } }),
        )
        .await;
    assert_eq!(out["rows"], json!([[3]]));
    h.stop().await;
}

#[tokio::test]
async fn run_saved_query_refusals() {
    let h = harness().await;
    let run = |saved: &str, params: Json| json!({ "connection": "lite", "saved_query": saved, "params": params });
    let text = h.err("run_saved_query", run("items from", json!({}))).await;
    assert_code(&text, "INVALID_PARAMETERS");
    assert!(text.contains("no value for min_id"), "{text}");
    let text = h
        .err(
            "run_saved_query",
            run("items from", json!({ "min_id": 1, "limit": 3 })),
        )
        .await;
    assert_code(&text, "INVALID_PARAMETERS");
    assert!(text.contains("takes no limit"), "{text}");
    let text = h
        .err("run_saved_query", run("all items", json!({ "x": 1 })))
        .await;
    assert_code(&text, "INVALID_PARAMETERS");
    let text = h
        .err("run_saved_query", run("sneaky", json!({ "id": 1 })))
        .await;
    assert_code(&text, "READ_ONLY");
    let text = h.err("run_saved_query", run("dup", json!({}))).await;
    assert_code(&text, "AMBIGUOUS_SAVED_QUERY");
    h.ok("run_saved_query", run("q-dup-2", json!({}))).await;
    // Another project's saved query, by id or name.
    for other in ["q-other", "other project query", "nope"] {
        let text = h.err("run_saved_query", run(other, json!({}))).await;
        assert_code(&text, "SAVED_QUERY_NOT_FOUND");
    }
    let count = h
        .ok(
            "run_query",
            json!({ "connection": "lite", "sql": "SELECT count(*) FROM items" }),
        )
        .await;
    assert_eq!(count["rows"], json!([[ROWS]]));
    h.stop().await;
}

// ── Connecting, secrets and closing ───────────────────────────────────────

#[tokio::test]
async fn connections_open_lazily_once_and_close_with_the_server() {
    let h = harness().await;
    assert_eq!(h.core.connection_count(), 0);
    h.ok("list_connections", json!({})).await;
    assert_eq!(h.core.connection_count(), 0);
    h.ok(
        "run_query",
        json!({ "connection": "lite", "sql": "SELECT 1 AS one" }),
    )
    .await;
    h.ok("list_tables", json!({ "connection": "c-lite" })).await;
    h.ok(
        "run_query",
        json!({ "connection": "duck", "sql": "SELECT 1 AS one" }),
    )
    .await;
    assert_eq!(h.core.connection_count(), 2);
    assert_eq!(h.server.open_connection_count(), 2);
    let core = h.stop().await;
    assert_eq!(core.connection_count(), 0);
}

#[tokio::test]
async fn an_unreadable_secret_is_a_tool_error() {
    let h = harness().await;
    let text = h
        .err(
            "run_query",
            json!({ "connection": "pg denied", "sql": "SELECT 1" }),
        )
        .await;
    assert_code(&text, "SECRET_UNREADABLE");
    assert!(text.contains("pg denied"), "{text}");
    assert!(text.to_lowercase().contains("keychain"), "{text}");
    assert_eq!(h.core.connection_count(), 0);
    h.stop().await;
}

/// A workspace without a secret store (never the CLI's, which always has
/// the keychain) connects a saved row with no password, as a form would,
/// so the call reaches the driver, which refuses it.
#[tokio::test]
async fn a_workspace_without_a_secret_store_connects_with_no_password() {
    let seeded = seed(None).await;
    let core = Arc::new(
        plugins(seeded.dir.path())
            .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
            .build(),
    );
    let spec = WorkspaceSpec::new(seeded.dir.path()).with_storage_options(StorageOptions {
        read_only: true,
        ..StorageOptions::default()
    });
    let ws = core.open_workspace(spec).await.unwrap();
    let server = McpServer::start(core.clone(), ws, &standard(), ServerOptions::default())
        .await
        .unwrap();
    let (server_io, client_io) = tokio::io::duplex(1 << 16);
    let handler = server.clone();
    let serving = tokio::spawn(async move {
        handler
            .serve(server_io)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
    let result = client
        .call_tool(
            CallToolRequestParams::new("run_query").with_arguments(
                json!({ "connection": "pg refused", "sql": "SELECT 1" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert_code(&text(&result), "CONNECTION_ERROR");
    client.cancel().await.unwrap();
    serving.await.unwrap();
    server.close().await;
}

#[tokio::test]
async fn a_failed_connect_never_shows_the_password() {
    let h = harness().await;
    // The saved string has an invalid port, so the driver fails at once
    // with the password back in the URL it parses. `err` checks that SECRET
    // appears nowhere in each result.
    for (tool, args) in [
        (
            "run_query",
            json!({ "connection": "pg refused", "sql": "SELECT 1" }),
        ),
        ("list_schemas", json!({ "connection": "pg refused" })),
        (
            "explain_query",
            json!({ "connection": "pg refused", "sql": "SELECT 1" }),
        ),
        (
            "describe_table",
            json!({ "connection": "pg refused", "table": "t" }),
        ),
        (
            "run_saved_query",
            json!({ "connection": "pg refused", "saved_query": "all items" }),
        ),
    ] {
        let text = h.err(tool, args).await;
        assert_code(&text, "CONNECTION_ERROR");
    }
    let listed = h.ok("list_connections", json!({})).await;
    assert!(!listed.to_string().contains(SECRET));
    // A failed open isn't cached: nothing is open.
    assert_eq!(h.core.connection_count(), 0);
    assert_eq!(h.server.open_connection_count(), 0);
    h.stop().await;
}

// ── Live Postgres ─────────────────────────────────────────────────────────

#[tokio::test]
async fn every_tool_on_postgres() {
    if live("POSTGRES").is_none() {
        return;
    }
    let h = harness().await;
    let schemas = h.ok("list_schemas", json!({ "connection": "pg" })).await;
    assert!(
        schemas["schemas"]
            .as_array()
            .unwrap()
            .contains(&json!("public")),
        "{schemas}"
    );
    let tables = h
        .ok(
            "list_tables",
            json!({ "connection": "pg", "schema": "public" }),
        )
        .await;
    let first = tables["tables"][0].clone();
    assert!(first["name"].is_string(), "{tables}");
    let described = h
        .ok(
            "describe_table",
            json!({ "connection": "pg", "schema": "public", "table": first["name"] }),
        )
        .await;
    assert!(
        !described["columns"].as_array().unwrap().is_empty(),
        "{described}"
    );

    let out = h
        .ok(
            "run_query",
            json!({ "connection": "pg", "sql": "SELECT g AS n, 12.50::numeric AS d, 9007199254740993::bigint AS b, '\\x01ab'::bytea AS bytes, '{\"k\": [1]}'::jsonb AS j FROM generate_series(1, 5000) g", "max_rows": 3 }),
        )
        .await;
    assert_eq!(out["rowCount"], 3);
    assert_eq!(out["truncated"], true);
    assert_eq!(
        out["rows"][0],
        json!([1, "12.50", "9007199254740993", "\\x01ab", "{\"k\":[1]}"])
    );

    let text = h
        .err(
            "run_query",
            json!({ "connection": "pg", "sql": "CREATE TABLE mcp_probe (x int)" }),
        )
        .await;
    assert_code(&text, "READ_ONLY");
    let plan = h
        .ok(
            "explain_query",
            json!({ "connection": "pg", "sql": "SELECT * FROM generate_series(1, 10)" }),
        )
        .await;
    assert!(
        plan["plan"].as_str().unwrap().contains("Function Scan"),
        "{plan}"
    );

    // `items` doesn't exist on Postgres: the saved query is the project's,
    // it's bound as `$1`, and the database's error comes back.
    let text = h
        .err(
            "run_saved_query",
            json!({ "connection": "pg", "saved_query": "by name", "params": { "name": "x" } }),
        )
        .await;
    assert!(text.contains("items"), "{text}");
    h.stop().await;
}

#[tokio::test]
async fn a_slow_postgres_query_times_out_and_is_cancelled() {
    if live("POSTGRES").is_none() {
        return;
    }
    let h = start_with(
        seed(None).await,
        standard(),
        ServerOptions::default().with_call_timeout(Duration::from_millis(500)),
    )
    .await;
    h.ok(
        "run_query",
        json!({ "connection": "pg", "sql": "SELECT 1 AS one" }),
    )
    .await;
    let started = Instant::now();
    let text = h
        .err("run_query", json!({ "connection": "pg", "sql": "SELECT /* mcp-timeout-probe */ count(*) FROM generate_series(1, 100000) a, generate_series(1, 100000) b" }))
        .await;
    assert_code(&text, "TIMEOUT");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(h.core.running_stream_count(), 0);
    // Neither the cancel nor the statement timeout leaves the query running
    // on the server.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let left = h
        .ok(
            "run_query",
            json!({ "connection": "pg", "sql": "SELECT count(*) FROM pg_stat_activity \
                    WHERE query LIKE '%mcp-timeout-probe%' AND state <> 'idle' \
                    AND pid <> pg_backend_pid()" }),
        )
        .await;
    assert_eq!(left["rows"], json!([[0]]), "a backend is still running it");
    let out = h
        .ok(
            "run_query",
            json!({ "connection": "pg", "sql": "SELECT 2 AS two" }),
        )
        .await;
    assert_eq!(out["rows"], json!([[2]]));
    h.stop().await;
}

/// Review M3 of the DuckDB helper's probe fixes: the server closes its
/// connections at once, not one after another. Two DuckDB connections run
/// through a stand-in helper (a Perl proxy in front of a copy of the built
/// helper) that takes 1.5 s to pass `close` on, so closing them in turn
/// would take 3 s. Needs `SEAQUEL_TEST_DUCKDB_HELPER`.
#[cfg(unix)]
#[tokio::test]
async fn closing_closes_the_connections_concurrently() {
    use std::os::unix::fs::PermissionsExt;
    let Some(bin) = std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER").map(PathBuf::from) else {
        eprintln!("skipping: SEAQUEL_TEST_DUCKDB_HELPER is not set");
        return;
    };
    let seeded = seed(None).await;
    let dir = seeded.dir.path().to_path_buf();
    // A second DuckDB file and its connection.
    let duck2 = dir.join("second.duckdb");
    let first = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "duckdb"))
        .expect("the seeded DuckDB file");
    std::fs::copy(&first, &duck2).unwrap();
    {
        let core = seaquel_core::with_plugins(|_| false)
            .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
            .build();
        let ws = core.open_workspace(WorkspaceSpec::new(&dir)).await.unwrap();
        let path = duck2.to_str().unwrap();
        connections::save(
            ws.storage(),
            &connection(
                "c-duck2",
                "duck2",
                P1,
                json!({ "type": "duckdb", "databaseName": path,
                        "connectionString": format!("duckdb://{path}"),
                        "aiShareSchema": true, "aiShareData": true }),
            ),
        )
        .await
        .unwrap();
        ws.close().await;
    }
    // The stand-in helper, installed as a real one is laid out.
    let real = dir.join("real-seaquel-duckdb");
    if std::fs::hard_link(&bin, &real).is_err() {
        std::fs::copy(&bin, &real).unwrap();
    }
    let helper = install_helper(&dir.join("duckdb-helper"));
    let script = format!(
        r#"#!/usr/bin/perl
use strict;
use warnings;
use IPC::Open2;
$SIG{{PIPE}} = 'IGNORE';
my $pid = open2(my $from, my $to, '{real}', @ARGV);
binmode STDIN; binmode STDOUT; binmode $from; binmode $to;
sub put {{ my ($fh, $d) = @_; while (length $d) {{ my $w = syswrite($fh, $d); exit 1 unless defined $w; substr($d, 0, $w, ''); }} }}
my ($buf, $in, $out) = ('', 1, 1);
while ($out) {{
  my $rin = '';
  vec($rin, fileno(STDIN), 1) = 1 if $in;
  vec($rin, fileno($from), 1) = 1;
  next unless select(my $rout = $rin, undef, undef, undef) > 0;
  if ($in && vec($rout, fileno(STDIN), 1)) {{
    my $n = sysread(STDIN, my $chunk, 65536);
    if (!$n) {{ $in = 0; close $to; }}
    else {{
      $buf .= $chunk;
      while (length($buf) >= 4) {{
        my $len = unpack('V', substr($buf, 0, 4));
        last if length($buf) < 4 + $len;
        my $frame = substr($buf, 0, 4 + $len, '');
        my $body = substr($frame, 4, 1) eq "\0" ? substr($frame, 9) : '';
        select(undef, undef, undef, 1.5) if $body =~ /^\x7b"type":"close"/;
        put($to, $frame);
      }}
    }}
  }}
  if (vec($rout, fileno($from), 1)) {{
    my $n = sysread($from, my $chunk, 65536);
    if (!$n) {{ $out = 0; }} else {{ put(\*STDOUT, $chunk); }}
  }}
}}
waitpid($pid, 0);
exit($? >> 8);
"#,
        real = real.to_string_lossy().replace('\'', "\\'"),
    );
    let at = helper.path();
    std::fs::remove_file(&at).unwrap();
    std::fs::write(&at, script).unwrap();
    std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o700)).unwrap();

    let core = Arc::new(
        seaquel_core::with_plugins(|id| id != "duckdb")
            .duckdb_helper(helper)
            .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
            .build(),
    );
    let spec = WorkspaceSpec::new(&dir).with_storage_options(StorageOptions {
        read_only: true,
        ..StorageOptions::default()
    });
    let ws = core.open_workspace(spec).await.unwrap();
    let server = McpServer::start(
        core.clone(),
        ws,
        &expose(&["duck", "duck2"]),
        ServerOptions::default(),
    )
    .await
    .unwrap();
    let (server_io, client_io) = tokio::io::duplex(1 << 16);
    let handler = server.clone();
    let serving = tokio::spawn(async move {
        let running = handler.serve(server_io).await.unwrap();
        running.waiting().await.unwrap();
    });
    let client = ().serve(client_io).await.unwrap();
    for name in ["duck", "duck2"] {
        let mut params = CallToolRequestParams::new("run_query".to_string());
        if let Json::Object(map) = json!({ "connection": name, "sql": "SELECT 1 AS n" }) {
            params = params.with_arguments(map);
        }
        let result = client.call_tool(params).await.unwrap();
        assert_ne!(result.is_error, Some(true), "{name}: {}", text(&result));
    }
    assert_eq!(server.open_connection_count(), 2);
    client.cancel().await.unwrap();
    serving.await.unwrap();
    let started = Instant::now();
    server.close().await;
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(1400) && took < Duration::from_millis(2600),
        "{took:?}"
    );
    // Both helpers and their proxies are gone.
    let gone = |p: &Path| {
        std::process::Command::new("pgrep")
            .arg("-f")
            .arg(p)
            .output()
            .unwrap()
            .stdout
            .is_empty()
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    while !(gone(&real) && gone(&at)) {
        assert!(Instant::now() < deadline, "a helper outlived the server");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
