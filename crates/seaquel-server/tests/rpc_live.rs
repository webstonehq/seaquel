//! Live checks of `/rpc` and `/rpc/stream` on the server's real Core
//! (`web_core()`, the web connect policy) against the e2e Docker databases.
//! Each runs only when its variable is set to a ConnectConfig JSON with a
//! `connection_string`, as for the engine smoke tests:
//!
//! - `SEAQUEL_TEST_POSTGRES`, e.g.
//!   `{"driver":"postgres","connection_string":"postgres://postgres@127.0.0.1:5432/seaquel_test"}`
//! - `SEAQUEL_TEST_MYSQL`, e.g.
//!   `{"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3306/seaquel_test"}`
//!
//! Each creates and drops its own `t5c_<pid>` table and `v5c_<pid>` view.

use std::sync::{Arc, Mutex, Once, PoisonError};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use seaquel_server::web_core;
use serde_json::{json, Value};

mod common;
use common::{event_types, open_stream, quiet, send, start, start_run_with, until_end, Env};

fn connection_string(var: &str) -> Option<String> {
    let raw = std::env::var(var).ok()?;
    let config: Value = serde_json::from_str(&raw).expect("a ConnectConfig JSON");
    Some(config["connection_string"].as_str()?.to_string())
}

/// Every record the server's logger would write, and every record of
/// Seaquel's own crates at any level, with its key-values.
static RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        seaquel_server::startup::logs(metadata) || metadata.target().starts_with("seaquel")
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        struct Kvs(String);
        impl<'kvs> log::kv::VisitSource<'kvs> for Kvs {
            fn visit_pair(
                &mut self,
                key: log::kv::Key<'kvs>,
                value: log::kv::Value<'kvs>,
            ) -> Result<(), log::kv::Error> {
                self.0.push_str(&format!(" {key}={value}"));
                Ok(())
            }
        }
        let mut kvs = Kvs(String::new());
        let _ = record.key_values().visit(&mut kvs);
        RECORDS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{} {}{}", record.target(), record.args(), kvs.0));
    }

    fn flush(&self) {}
}

fn capture_logs() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        log::set_logger(&Capture).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
    });
}

/// How to see the sleep running from another connection: a count of
/// active statements whose text holds the marker, not counting its own.
fn running_sql(ty: &str, marker: &str) -> String {
    match ty {
        "postgres" => format!(
            "SELECT count(*) FROM pg_stat_activity WHERE state = 'active' \
             AND pid <> pg_backend_pid() AND query LIKE '%{marker}%'"
        ),
        _ => format!(
            "SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE \
             COMMAND IN ('Query', 'Execute') AND ID <> CONNECTION_ID() AND INFO LIKE '%{marker}%'"
        ),
    }
}

