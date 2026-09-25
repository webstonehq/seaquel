//! Wire types shared by every Seaquel interface.
//!
//! These cross process boundaries (Tauri IPC, HTTP, WebSocket), so their serde
//! shape is a contract with the TypeScript frontend. `tests/wire_format.rs`
//! pins the JSON, and `npm run types:gen` regenerates
//! `src/lib/types/generated/` from these definitions.
//!
//! This crate is pure: it must keep building for `wasm32-unknown-unknown`.

use serde::{Deserialize, Serialize};

/// Columnar result format for all drivers
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct QueryResult {
    pub columns: Vec<String>,
    #[cfg_attr(feature = "ts", ts(type = "unknown[][]"))]
    pub rows: Vec<Vec<serde_json::Value>>,
}

/// A batch of rows emitted by a streaming query.
/// `columns` is Some on the first batch (so the frontend can render headers)
/// and None on subsequent batches. `is_final` marks the terminal batch, which
/// may carry zero rows.
#[derive(Debug, Serialize, Clone)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct StreamBatch {
    pub columns: Option<Vec<String>>,
    #[cfg_attr(feature = "ts", ts(type = "unknown[][]"))]
    pub rows: Vec<Vec<serde_json::Value>>,
    pub is_final: bool,
}

/// Result of a write operation
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ExecuteResult {
    // ts-rs maps u64/i64 to `bigint`, but serde_json sends plain JSON numbers.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub rows_affected: u64,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_insert_id: Option<i64>,
}

/// Result of a connect operation
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConnectResult {
    pub connection_id: String,
}

/// Unified error type for all drivers
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DbError {
    pub message: String,
    pub code: String,
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DbError {}

impl DbError {
    pub fn connection_not_found(id: &str) -> Self {
        Self {
            message: format!("Connection not found: {}", id),
            code: "CONNECTION_NOT_FOUND".to_string(),
        }
    }

    pub fn connection_error(msg: impl std::fmt::Display) -> Self {
        Self {
            message: format!("Failed to connect: {}", msg),
            code: "CONNECTION_ERROR".to_string(),
        }
    }

    pub fn query_error(msg: impl std::fmt::Display) -> Self {
        Self {
            message: format!("Query failed: {}", msg),
            code: "QUERY_ERROR".to_string(),
        }
    }

    pub fn execute_error(msg: impl std::fmt::Display) -> Self {
        Self {
            message: format!("Execute failed: {}", msg),
            code: "EXECUTE_ERROR".to_string(),
        }
    }

    pub fn result_too_large(cap: usize) -> Self {
        Self {
            message: format!(
                "Result exceeds the {cap}-row cap for non-streaming queries. Use query_stream for large results, or add LIMIT {cap} to the query."
            ),
            code: "RESULT_TOO_LARGE".to_string(),
        }
    }

    /// The build doesn't include an engine for this driver (e.g. a slim CLI
    /// built without the `engine-mssql` feature).
    pub fn engine_not_available(driver: &str) -> Self {
        Self {
            message: format!("Database engine \"{}\" is not available in this build", driver),
            code: "ENGINE_NOT_AVAILABLE".to_string(),
        }
    }
}

/// Driver type discriminant
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum DriverType {
    Postgres,
    Mysql,
    Sqlite,
    Mssql,
    Duckdb,
}

impl DriverType {
    /// The wire name, which is also the id of the engine that handles it.
    pub fn as_str(self) -> &'static str {
        match self {
            DriverType::Postgres => "postgres",
            DriverType::Mysql => "mysql",
            DriverType::Sqlite => "sqlite",
            DriverType::Mssql => "mssql",
            DriverType::Duckdb => "duckdb",
        }
    }
}

/// Connection configuration — superset of all driver needs
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct ConnectConfig {
    pub driver: DriverType,
    /// Connection string for sqlx-based drivers (postgres, mysql, sqlite)
    pub connection_string: Option<String>,
    /// Individual fields for MSSQL
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub encrypt: Option<bool>,
    pub trust_cert: Option<bool>,
    /// File path for DuckDB
    pub path: Option<String>,
    /// SQLite only: create the database file (and its directory) if it doesn't
    /// exist. Off by default so a mistyped path fails instead of silently
    /// opening a new, empty database.
    pub create_if_missing: Option<bool>,
}

/// A single statement in a batch/transaction
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct BatchStatement {
    pub sql: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "unknown[]"))]
    pub params: Vec<serde_json::Value>,
}

/// Events delivered to a client for one streaming query, over a Tauri channel
/// or a WebSocket. A stream is zero or more `Batch` events followed by exactly
/// one `Done` or `Error` — or nothing more at all if the client cancelled.
///
/// `Batch` flattens the `StreamBatch` fields onto the event:
/// `{"type":"batch","columns":…,"rows":…,"is_final":…}`.
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum StreamEvent {
    Batch(StreamBatch),
    Done,
    Error { message: String, code: String },
}

impl From<DbError> for StreamEvent {
    fn from(err: DbError) -> Self {
        StreamEvent::Error {
            message: err.message,
            code: err.code,
        }
    }
}
