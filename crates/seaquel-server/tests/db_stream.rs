//! Integration test: WebSocket at /api/db/stream.
//!
//! Client flow:
//! 1. Open WS.
//! 2. Send a single Text frame with `{"query_id","connection_id","sql","values"}`.
//! 3. Receive a sequence of `{"type":"batch", ...StreamBatch}` frames.
//! 4. Receive a terminal `{"type":"done"}` frame.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use seaquel_server::{build_router, AppState};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

async fn post_json(app: axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

/// Spawn the server on 127.0.0.1:0 and return its bound address.
async fn spawn_server() -> (std::net::SocketAddr, axum::Router) {
    let app = build_router(AppState::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_app = app.clone();
    tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });
    (addr, app)
}

#[tokio::test]
async fn stream_multi_batch_sqlite_roundtrip() {
    let tmp = std::env::temp_dir().join(format!(
        "seaquel-stream-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let conn_str = format!("sqlite:{}", tmp.display());

    let (addr, app) = spawn_server().await;

    // Connect
    let (status, body) = post_json(
        app.clone(),
        "/api/db/connect",
        json!({ "driver": "sqlite", "connection_string": conn_str }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "connect failed: {body}");
    let connection_id = body["connection_id"].as_str().unwrap().to_string();

    // Seed a table with 12_345 rows — large enough to force multiple batches
    // regardless of the driver's BATCH_SIZE tuning (currently 5000).
    let (status, _) = post_json(
        app.clone(),
        "/api/db/execute",
        json!({
            "connection_id": connection_id,
            "sql": "CREATE TABLE nums (n INTEGER)",
            "values": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Insert using a recursive CTE — fast bulk insert without 1234 round trips.
    let (status, _) = post_json(
        app.clone(),
        "/api/db/execute",
        json!({
            "connection_id": connection_id,
            "sql": "WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<12345) INSERT INTO nums SELECT n FROM seq",
            "values": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Now open WS and stream.
    let ws_url = format!("ws://{}/api/db/stream", addr);
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    ws.send(Message::Text(
        serde_json::to_string(&json!({
            "query_id": uuid::Uuid::new_v4().to_string(),
            "connection_id": connection_id,
            "sql": "SELECT n FROM nums ORDER BY n",
            "values": []
        }))
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();

    let mut total_rows = 0usize;
    let mut columns_seen: Option<Vec<String>> = None;
    let mut batch_count = 0usize;
    let mut saw_final = false;
    let mut saw_done = false;

    while let Some(msg) = ws.next().await {
        let msg = msg.unwrap();
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let v: Value = serde_json::from_str(&text).unwrap();
        match v["type"].as_str().unwrap() {
            "batch" => {
                batch_count += 1;
                if columns_seen.is_none() {
                    if let Some(arr) = v["columns"].as_array() {
                        let cols: Vec<String> = arr
                            .iter()
                            .map(|x| x.as_str().unwrap().to_string())
                            .collect();
                        columns_seen = Some(cols);
                    }
                }
                let rows_in_batch = v["rows"].as_array().unwrap().len();
                total_rows += rows_in_batch;
                let is_final = v["is_final"].as_bool().unwrap();
                if is_final {
                    saw_final = true;
                }
            }
            "done" => {
                saw_done = true;
                break;
            }
            "error" => panic!("stream returned error: {v}"),
            other => panic!("unexpected event type: {other}"),
        }
    }

    assert_eq!(columns_seen.as_deref(), Some(&["n".to_string()][..]));
    assert_eq!(total_rows, 12_345, "expected all rows streamed across batches");
    assert!(
        batch_count >= 2,
        "expected multi-batch streaming, got {batch_count} batch(es)"
    );
    assert!(saw_final, "last batch must have is_final=true");
    assert!(saw_done, "must receive a done terminator");

    let _ = std::fs::remove_file(&tmp);
}
