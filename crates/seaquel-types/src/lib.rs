//! Wire types shared by every Seaquel interface.
//!
//! These cross process boundaries (Tauri IPC, HTTP, WebSocket), so their serde
//! shape is a contract with the TypeScript frontend. `tests/wire_format.rs`
//! pins the JSON, and `npm run types:gen` regenerates
//! `src/lib/types/generated/` from these definitions.
//!
//! This crate is pure: it must keep building for `wasm32-unknown-unknown`.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub mod connect;
mod dialect;
pub mod git;
pub mod license;
pub mod ssh;
pub mod storage;
mod value;
pub use dialect::*;
pub use value::{Value, MAX_SAFE_INTEGER};

/// Columnar result format for all drivers
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct QueryResult {
    pub columns: Vec<String>,
    #[cfg_attr(feature = "ts", ts(type = "unknown[][]"))]
    pub rows: Vec<Vec<Value>>,
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
    pub rows: Vec<Vec<Value>>,
    pub is_final: bool,
    /// Only on the final batch of a read-only query run with `max_rows`:
    /// the query had more rows than that, and only the first `max_rows`
    /// were sent. Absent (false) everywhere else.
    #[serde(default, skip_serializing_if = "is_false")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub truncated: bool,
}

/// `skip_serializing_if` for flags that are absent when false.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Result of a write operation
#[derive(Debug, Serialize, Deserialize)]
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

    /// Statement `index` (0-based) of a transaction affected fewer rows than
    /// its [`ExpectRows`]; the transaction was rolled back. The message
    /// carries the index as `(index N)`.
    pub fn no_rows_affected(index: usize, affected: u64, min: u64) -> Self {
        Self {
            message: format!(
                "Statement {} (index {index}) affected {affected} row{}, expected at least {min}. The transaction was rolled back.",
                index + 1,
                if affected == 1 { "" } else { "s" },
            ),
            code: "NO_ROWS_AFFECTED".to_string(),
        }
    }

    /// The database, or Seaquel's own check, refused a query run in
    /// read-only mode (`Driver::query_read_only`, Core's `read_only` option).
    /// The message is kept as given, with no prefix: Core passes the AI
    /// check's exact text, and drivers pass the database's own message.
    pub fn read_only(msg: impl std::fmt::Display) -> Self {
        Self {
            message: msg.to_string(),
            code: "READ_ONLY".to_string(),
        }
    }

    /// The build doesn't include an engine for this driver (e.g. a slim CLI
    /// built without the `engine-mssql` feature).
    pub fn engine_not_available(driver: &str) -> Self {
        Self {
            message: format!(
                "Database engine \"{}\" is not available in this build",
                driver
            ),
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

/// Connection configuration — superset of all driver needs.
///
/// It carries a password, so its `Debug` hides it: `password` shows as
/// `<redacted>`, and `connection_string` as the URL without its password (or
/// `<redacted>` when it can't safely tell where the password is).
#[derive(Deserialize, Clone)]
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
    /// DuckDB only: lock the database instance down. It opens with
    /// `enable_external_access`, `autoinstall_known_extensions` and
    /// `autoload_known_extensions` off and `lock_configuration` on, so a
    /// query can read only the database itself: no other files, no URLs, no
    /// `ATTACH`, no extension installs or loads, and no global `SET` to
    /// undo it.
    /// Off by default; the MCP server turns it on for the instances it opens.
    /// Other engines ignore it.
    pub restricted: Option<bool>,
    /// MSSQL only: the name the server's TLS certificate is checked against,
    /// when it isn't `host`. Set for a connection through an SSH tunnel,
    /// where `host` is `127.0.0.1` and this is the server's own name. The
    /// socket still goes to `host` and `port`.
    pub tls_server_name: Option<String>,
    /// DuckDB only: options the database opens with (`access_mode` =
    /// `read_only`, …), parsed out of a `duckdb://path?key=value` string. An
    /// option DuckDB doesn't know fails the connect. With `restricted`, the
    /// lock-down settings are applied after these and win.
    #[cfg_attr(feature = "ts", ts(type = "Record<string, string>", optional))]
    pub duckdb_config: Option<BTreeMap<String, String>>,
}

impl fmt::Debug for ConnectConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectConfig")
            .field("driver", &self.driver)
            .field(
                "connection_string",
                &self
                    .connection_string
                    .as_deref()
                    .map(connection_string_for_debug),
            )
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("encrypt", &self.encrypt)
            .field("trust_cert", &self.trust_cert)
            .field("path", &self.path)
            .field("create_if_missing", &self.create_if_missing)
            .field("restricted", &self.restricted)
            .field("tls_server_name", &self.tls_server_name)
            // Values can be credentials (`s3_secret_access_key`): keys only.
            .field(
                "duckdb_config",
                &self
                    .duckdb_config
                    .as_ref()
                    .map(|c| c.keys().map(|k| (k, "<redacted>")).collect::<Vec<_>>()),
            )
            .finish()
    }
}

