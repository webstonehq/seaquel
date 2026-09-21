//! Integration test: POST /api/db/connect + POST /api/db/query against SQLite.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use seaquel_server::{build_router, AppState};
use serde_json::{json, Value};
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
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body was not valid JSON: {e}\nbody={}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, json)
}

#[tokio::test]
async fn connect_and_query_sqlite_roundtrip() {
    let tmp = std::env::temp_dir().join(format!(
        "seaquel-test-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let conn_str = format!("sqlite:{}", tmp.display());

    let app = build_router(AppState::default());

    // POST /api/db/connect
    let (status, body) = post_json(
        app.clone(),
        "/api/db/connect",
        json!({ "driver": "sqlite", "connection_string": conn_str, "create_if_missing": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "connect failed: {body}");
    let connection_id = body["connection_id"]
        .as_str()
        .expect("connect response missing connection_id")
        .to_string();
    assert!(connection_id.starts_with("sqlite-"));

    // POST /api/db/query
    let (status, body) = post_json(
        app.clone(),
        "/api/db/query",
        json!({
            "connection_id": connection_id,
            "sql": "SELECT 1 AS one, 'hi' AS greeting",
            "values": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "query failed: {body}");
    assert_eq!(body["columns"], json!(["one", "greeting"]));
    assert_eq!(body["rows"], json!([[1, "hi"]]));

    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn query_with_unknown_connection_id_returns_404() {
    let app = build_router(AppState::default());

    let (status, body) = post_json(
        app,
        "/api/db/query",
        json!({
            "connection_id": "does-not-exist",
            "sql": "SELECT 1",
            "values": []
        }),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "body was {body}");
    assert_eq!(body["code"], "CONNECTION_NOT_FOUND");
}
