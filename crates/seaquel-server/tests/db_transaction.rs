//! Integration test: POST /api/db/transaction commits atomically when all
//! statements succeed and rolls back when any fails.

mod common;

use axum::http::StatusCode;
use common::{connect_sqlite, post_json, temp_sqlite};
use seaquel_server::{build_router, AppState};
use serde_json::json;

#[tokio::test]
async fn transaction_commits_all_statements_on_success() {
    let (conn_str, tmp) = temp_sqlite();
    let app = build_router(AppState::default());
    let connection_id = connect_sqlite(app.clone(), &conn_str).await;

    // Set up the table.
    let (status, _) = post_json(
        app.clone(),
        "/api/db/execute",
        json!({
            "connection_id": connection_id,
            "sql": "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)",
            "values": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Run a two-statement transaction.
    let (status, body) = post_json(
        app.clone(),
        "/api/db/transaction",
        json!({
            "connection_id": connection_id,
            "statements": [
                { "sql": "INSERT INTO items (name) VALUES (?)", "params": ["alpha"] },
                { "sql": "INSERT INTO items (name) VALUES (?)", "params": ["beta"] },
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");

    // Both rows must be persisted.
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
    assert_eq!(body["rows"], json!([["alpha"], ["beta"]]));

    let _ = std::fs::remove_file(&tmp);
}

#[tokio::test]
async fn transaction_rolls_back_on_failure() {
    let (conn_str, tmp) = temp_sqlite();
    let app = build_router(AppState::default());
    let connection_id = connect_sqlite(app.clone(), &conn_str).await;

    let (status, _) = post_json(
        app.clone(),
        "/api/db/execute",
        json!({
            "connection_id": connection_id,
            "sql": "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
            "values": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Second statement violates NOT NULL and must abort the transaction.
    let (status, body) = post_json(
        app.clone(),
        "/api/db/transaction",
        json!({
            "connection_id": connection_id,
            "statements": [
                { "sql": "INSERT INTO items (name) VALUES (?)", "params": ["alpha"] },
                { "sql": "INSERT INTO items (name) VALUES (?)", "params": [null] },
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    assert_eq!(body["code"], "EXECUTE_ERROR");

    // The first insert must have been rolled back — zero rows present.
    let (status, body) = post_json(
        app,
        "/api/db/query",
        json!({
            "connection_id": connection_id,
            "sql": "SELECT COUNT(*) AS c FROM items",
            "values": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rows"], json!([[0]]), "rollback must leave no rows");

    let _ = std::fs::remove_file(&tmp);
}