/// A connection string for `Debug`: a `scheme://…` URL with the password
/// taken out of its user info, or `<redacted>` for anything it can't read
/// with certainty: a string that isn't `scheme://…` (key=value strings carry
/// `Password=`), an `@` after the authority (a raw `/` in a password, or a
/// TablePlus `+ssh` URL), or a `password`/`pwd` query parameter.
pub(crate) fn connection_string_for_debug(s: &str) -> String {
    const REDACTED: &str = "<redacted>";
    let Some((scheme, rest)) = s.split_once("://") else {
        return REDACTED.to_string();
    };
    let valid_scheme = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid_scheme {
        return REDACTED.to_string();
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if tail.contains('@') {
        return REDACTED.to_string();
    }
    if let Some((_, query)) = tail.split_once('?') {
        let has_password = query.split(['&', ';']).any(|pair| {
            let key = pair.split('=').next().unwrap_or("").to_ascii_lowercase();
            key == "password" || key == "pwd"
        });
        if has_password {
            return REDACTED.to_string();
        }
    }
    let authority = match authority.rsplit_once('@') {
        Some((userinfo, host)) => {
            let user = userinfo.split(':').next().unwrap_or("");
            format!("{user}@{host}")
        }
        None => authority.to_string(),
    };
    format!("{scheme}://{authority}{tail}")
}

/// A single statement in a batch/transaction
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct BatchStatement {
    pub sql: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "unknown[]"))]
    pub params: Vec<Value>,
    /// How many rows the statement must affect. `transaction` checks it
    /// before COMMIT and, on a shortfall, rolls back and fails with
    /// `NO_ROWS_AFFECTED`. Set it for a keyed UPDATE or DELETE (`{ min: 1 }`),
    /// so an edit whose key went stale fails instead of silently doing
    /// nothing; leave it out for DDL and INSERT.
    #[serde(default)]
    pub expect_rows: Option<ExpectRows>,
}

impl BatchStatement {
    /// Checks the rows statement `index` of a batch affected against
    /// [`BatchStatement::expect_rows`].
    pub fn check_affected(&self, index: usize, affected: u64) -> Result<(), DbError> {
        self.expect_rows
            .map_or(Ok(()), |expect| expect.check(index, affected))
    }
}

/// The rows a batch statement must affect (see [`BatchStatement::expect_rows`]).
///
/// Engines count differently, which matters only for `min`: MySQL counts
/// matched rows (sqlx connects with `CLIENT_FOUND_ROWS`), MSSQL adds the rows
/// its triggers touch, and Postgres (DO INSTEAD rules) and SQLite (INSTEAD OF
/// triggers on views) report 0 for a write a rule or trigger carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ExpectRows {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub min: u64,
}

impl ExpectRows {
    /// `NO_ROWS_AFFECTED` when statement `index` (0-based) affected fewer
    /// than `min` rows.
    pub fn check(self, index: usize, affected: u64) -> Result<(), DbError> {
        if affected >= self.min {
            return Ok(());
        }
        Err(DbError::no_rows_affected(index, affected, self.min))
    }
}

/// Generated SQL plus the values for its placeholders. `bind_values` is absent
/// for dialects that inline literals (MSSQL, DuckDB).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct SqlWithBindings {
    pub sql: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "unknown[]"))]
    pub bind_values: Option<Vec<Value>>,
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
