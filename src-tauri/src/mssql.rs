use async_native_tls::TlsStream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::State;
use tiberius::{AuthMethod, Client, Config, Query, Row};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

#[derive(Debug, Serialize, Deserialize)]
pub struct MssqlConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
    pub encrypt: Option<bool>,
    pub trust_cert: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct MssqlConnection {
    pub connection_id: String,
}

#[derive(Debug, Serialize)]
pub struct MssqlQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<serde_json::Value>,
    pub rows_affected: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MssqlError {
    pub message: String,
    pub code: String,
}

impl std::fmt::Display for MssqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for MssqlError {}

type MssqlClient = Client<TlsStream<Compat<TcpStream>>>;

struct ConnectionHandle {
    client: MssqlClient,
}

pub struct MssqlConnectionManager {
    connections: Arc<Mutex<HashMap<String, ConnectionHandle>>>,
    next_id: Arc<Mutex<u64>>,
}

impl MssqlConnectionManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }
}

impl Default for MssqlConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

fn row_to_json(row: &Row) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for col in row.columns() {
        let col_name = col.name().to_string();
        // Try to get value as different types, falling back through common types
        // Start with string since SQL Server often returns nvarchar
        let value = if let Some(v) = row.try_get::<&str, _>(col_name.as_str()).ok().flatten() {
            serde_json::json!(v)
        } else if let Some(v) = row.try_get::<i64, _>(col_name.as_str()).ok().flatten() {
            serde_json::json!(v)
        } else if let Some(v) = row.try_get::<i32, _>(col_name.as_str()).ok().flatten() {
            serde_json::json!(v)
        } else if let Some(v) = row.try_get::<i16, _>(col_name.as_str()).ok().flatten() {
            serde_json::json!(v)
        } else if let Some(v) = row.try_get::<u8, _>(col_name.as_str()).ok().flatten() {
            serde_json::json!(v)
        } else if let Some(v) = row.try_get::<f64, _>(col_name.as_str()).ok().flatten() {
            serde_json::json!(v)
        } else if let Some(v) = row.try_get::<f32, _>(col_name.as_str()).ok().flatten() {
            serde_json::json!(v)
        } else if let Some(v) = row.try_get::<bool, _>(col_name.as_str()).ok().flatten() {
            serde_json::json!(v)
        } else if let Some(v) = row.try_get::<&[u8], _>(col_name.as_str()).ok().flatten() {
            // Binary data - encode as base64
            use base64::{Engine as _, engine::general_purpose::STANDARD};
            serde_json::json!(STANDARD.encode(v))
        } else {
            // NULL or unsupported type (dates, decimals, GUIDs handled as NULL for now)
            // These would require additional feature flags in tiberius
            serde_json::Value::Null
        };
        obj.insert(col_name, value);
    }
    serde_json::Value::Object(obj)
}

