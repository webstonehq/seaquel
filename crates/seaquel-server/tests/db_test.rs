//! Integration test: POST /api/db/test validates a ConnectConfig without
//! persisting a connection.

mod common;

use axum::http::StatusCode;
use common::{post_json, temp_sqlite};
use seaquel_server::{build_router, AppState};
use serde_json::json;

#[tokio::test]
async fn test_valid_sqlite_config_returns_200() {
    let (conn_str, tmp) = temp_sqlite();
    let app = build_router(AppState::default());

    let (status, body) = post_json(
        app,
        "/api/db/test",
        json!({ "driver": "sqlite", "connection_string": conn_str, "create_if_missing": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");

    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn test_missing_connection_string_returns_bad_gateway() {
    // SQLite driver requires connection_string. Passing none triggers
    // DbError::connection_error which maps to 502.
    let app = build_router(AppState::default());

    let (status, body) = post_json(
        app,
        "/api/db/test",
        json!({ "driver": "sqlite" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body={body}");
    assert_eq!(body["code"], "CONNECTION_ERROR");
}

#[tokio::test]
async fn test_does_not_register_connection() {
    // Verify the contract: /api/db/test must NOT leave a connection in the
    // manager. We can't observe the manager directly from outside, but we can
    // check that no connection_id was returned in the response body.
    let (conn_str, tmp) = temp_sqlite();
    let app = build_router(AppState::default());

    let (status, body) = post_json(
        app,
        "/api/db/test",
        json!({ "driver": "sqlite", "connection_string": conn_str, "create_if_missing": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.is_null() || body.get("connection_id").is_none(),
        "test endpoint must not return a connection_id; got: {body}"
    );

    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn test_missing_sqlite_file_is_not_created() {
    // A mistyped path must fail rather than silently creating an empty database.
    let (conn_str, tmp) = temp_sqlite();
    let app = build_router(AppState::default());

    let (status, body) = post_json(
        app,
        "/api/db/test",
        json!({ "driver": "sqlite", "connection_string": conn_str }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    assert_eq!(body["code"], "FILE_NOT_FOUND");
    assert!(!tmp.exists(), "database file must not be created");
}

#[tokio::test]
async fn test_create_if_missing_creates_file_and_directory() {
    let (_, tmp) = temp_sqlite();
    let dir = tmp.with_extension("d");
    let db = dir.join("nested").join("new.sqlite");
    let app = build_router(AppState::default());

    let (status, body) = post_json(
        app,
        "/api/db/test",
        json!({
            "driver": "sqlite",
            "connection_string": format!("sqlite:{}", db.display()),
            "create_if_missing": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert!(db.exists(), "database file should be created");

    let _ = std::fs::remove_dir_all(&dir);
}
