//! The `db` group: wire shapes, tag order, redacted `Debug`, and dispatch
//! onto workspaces with SQLite temp databases (ownership, streams, cancel,
//! eviction events). One Postgres case runs when `SEAQUEL_TEST_POSTGRES`
//! holds a `ConnectConfig` JSON (the e2e Docker database); it fails instead
//! of skipping when `SEAQUEL_TEST_REQUIRE_ENGINES` is set.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::domain::run::RunEvent;
use seaquel_core::{with_plugins, ConnectPolicy, Core, Workspace, WorkspaceSpec};
use seaquel_rpc::{
    dispatch_stream, dispatch_workspace, parse_request, workspace_events, CoreEvent, DbRequest,
    DbResponse, Request, Response, RpcError,
};
use seaquel_types::{DbError, StreamEvent};
use serde_json::{json, Value as Json};

// ── Helpers ──

const MANY_ROWS: &str = "SELECT a.x, b.x FROM t a, t b";

struct Env {
    core: Arc<Core>,
    a: Arc<Workspace>,
    b: Arc<Workspace>,
    dir: tempfile::TempDir,
}

async fn env_with(core: Core) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let a = core
        .open_workspace(WorkspaceSpec::new(dir.path().join("a")))
        .await
        .unwrap();
    let b = core
        .open_workspace(WorkspaceSpec::new(dir.path().join("b")))
        .await
        .unwrap();
    Env {
        core: Arc::new(core),
        a,
        b,
        dir,
    }
}

async fn env() -> Env {
    env_with(
        with_plugins(|id| id == "sqlite")
            .connect_policy(ConnectPolicy::Unrestricted)
            .executor(Arc::new(seaquel_runtime::TokioExecutor))
            .build(),
    )
    .await
}

fn db(method: &str, params: Json) -> Json {
    json!({"method": "db", "params": {"method": method, "params": params}})
}

/// Parse `body` from its bytes, as the transports do.
fn parse(body: &Json) -> Request {
    parse_request(body.to_string().as_bytes()).unwrap()
}

impl Env {
    /// One `db` call on `ws`; the `result` of the call's JSON response.
    async fn call(&self, ws: &Workspace, method: &str, params: Json) -> Result<Json, RpcError> {
        let res = dispatch_workspace(&self.core, ws, parse(&db(method, params))).await?;
        let res = serde_json::to_value(&res).unwrap();
        assert_eq!(res["method"], "db");
        assert_eq!(res["result"]["method"], method);
        Ok(res["result"]["result"].clone())
    }

    /// A SQLite connection in `ws` through `db.connect`, with a table `t`
    /// of 200,000 rows for streams.
    async fn sqlite(&self, ws: &Workspace, name: &str) -> String {
        let file = self.dir.path().join(format!("{name}.db"));
        let res = self
            .call(ws, "connect", connect_form(&file.display().to_string()))
            .await
            .unwrap();
        let id = res["connectionId"].as_str().unwrap().to_string();
        self.call(
            ws,
            "execute",
            json!({"connectionId": id, "sql": "CREATE TABLE IF NOT EXISTS t AS WITH RECURSIVE \
                c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 200000) SELECT x FROM c"}),
        )
        .await
        .unwrap();
        id
    }

    fn stream<'a>(
        &'a self,
        ws: &'a Workspace,
        params: Json,
    ) -> Result<seaquel_engine::BoxStream<'a, CoreEvent>, RpcError> {
        dispatch_stream(&self.core, ws, parse(&db("queryStream", params)))
    }

    /// A `db.run` or `db.page` stream on `ws`.
    fn run<'a>(
        &'a self,
        ws: &'a Workspace,
        method: &str,
        params: Json,
    ) -> Result<seaquel_engine::BoxStream<'a, CoreEvent>, RpcError> {
        dispatch_stream(&self.core, ws, parse(&db(method, params)))
    }

    /// A `db.run` or `db.page` to its end: its events as JSON.
    async fn run_all(&self, ws: &Workspace, method: &str, params: Json) -> Vec<Json> {
        let events = self.run(ws, method, params).unwrap();
        tokio::time::timeout(Duration::from_secs(30), events.map(|e| wire(&e)).collect())
            .await
            .expect("the run ends")
    }
}

fn connect_form(file: &str) -> Json {
    json!({
        "target": {"type": "form", "form": {
            "name": "Lite", "type": "sqlite", "databaseName": file,
        }},
        "createIfMissing": true,
    })
}

fn not_found(result: Result<Json, RpcError>) {
    let err = result.unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND", "{err}");
}

fn wire(event: &CoreEvent) -> Json {
    serde_json::to_value(event).unwrap()
}

// ── Wire shapes ──

/// Each request parses from its wire JSON and serializes back to it, key
/// for key: the shapes the TypeScript sends.
#[test]
fn request_wire_snapshots() {
    let cases = [
        r#"{"method":"db","params":{"method":"connect","params":{"target":{"type":"saved","id":"c1"},"secrets":{"db":"pw"},"trustHostKey":"SHA256:abc","createIfMissing":false}}}"#,
        r#"{"method":"db","params":{"method":"connect","params":{"target":{"type":"form","form":{"name":"n","type":"postgres","host":"h","port":5432.0,"databaseName":"d","username":"u","sslMode":"require","connectionString":"","sshEnabled":false,"sshHost":"","sshPort":0.0,"sshUsername":"","sshAuthMethod":"","sshKeyPath":"","savePassword":true,"saveSshPassword":false,"saveSshKeyPassphrase":false}},"secrets":{"db":"pw","ssh":"sp","sshKey":"kp"},"createIfMissing":false}}}"#,
        r#"{"method":"db","params":{"method":"test","params":{"target":{"type":"saved","id":"c1"},"secrets":{},"createIfMissing":true}}}"#,
        r#"{"method":"db","params":{"method":"disconnect","params":{"connectionId":"c1"}}}"#,
        r#"{"method":"db","params":{"method":"query","params":{"connectionId":"c1","sql":"SELECT $1, $2","params":[1,{"$sq":"bigint","v":"9007199254740993"}]}}}"#,
        r#"{"method":"db","params":{"method":"execute","params":{"connectionId":"c1","sql":"DELETE FROM t","params":[]}}}"#,
        r#"{"method":"db","params":{"method":"transaction","params":{"connectionId":"c1","statements":[{"sql":"UPDATE t SET a = ?","params":["x"],"expectRows":{"min":1}}]}}}"#,
        r#"{"method":"db","params":{"method":"engine","params":{"connectionId":"c1","request":{"method":"tableMetadata","params":{"schema":"public","table":"t"}}}}}"#,
        r#"{"method":"db","params":{"method":"cancel","params":{"streamId":"s1"}}}"#,
        r#"{"method":"db","params":{"method":"queryStream","params":{"connectionId":"c1","streamId":"s1","sql":"SELECT 1","params":[],"readOnly":true,"maxRows":10,"maxBytes":1000,"timeoutMs":5000}}}"#,
    ];
    for case in cases {
        let req = parse_request(case.as_bytes()).unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(serde_json::to_string(&req).unwrap(), case);
    }
}

/// Optional fields may be left out.
#[test]
fn optional_request_fields_default() {
    let req = parse(&db(
        "connect",
        json!({"target": {"type": "form", "form": {"type": "sqlite"}}}),
    ));
    assert_eq!(req.method(), "connect");
    assert_eq!(
        serde_json::to_value(&req).unwrap()["params"]["params"],
        json!({"target": {"type": "form", "form": {
            "name": "", "type": "sqlite", "host": "", "port": 0.0, "databaseName": "",
            "username": "", "connectionString": "", "sshEnabled": false, "sshHost": "",
            "sshPort": 0.0, "sshUsername": "", "sshAuthMethod": "", "sshKeyPath": "",
            "savePassword": false, "saveSshPassword": false, "saveSshKeyPassphrase": false,
        }}, "secrets": {}, "createIfMissing": false})
    );
    let req = parse(&db(
        "queryStream",
        json!({"connectionId": "c", "streamId": "s", "sql": "SELECT 1"}),
    ));
    assert_eq!(
        serde_json::to_value(&req).unwrap()["params"]["params"],
        json!({"connectionId": "c", "streamId": "s", "sql": "SELECT 1", "params": [],
               "readOnly": false})
    );
}