#[tauri::command]
pub async fn mssql_connect(
    config: MssqlConfig,
    manager: State<'_, MssqlConnectionManager>,
) -> Result<MssqlConnection, MssqlError> {
    let mut tiberius_config = Config::new();

    tiberius_config.host(&config.host);
    tiberius_config.port(config.port);
    tiberius_config.database(&config.database);
    tiberius_config.authentication(AuthMethod::sql_server(&config.username, &config.password));

    // We handle TLS manually, so tell tiberius not to do encryption
    tiberius_config.encryption(tiberius::EncryptionLevel::NotSupported);

    // Connect with timeout
    let tcp = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        TcpStream::connect(tiberius_config.get_addr()),
    )
    .await
    .map_err(|_| MssqlError {
        message: "Connection timed out".to_string(),
        code: "TIMEOUT".to_string(),
    })?
    .map_err(|e| MssqlError {
        message: format!("Failed to connect: {}", e),
        code: "CONNECTION_ERROR".to_string(),
    })?;

    tcp.set_nodelay(true).map_err(|e| MssqlError {
        message: format!("Failed to set TCP nodelay: {}", e),
        code: "TCP_ERROR".to_string(),
    })?;

    // Wrap TCP stream with compat for futures-io trait compatibility
    let tcp_compat = tcp.compat();

    // Wrap with TLS - Azure SQL requires encryption
    let tls_connector = async_native_tls::TlsConnector::new()
        .danger_accept_invalid_certs(config.trust_cert.unwrap_or(true))
        .use_sni(true);

    let tls_stream = tls_connector
        .connect(&config.host, tcp_compat)
        .await
        .map_err(|e| MssqlError {
            message: format!("TLS connection failed: {}", e),
            code: "TLS_ERROR".to_string(),
        })?;

    // TlsStream already implements futures-io traits, pass directly to tiberius
    let client = Client::connect(tiberius_config, tls_stream)
        .await
        .map_err(|e| MssqlError {
            message: format!("Failed to connect to SQL Server: {}", e),
            code: "AUTH_ERROR".to_string(),
        })?;

    // Generate connection ID
    let connection_id = {
        let mut next_id = manager.next_id.lock().await;
        let id = format!("mssql-{}", *next_id);
        *next_id += 1;
        id
    };

    // Store connection
    {
        let mut connections = manager.connections.lock().await;
        connections.insert(connection_id.clone(), ConnectionHandle { client });
    }

    Ok(MssqlConnection { connection_id })
}

#[tauri::command]
pub async fn mssql_disconnect(
    connection_id: String,
    manager: State<'_, MssqlConnectionManager>,
) -> Result<(), MssqlError> {
    let mut connections = manager.connections.lock().await;

    if connections.remove(&connection_id).is_some() {
        Ok(())
    } else {
        Err(MssqlError {
            message: format!("Connection not found: {}", connection_id),
            code: "CONNECTION_NOT_FOUND".to_string(),
        })
    }
}

#[tauri::command]
pub async fn mssql_query(
    connection_id: String,
    sql: String,
    manager: State<'_, MssqlConnectionManager>,
) -> Result<MssqlQueryResult, MssqlError> {
    let mut connections = manager.connections.lock().await;

    let handle = connections.get_mut(&connection_id).ok_or(MssqlError {
        message: format!("Connection not found: {}", connection_id),
        code: "CONNECTION_NOT_FOUND".to_string(),
    })?;

    let query = Query::new(&sql);
    let stream = query.query(&mut handle.client).await.map_err(|e| MssqlError {
        message: format!("Query failed: {}", e),
        code: "QUERY_ERROR".to_string(),
    })?;

    let rows = stream.into_first_result().await.map_err(|e| MssqlError {
        message: format!("Failed to fetch results: {}", e),
        code: "FETCH_ERROR".to_string(),
    })?;

    // Get column names from first row or return empty result
    let columns: Vec<String> = if !rows.is_empty() {
        rows[0].columns().iter().map(|c| c.name().to_string()).collect()
    } else {
        vec![]
    };

    // Convert rows to JSON
    let json_rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| row_to_json(row))
        .collect();

    Ok(MssqlQueryResult {
        columns,
        rows: json_rows,
        rows_affected: 0,
    })
}

#[tauri::command]
pub async fn mssql_execute(
    connection_id: String,
    sql: String,
    manager: State<'_, MssqlConnectionManager>,
) -> Result<MssqlQueryResult, MssqlError> {
    let mut connections = manager.connections.lock().await;

    let handle = connections.get_mut(&connection_id).ok_or(MssqlError {
        message: format!("Connection not found: {}", connection_id),
        code: "CONNECTION_NOT_FOUND".to_string(),
    })?;

    let result = handle.client.execute(&sql, &[]).await.map_err(|e| MssqlError {
        message: format!("Execute failed: {}", e),
        code: "EXECUTE_ERROR".to_string(),
    })?;

    Ok(MssqlQueryResult {
        columns: vec![],
        rows: vec![],
        rows_affected: result.rows_affected().iter().sum(),
    })
}