async fn run(ty: &str, conn_str: String, sleep_sql: &str) {
    capture_logs();
    let env = Env::with_core(Arc::new(web_core()), 4, Arc::default());
    let addr = env.serve().await;
    let form = json!({"type": ty, "name": "live", "connectionString": conn_str});

    let (status, body) = env
        .db(
            "alice",
            "test",
            json!({"target": {"type": "form", "form": form}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "test: {body}");
    let c = env.connect("alice", form.clone()).await;

    let (status, body) = env
        .db(
            "alice",
            "query",
            json!({"connectionId": c, "sql": "SELECT 41 + 1 AS n"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["result"]["rows"], json!([[42]]), "{body}");
    let (status, body) = env
        .db(
            "bob",
            "query",
            json!({"connectionId": c, "sql": "SELECT 1"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let mut ws = open_stream(addr, "alice").await;
    send(
        &mut ws,
        &start("s1", &c, "SELECT 1 AS a UNION ALL SELECT 2"),
    )
    .await;
    let got = until_end(&mut ws, "s1", &mut Vec::new()).await;
    assert_eq!(got.last().unwrap()["event"]["type"], "done", "{got:?}");
    let rows: Vec<Value> = got
        .iter()
        .flat_map(|f| f["event"]["rows"].as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(rows, vec![json!([1]), json!([2])]);

    // A long query stops on cancel, and the connection keeps working.
    send(&mut ws, &start("s2", &c, sleep_sql)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    send(&mut ws, &json!({"op": "cancel", "streamId": "s2"})).await;
    send(&mut ws, &start("s3", &c, "SELECT 3")).await;
    let got = tokio::time::timeout(
        Duration::from_secs(10),
        until_end(&mut ws, "s3", &mut Vec::new()),
    )
    .await
    .expect("the cancelled query held the connection");
    assert_eq!(got.last().unwrap()["event"]["type"], "done", "{got:?}");

    // ── db.run over the socket ──

    // Run all, with a bound parameter, a canary in the text, the value and
    // the history context (whose append fails: the connection isn't saved).
    let canary = format!("canary5b{}", std::process::id());
    send(
        &mut ws,
        &start_run_with(
            "r1",
            json!({"connectionId": c, "target": {"type": "all"}, "pageSize": 2,
                   "text": format!("SELECT '{canary}-text' AS c, {{{{p}}}} AS v; \
                                    SELECT 1 AS n UNION ALL SELECT 2 UNION ALL SELECT 3; SELECT nope_{canary}"),
                   "params": [{"name": "p", "value": format!("{canary}-value")}],
                   "history": {"connectionId": format!("{canary}-saved"), "connectionName": format!("{canary}-name"),
                               "connectionLabels": [{"name": format!("{canary}-label")}]}}),
        ),
    )
    .await;
    let got = until_end(&mut ws, "r1", &mut Vec::new()).await;
    assert!(got.iter().all(|f| f["type"] == "run"), "{got:?}");
    assert_eq!(
        event_types(&got),
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
        "{got:?}"
    );
    assert_eq!(
        got[1]["event"]["rows"],
        json!([[format!("{canary}-text"), format!("{canary}-value")]])
    );
    assert_eq!(got[4]["event"]["rows"], json!([[1], [2]]));
    assert_eq!(got[5]["event"]["totalRows"], 3);
    assert_eq!(got[5]["event"]["totalPages"], 2);

    // Page 2 of the second statement, from its source.
    let source = got[3]["event"]["source"].clone();
    send(
        &mut ws,
        &json!({"op": "start", "streamId": "p1", "request": common::db("page", json!({
            "connectionId": c, "streamId": "p1", "source": source, "page": 2, "pageSize": 2,
        }))}),
    )
    .await;
    let got = until_end(&mut ws, "p1", &mut Vec::new()).await;
    assert_eq!(
        event_types(&got),
        ["statementStart", "batch", "statementDone", "done"],
        "{got:?}"
    );
    assert_eq!(got[1]["event"]["rows"], json!([[3]]));

    // A cancel frame stops the sleep on the server within 2 s, and the
    // statement after it never runs.
    let marker = format!("m5b_{}", std::process::id());
    send(
        &mut ws,
        &start_run_with(
            "r2",
            json!({"connectionId": c, "target": {"type": "all"}, "pageSize": 100,
                   "text": format!("{sleep_sql} AS {marker};\nSELECT 42 AS after_{marker}")}),
        ),
    )
    .await;
    let running = running_sql(ty, &marker);
    let count = |body: Value| body["result"]["result"]["rows"][0][0].as_i64().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (status, body) = env
            .db("alice", "query", json!({"connectionId": c, "sql": running}))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if count(body) == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "the sleep never started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    send(&mut ws, &json!({"op": "cancel", "streamId": "r2"})).await;
    let cancelled = Instant::now();
    loop {
        let (_, body) = env
            .db("alice", "query", json!({"connectionId": c, "sql": running}))
            .await;
        if count(body) == 0 {
            break;
        }
        assert!(
            cancelled.elapsed() < Duration::from_secs(2),
            "the sleep still runs on the server"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Only the sleep's start came, and nothing after the cancel.
    let got = until_end_or_quiet(&mut ws, "r2").await;
    assert_eq!(event_types(&got), ["statementStart"], "{got:?}");
    quiet(&mut ws, 300).await;

    let records = RECORDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(
        records.iter().any(|r| r.contains("activity=db.run")),
        "{records:#?}"
    );
    for record in &records {
        assert!(!record.contains(&canary), "{record}");
        assert!(!record.contains(&marker), "{record}");
    }

    edits(&env, &mut ws, ty, &c).await;

    let (status, body) = env
        .db("alice", "disconnect", json!({"connectionId": c}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The edits service over `/rpc` and a table page over `/rpc/stream`
/// (phase 5c): plan, an atomic apply, an atomic failure that leaves
/// nothing, a counted page, a cancelled page that stops on the server, and
/// the sidebar's DROP after confirmation. Nothing it logs holds a key, a
/// value or a filter value.
async fn edits(env: &Env, ws: &mut common::Ws, ty: &str, c: &str) {
    let pid = std::process::id();
    let table = format!("t5c_{pid}");
    let view = format!("v5c_{pid}");
    let schema = if ty == "postgres" {
        "public"
    } else {
        "seaquel_test"
    };
    let target = json!({"schema": schema, "table": table});
    let canary = format!("canary5c{pid}");
    let exec = |sql: String| async move {
        let (status, body) = env
            .db("alice", "execute", json!({"connectionId": c, "sql": sql}))
            .await;
        assert_eq!(status, StatusCode::OK, "{sql}: {body}");
    };
    let query = |sql: String| async move {
        let (status, body) = env
            .db("alice", "query", json!({"connectionId": c, "sql": sql}))
            .await;
        assert_eq!(status, StatusCode::OK, "{sql}: {body}");
        body["result"]["result"]["rows"].clone()
    };
    exec(format!("DROP VIEW IF EXISTS {view}")).await;
    exec(format!("DROP TABLE IF EXISTS {table}")).await;
    exec(format!(
        "CREATE TABLE {table} (id int PRIMARY KEY, name varchar(200))"
    ))
    .await;
    let values: Vec<String> = (1..=25).map(|i| format!("({i}, 'n{i}')")).collect();
    exec(format!(
        "INSERT INTO {table} (id, name) VALUES {}",
        values.join(", ")
    ))
    .await;

    let update = |id: i64, name: String| json!({"type": "updateCell", "target": target, "key": [["id", id]], "column": "name", "value": name});
    let (status, body) = env
        .db(
            "alice",
            "planEdits",
            json!({"connectionId": c, "edits": [update(1, format!("{canary}-value"))]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["result"][0]["queryType"], "update", "{body}");

    let apply = |changes: Value, confirmed: bool| async move {
        let (status, body) = env
            .db(
                "alice",
                "applyChanges",
                json!({"connectionId": c, "changes": changes, "confirmed": confirmed}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["result"]["result"].clone()
    };
    let outcome = apply(
        json!([
            {"type": "edit", "id": "p1", "edit": update(1, format!("{canary}-value"))},
            {"type": "edit", "id": "p2", "edit": {"type": "insertRow", "target": target,
             "values": [["id", 100], ["name", format!("{canary}-insert")]]}},
            {"type": "edit", "id": "p3", "edit": {"type": "deleteRow", "target": target, "key": [["id", 2]]}},
        ]),
        false,
    )
    .await;
    assert_eq!(outcome["mode"], "atomic", "{outcome}");
    assert_eq!(outcome["applied"], 3, "{outcome}");
    assert_eq!(
        query(format!("SELECT COUNT(*) FROM {table}")).await,
        json!([[25]])
    );

    // The second change breaks the primary key: the first is rolled back.
    let outcome = apply(
        json!([
            {"type": "edit", "id": "q1", "edit": update(3, format!("{canary}-lost"))},
            {"type": "edit", "id": "q2", "edit": {"type": "insertRow", "target": target,
             "values": [["id", 100], ["name", "dup"]]}},
        ]),
        false,
    )
    .await;
    assert_eq!(outcome["applied"], 0, "{outcome}");
    assert_eq!(outcome["failed"]["index"], 1, "{outcome}");
    assert_eq!(outcome["failed"]["id"], "q2", "{outcome}");
    assert_eq!(
        query(format!("SELECT name FROM {table} WHERE id = 3")).await,
        json!([["n3"]])
    );

    // A full page is counted: n3..n25, 23 rows over 3 pages of 10.
    send(
        ws,
        &json!({"op": "start", "streamId": "tp1", "request": common::db("tablePage", json!({
            "connectionId": c, "streamId": "tp1", "page": 1, "pageSize": 10,
            "query": {"target": target,
                      "filters": [{"column": "name", "op": "LIKE", "value": "n%"},
                                  {"column": "id", "op": "NOT IN", "value": format!("{canary}-in, 0")}],
                      "sort": [{"column": "id", "direction": "DESC"}]},
        }))}),
    )
    .await;
    let got = until_end(ws, "tp1", &mut Vec::new()).await;
    assert_eq!(
        event_types(&got),
        ["statementStart", "batch", "statementDone", "done"],
        "{got:?}"
    );
    let rows = got[1]["event"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 10, "{got:?}");
    assert_eq!(rows[0], json!([25, "n25"]));
    assert_eq!(got[2]["event"]["totalRows"], 23, "{got:?}");
    assert_eq!(got[2]["event"]["totalPages"], 3);
    assert_eq!(got[2]["event"]["countEstimated"], false);

    // A slow page stops on the server within 2 s of its cancel frame.
    let slow = if ty == "postgres" {
        format!("CREATE VIEW {view} AS SELECT 1 AS id FROM pg_sleep(30)")
    } else {
        format!("CREATE VIEW {view} AS SELECT SLEEP(30) AS s")
    };
    exec(slow).await;
    send(
        ws,
        &json!({"op": "start", "streamId": "tp2", "request": common::db("tablePage", json!({
            "connectionId": c, "streamId": "tp2", "page": 1, "pageSize": 10,
            "query": {"target": {"schema": schema, "table": view}},
        }))}),
    )
    .await;
    let running = running_sql(ty, &view);
    let count = |rows: Value| rows[0][0].as_i64().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while count(query(running.clone()).await) != 1 {
        assert!(Instant::now() < deadline, "the page never started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    send(ws, &json!({"op": "cancel", "streamId": "tp2"})).await;
    let cancelled = Instant::now();
    while count(query(running.clone()).await) != 0 {
        assert!(
            cancelled.elapsed() < Duration::from_secs(2),
            "the page still runs on the server"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let got = until_end_or_quiet(ws, "tp2").await;
    assert_eq!(event_types(&got), ["statementStart"], "{got:?}");

    // The sidebar's DROPs ask first, then apply in order.
    let drops = json!([
        {"type": "edit", "id": "d1", "edit": {"type": "dropObject", "target": {"schema": schema, "table": view}, "kind": "view"}},
        {"type": "edit", "id": "d2", "edit": {"type": "dropObject", "target": target, "kind": "table"}},
    ]);
    let outcome = apply(drops.clone(), false).await;
    assert_eq!(outcome["outcome"], "confirmRequired", "{outcome}");
    assert_eq!(outcome["destructiveTotal"], 2);
    let outcome = apply(drops, true).await;
    assert_eq!(outcome["mode"], "inOrder", "{outcome}");
    assert_eq!(outcome["applied"], 2, "{outcome}");
    assert_eq!(outcome["ddl"], true);

    let records = RECORDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(
        records
            .iter()
            .any(|r| r.contains("activity=db.applyChanges")),
        "{records:#?}"
    );
    assert!(
        records.iter().any(|r| r.contains("activity=db.tablePage")),
        "{records:#?}"
    );
    for record in &records {
        assert!(!record.contains(&canary), "{record}");
    }
}

/// Frames for `stream_id` until 300 ms pass with none.
async fn until_end_or_quiet(ws: &mut common::Ws, stream_id: &str) -> Vec<Value> {
    use futures::StreamExt;
    let mut out = Vec::new();
    while let Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t)))) =
        tokio::time::timeout(Duration::from_millis(300), ws.next()).await
    {
        let frame: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(frame["streamId"], stream_id, "{frame}");
        out.push(frame);
    }
    out
}

#[tokio::test]
async fn postgres() {
    let Some(conn_str) = connection_string("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    run("postgres", conn_str, "SELECT pg_sleep(30)").await;
}

#[tokio::test]
async fn mysql() {
    let Some(conn_str) = connection_string("SEAQUEL_TEST_MYSQL") else {
        return;
    };
    run("mysql", conn_str, "SELECT SLEEP(30)").await;
}

/// Probe M5: sqlx logs a Postgres notice (`RAISE WARNING`, text the query
/// chose) under `sqlx::postgres::notice` at WARN. The server's logger drops
/// that target, so the canary never reaches the log.
#[tokio::test]
async fn postgres_notices_are_not_logged() {
    let Some(conn_str) = connection_string("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    capture_logs();
    let env = Env::with_core(Arc::new(web_core()), 4, Arc::default());
    let form = json!({"type": "postgres", "name": "live", "connectionString": conn_str});
    let c = env.connect("alice", form).await;
    let canary = format!("noticecanary{}", std::process::id());
    for (method, severity) in [("execute", "WARNING"), ("query", "NOTICE")] {
        let (status, body) = env
            .db(
                "alice",
                method,
                json!({"connectionId": c,
                       "sql": format!("DO $$ BEGIN RAISE {severity} '{canary}'; END $$")}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let records = RECORDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for record in &records {
        assert!(!record.contains(&canary), "{record}");
    }
}
