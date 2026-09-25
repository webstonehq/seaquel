use std::path::PathBuf;
use std::str::FromStr;

use sqlx::{migrate::MigrateDatabase, sqlite::SqliteConnectOptions, Pool, Sqlite};

use seaquel_engine::{ConnectConfig, DbError};

/// Directory containing the database file, resolved the same way sqlx will
/// open it. Stripping the scheme by hand breaks on Windows: `sqlite://C:\db`
/// minus `sqlite:` leaves `//C:\db`, which Windows treats as a UNC network path.
fn database_parent_dir(conn_str: &str) -> Result<Option<PathBuf>, DbError> {
    let options = SqliteConnectOptions::from_str(conn_str).map_err(DbError::connection_error)?;
    Ok(options
        .get_filename()
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(PathBuf::from))
}

pub struct SqliteDriver {
    pool: Pool<Sqlite>,
}

impl SqliteDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let conn_str = config
            .connection_string
            .as_deref()
            .ok_or_else(|| DbError::connection_error("connection_string is required for SQLite"))?;

        let exists = Sqlite::database_exists(conn_str)
            .await
            .map_err(DbError::connection_error)?;

        if !exists {
            if !config.create_if_missing.unwrap_or(false) {
                let path = SqliteConnectOptions::from_str(conn_str)
                    .map_err(DbError::connection_error)?
                    .get_filename()
                    .display()
                    .to_string();
                return Err(DbError {
                    message: format!("Database file not found: {}", path),
                    code: "FILE_NOT_FOUND".to_string(),
                });
            }

            if let Some(parent) = database_parent_dir(conn_str)? {
                if !parent.exists() {
                    std::fs::create_dir_all(&parent)
                        .map_err(|e| DbError::connection_error(format!("Failed to create database directory: {}", e)))?;
                }
            }

            Sqlite::create_database(conn_str)
                .await
                .map_err(DbError::connection_error)?;
        }

        let pool = Pool::<Sqlite>::connect(conn_str)
            .await
            .map_err(DbError::connection_error)?;

        Ok(Self { pool })
    }
}

seaquel_engine::impl_sqlx_driver!(
    SqliteDriver,
    Sqlite,
    sqlx::sqlite::SqliteArguments<'q>,
    decode_fn = crate::decode::to_json,
    last_insert_id = |r: &sqlx::sqlite::SqliteQueryResult| Some(r.last_insert_rowid())
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn filename(conn_str: &str) -> PathBuf {
        SqliteConnectOptions::from_str(conn_str).unwrap().get_filename().to_path_buf()
    }

    #[test]
    fn windows_path_has_no_unc_prefix() {
        let path = r"C:\Autoit\Project\no_enc\s17.db";
        assert_eq!(filename(&format!("sqlite://{path}")), Path::new(path));
        assert_eq!(filename(&format!("sqlite:{path}")), Path::new(path));
    }

    #[test]
    fn unix_path_parent_dir() {
        assert_eq!(
            database_parent_dir("sqlite:///Users/me/data/app.db").unwrap(),
            Some(PathBuf::from("/Users/me/data"))
        );
    }

    #[test]
    fn query_params_are_not_part_of_the_path() {
        assert_eq!(
            database_parent_dir("sqlite:///tmp/x/app.db?mode=rwc").unwrap(),
            Some(PathBuf::from("/tmp/x"))
        );
    }

    #[test]
    fn in_memory_and_bare_filenames_have_no_parent_dir() {
        assert_eq!(database_parent_dir("sqlite::memory:").unwrap(), None);
        assert_eq!(database_parent_dir("sqlite://app.db").unwrap(), None);
    }
}
