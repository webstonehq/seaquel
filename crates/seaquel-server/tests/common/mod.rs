//! Shared helpers for integration tests.
//!
//! Each test binary lives in its own crate, so this module is pulled in via
//! `mod common;` at the top of each test file.

#![allow(dead_code)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

/// Issue a POST with a JSON body against the router and return `(status, body_json)`.
/// Empty response bodies come back as `Value::Null`.
pub async fn post_json(app: axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
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
    let json = if bytes.is_empty() {
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

/// Create a unique tempfile-backed SQLite connection string. Returns
/// `(conn_str, path_to_cleanup)`.
pub fn temp_sqlite() -> (String, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!("seaquel-test-{}.sqlite", uuid::Uuid::new_v4()));
    let conn_str = format!("sqlite:{}", tmp.display());
    (conn_str, tmp)
}

/// Connect to SQLite at `conn_str` via the HTTP API and return the new connection_id.
pub async fn connect_sqlite(app: axum::Router, conn_str: &str) -> String {
    let (status, body) = post_json(
        app,
        "/api/db/connect",
        serde_json::json!({
            "driver": "sqlite",
            "connection_string": conn_str,
            "create_if_missing": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "connect failed: {body}");
    body["connection_id"].as_str().unwrap().to_string()
}