#[test]
fn response_wire_snapshots() {
    let cases: Vec<(DbResponse, &str)> = vec![
        (
            DbResponse::Connect(seaquel_rpc::Connected {
                connection_id: "sqlite-1".into(),
            }),
            r#"{"method":"db","result":{"method":"connect","result":{"connectionId":"sqlite-1"}}}"#,
        ),
        (
            DbResponse::Test(()),
            r#"{"method":"db","result":{"method":"test","result":null}}"#,
        ),
        (
            DbResponse::Disconnect(()),
            r#"{"method":"db","result":{"method":"disconnect","result":null}}"#,
        ),
        (
            DbResponse::Query(seaquel_types::QueryResult {
                columns: vec!["a".into()],
                rows: vec![vec![seaquel_types::Value::Int(1)]],
            }),
            r#"{"method":"db","result":{"method":"query","result":{"columns":["a"],"rows":[[1]]}}}"#,
        ),
        (
            DbResponse::Execute(seaquel_types::ExecuteResult {
                rows_affected: 2,
                last_insert_id: Some(7),
            }),
            r#"{"method":"db","result":{"method":"execute","result":{"rows_affected":2,"last_insert_id":7}}}"#,
        ),
        (
            DbResponse::Transaction(()),
            r#"{"method":"db","result":{"method":"transaction","result":null}}"#,
        ),
        (
            DbResponse::Engine(seaquel_rpc::EngineResponse::Schemas(vec!["main".into()])),
            r#"{"method":"db","result":{"method":"engine","result":{"kind":"schemas","data":["main"]}}}"#,
        ),
        (
            DbResponse::Cancel(()),
            r#"{"method":"db","result":{"method":"cancel","result":null}}"#,
        ),
    ];
    // Responses only go out (the GUIs read them), so they aren't parsed back.
    for (res, expected) in cases {
        let text = serde_json::to_string(&Response::Db(res)).unwrap();
        assert_eq!(text, expected);
    }
}

#[test]
fn core_event_wire_snapshots() {
    let batch = CoreEvent::Stream {
        stream_id: "s1".into(),
        event: StreamEvent::Batch(seaquel_types::StreamBatch {
            columns: Some(vec!["a".into()]),
            rows: vec![vec![seaquel_types::Value::Int(1)]],
            is_final: true,
            truncated: false,
        }),
    };
    assert_eq!(
        serde_json::to_string(&batch).unwrap(),
        r#"{"type":"stream","streamId":"s1","event":{"type":"batch","columns":["a"],"rows":[[1]],"is_final":true}}"#
    );
    let done = CoreEvent::Stream {
        stream_id: "s1".into(),
        event: StreamEvent::Done,
    };
    assert_eq!(
        serde_json::to_string(&done).unwrap(),
        r#"{"type":"stream","streamId":"s1","event":{"type":"done"}}"#
    );
    let error = CoreEvent::Stream {
        stream_id: "s1".into(),
        event: StreamEvent::Error {
            message: "m".into(),
            code: "C".into(),
        },
    };
    assert_eq!(
        serde_json::to_string(&error).unwrap(),
        r#"{"type":"stream","streamId":"s1","event":{"type":"error","message":"m","code":"C"}}"#
    );
    let closed = CoreEvent::ConnectionClosed {
        connection_id: "c1".into(),
        code: seaquel_rpc::WORKSPACE_EVICTED.into(),
        message: "m".into(),
    };
    assert_eq!(
        serde_json::to_string(&closed).unwrap(),
        r#"{"type":"connectionClosed","connectionId":"c1","code":"WORKSPACE_EVICTED","message":"m"}"#
    );
    assert_eq!(seaquel_rpc::CONNECTION_CLOSED, "CONNECTION_CLOSED");
    assert_eq!(seaquel_rpc::TUNNEL_CLOSED, "TUNNEL_CLOSED");
}

/// `params` before `method` is refused at both levels, for `db` like every
/// group.
#[test]
fn method_must_come_before_params() {
    for body in [
        r#"{"params":{"method":"cancel","params":{"streamId":"s"}},"method":"db"}"#,
        r#"{"method":"db","params":{"params":{"streamId":"s"},"method":"cancel"}}"#,
        r#"{"method":"db","params":{"params":{"connectionId":"c","sql":"x","params":[]},"method":"query"}}"#,
    ] {
        let err = parse_request(body.as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
        assert!(
            err.message
                .contains("\"method\" must come before \"params\""),
            "{body}: {}",
            err.message
        );
    }
    // A query's own `params` (its values) is the third level, where order
    // is free.
    parse_request(
        br#"{"method":"db","params":{"method":"query","params":{"params":[1],"connectionId":"c","sql":"x"}}}"#,
    )
    .unwrap();
}

/// `Debug` of a connect request shows no secret and no connection string.
#[test]
fn connect_debug_is_redacted() {
    let req = parse(&db(
        "connect",
        json!({
            "target": {"type": "form", "form": {
                "type": "postgres",
                "connectionString": "postgres://u:hunter2-url@h/db",
            }},
            "secrets": {"db": "hunter2-db", "ssh": "hunter2-ssh", "sshKey": "hunter2-key"},
        }),
    ));
    let debug = format!("{req:?} {req:#?}");
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(debug.contains("<redacted>"), "{debug}");
    let Request::Db(DbRequest::Connect(params)) = req else {
        panic!()
    };
    let debug = format!("{params:?}");
    assert!(!debug.contains("hunter2"), "{debug}");
}

// ── Dispatch ──

/// Connect, query, execute, run a transaction, make an engine call and
/// disconnect, all through `dispatch_workspace`.
#[tokio::test]
async fn a_connection_round_trip() {
    let e = env().await;
    let file = e.dir.path().join("rt.db");
    let res = e
        .call(&e.a, "connect", connect_form(&file.display().to_string()))
        .await
        .unwrap();
    let id = res["connectionId"].as_str().unwrap().to_string();

    e.call(
        &e.a,
        "execute",
        json!({"connectionId": id, "sql": "CREATE TABLE t (a INTEGER, b TEXT)"}),
    )
    .await
    .unwrap();
    let res = e
        .call(
            &e.a,
            "execute",
            json!({"connectionId": id, "sql": "INSERT INTO t VALUES (?, ?)",
                   "params": [{"$sq": "bigint", "v": "9007199254740993"}, "x"]}),
        )
        .await
        .unwrap();
    assert_eq!(res["rows_affected"], 1);
    e.call(
        &e.a,
        "transaction",
        json!({"connectionId": id, "statements": [
            {"sql": "UPDATE t SET b = ?", "params": ["y"], "expectRows": {"min": 1}},
        ]}),
    )
    .await
    .unwrap();
    let res = e
        .call(
            &e.a,
            "query",
            json!({"connectionId": id, "sql": "SELECT a, b FROM t"}),
        )
        .await
        .unwrap();
    assert_eq!(
        res,
        json!({"columns": ["a", "b"], "rows": [[{"$sq": "bigint", "v": "9007199254740993"}, "y"]]})
    );
    let res = e
        .call(
            &e.a,
            "engine",
            json!({"connectionId": id, "request": {"method": "listSchemas"}}),
        )
        .await
        .unwrap();
    assert_eq!(res["kind"], "schemas");

    // `test` opens nothing that stays.
    e.call(&e.a, "test", connect_form(&file.display().to_string()))
        .await
        .unwrap();
    assert_eq!(e.a.connection_ids(&e.core), vec![id.clone()]);

    e.call(&e.a, "disconnect", json!({"connectionId": id}))
        .await
        .unwrap();
    assert!(e.a.connection_ids(&e.core).is_empty());
    not_found(
        e.call(&e.a, "disconnect", json!({"connectionId": id}))
            .await,
    );
}

/// `db.transaction`'s failures keep their wire: the error's code and
/// message, exactly as before Core learned which statement failed.
#[tokio::test]
async fn a_failed_transaction_keeps_its_wire_error() {
    let e = env().await;
    let id = e.sqlite(&e.a, "tx").await;
    let count = || async {
        e.call(
            &e.a,
            "query",
            json!({"connectionId": id, "sql": "SELECT COUNT(*) AS c FROM t"}),
        )
        .await
        .unwrap()["rows"][0][0]
            .clone()
    };
    let before = count().await;

    let err = e
        .call(
            &e.a,
            "transaction",
            json!({"connectionId": id, "statements": [
                {"sql": "INSERT INTO t VALUES (0)"},
                {"sql": "UPDATE t SET x = 1 WHERE x = ?", "params": [-5], "expectRows": {"min": 1}},
            ]}),
        )
        .await
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(&err).unwrap(),
        json!({
            "code": "NO_ROWS_AFFECTED",
            "message": "Statement 2 (index 1) affected 0 rows, expected at least 1. \
                        The transaction was rolled back.",
        })
    );

    let err = e
        .call(
            &e.a,
            "transaction",
            json!({"connectionId": id, "statements": [
                {"sql": "INSERT INTO t VALUES (0)"},
                {"sql": "INSERT INTO no_such_table VALUES (0)"},
            ]}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "EXECUTE_ERROR");
    assert!(
        err.message.starts_with("Execute failed: ") && err.message.contains("no_such_table"),
        "the database's message, nothing added: {}",
        err.message
    );
    assert_eq!(count().await, before, "both rolled back");
}

/// Workspace B can't reach A's connection through any `db` call: every
/// refusal is `CONNECTION_NOT_FOUND`, and A's connection keeps working.
#[tokio::test]
async fn another_workspace_is_refused_through_dispatch() {
    let e = env().await;
    let a_id = e.sqlite(&e.a, "a").await;
    let b = &e.b;

    not_found(
        e.call(b, "query", json!({"connectionId": a_id, "sql": "SELECT 1"}))
            .await,
    );
    not_found(
        e.call(
            b,
            "execute",
            json!({"connectionId": a_id, "sql": "DELETE FROM t"}),
        )
        .await,
    );
    not_found(
        e.call(
            b,
            "transaction",
            json!({"connectionId": a_id, "statements": [{"sql": "DELETE FROM t"}]}),
        )
        .await,
    );
    not_found(
        e.call(
            b,
            "engine",
            json!({"connectionId": a_id, "request": {"method": "listSchemas"}}),
        )
        .await,
    );
    not_found(e.call(b, "disconnect", json!({"connectionId": a_id})).await);

    // A stream on A's connection: one error event, nothing registered.
    let events: Vec<CoreEvent> = e
        .stream(
            b,
            json!({"connectionId": a_id, "streamId": "s", "sql": "SELECT 1"}),
        )
        .unwrap()
        .collect()
        .await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        wire(&events[0])["event"]["code"],
        "CONNECTION_NOT_FOUND",
        "{events:?}"
    );
    assert_eq!(b.stream_count(&e.core), 0);

    // A still has it.
    let count = e
        .call(
            &e.a,
            "query",
            json!({"connectionId": a_id, "sql": "SELECT count(*) AS n FROM t"}),
        )
        .await
        .unwrap();
    assert_eq!(count["rows"], json!([[200000]]));
    assert_eq!(e.a.connection_ids(&e.core), vec![a_id]);
}

