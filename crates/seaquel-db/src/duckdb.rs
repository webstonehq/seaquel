use async_trait::async_trait;
use duckdb::{params_from_iter, types::Value as DuckValue, types::ValueRef, Connection};
use std::sync::Mutex;

use super::{BatchStatement, ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};

/// Convert a JSON parameter into a DuckDB `Value` suitable for parameter
/// binding. Returns `Err` for nested arrays/objects, which aren't expressible
/// as a scalar bind — callers should either flatten upstream or use an
/// inline literal for these.
fn json_to_duckdb_param(v: &serde_json::Value) -> Result<DuckValue, DbError> {
    Ok(match v {
        serde_json::Value::Null => DuckValue::Null,
        serde_json::Value::Bool(b) => DuckValue::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                DuckValue::BigInt(i)
            } else if let Some(f) = n.as_f64() {
                DuckValue::Double(f)
            } else {
                return Err(DbError::query_error(
                    "unsupported numeric parameter (outside i64/f64 range)",
                ));
            }
        }
        serde_json::Value::String(s) => DuckValue::Text(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            return Err(DbError::query_error(
                "array/object parameters are not supported; pass a scalar or use an inline literal",
            ));
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
            Connection::open(path)
        }
        .map_err(|e| DbError::connection_error(e))?;

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

#[async_trait]
impl Driver for DuckdbDriver {
    async fn query(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<QueryResult, DbError> {
        let sql = sql.to_string();
        let bound: Vec<DuckValue> = params
            .iter()
            .map(json_to_duckdb_param)
            .collect::<Result<_, _>>()?;

        // DuckDB is synchronous — we can't hold a MutexGuard across await,
        // but since this is a std::sync::Mutex (not tokio), we do the work inline.
        let conn = self.connection.lock().map_err(|e| DbError {
            message: format!("Failed to lock connection: {}", e),
            code: "LOCK_ERROR".to_string(),
        })?;

        let mut stmt = conn.prepare(&sql).map_err(|e| DbError::query_error(e))?;

        let mut result_rows = stmt
            .query(params_from_iter(bound.iter()))
            .map_err(|e| DbError::query_error(e))?;

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

        let cap = super::max_query_rows();
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
        while let Some(row) = result_rows.next().map_err(|e| DbError::query_error(e))? {
            if rows.len() >= cap {
                return Err(DbError::result_too_large(cap));
            }
            let mut values = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let value = row.get_ref(i).map_err(|e| DbError::query_error(e))?;
                values.push(convert_value_to_json(value));
            }
            rows.push(values);
        }

        Ok(QueryResult { columns, rows })
    }

    async fn execute(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<ExecuteResult, DbError> {
        let bound: Vec<DuckValue> = params
            .iter()
            .map(json_to_duckdb_param)
            .collect::<Result<_, _>>()?;

        let conn = self.connection.lock().map_err(|e| DbError {
            message: format!("Failed to lock connection: {}", e),
            code: "LOCK_ERROR".to_string(),
        })?;

        let rows_affected = conn
            .execute(sql, params_from_iter(bound.iter()))
            .map_err(|e| DbError::execute_error(e))?;

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
            .map_err(|e| DbError::execute_error(e))?;

        for stmt in statements {
            let bound: Vec<DuckValue> = stmt
                .params
                .iter()
                .map(json_to_duckdb_param)
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
            .map_err(|e| DbError::execute_error(e))?;
        Ok(())
    }

    async fn close(&self) -> Result<(), DbError> {
        // DuckDB Connection is closed on drop
        Ok(())
    }
}
