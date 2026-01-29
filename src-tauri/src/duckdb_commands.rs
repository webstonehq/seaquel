use duckdb::{Connection, types::ValueRef};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::State;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct DuckDBError {
    pub message: String,
    pub code: String,
}

impl std::fmt::Display for DuckDBError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DuckDBError {}

/// State for managing DuckDB connections
pub struct DuckDBState {
    connections: Mutex<HashMap<String, Connection>>,
}

impl Default for DuckDBState {
    fn default() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }
}

#[derive(Serialize)]
pub struct DuckDBConnectResult {
    connection_id: String,
}

#[derive(Serialize)]
pub struct DuckDBQueryResult {
    columns: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
}

#[derive(Serialize)]
pub struct DuckDBExecuteResult {
    rows_affected: usize,
}

/// Connect to a DuckDB database
#[tauri::command]
pub fn duckdb_connect(
    state: State<DuckDBState>,
    path: String,
) -> Result<DuckDBConnectResult, DuckDBError> {
    let conn = if path == ":memory:" || path.is_empty() {
        Connection::open_in_memory()
    } else {
        Connection::open(&path)
    }
    .map_err(|e| DuckDBError {
        message: format!("Failed to open connection: {}", e),
        code: "CONNECTION_ERROR".to_string(),
    })?;

    let connection_id = format!("duckdb-{}", Uuid::new_v4());
    state
        .connections
        .lock()
        .map_err(|e| DuckDBError {
            message: format!("Failed to lock connections: {}", e),
            code: "LOCK_ERROR".to_string(),
        })?
        .insert(connection_id.clone(), conn);

    Ok(DuckDBConnectResult { connection_id })
}

/// Disconnect from a DuckDB database
#[tauri::command]
pub fn duckdb_disconnect(
    state: State<DuckDBState>,
    connection_id: String,
) -> Result<(), DuckDBError> {
    state
        .connections
        .lock()
        .map_err(|e| DuckDBError {
            message: format!("Failed to lock connections: {}", e),
            code: "LOCK_ERROR".to_string(),
        })?
        .remove(&connection_id);
    Ok(())
}

/// Execute a SELECT query and return results
#[tauri::command]
pub fn duckdb_query(
    state: State<DuckDBState>,
    connection_id: String,
    sql: String,
) -> Result<DuckDBQueryResult, DuckDBError> {
    let connections = state.connections.lock().map_err(|e| DuckDBError {
        message: format!("Failed to lock connections: {}", e),
        code: "LOCK_ERROR".to_string(),
    })?;
    let conn = connections
        .get(&connection_id)
        .ok_or(DuckDBError {
            message: format!("Connection not found: {}", connection_id),
            code: "CONNECTION_NOT_FOUND".to_string(),
        })?;

    let mut stmt = conn.prepare(&sql).map_err(|e| DuckDBError {
        message: format!("Failed to prepare query: {}", e),
        code: "QUERY_ERROR".to_string(),
    })?;

    // Execute query first - column metadata is only available after execution
    let mut result_rows = stmt.query([]).map_err(|e| DuckDBError {
        message: format!("Failed to execute query: {}", e),
        code: "QUERY_ERROR".to_string(),
    })?;

    // Get column info from the executed statement
    let column_count = result_rows.as_ref().map(|s| s.column_count()).unwrap_or(0);
    let columns: Vec<String> = (0..column_count)
        .map(|i| {
            result_rows
                .as_ref()
                .and_then(|s| s.column_name(i).ok())
                .map(|s| s.to_string())
                .unwrap_or_default()
        })
        .collect();

    let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();

    while let Some(row) = result_rows.next().map_err(|e| DuckDBError {
        message: format!("Failed to read row: {}", e),
        code: "QUERY_ERROR".to_string(),
    })? {
        let mut values: Vec<serde_json::Value> = Vec::new();
        for i in 0..column_count {
            let value = row.get_ref(i).map_err(|e| DuckDBError {
                message: format!("Failed to get column value: {}", e),
                code: "QUERY_ERROR".to_string(),
            })?;
            let json_value = convert_value_to_json(value);
            values.push(json_value);
        }
        rows.push(values);
    }

    Ok(DuckDBQueryResult { columns, rows })
}

/// Execute a non-SELECT SQL statement (INSERT, UPDATE, DELETE, CREATE, etc.)
#[tauri::command]
pub fn duckdb_execute(
    state: State<DuckDBState>,
    connection_id: String,
    sql: String,
) -> Result<DuckDBExecuteResult, DuckDBError> {
    let connections = state.connections.lock().map_err(|e| DuckDBError {
        message: format!("Failed to lock connections: {}", e),
        code: "LOCK_ERROR".to_string(),
    })?;
    let conn = connections
        .get(&connection_id)
        .ok_or(DuckDBError {
            message: format!("Connection not found: {}", connection_id),
            code: "CONNECTION_NOT_FOUND".to_string(),
        })?;

    let rows_affected = conn.execute(&sql, []).map_err(|e| DuckDBError {
        message: format!("Failed to execute statement: {}", e),
        code: "EXECUTE_ERROR".to_string(),
    })?;

    Ok(DuckDBExecuteResult { rows_affected })
}

/// Test a DuckDB connection by opening and immediately closing it
#[tauri::command]
pub fn duckdb_test(path: String) -> Result<(), DuckDBError> {
    let _conn = if path == ":memory:" || path.is_empty() {
        Connection::open_in_memory()
    } else {
        Connection::open(&path)
    }
    .map_err(|e| DuckDBError {
        message: format!("Failed to open connection: {}", e),
        code: "CONNECTION_ERROR".to_string(),
    })?;

    Ok(())
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
        ValueRef::Blob(b) => serde_json::json!(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b)),
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
    }
}
