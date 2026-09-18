use sqlx::{migrate::MigrateDatabase, Pool, Sqlite};

use super::{ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};

pub struct SqliteDriver {
    pool: Pool<Sqlite>,
}

impl SqliteDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let conn_str = config
            .connection_string
            .as_deref()
            .ok_or_else(|| DbError::connection_error("connection_string is required for SQLite"))?;

        // Ensure the parent directory exists before creating the database file
        if let Some(path) = conn_str.strip_prefix("sqlite:") {
            if let Some(parent) = std::path::Path::new(path).parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| DbError::connection_error(format!("Failed to create database directory: {}", e)))?;
                }
            }
        }

        // Ensure the database file exists for SQLite
        if !sqlx::sqlite::Sqlite::database_exists(conn_str)
            .await
            .unwrap_or(false)
        {
            sqlx::sqlite::Sqlite::create_database(conn_str)
                .await
                .map_err(|e| DbError::connection_error(e))?;
        }

        let pool = Pool::<Sqlite>::connect(conn_str)
            .await
            .map_err(|e| DbError::connection_error(e))?;

        Ok(Self { pool })
    }
}

super::impl_sqlx_driver!(
    SqliteDriver,
    Sqlite,
    sqlx::sqlite::SqliteArguments<'q>,
    decode_fn = super::decode::sqlite::to_json,
    last_insert_id = |r: &sqlx::sqlite::SqliteQueryResult| Some(r.last_insert_rowid())
);
