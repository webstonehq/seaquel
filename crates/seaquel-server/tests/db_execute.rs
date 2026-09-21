//! Integration test: POST /api/db/execute creates a table, inserts rows,
//! and returns rows_affected.

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
async fn execute_create_and_insert_reports_rows_affected() {
    let tmp = std::env::temp_dir().join(format!(
        "seaquel-test-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let conn_str = format!("sqlite:{}", tmp.display());

    let app = build_router(AppState::default());

    // Connect
    let (status, body) = post_json(
        app.clone(),
        "/api/db/connect",
        json!({ "driver": "sqlite", "connection_string": conn_str, "create_if_missing": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "connect failed: {body}");
    let connection_id = body["connection_id"].as_str().unwrap().to_string();

    // CREATE TABLE
    let (status, body) = post_json(
        app.clone(),
        "/api/db/execute",
        json!({
            "connection_id": connection_id,
            "sql": "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)",
            "values": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {body}");
    assert_eq!(body["rows_affected"], 0);

    // INSERT 3 rows via three separate executes
    for name in ["alpha", "beta", "gamma"] {
        let (status, body) = post_json(
            app.clone(),
            "/api/db/execute",
            json!({
                "connection_id": connection_id,
                "sql": "INSERT INTO items (name) VALUES (?)",
                "values": [name]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "insert failed: {body}");
        assert_eq!(body["rows_affected"], 1);
        // SQLite populates last_insert_id; it must be non-null on each insert.
        assert!(
            body["last_insert_id"].is_number(),
            "expected numeric last_insert_id, got: {}",
            body["last_insert_id"]
        );
    }

    // Verify rows persisted via SELECT
    let (status, body) = post_json(
        app,
        "/api/db/query",
        json!({
            "connection_id": connection_id,
            "sql": "SELECT name FROM items ORDER BY id",
            "values": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rows"], json!([["alpha"], ["beta"], ["gamma"]]));

    let _ = std::fs::remove_file(&tmp);
}
