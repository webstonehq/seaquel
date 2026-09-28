//! Live checks of `/rpc` and `/rpc/stream` on the server's real Core
//! (`web_core()`, the web connect policy) against the e2e Docker databases.
//! Each runs only when its variable is set to a ConnectConfig JSON with a
//! `connection_string`, as for the engine smoke tests:
//!
//! - `SEAQUEL_TEST_POSTGRES`, e.g.
//!   `{"driver":"postgres","connection_string":"postgres://postgres@127.0.0.1:5432/seaquel_test"}`
//! - `SEAQUEL_TEST_MYSQL`, e.g.
//!   `{"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3306/seaquel_test"}`

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

    let (status, body) = env
        .db("alice", "disconnect", json!({"connectionId": c}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
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
