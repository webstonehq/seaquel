use duckdb::{params_from_iter, types::Value as DuckValue, types::ValueRef, Connection};
use std::sync::Mutex;

use seaquel_engine::{
    BatchStatement, ConnectConfig, DbError, Driver, ExecuteResult, QueryResult, Value,
};

/// Convert a parameter into a DuckDB `Value` for binding. `Int` binds as
/// BIGINT, so integers are exact. `Decimal` binds as its literal text and
/// `Json` as its JSON text; DuckDB casts both on assignment. Arrays are
/// rejected: use an inline literal for these.
fn to_duckdb_param(v: &Value) -> Result<DuckValue, DbError> {
    Ok(match v {
        Value::Null => DuckValue::Null,
        Value::Bool(b) => DuckValue::Boolean(*b),
        Value::Int(i) => DuckValue::BigInt(*i),
        Value::Float(f) => DuckValue::Double(*f),
        Value::Decimal(s) | Value::Text(s) => DuckValue::Text(s.clone()),
        Value::Bytes(b) => DuckValue::Blob(b.clone()),
        Value::Json(j) => DuckValue::Text(j.to_string()),
        Value::Array(_) => {
            return Err(DbError::query_error("array parameters are not supported"));
        }
    })
}

pub struct DuckdbDriver {
    connection: Mutex<Connection>,
}

impl DuckdbDriver {
    pub fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let path = config.path.as_deref().unwrap_or(":memory:");

        let conn = if path == ":memory:" || path.is_empty() {
            Connection::open_in_memory()
        } else {
            // DuckDB creates missing files on open; only allow that when asked
            // so a mistyped path fails instead of opening a new, empty database.
            let file = std::path::Path::new(path);
            if !file.exists() {
                if !config.create_if_missing.unwrap_or(false) {
                    return Err(DbError {
                        message: format!("Database file not found: {}", path),
                        code: "FILE_NOT_FOUND".to_string(),
                    });
                }
                if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        DbError::connection_error(format!("Failed to create database directory: {}", e))
                    })?;
                }
            }
            Connection::open(path)
        }
        .map_err(DbError::connection_error)?;

        Ok(Self {
            connection: Mutex::new(conn),
        })
    }
}

/// Convert a DuckDB ValueRef to a serde_json::Value
fn convert_value_to_json(value: ValueRef) -> serde_json::Value {
    match value {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Boolean(b) => serde_json::json!(b),
        ValueRef::TinyInt(i) => serde_json::json!(i),
        ValueRef::SmallInt(i) => serde_json::json!(i),
        ValueRef::Int(i) => serde_json::json!(i),
        ValueRef::BigInt(i) => serde_json::json!(i),
        ValueRef::HugeInt(i) => serde_json::json!(i.to_string()),
        ValueRef::UTinyInt(i) => serde_json::json!(i),
        ValueRef::USmallInt(i) => serde_json::json!(i),
        ValueRef::UInt(i) => serde_json::json!(i),
        ValueRef::UBigInt(i) => serde_json::json!(i),
        ValueRef::Float(f) => serde_json::json!(f),
        ValueRef::Double(f) => serde_json::json!(f),
        ValueRef::Decimal(d) => serde_json::json!(d.to_string()),
        ValueRef::Text(s) => serde_json::json!(String::from_utf8_lossy(s)),
        ValueRef::Blob(b) => {
            serde_json::json!(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                b
            ))
        }
        ValueRef::Date32(d) => serde_json::json!(d),
        ValueRef::Time64(..) => serde_json::json!(format!("{:?}", value)),
        ValueRef::Timestamp(..) => serde_json::json!(format!("{:?}", value)),
        ValueRef::Interval { .. } => serde_json::json!(format!("{:?}", value)),
        ValueRef::List(..) => serde_json::json!(format!("{:?}", value)),
        ValueRef::Enum(..) => serde_json::json!(format!("{:?}", value)),
        ValueRef::Struct(..) => serde_json::json!(format!("{:?}", value)),
        ValueRef::Map(..) => serde_json::json!(format!("{:?}", value)),
        ValueRef::Array(..) => serde_json::json!(format!("{:?}", value)),
        ValueRef::Union(..) => serde_json::json!(format!("{:?}", value)),
        // `ValueRef` is `#[non_exhaustive]`; fall back to the debug rendering
        // used for the other composite types.
        _ => serde_json::json!(format!("{:?}", value)),
    }
}

