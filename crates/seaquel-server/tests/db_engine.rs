//! Integration test: POST /api/db/engine dispatches an `EngineCall` and maps
//! Core's errors to HTTP statuses like the other routes.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::{connect_sqlite, post_json, temp_sqlite};
use seaquel_engine::{ConnectConfig, DbError, Dialect, Driver, Engine, ExecuteResult, QueryResult};
use seaquel_engine_postgres::PostgresDialect;
use seaquel_server::{build_router, AppState};
use seaquel_types::Value;
use serde_json::json;

/// No database: SQL generation only needs the dialect.
struct NoDatabase;

#[seaquel_runtime::async_trait]
impl Driver for NoDatabase {
    async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        Err(DbError::query_error("no database in this test"))
    }
    async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        Err(DbError::execute_error("no database in this test"))
    }
    async fn close(&self) -> Result<(), DbError> {
        Ok(())
    }
}

/// "postgres" with the real Postgres dialect over [`NoDatabase`].
struct OfflinePostgres;

#[seaquel_runtime::async_trait]
impl Engine for OfflinePostgres {
    fn id(&self) -> &'static str {
        "postgres"
    }
    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(NoDatabase))
    }
    fn dialect(&self) -> Option<&dyn Dialect> {
        Some(&PostgresDialect)
    }
}

fn offline_app() -> axum::Router {
    let core = seaquel_core::Core::builder()
        .engine(Arc::new(OfflinePostgres))
        .build();
    build_router(AppState {
        core: Arc::new(core),
    })
}

async fn connect_offline(app: axum::Router) -> String {
    let (status, body) = post_json(app, "/api/db/connect", json!({ "driver": "postgres" })).await;
    assert_eq!(status, StatusCode::OK, "connect failed: {body}");
    body["connection_id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn engine_builds_sql_with_the_connections_dialect() {
    let app = offline_app();
    let connection_id = connect_offline(app.clone()).await;

    let (status, body) = post_json(
        app,
        "/api/db/engine",
        json!({
            "connection_id": connection_id,
            "request": { "method": "buildUpdate", "params": {
                "schema": "public", "table": "users", "column": "balance",
                "value": { "$sq": "decimal", "v": "12.50" },
                "primary_keys": ["id"],
                "row": [["id", { "$sq": "bigint", "v": "9007199254740993" }], ["balance", 1]],
            } }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["kind"], "sqlWithBindings");
    assert_eq!(
        body["data"]["sql"],
        r#"UPDATE "public"."users" SET "balance" = $1 WHERE "id" = $2"#
    );
    assert_eq!(
        body["data"]["bindValues"],
        json!([{ "$sq": "decimal", "v": "12.50" }, { "$sq": "bigint", "v": "9007199254740993" }])
    );
}

#[tokio::test]
async fn engine_with_unknown_connection_id_returns_404() {
    let app = build_router(AppState::default());
    let (status, body) = post_json(
        app,
        "/api/db/engine",
        json!({ "connection_id": "does-not-exist", "request": { "method": "schemaTables" } }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body={body}");
    assert_eq!(body["code"], "CONNECTION_NOT_FOUND");
}

#[tokio::test]
async fn engine_without_a_rust_dialect_returns_501() {
    let (conn_str, tmp) = temp_sqlite();
    let app = build_router(AppState::default());
    let connection_id = connect_sqlite(app.clone(), &conn_str).await;

    for request in [
        json!({ "method": "schemaTables" }),
        json!({ "method": "paginate", "params": { "sql": "SELECT 1", "limit": 10, "offset": 0 } }),
    ] {
        let (status, body) = post_json(
            app.clone(),
            "/api/db/engine",
            json!({ "connection_id": connection_id, "request": request }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "body={body}");
        assert_eq!(body["code"], "NOT_SUPPORTED");
    }

    let _ = std::fs::remove_file(&tmp);
}