/// A stream dispatch yields `stream` events tagged with its id: batches,
/// then `done`.
#[tokio::test]
async fn a_stream_yields_batches_then_done() {
    let e = env().await;
    let id = e.sqlite(&e.a, "s").await;
    let events: Vec<Json> = e
        .stream(
            &e.a,
            json!({"connectionId": id, "streamId": "s1",
                   "sql": "SELECT x FROM t WHERE x <= ? ORDER BY x", "params": [3]}),
        )
        .unwrap()
        .map(|event| wire(&event))
        .collect()
        .await;
    let (last, batches) = events.split_last().unwrap();
    assert_eq!(
        last,
        &json!({"type": "stream", "streamId": "s1", "event": {"type": "done"}})
    );
    assert!(!batches.is_empty());
    let mut rows = Vec::new();
    for batch in batches {
        assert_eq!(batch["type"], "stream");
        assert_eq!(batch["streamId"], "s1");
        assert_eq!(batch["event"]["type"], "batch");
        rows.extend(batch["event"]["rows"].as_array().unwrap().iter().cloned());
    }
    assert_eq!(batches[0]["event"]["columns"], json!(["x"]));
    assert_eq!(rows, vec![json!([1]), json!([2]), json!([3])]);
    assert_eq!(e.a.stream_count(&e.core), 0);
}

/// Options reach Core: a row limit without `readOnly` is `INVALID_OPTIONS`,
/// and with it the final batch is truncated.
#[tokio::test]
async fn stream_options_reach_core() {
    let e = env().await;
    let id = e.sqlite(&e.a, "o").await;
    let events: Vec<Json> = e
        .stream(
            &e.a,
            json!({"connectionId": id, "streamId": "s", "sql": "SELECT x FROM t", "maxRows": 2}),
        )
        .unwrap()
        .map(|event| wire(&event))
        .collect()
        .await;
    assert_eq!(events.last().unwrap()["event"]["code"], "INVALID_OPTIONS");

    let events: Vec<Json> = e
        .stream(
            &e.a,
            json!({"connectionId": id, "streamId": "s", "sql": "SELECT x FROM t",
                   "readOnly": true, "maxRows": 2, "timeoutMs": 60000}),
        )
        .unwrap()
        .map(|event| wire(&event))
        .collect()
        .await;
    let batch = events
        .iter()
        .find(|e| e["event"]["is_final"] == true)
        .unwrap_or_else(|| panic!("{events:?}"));
    assert_eq!(batch["event"]["truncated"], true);
}

/// `db.cancel` is scoped to the workspace: B cancelling A's stream id
/// doesn't stop it; A's cancel does, with no `done` after it.
#[tokio::test]
async fn cancel_is_scoped_to_the_workspace() {
    let e = env().await;
    let id = e.sqlite(&e.a, "c").await;
    let mut stream = e
        .stream(
            &e.a,
            json!({"connectionId": id, "streamId": "s", "sql": MANY_ROWS}),
        )
        .unwrap();
    assert_eq!(
        wire(&stream.next().await.unwrap())["event"]["type"],
        "batch"
    );

    e.call(&e.b, "cancel", json!({"streamId": "s"}))
        .await
        .unwrap();
    assert_eq!(e.a.stream_count(&e.core), 1);
    assert_eq!(
        wire(&stream.next().await.unwrap())["event"]["type"],
        "batch"
    );

    e.call(&e.a, "cancel", json!({"streamId": "s"}))
        .await
        .unwrap();
    let rest: Vec<Json> = tokio::time::timeout(
        Duration::from_secs(30),
        stream.map(|event| wire(&event)).collect(),
    )
    .await
    .expect("the cancelled stream ends");
    assert!(
        rest.iter().all(|e| e["event"]["type"] == "batch"),
        "{rest:?}"
    );
    assert_eq!(e.a.stream_count(&e.core), 0);
}

/// `close_all` announces each connection it closes on the workspace's
/// events, as `connectionClosed` with `WORKSPACE_EVICTED`, to every
/// subscriber. The other workspace hears nothing.
#[tokio::test]
async fn close_all_sends_workspace_evicted() {
    let e = env().await;
    let mut events = workspace_events(&e.a);
    let mut second = workspace_events(&e.a);
    let mut b_events = workspace_events(&e.b);
    let a1 = e.sqlite(&e.a, "e1").await;
    let a2 = e.sqlite(&e.a, "e2").await;
    let b1 = e.sqlite(&e.b, "e3").await;

    e.a.close_all(&e.core).await;

    let mut closed = Vec::new();
    for _ in 0..2 {
        let event = tokio::time::timeout(Duration::from_secs(5), events.next())
            .await
            .unwrap()
            .unwrap();
        let event = wire(&event);
        assert_eq!(event["type"], "connectionClosed", "{event}");
        assert_eq!(event["code"], "WORKSPACE_EVICTED", "{event}");
        assert!(!event["message"].as_str().unwrap().is_empty());
        closed.push(event["connectionId"].as_str().unwrap().to_string());
    }
    closed.sort();
    let mut expected = vec![a1, a2];
    expected.sort();
    assert_eq!(closed, expected);
    for _ in 0..2 {
        assert!(second.next().await.is_some());
    }
    // Nothing else is pending for A, and nothing at all for B.
    assert!(
        tokio::time::timeout(Duration::from_millis(50), events.next())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), b_events.next())
            .await
            .is_err()
    );

    // A can't connect again; B still works.
    let err = e
        .call(&e.a, "connect", connect_form("unused.db"))
        .await
        .unwrap_err();
    assert_eq!(err.code, "WORKSPACE_CLOSED", "{err}");
    e.call(&e.b, "disconnect", json!({"connectionId": b1}))
        .await
        .unwrap();
}

