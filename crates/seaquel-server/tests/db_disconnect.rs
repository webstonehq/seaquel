//! Integration test: POST /api/db/disconnect removes the connection from the
//! ConnectionManager so subsequent queries return CONNECTION_NOT_FOUND.

mod common;

use axum::http::StatusCode;
use common::{connect_sqlite, post_json, temp_sqlite};
use seaquel_server::{build_router, AppState};
use serde_json::json;

#[tokio::test]
async fn disconnect_drops_connection() {
    let (conn_str, tmp) = temp_sqlite();
    let app = build_router(AppState::default());

    let connection_id = connect_sqlite(app.clone(), &conn_str).await;

    // Sanity: a query works while the connection is live.
    let (status, _) = post_json(
        app.clone(),
        "/api/db/query",
        json!({ "connection_id": connection_id, "sql": "SELECT 1", "values": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "pre-disconnect query should succeed");

    // Disconnect.
    let (status, body) = post_json(
        app.clone(),
        "/api/db/disconnect",
        json!({ "connection_id": connection_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "disconnect failed: {body}");

    // Same connection_id must now be unknown.
    let (status, body) = post_json(
        app,
        "/api/db/query",
        json!({ "connection_id": connection_id, "sql": "SELECT 1", "values": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body={body}");
    assert_eq!(body["code"], "CONNECTION_NOT_FOUND");

    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn disconnect_unknown_id_is_noop() {
    // Disconnecting an id that was never registered must not error — matches
    // the Tauri side, which treats it as idempotent.
    let app = build_router(AppState::default());
    let (status, _) = post_json(
        app,
        "/api/db/disconnect",
        json!({ "connection_id": "never-existed" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
