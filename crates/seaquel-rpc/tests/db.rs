//! The `db` group: wire shapes, tag order, redacted `Debug`, and dispatch
//! onto workspaces with SQLite temp databases (ownership, streams, cancel,
//! eviction events). One Postgres case runs when `SEAQUEL_TEST_POSTGRES`
//! holds a `ConnectConfig` JSON (the e2e Docker database); it fails instead
//! of skipping when `SEAQUEL_TEST_REQUIRE_ENGINES` is set.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
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

    fn stream(
        &self,
        ws: &Workspace,
        params: Json,
    ) -> Result<seaquel_engine::BoxStream<'_, CoreEvent>, RpcError> {
        dispatch_stream(&self.core, ws, parse(&db("queryStream", params)))
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
    for (res, expected) in cases {
        let text = serde_json::to_string(&Response::Db(res)).unwrap();
        assert_eq!(text, expected);
        // And back.
        let back: Response = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), expected);
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
