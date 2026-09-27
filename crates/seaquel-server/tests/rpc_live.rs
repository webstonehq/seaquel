//! Live checks of `/rpc` and `/rpc/stream` on the server's real Core
//! (`web_core()`, the web connect policy) against the e2e Docker databases.
//! Each runs only when its variable is set to a ConnectConfig JSON with a
//! `connection_string`, as for the engine smoke tests:
//!
//! - `SEAQUEL_TEST_POSTGRES`, e.g.
//!   `{"driver":"postgres","connection_string":"postgres://postgres@127.0.0.1:5432/seaquel_test"}`
//! - `SEAQUEL_TEST_MYSQL`, e.g.
//!   `{"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3306/seaquel_test"}`

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use seaquel_server::web_core;
use serde_json::{json, Value};

mod common;
use common::{open_stream, send, start, until_end, Env};

fn connection_string(var: &str) -> Option<String> {
    let raw = std::env::var(var).ok()?;
    let config: Value = serde_json::from_str(&raw).expect("a ConnectConfig JSON");
    Some(config["connection_string"].as_str()?.to_string())
}

async fn run(ty: &str, conn_str: String, sleep_sql: &str) {
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

    let (status, body) = env
        .db("alice", "disconnect", json!({"connectionId": c}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
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
