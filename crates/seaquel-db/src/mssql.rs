use async_trait::async_trait;
use futures::FutureExt;
use log::{error, info, warn};
use std::panic::AssertUnwindSafe;
use tiberius::{AuthMethod, Client, Config, Query, Row};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use super::{ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};

/// rustls reports certificate verification failures as I/O errors rather
/// than `Error::Tls`, so match on the message for those.
fn is_tls_error(e: &tiberius::error::Error) -> bool {
    match e {
        tiberius::error::Error::Tls(_) => true,
        tiberius::error::Error::Io { message, .. } => message.contains("certificate"),
        _ => false,
    }
}

async fn run_query(
    client: &mut Client<Compat<TcpStream>>,
    query: Query<'_>,
) -> Result<Vec<Row>, tiberius::error::Error> {
    query.query(client).await?.into_first_result().await
}

/// Bind a JSON parameter onto a tiberius `Query`. Scalars (null, bool, int,
/// float, string) are translated directly. Nested arrays/objects are
/// rejected — callers should flatten upstream or use an inline literal.
///
/// For SQL NULL we bind `None::<&str>`; tiberius treats it as a nullable
/// varchar parameter, which SQL Server accepts for any nullable column.
fn bind_mssql_param(query: &mut Query<'_>, v: &serde_json::Value) -> Result<(), DbError> {
    match v {
        serde_json::Value::Null => {
            query.bind(None::<&str>);
        }
        serde_json::Value::Bool(b) => {
            query.bind(*b);
        }
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                query.bind(i);
            } else if let Some(f) = n.as_f64() {
                query.bind(f);
            } else {
                return Err(DbError::query_error(
                    "unsupported numeric parameter (outside i64/f64 range)",
                ));
            }
        }
        serde_json::Value::String(s) => {
            query.bind(s.clone());
        }
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            return Err(DbError::query_error(
                "array/object parameters are not supported",
            ));
        }
    }
    Ok(())
}

pub struct MssqlDriver {
    client: Mutex<Client<Compat<TcpStream>>>,
}

impl MssqlDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let host = config.host.as_deref().unwrap_or("localhost");
        let port = config.port.unwrap_or(1433);
        let database = config.database.as_deref().unwrap_or("master");
        let username = config.username.as_deref().unwrap_or("");
        let password = config.password.as_deref().unwrap_or("");
        let encrypt = config.encrypt.unwrap_or(true);

        info!(activity = "db.connect", driver = "mssql", encrypt = encrypt; "Connecting");

        let mut tiberius_config = Config::new();
        tiberius_config.host(host);
        tiberius_config.port(port);
        tiberius_config.database(database);
        tiberius_config.authentication(AuthMethod::sql_server(username, password));
        tiberius_config.encryption(if encrypt {
            tiberius::EncryptionLevel::Required
        } else {
            tiberius::EncryptionLevel::NotSupported
        });

        // Connect with timeout
        let tcp = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            TcpStream::connect(tiberius_config.get_addr()),
        )
        .await
        .map_err(|_| DbError {
            message: "Connection timed out".to_string(),
            code: "TIMEOUT".to_string(),
        })?
        .map_err(|e| DbError::connection_error(e))?;

        tcp.set_nodelay(true).map_err(|e| DbError {
            message: format!("Failed to set TCP nodelay: {}", e),
            code: "TCP_ERROR".to_string(),
        })?;

        // TLS is negotiated by tiberius itself: TDS sends a plaintext PRELOGIN
        // packet first and only then runs the TLS handshake inside TDS packets.
        // Wrapping the raw TCP stream in TLS up front makes the server reject
        // the handshake (e.g. Azure SQL, which always requires encryption).
        if encrypt && config.trust_cert.unwrap_or(false) {
            // The operator explicitly opted in to skipping cert validation.
            // This is unsafe against MITM — log it prominently so it shows
            // up in audits. `tls = "insecure"` is the structured field to
            // grep for in aggregated logs.
            warn!(
                activity = "db.connect",
                driver = "mssql",
                tls = "insecure",
                host = host;
                "MSSQL connecting with TLS certificate verification disabled (trust_cert=true)"
            );
            tiberius_config.trust_cert();
        }

        let client = Client::connect(tiberius_config, tcp.compat_write())
            .await
            .map_err(|e| {
                if is_tls_error(&e) {
                    DbError {
                        message: format!(
                            "TLS connection failed: {}. Try setting SSL Mode to 'disable' for servers without TLS, or 'prefer' to skip certificate verification.",
                            e
                        ),
                        code: "TLS_ERROR".to_string(),
                    }
                } else {
                    DbError {
                        message: format!("Failed to connect to SQL Server: {}", e),
                        code: "AUTH_ERROR".to_string(),
                    }
                }
            })?;

        Ok(Self {
            client: Mutex::new(client),
        })
    }
}

