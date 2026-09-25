use std::path::PathBuf;
use std::str::FromStr;

use sqlx::{
    migrate::MigrateDatabase,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Pool, Sqlite,
};

use seaquel_engine::{
    ConnectConfig, DatabaseStatistics, DbError, ExplainResult, SchemaColumn, SchemaIndex,
    SchemaTable, Value,
};

use crate::introspect;

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

/// Whether `conn_str` opens an in-memory database: `sqlite::memory:`,
/// `:memory:`, or `mode=memory` (the rules sqlx's parser applies).
fn is_in_memory(conn_str: &str) -> bool {
    let (path, query) = conn_str.split_once('?').unwrap_or((conn_str, ""));
    let path = path
        .strip_prefix("sqlite://")
        .or_else(|| path.strip_prefix("sqlite:"))
        .unwrap_or(path);
    path == ":memory:" || query.split('&').any(|kv| kv == "mode=memory")
}

/// Pool settings for `conn_str`. An in-memory database (sqlx gives the pool's
/// connections one shared-cache database) lives only while a connection to
/// it is open, so its pool keeps one open for good: the defaults (no minimum,
/// idle connections closed after 10 minutes, every connection replaced after
/// 30) would wipe it, e.g. the desktop tutorial's `sqlite::memory:` after it
/// sat idle. File databases keep the defaults.
fn pool_options(conn_str: &str) -> SqlitePoolOptions {
    let options = SqlitePoolOptions::new();
    if is_in_memory(conn_str) {
        options
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
    } else {
        options
    }
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
                    std::fs::create_dir_all(&parent).map_err(|e| {
                        DbError::connection_error(format!(
                            "Failed to create database directory: {}",
                            e
                        ))
                    })?;
                }
            }

            Sqlite::create_database(conn_str)
                .await
                .map_err(DbError::connection_error)?;
        }

        let pool = pool_options(conn_str)
            .connect(conn_str)
            .await
            .map_err(DbError::connection_error)?;

        Ok(Self { pool })
    }
}

seaquel_engine::impl_sqlx_driver!(
    SqliteDriver,
    Sqlite,
    sqlx::sqlite::SqliteArguments<'q>,
    decode_fn = crate::decode::to_value,
    bind_fn = crate::bind::bind_value,
    last_insert_id = |r: &sqlx::sqlite::SqliteQueryResult| Some(r.last_insert_rowid()),
    introspection = {
        /// Always `main` (TS `SELECT 'main' as schema_name`).
        async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
            Ok(introspect::schemas())
        }

        async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
            let r = self.query(introspect::SCHEMA_SQL, vec![]).await?;
            Ok(introspect::parse_schema(&r))
        }

        /// The schema is ignored, as in the TS: SQLite resolves the bound
        /// table name in `main` (then attached databases).
        async fn table_metadata(
            &self,
            _schema: &str,
            table: &str,
        ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
            let (columns, foreign_keys, indexes) = futures::join!(
                self.query(introspect::COLUMNS_SQL, vec![Value::from(table)]),
                self.query(introspect::FOREIGN_KEYS_SQL, vec![Value::from(table)]),
                self.query(introspect::INDEXES_SQL, vec![Value::from(table)]),
            );
            let mut columns = introspect::parse_columns(&columns?, Some(&foreign_keys?));
            let indexes = indexes?;
            let partial = introspect::partial_indexes(&indexes);
            let indexes = introspect::parse_indexes(&indexes);
            // Task 18: UNIQUE from the indexes (the parse stays the TS's),
            // without partial unique indexes.
            let plain: Vec<SchemaIndex> = indexes
                .iter()
                .filter(|i| !partial.contains(&i.name))
                .cloned()
                .collect();
            seaquel_engine::introspect::apply_unique_indexes(&mut columns, &plain);
            Ok((columns, indexes))
        }

        /// Row counts come from a `COUNT(*)` per table (moved here from
        /// TsEngineClient); a count that fails leaves the table at 0.
        async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
            let (overview, table_sizes, index_usage) = futures::join!(
                self.query(introspect::OVERVIEW_SQL, vec![]),
                self.query(introspect::TABLE_SIZES_SQL, vec![]),
                self.query(introspect::INDEX_USAGE_SQL, vec![]),
            );
            let mut table_sizes = introspect::parse_table_sizes(&table_sizes?);
            let counts = futures::future::join_all(table_sizes.iter().map(|t| {
                let sql = introspect::row_count_sql(&t.name);
                async move { self.query(&sql, vec![]).await }
            }))
            .await;
            for (table, count) in table_sizes.iter_mut().zip(counts) {
                if let Ok(r) = count {
                    table.row_count = introspect::parse_row_count(&r);
                }
            }
            Ok(DatabaseStatistics {
                overview: introspect::parse_overview(&overview?),
                table_sizes,
                index_usage: introspect::parse_index_usage(&index_usage?),
            })
        }

        /// `EXPLAIN QUERY PLAN`. SQLite has no analyzing EXPLAIN, so with
        /// `analyze` the statement itself runs first (it **executes**, as
        /// Postgres's `EXPLAIN ANALYZE` does, writes included), timed here
        /// (moved from TsEngineClient): the root gets the actual row count
        /// and time, and the result the execution time. There is no
        /// per-operator breakdown, and no estimate.
        async fn explain(
            &self,
            sql: &str,
            params: Vec<Value>,
            analyze: bool,
        ) -> Result<ExplainResult, DbError> {
            let measured = if analyze {
                let (result, ms) = crate::timing::timed(self.query(sql, params.clone())).await;
                Some((result?.rows.len(), ms))
            } else {
                None
            };
            let r = self.query(&introspect::explain_sql(sql), params).await?;
            let mut explain = introspect::parse_explain(&r, analyze);
            if let Some((rows, ms)) = measured {
                explain.plan.actual_rows = Some(rows as f64);
                explain.plan.actual_total_time = Some(ms);
                explain.execution_time = Some(ms);
            }
            Ok(explain)
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn filename(conn_str: &str) -> PathBuf {
        SqliteConnectOptions::from_str(conn_str)
            .unwrap()
            .get_filename()
            .to_path_buf()
    }

    #[test]
    fn in_memory_databases_keep_a_connection_open() {
        for conn_str in [
            "sqlite::memory:",
            "sqlite://:memory:",
            ":memory:",
            "sqlite:shared?mode=memory&cache=shared",
            "sqlite::memory:?cache=shared",
        ] {
            let o = pool_options(conn_str);
            assert_eq!(
                (
                    o.get_min_connections(),
                    o.get_idle_timeout(),
                    o.get_max_lifetime()
                ),
                (1, None, None),
                "{conn_str}"
            );
        }
        let defaults = SqlitePoolOptions::new();
        for conn_str in [
            "sqlite:///tmp/app.db",
            "sqlite:app.db?mode=rwc",
            "sqlite:memory.db",
        ] {
            let o = pool_options(conn_str);
            assert_eq!(
                (
                    o.get_min_connections(),
                    o.get_idle_timeout(),
                    o.get_max_lifetime()
                ),
                (
                    defaults.get_min_connections(),
                    defaults.get_idle_timeout(),
                    defaults.get_max_lifetime()
                ),
                "{conn_str}"
            );
        }
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