#[seaquel_runtime::async_trait]
impl Driver for DuckdbDriver {
    async fn query(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<QueryResult, DbError> {
        let sql = sql.to_string();
        let bound: Vec<DuckValue> = params
            .iter()
            .map(to_duckdb_param)
            .collect::<Result<_, _>>()?;

        // DuckDB is synchronous — we can't hold a MutexGuard across await,
        // but since this is a std::sync::Mutex (not tokio), we do the work inline.
        let conn = self.connection.lock().map_err(|e| DbError {
            message: format!("Failed to lock connection: {}", e),
            code: "LOCK_ERROR".to_string(),
        })?;

        let mut stmt = conn.prepare(&sql).map_err(DbError::query_error)?;

        let mut result_rows = stmt
            .query(params_from_iter(bound.iter()))
            .map_err(DbError::query_error)?;

        let column_count = result_rows
            .as_ref()
            .map(|s| s.column_count())
            .unwrap_or(0);
        let columns: Vec<String> = (0..column_count)
            .map(|i| {
                result_rows
                    .as_ref()
                    .and_then(|s| s.column_name(i).ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            })
            .collect();

        let cap = seaquel_engine::max_query_rows();
        let mut rows: Vec<Vec<Value>> = Vec::new();
        while let Some(row) = result_rows.next().map_err(DbError::query_error)? {
            if rows.len() >= cap {
                return Err(DbError::result_too_large(cap));
            }
            let mut values = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let value = row.get_ref(i).map_err(DbError::query_error)?;
                values.push(Value::from_json_cell(convert_value_to_json(value)));
            }
            rows.push(values);
        }

        Ok(QueryResult { columns, rows })
    }

    async fn execute(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<ExecuteResult, DbError> {
        let bound: Vec<DuckValue> = params
            .iter()
            .map(to_duckdb_param)
            .collect::<Result<_, _>>()?;

        let conn = self.connection.lock().map_err(|e| DbError {
            message: format!("Failed to lock connection: {}", e),
            code: "LOCK_ERROR".to_string(),
        })?;

        let rows_affected = conn
            .execute(sql, params_from_iter(bound.iter()))
            .map_err(DbError::execute_error)?;

        Ok(ExecuteResult {
            rows_affected: rows_affected as u64,
            last_insert_id: None,
        })
    }

    async fn transaction(&self, statements: Vec<BatchStatement>) -> Result<(), DbError> {
        // DuckDB has one Mutex-guarded Connection, so holding the lock for
        // the duration of the batch gives us real atomicity: no other caller
        // can interleave a statement between BEGIN and COMMIT.
        let conn = self.connection.lock().map_err(|e| DbError {
            message: format!("Failed to lock connection: {}", e),
            code: "LOCK_ERROR".to_string(),
        })?;

        conn.execute("BEGIN", params_from_iter(std::iter::empty::<DuckValue>()))
            .map_err(DbError::execute_error)?;

        for stmt in statements {
            let bound: Vec<DuckValue> = stmt
                .params
                .iter()
                .map(to_duckdb_param)
                .collect::<Result<_, _>>()?;
            if let Err(e) = conn.execute(&stmt.sql, params_from_iter(bound.iter())) {
                // Best-effort rollback; surface the original error regardless
                // of whether the rollback itself succeeds.
                let _ =
                    conn.execute("ROLLBACK", params_from_iter(std::iter::empty::<DuckValue>()));
                return Err(DbError::execute_error(e));
            }
        }

        conn.execute("COMMIT", params_from_iter(std::iter::empty::<DuckValue>()))
            .map_err(DbError::execute_error)?;
        Ok(())
    }

    async fn close(&self) -> Result<(), DbError> {
        // DuckDB Connection is closed on drop
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(path: &std::path::Path, create_if_missing: bool) -> ConnectConfig {
        serde_json::from_value(serde_json::json!({
            "driver": "duckdb",
            "path": path.to_str().unwrap(),
            "create_if_missing": create_if_missing,
        }))
        .unwrap()
    }

    fn temp_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("seaquel-duckdb-test-{}", std::process::id()))
    }

    #[test]
    fn missing_file_is_not_created() {
        let path = temp_path().join("missing.duckdb");
        let err = DuckdbDriver::connect(&config(&path, false)).err().unwrap();
        assert_eq!(err.code, "FILE_NOT_FOUND");
        assert!(!path.exists(), "database file must not be created");
    }

    #[test]
    fn create_if_missing_creates_file_and_directory() {
        let dir = temp_path().join("create");
        let path = dir.join("nested").join("new.duckdb");
        DuckdbDriver::connect(&config(&path, true)).unwrap();
        assert!(path.exists(), "database file should be created");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_memory_needs_no_file() {
        let cfg: ConnectConfig =
            serde_json::from_value(serde_json::json!({ "driver": "duckdb", "path": ":memory:" })).unwrap();
        assert!(DuckdbDriver::connect(&cfg).is_ok());
    }
}