/// Convert a tiberius Row to a vector of JSON values (positional)
fn row_to_values(row: &Row) -> Vec<serde_json::Value> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use tiberius::numeric::Numeric;
    use tiberius::time::chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};
    use tiberius::xml::XmlData;
    use tiberius::Uuid;

    row.columns()
        .iter()
        .map(|col| {
            let col_name = col.name();
            if let Some(v) = row.try_get::<&str, _>(col_name).ok().flatten() {
                serde_json::json!(v)
            } else if let Some(v) = row.try_get::<i64, _>(col_name).ok().flatten() {
                serde_json::json!(v)
            } else if let Some(v) = row.try_get::<i32, _>(col_name).ok().flatten() {
                serde_json::json!(v)
            } else if let Some(v) = row.try_get::<i16, _>(col_name).ok().flatten() {
                serde_json::json!(v)
            } else if let Some(v) = row.try_get::<u8, _>(col_name).ok().flatten() {
                serde_json::json!(v)
            } else if let Some(v) = row.try_get::<f64, _>(col_name).ok().flatten() {
                serde_json::json!(v)
            } else if let Some(v) = row.try_get::<f32, _>(col_name).ok().flatten() {
                serde_json::json!(v)
            } else if let Some(v) = row.try_get::<bool, _>(col_name).ok().flatten() {
                serde_json::json!(v)
            } else if let Some(v) = row.try_get::<Numeric, _>(col_name).ok().flatten() {
                serde_json::json!(f64::from(v))
            } else if let Some(v) = row
                .try_get::<NaiveDateTime, _>(col_name)
                .ok()
                .flatten()
            {
                serde_json::json!(v.to_string())
            } else if let Some(v) = row.try_get::<NaiveDate, _>(col_name).ok().flatten() {
                serde_json::json!(v.to_string())
            } else if let Some(v) = row.try_get::<NaiveTime, _>(col_name).ok().flatten() {
                serde_json::json!(v.to_string())
            } else if let Some(v) = row
                .try_get::<DateTime<FixedOffset>, _>(col_name)
                .ok()
                .flatten()
            {
                serde_json::json!(v.to_rfc3339())
            } else if let Some(v) = row.try_get::<Uuid, _>(col_name).ok().flatten() {
                serde_json::json!(v.to_string())
            } else if let Some(v) = row.try_get::<&XmlData, _>(col_name).ok().flatten() {
                serde_json::json!(v.as_ref())
            } else if let Some(v) = row.try_get::<&[u8], _>(col_name).ok().flatten() {
                serde_json::json!(STANDARD.encode(v))
            } else {
                serde_json::Value::Null
            }
        })
        .collect()
}

#[async_trait]
impl Driver for MssqlDriver {
    async fn query(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<QueryResult, DbError> {
        let mut query = Query::new(sql.to_string());
        for p in &params {
            bind_mssql_param(&mut query, p)?;
        }

        let mut client = self.client.lock().await;

        // tiberius 0.12.3 panics via `todo!()` on SQL_VARIANT / UDT column
        // metadata (token_col_metadata.rs:174/204). Catch the panic so the
        // Tauri command returns an error instead of hanging the UI forever.
        // TODO: drop the catch_unwind once tiberius >0.12.3 ships fixes for
        // token_col_metadata SQL_VARIANT/UDT decoding.
        let rows = match AssertUnwindSafe(run_query(&mut client, query)).catch_unwind().await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                error!(activity = "db.query", driver = "mssql", error_code = "QUERY_ERROR"; "Query failed");
                return Err(DbError::query_error(e));
            }
            Err(_) => {
                error!(activity = "db.query", driver = "mssql", error_code = "UNSUPPORTED_TYPE"; "Tiberius panicked on column metadata");
                return Err(DbError {
                    message: "SQL Server driver does not support a column type in this result (SQL_VARIANT or user-defined type). CAST the column to NVARCHAR(MAX) in your query.".to_string(),
                    code: "UNSUPPORTED_TYPE".to_string(),
                });
            }
        };

        // tiberius loads the full result set into a Vec via
        // `into_first_result`, so the cap here is post-hoc (prevents the
        // monster payload from reaching the frontend, but doesn't prevent
        // the Vec allocation itself). A proper row-by-row iteration would
        // require refactoring run_query; leave as TODO.
        let cap = super::max_query_rows();
        if rows.len() > cap {
            return Err(DbError::result_too_large(cap));
        }

        let columns: Vec<String> = if !rows.is_empty() {
            rows[0]
                .columns()
                .iter()
                .map(|c| c.name().to_string())
                .collect()
        } else {
            vec![]
        };

        let result_rows: Vec<Vec<serde_json::Value>> =
            rows.iter().map(|row| row_to_values(row)).collect();

        Ok(QueryResult {
            columns,
            rows: result_rows,
        })
    }

    async fn execute(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<ExecuteResult, DbError> {
        let mut query = Query::new(sql.to_string());
        for p in &params {
            bind_mssql_param(&mut query, p)?;
        }

        let mut client = self.client.lock().await;

        let result = match AssertUnwindSafe(query.execute(&mut *client)).catch_unwind().await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                error!(activity = "db.execute", driver = "mssql", error_code = "EXECUTE_ERROR"; "Execute failed");
                return Err(DbError::execute_error(e));
            }
            Err(_) => {
                error!(activity = "db.execute", driver = "mssql", error_code = "UNSUPPORTED_TYPE"; "Tiberius panicked");
                return Err(DbError {
                    message: "SQL Server driver panicked. A column type in the result is unsupported (SQL_VARIANT or user-defined type).".to_string(),
                    code: "UNSUPPORTED_TYPE".to_string(),
                });
            }
        };

        let rows_affected: u64 = result.rows_affected().iter().sum();

        Ok(ExecuteResult {
            rows_affected,
            last_insert_id: None,
        })
    }

    async fn close(&self) -> Result<(), DbError> {
        // tiberius Client doesn't have an explicit close method;
        // dropping the client closes the connection
        Ok(())
    }
}