/// `db.queryStream` isn't a single call, and only it is a stream.
#[tokio::test]
async fn streams_and_calls_dont_mix() {
    let e = env().await;
    let err = dispatch_workspace(
        &e.core,
        &e.a,
        parse(&db(
            "queryStream",
            json!({"connectionId": "c", "streamId": "s", "sql": "SELECT 1"}),
        )),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    let Err(err) = dispatch_stream(
        &e.core,
        &e.a,
        parse(&db("cancel", json!({"streamId": "s"}))),
    ) else {
        panic!("a cancel isn't a stream")
    };
    assert_eq!(err.code, "INVALID_ARGUMENT");
}

/// A `Checked` policy (the web server's, Task 5) runs its check for both
/// `db.connect` and `db.test`, on a form and on a saved row, and its error
/// comes back as it is.
#[tokio::test]
async fn the_policy_check_covers_connect_and_test() {
    let core = with_plugins(|id| id == "sqlite")
        .connect_policy(ConnectPolicy::checked(
            |config| {
                Err(DbError {
                    message: format!("{} refused", config.driver.as_str()),
                    code: "CONNECTION_OPTION_NOT_ALLOWED".into(),
                })
            },
            false,
        ))
        .build();
    let e = env_with(core).await;
    let file = e.dir.path().join("x.db");
    let path = file.display().to_string();
    save_sqlite_row(&e, "c-saved", &path).await;
    let saved = json!({"target": {"type": "saved", "id": "c-saved"}, "createIfMissing": true});
    for target in [connect_form(&path), saved] {
        for method in ["connect", "test"] {
            let err = e.call(&e.a, method, target.clone()).await.unwrap_err();
            assert_eq!(
                err.code, "CONNECTION_OPTION_NOT_ALLOWED",
                "{method} {target}: {err}"
            );
            assert_eq!(err.message, "sqlite refused");
        }
    }
    assert!(!file.exists(), "nothing was opened");
    assert_eq!(e.core.connection_count(), 0);
}

/// A saved SQLite row `id` for the file at `path`, in workspace A.
async fn save_sqlite_row(e: &Env, id: &str, path: &str) {
    let storage = |method: &str, params: Json| json!({"method": "storage", "params": {"method": method, "params": params}});
    dispatch_workspace(
        &e.core,
        &e.a,
        parse(&storage(
            "projectsSave",
            json!({"project": {"id": "p1", "name": "P", "createdAt": "2026-01-02T03:04:05.000Z",
                   "updatedAt": "2026-01-02T03:04:05.000Z", "customLabels": []}}),
        )),
    )
    .await
    .unwrap();
    dispatch_workspace(
        &e.core,
        &e.a,
        parse(&storage(
            "connectionsSave",
            json!({"connection": {"id": id, "projectId": "p1", "name": "Saved", "type": "sqlite",
                   "host": "localhost", "port": 0, "databaseName": path, "username": "", "labelIds": [],
                   "connectionString": format!("sqlite://{path}")}}),
        )),
    )
    .await
    .unwrap();
}

/// Without a policy (the web server's Core until Task 5 sets one),
/// `db.connect` and `db.test` are refused for a form and a saved row.
#[tokio::test]
async fn without_a_policy_connect_and_test_are_refused() {
    let e = env_with(with_plugins(|id| id == "sqlite").build()).await;
    let file = e.dir.path().join("np.db");
    let path = file.display().to_string();
    save_sqlite_row(&e, "c-np", &path).await;
    let saved = json!({"target": {"type": "saved", "id": "c-np"}, "createIfMissing": true});
    for target in [connect_form(&path), saved] {
        for method in ["connect", "test"] {
            let err = e.call(&e.a, method, target.clone()).await.unwrap_err();
            assert_eq!(err.code, "NOT_SUPPORTED", "{method} {target}: {err}");
        }
    }
    assert!(!file.exists());
}

/// A saved id that isn't stored is `CONNECTION_NOT_FOUND`, with no secret
/// in the message.
#[tokio::test]
async fn a_missing_saved_connection() {
    let e = env().await;
    let err = e
        .call(
            &e.a,
            "connect",
            json!({"target": {"type": "saved", "id": "nope"}, "secrets": {"db": "hunter2"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND", "{err}");
    assert!(!err.message.contains("hunter2"));
}

// ── Live ──

/// Postgres through `db.connect` with a typed string and a supplied
/// password, then a query, a stream and a disconnect.
#[tokio::test]
async fn postgres_live() {
    let config = match std::env::var("SEAQUEL_TEST_POSTGRES") {
        Ok(raw) => serde_json::from_str::<Json>(&raw).expect("SEAQUEL_TEST_POSTGRES"),
        Err(_) if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("SEAQUEL_TEST_POSTGRES is not set, and SEAQUEL_TEST_REQUIRE_ENGINES requires it")
        }
        Err(_) => {
            eprintln!("skipping: SEAQUEL_TEST_POSTGRES is not set");
            return;
        }
    };
    let url = config["connection_string"].as_str().unwrap().to_string();
    let e = env_with(
        with_plugins(|id| id == "postgres")
            .connect_policy(ConnectPolicy::Unrestricted)
            .build(),
    )
    .await;
    let res = e
        .call(
            &e.a,
            "connect",
            json!({"target": {"type": "form", "form": {
                "name": "pg", "type": "postgres", "connectionString": url,
            }}}),
        )
        .await
        .unwrap();
    let id = res["connectionId"].as_str().unwrap().to_string();
    let res = e
        .call(
            &e.a,
            "query",
            json!({"connectionId": id, "sql": "SELECT $1::int + 1 AS n", "params": [41]}),
        )
        .await
        .unwrap();
    assert_eq!(res, json!({"columns": ["n"], "rows": [[42]]}));
    not_found(
        e.call(
            &e.b,
            "query",
            json!({"connectionId": id, "sql": "SELECT 1"}),
        )
        .await,
    );
    let events: Vec<Json> = e
        .stream(
            &e.a,
            json!({"connectionId": id, "streamId": "pg", "sql": "SELECT generate_series(1, 3)"}),
        )
        .unwrap()
        .map(|event| wire(&event))
        .collect()
        .await;
    assert_eq!(
        events.last().unwrap()["event"]["type"],
        "done",
        "{events:?}"
    );
    e.call(&e.a, "disconnect", json!({"connectionId": id}))
        .await
        .unwrap();
}

// ── db.run and db.page ──

/// Each run and page request parses from its wire JSON and serializes back
/// to it, `method` before `params`, absent optionals left out; the run
/// events go out as `{"type":"run","streamId",…,"event":{…}}`.
#[test]
fn run_and_page_wire_shapes() {
    let cases = [
        r#"{"method":"db","params":{"method":"run","params":{"connectionId":"c1","streamId":"r1","text":"SELECT 1; SELECT 2","target":{"type":"all"},"pageSize":100,"confirmed":false,"deferWrites":false}}}"#,
        r#"{"method":"db","params":{"method":"run","params":{"connectionId":"c1","streamId":"r2","text":"SELECT {{a}}","target":{"type":"current","cursor":3},"params":[{"name":"a","value":{"$sq":"bigint","v":"9007199254740993"}}],"pageSize":0,"confirmed":true,"deferWrites":true,"history":{"connectionId":"saved-1","connectionName":"Prod","connectionLabels":[{"id":"l1","name":"prod","color":"red"}]}}}}"#,
        r#"{"method":"db","params":{"method":"page","params":{"connectionId":"c1","streamId":"p1","source":{"sql":"SELECT $1","params":[1]},"page":2,"pageSize":100}}}"#,
    ];
    for case in cases {
        let req = parse_request(case.as_bytes()).unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(serde_json::to_string(&req).unwrap(), case);
    }
    // Defaults: confirmed and deferWrites may be left out.
    let req = parse(&db(
        "run",
        json!({"connectionId": "c", "streamId": "r", "text": "x", "target": {"type": "all"}, "pageSize": 1}),
    ));
    assert_eq!(req.method(), "run");
    assert_eq!(
        serde_json::to_value(&req).unwrap()["params"]["params"],
        json!({"connectionId": "c", "streamId": "r", "text": "x", "target": {"type": "all"},
               "pageSize": 1, "confirmed": false, "deferWrites": false})
    );
    // `params` before `method` is refused for run and page too.
    for body in [
        r#"{"method":"db","params":{"params":{"connectionId":"c","streamId":"r","text":"x","target":{"type":"all"},"pageSize":1},"method":"run"}}"#,
        r#"{"params":{"method":"page","params":{}},"method":"db"}"#,
    ] {
        let err = parse_request(body.as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{body}");
    }
    // History labels must be an array.
    let err = parse_request(
        db(
            "run",
            json!({"connectionId": "c", "streamId": "r", "text": "x", "target": {"type": "all"},
                   "pageSize": 1, "history": {"connectionId": "s", "connectionName": "n",
                   "connectionLabels": {"not": "an array"}}}),
        )
        .to_string()
        .as_bytes(),
    )
    .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");

    let run = |event| CoreEvent::Run {
        stream_id: "r1".into(),
        event,
    };
    let cases: Vec<(CoreEvent, &str)> = vec![
        (
            run(RunEvent::Batch(seaquel_types::StreamBatch {
                columns: Some(vec!["a".into()]),
                rows: vec![vec![seaquel_types::Value::Int(1)]],
                is_final: true,
                truncated: false,
            })),
            r#"{"type":"run","streamId":"r1","event":{"type":"batch","columns":["a"],"rows":[[1]],"is_final":true}}"#,
        ),
        (
            run(RunEvent::StatementDone {
                index: 0,
                elapsed_ms: 1.5,
                total_rows: 1,
                total_pages: 1,
                count_estimated: false,
                rows_affected: None,
                last_insert_id: None,
            }),
            r#"{"type":"run","streamId":"r1","event":{"type":"statementDone","index":0,"elapsedMs":1.5,"totalRows":1,"totalPages":1,"countEstimated":false}}"#,
        ),
        (
            run(RunEvent::Done {
                statements: 0,
                succeeded: false,
                history: None,
            }),
            r#"{"type":"run","streamId":"r1","event":{"type":"done","statements":0,"succeeded":false}}"#,
        ),
        (
            run(RunEvent::error("CONFIRM_REQUIRED", "m")),
            r#"{"type":"run","streamId":"r1","event":{"type":"error","code":"CONFIRM_REQUIRED","message":"m"}}"#,
        ),
    ];
    for (event, expected) in cases {
        assert_eq!(serde_json::to_string(&event).unwrap(), expected);
        assert_eq!(
            event.is_terminal(),
            expected.contains(r#""event":{"type":"done""#)
                || expected.contains(r#""event":{"type":"error""#)
        );
        assert_eq!(event.stream_id(), Some("r1"));
    }
}

/// `db.run` and `db.page` are streams: `dispatch_workspace` refuses them.
#[tokio::test]
async fn run_is_stream_only() {
    let e = env().await;
    for (method, params) in [
        (
            "run",
            json!({"connectionId": "c", "streamId": "r", "text": "SELECT 1", "target": {"type": "all"}, "pageSize": 10}),
        ),
        (
            "page",
            json!({"connectionId": "c", "streamId": "p", "source": {"sql": "SELECT 1", "params": []}, "page": 1, "pageSize": 10}),
        ),
    ] {
        let err = dispatch_workspace(&e.core, &e.a, parse(&db(method, params)))
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{method}: {err}");
    }
}

/// Every event of a run or a page is a `run` event with its `streamId`,
/// and the stream ends with exactly one terminal event.
#[tokio::test]
async fn run_events_carry_the_stream_id() {
    let e = env().await;
    let id = e.sqlite(&e.a, "r").await;
    let events = e
        .run_all(
            &e.a,
            "run",
            json!({"connectionId": id, "streamId": "r1", "pageSize": 100,
                   "text": "SELECT 1 AS a; SELECT x FROM t WHERE x <= 3; SELECT nope",
                   "target": {"type": "all"}}),
        )
        .await;
    for event in &events {
        assert_eq!(event["type"], "run", "{event}");
        assert_eq!(event["streamId"], "r1", "{event}");
    }
    let types: Vec<&str> = events
        .iter()
        .map(|e| e["event"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        [
            "statementStart",
            "batch",
            "statementDone",
            "statementStart",
            "batch",
            "statementDone",
            "statementStart",
            "statementError",
            "done"
        ],
        "{events:?}"
    );
    assert_eq!(events[4]["event"]["rows"], json!([[1], [2], [3]]));
    assert_eq!(events[8]["event"]["succeeded"], false);

    // Page 2 of the source the run sent.
    let source = events[3]["event"]["source"].clone();
    let events = e
        .run_all(
            &e.a,
            "page",
            json!({"connectionId": id, "streamId": "p1", "source": source,
                   "page": 2, "pageSize": 2}),
        )
        .await;
    assert!(events
        .iter()
        .all(|e| e["type"] == "run" && e["streamId"] == "p1"));
    let batch = events
        .iter()
        .find(|e| e["event"]["type"] == "batch")
        .unwrap();
    assert_eq!(batch["event"]["rows"], json!([[3]]));
    let done = events
        .iter()
        .find(|e| e["event"]["type"] == "statementDone")
        .unwrap();
    assert_eq!(done["event"]["totalRows"], 3);
    assert_eq!(done["event"]["totalPages"], 2);
    assert_eq!(events.last().unwrap()["event"]["type"], "done");

    // A refused page is one terminal error.
    let events = e
        .run_all(
            &e.a,
            "page",
            json!({"connectionId": id, "streamId": "p2", "source": {"sql": "DELETE FROM t", "params": []},
                   "page": 1, "pageSize": 2}),
        )
        .await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["event"]["code"], "INVALID_ARGUMENT");
    // And the table is untouched.
    let res = e
        .call(
            &e.a,
            "query",
            json!({"connectionId": id, "sql": "SELECT count(*) AS n FROM t"}),
        )
        .await
        .unwrap();
    assert_eq!(res["rows"], json!([[200000]]));
}

/// A destructive run without `confirmed` is one `CONFIRM_REQUIRED` error
/// listing the statements, and runs nothing.
#[tokio::test]
async fn an_unconfirmed_destructive_run_runs_nothing() {
    let e = env().await;
    let id = e.sqlite(&e.a, "d").await;
    let run = |confirmed: bool| {
        json!({"connectionId": id, "streamId": format!("d-{confirmed}"), "pageSize": 100,
               "text": "SELECT 1; DELETE FROM t", "target": {"type": "all"}, "confirmed": confirmed})
    };
    let events = e.run_all(&e.a, "run", run(false)).await;
    assert_eq!(events.len(), 1, "{events:?}");
    let error = &events[0]["event"];
    assert_eq!(error["type"], "error");
    assert_eq!(error["code"], "CONFIRM_REQUIRED");
    assert_eq!(error["destructive"][0]["index"], 1);
    assert_eq!(error["destructive"][0]["sql"], "DELETE FROM t");
    let count = || async {
        e.call(
            &e.a,
            "query",
            json!({"connectionId": id, "sql": "SELECT count(*) FROM t"}),
        )
        .await
        .unwrap()["rows"][0][0]
            .clone()
    };
    assert_eq!(count().await, 200000);
    let events = e.run_all(&e.a, "run", run(true)).await;
    assert_eq!(events.last().unwrap()["event"]["succeeded"], true);
    assert_eq!(count().await, 0);
}

/// `Debug` of a run or page request shows no text, SQL, parameter value or
/// history context.
#[test]
fn run_params_debug_shows_no_text_or_values() {
    let run = parse(&db(
        "run",
        json!({"connectionId": "c", "streamId": "r", "text": "SELECT 'canary-text' WHERE {{p}}",
               "target": {"type": "current", "cursor": 4},
               "params": [{"name": "p", "value": "canary-value"}], "pageSize": 10,
               "history": {"connectionId": "s", "connectionName": "canary-name",
                           "connectionLabels": [{"name": "canary-label"}]}}),
    ));
    let page = parse(&db(
        "page",
        json!({"connectionId": "c", "streamId": "p", "source": {"sql": "SELECT 'canary-sql'", "params": ["canary-bind"]},
               "page": 1, "pageSize": 10}),
    ));
    for req in [run, page] {
        let debug = format!("{req:?} {req:#?}");
        assert!(!debug.contains("canary"), "{debug}");
    }
    let event = CoreEvent::Run {
        stream_id: "r".into(),
        event: RunEvent::StatementError {
            index: 0,
            code: "C".into(),
            message: "m".into(),
            elapsed_ms: 0.0,
            sql: Some("SELECT 'canary-sql'".into()),
        },
    };
    let debug = format!("{event:?}");
    assert!(!debug.contains("canary"), "{debug}");
}

/// B can't run or page on A's connection, and B's `db.cancel` with A's run
/// id doesn't stop it; A's does, with no terminal event after it.
#[tokio::test]
async fn a_foreign_connection_is_not_found_through_dispatch() {
    let e = env().await;
    let id = e.sqlite(&e.a, "f").await;
    for (method, params) in [
        (
            "run",
            json!({"connectionId": id, "streamId": "x", "text": "SELECT 1", "target": {"type": "all"}, "pageSize": 10}),
        ),
        (
            "page",
            json!({"connectionId": id, "streamId": "y", "source": {"sql": "SELECT 1", "params": []}, "page": 1, "pageSize": 10}),
        ),
    ] {
        let events = e.run_all(&e.b, method, params).await;
        assert_eq!(events.len(), 1, "{method}: {events:?}");
        assert_eq!(events[0]["type"], "run");
        assert_eq!(events[0]["event"]["type"], "error");
        assert_eq!(events[0]["event"]["code"], "CONNECTION_NOT_FOUND");
    }

    // A long run of A's, streamed (page size 0).
    let mut run = e
        .run(
            &e.a,
            "run",
            json!({"connectionId": id, "streamId": "long", "pageSize": 0,
                   "text": format!("{MANY_ROWS}; SELECT 42"), "target": {"type": "all"}}),
        )
        .unwrap();
    assert_eq!(
        wire(&run.next().await.unwrap())["event"]["type"],
        "statementStart"
    );
    assert_eq!(wire(&run.next().await.unwrap())["event"]["type"], "batch");
    e.call(&e.b, "cancel", json!({"streamId": "long"}))
        .await
        .unwrap();
    assert_eq!(e.a.stream_count(&e.core), 1);
    assert_eq!(wire(&run.next().await.unwrap())["event"]["type"], "batch");

    e.call(&e.a, "cancel", json!({"streamId": "long"}))
        .await
        .unwrap();
    let rest: Vec<Json> = tokio::time::timeout(
        Duration::from_secs(30),
        run.map(|event| wire(&event)).collect(),
    )
    .await
    .expect("the cancelled run ends");
    assert!(
        rest.iter().all(|e| e["event"]["type"] == "batch"),
        "{rest:?}"
    );
    assert_eq!(e.a.stream_count(&e.core), 0);
}

/// A Core with no executor can't run: one `NOT_SUPPORTED` error event.
#[tokio::test]
async fn without_an_executor_run_is_not_supported() {
    let e = env_with(
        with_plugins(|id| id == "sqlite")
            .connect_policy(ConnectPolicy::Unrestricted)
            .build(),
    )
    .await;
    let id = e.sqlite(&e.a, "n").await;
    let events = e
        .run_all(
            &e.a,
            "run",
            json!({"connectionId": id, "streamId": "r", "text": "SELECT 1", "target": {"type": "all"}, "pageSize": 10}),
        )
        .await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["event"]["code"], "NOT_SUPPORTED");
}

// ── db.planEdits, db.applyChanges, db.tablePage, db.duckdbExtension ──

/// Each edit request parses from its wire JSON and serializes back to it:
/// `method` before `params`, absent optionals left out, filter operators as
/// the data tab's text.
#[test]
fn edits_wire_shapes() {
    let cases = [
        r#"{"method":"db","params":{"method":"planEdits","params":{"connectionId":"c1","edits":[{"type":"updateCell","target":{"schema":"public","table":"t"},"key":[["id",1],["k",{"$sq":"bigint","v":"9007199254740993"}]],"column":"name","value":"x"},{"type":"setDefault","target":{"schema":"public","table":"t"},"key":[["id",1]],"column":"c"},{"type":"insertRow","target":{"schema":"main.s","table":"t"},"values":[["b",null],["a",{"$sq":"bytes","v":"AQI="}]]},{"type":"deleteRow","target":{"schema":"public","table":"t"},"key":[["id",2]]},{"type":"truncateTable","target":{"schema":"public","table":"t"}},{"type":"dropObject","target":{"schema":"public","table":"v"},"kind":"materializedView"}]}}}"#,
        r#"{"method":"db","params":{"method":"applyChanges","params":{"connectionId":"c1","changes":[{"type":"edit","id":"p1","edit":{"type":"deleteRow","target":{"schema":"public","table":"t"},"key":[["id",1]]}},{"type":"sql","id":"p2","sql":"UPDATE t SET a = $1","params":[{"$sq":"decimal","v":"1.50"}]}],"confirmed":true,"history":{"connectionId":"saved-1","connectionName":"Prod","connectionLabels":[{"id":"l1","name":"prod","color":"red"}]}}}}"#,
        r#"{"method":"db","params":{"method":"applyChanges","params":{"connectionId":"c1","changes":[],"confirmed":false}}}"#,
        r#"{"method":"db","params":{"method":"tablePage","params":{"connectionId":"c1","streamId":"t1","query":{"target":{"schema":"public","table":"t"},"filters":[{"column":"a","op":"NOT IN","value":"1, 2"},{"column":"b","op":"IS NOT NULL","value":""},{"column":"c","op":"!=","value":"x"}],"logic":"OR","sort":[{"column":"a","direction":"DESC"}]},"page":2,"pageSize":100}}}"#,
        r#"{"method":"db","params":{"method":"duckdbExtension","params":{"connectionId":"c1","action":{"type":"list"}}}}"#,
        r#"{"method":"db","params":{"method":"duckdbExtension","params":{"connectionId":"c1","action":{"type":"installAndLoad","name":"httpfs"}}}}"#,
    ];
    for case in cases {
        let req = parse_request(case.as_bytes()).unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(serde_json::to_string(&req).unwrap(), case);
    }

    // Defaults: confirmed, history, a typed change's params, a table
    // query's filters, logic and sort, a filter's value.
    let req = parse(&db(
        "applyChanges",
        json!({"connectionId": "c", "changes": [{"type": "sql", "id": "p", "sql": "DELETE FROM t"}]}),
    ));
    assert_eq!(req.method(), "applyChanges");
    assert_eq!(
        serde_json::to_value(&req).unwrap()["params"]["params"],
        json!({"connectionId": "c", "changes": [{"type": "sql", "id": "p", "sql": "DELETE FROM t", "params": []}],
               "confirmed": false})
    );
    let req = parse(&db(
        "tablePage",
        json!({"connectionId": "c", "streamId": "t", "page": 1, "pageSize": 10,
               "query": {"target": {"schema": "s", "table": "t"}, "filters": [{"column": "a", "op": "IS NULL"}]}}),
    ));
    assert_eq!(
        serde_json::to_value(&req).unwrap()["params"]["params"]["query"],
        json!({"target": {"schema": "s", "table": "t"},
               "filters": [{"column": "a", "op": "IS NULL", "value": ""}], "logic": "AND", "sort": []})
    );

    // Refused when read: an unknown operator, logic or direction, a key as
    // an object, an unknown edit or action, a bad value tag.
    for params in [
        (
            "tablePage",
            json!({"connectionId": "c", "streamId": "t", "page": 1, "pageSize": 10,
            "query": {"target": {"schema": "s", "table": "t"}, "filters": [{"column": "a", "op": "eq", "value": "1"}]}}),
        ),
        (
            "tablePage",
            json!({"connectionId": "c", "streamId": "t", "page": 1, "pageSize": 10,
            "query": {"target": {"schema": "s", "table": "t"}, "logic": "XOR"}}),
        ),
        (
            "tablePage",
            json!({"connectionId": "c", "streamId": "t", "page": 1, "pageSize": 10,
            "query": {"target": {"schema": "s", "table": "t"}, "sort": [{"column": "a", "direction": "down"}]}}),
        ),
        (
            "planEdits",
            json!({"connectionId": "c", "edits": [{"type": "deleteRow",
            "target": {"schema": "s", "table": "t"}, "key": {"id": 1}}]}),
        ),
        (
            "planEdits",
            json!({"connectionId": "c", "edits": [{"type": "dropDatabase",
            "target": {"schema": "s", "table": "t"}}]}),
        ),
        (
            "applyChanges",
            json!({"connectionId": "c", "changes": [{"type": "sql", "id": "p",
            "sql": "x", "params": [{"$sq": "nope", "v": 1}]}]}),
        ),
        (
            "duckdbExtension",
            json!({"connectionId": "c", "action": {"type": "uninstall", "name": "x"}}),
        ),
    ] {
        let err = parse_request(db(params.0, params.1.clone()).to_string().as_bytes()).unwrap_err();
        assert_eq!(
            err.code, "INVALID_ARGUMENT",
            "{}: {}",
            params.1, err.message
        );
    }
}

/// The unary results as they go out.
#[test]
fn edits_response_wire_shapes() {
    use seaquel_core::domain::edits::{
        ApplyFailure, ApplyMode, ApplyOutcome, ChangeResult, PlannedChange,
    };
    use seaquel_core::domain::run::DestructiveStatement;
    use seaquel_core::sql::statements::{ChangeSummary, ChangeVerb, DestructiveReason, QueryType};

    let cases: Vec<(DbResponse, &str)> = vec![
        (
            DbResponse::PlanEdits(vec![
                PlannedChange {
                    sql: "UPDATE \"t\" SET \"name\" = $1 WHERE \"id\" = $2".into(),
                    params: vec![
                        seaquel_types::Value::Text("x".into()),
                        seaquel_types::Value::Int(9007199254740993),
                    ],
                    query_type: QueryType::Update,
                    dml: true,
                    summary: Some(ChangeSummary {
                        verb: ChangeVerb::Update,
                        table: "t".into(),
                        column: Some("name".into()),
                    }),
                },
                PlannedChange {
                    sql: "DROP VIEW \"v\"".into(),
                    params: vec![],
                    query_type: QueryType::Other,
                    dml: false,
                    summary: None,
                },
            ]),
            r#"{"method":"db","result":{"method":"planEdits","result":[{"sql":"UPDATE \"t\" SET \"name\" = $1 WHERE \"id\" = $2","params":["x",{"$sq":"bigint","v":"9007199254740993"}],"queryType":"update","dml":true,"summary":{"verb":"update","table":"t","column":"name"}},{"sql":"DROP VIEW \"v\"","params":[],"queryType":"other","dml":false}]}}"#,
        ),
        (
            DbResponse::ApplyChanges(ApplyOutcome::Applied {
                mode: ApplyMode::InOrder,
                applied: 1,
                results: vec![ChangeResult {
                    id: "p1".into(),
                    rows_affected: 1,
                    last_insert_id: Some(7),
                }],
                failed: Some(ApplyFailure {
                    id: Some("p2".into()),
                    index: Some(1),
                    code: "NO_ROWS_AFFECTED".into(),
                    message: "m".into(),
                }),
                ddl: false,
                history: vec![],
            }),
            r#"{"method":"db","result":{"method":"applyChanges","result":{"outcome":"applied","mode":"inOrder","applied":1,"results":[{"id":"p1","rowsAffected":1,"lastInsertId":7}],"failed":{"id":"p2","index":1,"code":"NO_ROWS_AFFECTED","message":"m"},"ddl":false,"history":[]}}}"#,
        ),
        (
            DbResponse::ApplyChanges(ApplyOutcome::Applied {
                mode: ApplyMode::Atomic,
                applied: 0,
                results: vec![],
                failed: Some(ApplyFailure {
                    id: None,
                    index: None,
                    code: "EXECUTE_ERROR".into(),
                    message: "m".into(),
                }),
                ddl: false,
                history: vec![],
            }),
            r#"{"method":"db","result":{"method":"applyChanges","result":{"outcome":"applied","mode":"atomic","applied":0,"results":[],"failed":{"code":"EXECUTE_ERROR","message":"m"},"ddl":false,"history":[]}}}"#,
        ),
        (
            DbResponse::ApplyChanges(ApplyOutcome::ConfirmRequired {
                destructive: vec![DestructiveStatement {
                    index: 0,
                    sql: "TRUNCATE t".into(),
                    reason: DestructiveReason::Truncate,
                }],
                destructive_total: 1,
            }),
            r#"{"method":"db","result":{"method":"applyChanges","result":{"outcome":"confirmRequired","destructive":[{"index":0,"sql":"TRUNCATE t","reason":"truncate"}],"destructiveTotal":1}}}"#,
        ),
        (
            DbResponse::DuckdbExtension(None),
            r#"{"method":"db","result":{"method":"duckdbExtension","result":null}}"#,
        ),
        (
            DbResponse::DuckdbExtension(Some(seaquel_types::QueryResult {
                columns: vec!["extension_name".into()],
                rows: vec![vec![seaquel_types::Value::Text("json".into())]],
            })),
            r#"{"method":"db","result":{"method":"duckdbExtension","result":{"columns":["extension_name"],"rows":[["json"]]}}}"#,
        ),
    ];
    for (res, expected) in cases {
        assert_eq!(serde_json::to_string(&Response::Db(res)).unwrap(), expected);
    }
}

/// `db.tablePage` is a stream: `dispatch_workspace` refuses it. Its
/// `streamId` is the request's, and its events are run events.
#[tokio::test]
async fn table_page_is_stream_only() {
    let e = env().await;
    let req = parse(&db(
        "tablePage",
        json!({"connectionId": "c", "streamId": "tp", "page": 1, "pageSize": 10,
               "query": {"target": {"schema": "main", "table": "t"}}}),
    ));
    let Request::Db(db_req) = &req else { panic!() };
    assert_eq!(db_req.stream_id(), Some("tp"));
    assert!(db_req.is_run());
    let err = dispatch_workspace(&e.core, &e.a, req).await.unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT", "{err}");
}

/// `db.planEdits`, `db.applyChanges` and `db.duckdbExtension` are unary:
/// the stream transport refuses them, and they have no stream id.
#[tokio::test]
async fn apply_changes_is_unary() {
    let e = env().await;
    for (method, params) in [
        ("planEdits", json!({"connectionId": "c", "edits": []})),
        ("applyChanges", json!({"connectionId": "c", "changes": []})),
        (
            "duckdbExtension",
            json!({"connectionId": "c", "action": {"type": "list"}}),
        ),
    ] {
        let req = parse(&db(method, params));
        let Request::Db(db_req) = &req else { panic!() };
        assert_eq!(db_req.method(), method);
        assert_eq!(db_req.stream_id(), None, "{method}");
        assert!(!db_req.is_run(), "{method}");
        let Err(err) = dispatch_stream(&e.core, &e.a, req) else {
            panic!("{method} streamed")
        };
        assert_eq!(err.code, "INVALID_ARGUMENT", "{method}: {err}");
    }
}

/// `Debug` of every new request shows no value, key, filter value or SQL.
#[test]
fn edit_params_debug_shows_no_values() {
    let key = json!([["id", "canary-key"]]);
    let target = json!({"schema": "public", "table": "t"});
    let edits = json!([
        {"type": "updateCell", "target": target, "key": key, "column": "c", "value": "canary-value"},
        {"type": "setDefault", "target": target, "key": key, "column": "c"},
        {"type": "insertRow", "target": target, "values": [["c", "canary-insert"]]},
        {"type": "deleteRow", "target": target, "key": key},
    ]);
    let history = json!({"connectionId": "s", "connectionName": "canary-name",
                         "connectionLabels": [{"name": "canary-label"}]});
    let requests = [
        parse(&db(
            "planEdits",
            json!({"connectionId": "c", "edits": edits}),
        )),
        parse(&db(
            "applyChanges",
            json!({"connectionId": "c", "history": history, "changes": [
                {"type": "edit", "id": "p1", "edit": edits[0]},
                {"type": "sql", "id": "p2", "sql": "DELETE FROM t WHERE a = 'canary-sql'", "params": ["canary-param"]},
            ]}),
        )),
        parse(&db(
            "tablePage",
            json!({"connectionId": "c", "streamId": "t", "page": 1, "pageSize": 10,
            "query": {"target": target, "filters": [
                {"column": "a", "op": "=", "value": "canary-filter"},
                {"column": "b", "op": "IN", "value": "canary-in, 2"},
            ]}}),
        )),
    ];
    for req in requests {
        let debug = format!("{req:?} {req:#?}");
        assert!(!debug.contains("canary"), "{debug}");
    }
}

/// Plan, apply and page on a SQLite table through dispatch; every page
/// event is a run event under its `streamId`, ending with one `done`.
#[tokio::test]
async fn edits_round_trip_through_dispatch() {
    let e = env().await;
    let id = e.sqlite(&e.a, "ed").await;
    e.call(
        &e.a,
        "execute",
        json!({"connectionId": id, "sql": "CREATE TABLE p (id INTEGER PRIMARY KEY, name TEXT)"}),
    )
    .await
    .unwrap();
    let target = json!({"schema": "main", "table": "p"});

    let planned = e
        .call(
            &e.a,
            "planEdits",
            json!({"connectionId": id, "edits": [
                {"type": "insertRow", "target": target, "values": [["id", 1], ["name", "a"]]},
                {"type": "updateCell", "target": target, "key": [["id", 1]], "column": "name", "value": "b"},
            ]}),
        )
        .await
        .unwrap();
    assert_eq!(planned[0]["queryType"], "insert", "{planned}");
    assert_eq!(planned[0]["dml"], true);
    assert_eq!(planned[1]["queryType"], "update");
    assert_eq!(planned[1]["params"], json!(["b", 1]));
    assert_eq!(planned[1]["summary"]["verb"], "update");

    // Two DML changes: one transaction.
    let outcome = e
        .call(
            &e.a,
            "applyChanges",
            json!({"connectionId": id, "changes": [
                {"type": "edit", "id": "p1", "edit": {"type": "insertRow", "target": target, "values": [["id", 1], ["name", "a"]]}},
                {"type": "edit", "id": "p2", "edit": {"type": "updateCell", "target": target, "key": [["id", 1]], "column": "name", "value": "b"}},
            ]}),
        )
        .await
        .unwrap();
    assert_eq!(outcome["outcome"], "applied", "{outcome}");
    assert_eq!(outcome["mode"], "atomic");
    assert_eq!(outcome["applied"], 2);
    assert!(outcome.get("failed").is_none(), "{outcome}");

    // A stale key: NO_ROWS_AFFECTED on its change, as an outcome.
    let outcome = e
        .call(
            &e.a,
            "applyChanges",
            json!({"connectionId": id, "changes": [
                {"type": "edit", "id": "gone", "edit": {"type": "deleteRow", "target": target, "key": [["id", 99]]}},
            ]}),
        )
        .await
        .unwrap();
    assert_eq!(outcome["mode"], "single", "{outcome}");
    assert_eq!(outcome["applied"], 0);
    assert_eq!(outcome["failed"]["code"], "NO_ROWS_AFFECTED");
    assert_eq!(outcome["failed"]["id"], "gone");

    // A key that isn't the primary key is refused before anything runs.
    let outcome = e
        .call(
            &e.a,
            "applyChanges",
            json!({"connectionId": id, "changes": [
                {"type": "edit", "id": "k", "edit": {"type": "deleteRow", "target": target, "key": [["name", "b"]]}},
            ]}),
        )
        .await
        .unwrap();
    assert_eq!(outcome["failed"]["code"], "NOT_EDITABLE", "{outcome}");

    // A destructive change asks first, then applies when confirmed.
    let apply = |confirmed: bool| {
        json!({"connectionId": id, "confirmed": confirmed, "changes": [
            {"type": "edit", "id": "tr", "edit": {"type": "truncateTable", "target": {"schema": "main", "table": "t"}}},
        ]})
    };
    let outcome = e.call(&e.a, "applyChanges", apply(false)).await.unwrap();
    assert_eq!(outcome["outcome"], "confirmRequired", "{outcome}");
    assert_eq!(outcome["destructiveTotal"], 1);

    // A page of `p`, filtered and sorted.
    let events = e
        .run_all(
            &e.a,
            "tablePage",
            json!({"connectionId": id, "streamId": "tp1", "page": 1, "pageSize": 10,
                   "query": {"target": target, "filters": [{"column": "name", "op": "IN", "value": "b, c"}],
                             "sort": [{"column": "id", "direction": "DESC"}]}}),
        )
        .await;
    for event in &events {
        assert_eq!(event["type"], "run", "{event}");
        assert_eq!(event["streamId"], "tp1", "{event}");
    }
    let types: Vec<&str> = events
        .iter()
        .map(|e| e["event"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        ["statementStart", "batch", "statementDone", "done"],
        "{events:?}"
    );
    assert_eq!(events[0]["event"]["kind"], "page");
    assert_eq!(events[1]["event"]["rows"], json!([[1, "b"]]));
    assert_eq!(events[2]["event"]["totalRows"], 1);

    // A refused page is one terminal run error.
    let events = e
        .run_all(
            &e.a,
            "tablePage",
            json!({"connectionId": id, "streamId": "tp2", "page": 0, "pageSize": 10,
                   "query": {"target": target}}),
        )
        .await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["type"], "run");
    assert_eq!(events[0]["event"]["code"], "INVALID_ARGUMENT");

    // Extensions are DuckDB's only.
    let err = e
        .call(
            &e.a,
            "duckdbExtension",
            json!({"connectionId": id, "action": {"type": "list"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "NOT_SUPPORTED", "{err}");
}

/// Workspace B can't plan, apply, page or run an extension action on A's
/// connection: `CONNECTION_NOT_FOUND`, and A's table is untouched.
#[tokio::test]
async fn a_foreign_connection_is_not_found_for_edits() {
    let e = env().await;
    let id = e.sqlite(&e.a, "fe").await;
    let target = json!({"schema": "main", "table": "t"});
    for (method, params) in [
        (
            "planEdits",
            json!({"connectionId": id, "edits": [{"type": "truncateTable", "target": target}]}),
        ),
        (
            "applyChanges",
            json!({"connectionId": id, "confirmed": true, "changes": [
                {"type": "edit", "id": "x", "edit": {"type": "truncateTable", "target": target}}]}),
        ),
        (
            "duckdbExtension",
            json!({"connectionId": id, "action": {"type": "list"}}),
        ),
    ] {
        not_found(e.call(&e.b, method, params).await);
    }
    let events = e
        .run_all(
            &e.b,
            "tablePage",
            json!({"connectionId": id, "streamId": "x", "page": 1, "pageSize": 10, "query": {"target": target}}),
        )
        .await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["type"], "run");
    assert_eq!(events[0]["event"]["code"], "CONNECTION_NOT_FOUND");
    let res = e
        .call(
            &e.a,
            "query",
            json!({"connectionId": id, "sql": "SELECT count(*) FROM t"}),
        )
        .await
        .unwrap();
    assert_eq!(res["rows"], json!([[200000]]));
}

/// `db.cancel` stops a table page: nothing more arrives, and no terminal
/// event (the transports add their own).
#[tokio::test]
async fn db_cancel_stops_a_table_page() {
    let e = env().await;
    let id = e.sqlite(&e.a, "tc").await;
    // A view whose every row is a scan of a 200,000 × 200,000 join.
    e.call(
        &e.a,
        "execute",
        json!({"connectionId": id, "sql": "CREATE VIEW slow AS SELECT a.x AS x, b.x AS y FROM t a, t b"}),
    )
    .await
    .unwrap();
    let mut events = e
        .run(
            &e.a,
            "tablePage",
            json!({"connectionId": id, "streamId": "slow", "page": 1, "pageSize": 10,
                   "query": {"target": {"schema": "main", "table": "slow"},
                             "filters": [{"column": "y", "op": "=", "value": "-1"}]}}),
        )
        .unwrap();
    assert_eq!(
        wire(&events.next().await.unwrap())["event"]["type"],
        "statementStart"
    );
    assert_eq!(e.a.stream_count(&e.core), 1);
    e.call(&e.a, "cancel", json!({"streamId": "slow"}))
        .await
        .unwrap();
    let rest: Vec<Json> = tokio::time::timeout(
        Duration::from_secs(30),
        events.map(|event| wire(&event)).collect(),
    )
    .await
    .expect("the cancelled page ends");
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(e.a.stream_count(&e.core), 0);
}
