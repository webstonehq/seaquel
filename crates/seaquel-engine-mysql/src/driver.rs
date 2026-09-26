use sqlx::{MySql, Pool};

use seaquel_engine::{
    ConnectConfig, DatabaseStatistics, DbError, ExplainResult, QueryResult, SchemaColumn,
    SchemaIndex, SchemaTable, Value,
};

use crate::introspect::{self, Flavor};

pub struct MysqlDriver {
    pool: Pool<MySql>,
    /// MySQL or MariaDB, from `SELECT VERSION()` at connect.
    flavor: Flavor,
}

impl MysqlDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let conn_str = config
            .connection_string
            .as_deref()
            .ok_or_else(|| DbError::connection_error("connection_string is required for MySQL"))?;

        let pool = Pool::<MySql>::connect(conn_str)
            .await
            .map_err(DbError::connection_error)?;

        let version: (String,) = sqlx::query_as(introspect::VERSION_SQL)
            .fetch_one(&pool)
            .await
            .map_err(DbError::connection_error)?;

        Ok(Self {
            pool,
            flavor: Flavor::from_version(&version.0),
        })
    }
}

/// The MySQL error number, SQLSTATE and message of a server error, from the
/// text sqlx gives it (`error returned from database: 1142 (42000): …`),
/// which is all a `DbError` keeps.
fn server_error(e: &DbError) -> Option<(u16, &str, &str)> {
    let rest = e.message.split_once("error returned from database: ")?.1;
    let (number, rest) = rest.split_once(" (")?;
    let (sqlstate, message) = rest.split_once("): ")?;
    Some((number.parse().ok()?, sqlstate, message))
}

/// ER_CANT_EXECUTE_IN_READ_ONLY_TRANSACTION (1792), on MySQL and MariaDB
/// alike, as `READ_ONLY` with the server's message. Anything else as it is.
fn read_only_refusal(e: DbError) -> DbError {
    match server_error(&e) {
        Some((1792, _, message)) => DbError::read_only(message),
        _ => e,
    }
}

/// A read the server refused for lack of privileges: ER_TABLEACCESS_DENIED
/// (1142), ER_COLUMNACCESS_DENIED (1143) or ER_DBACCESS_DENIED (1044), all
/// SQLSTATE 42000, on MySQL and MariaDB alike.
fn is_permission_error(e: &DbError) -> bool {
    matches!(server_error(e), Some((1142 | 1143 | 1044, "42000", _)))
}

seaquel_engine::impl_sqlx_driver!(
    MysqlDriver,
    MySql,
    sqlx::mysql::MySqlArguments,
    decode_fn = crate::decode::to_value,
    bind_fn = crate::bind::bind_value,
    last_insert_id = |r: &sqlx::mysql::MySqlQueryResult| Some(r.last_insert_id() as i64),
    introspection = {
        async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
            let r = self.query(introspect::SCHEMAS_SQL, vec![]).await?;
            Ok(introspect::parse_schemas(&r))
        }

        async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
            let r = self.query(introspect::SCHEMA_SQL, vec![]).await?;
            Ok(introspect::parse_schema(&r))
        }

        async fn table_metadata(
            &self,
            schema: &str,
            table: &str,
        ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
            // Bug fix 1: bound as ? = table, ? = schema.
            let params = || vec![Value::from(table), Value::from(schema)];
            let (columns, indexes) = futures::join!(
                self.query(introspect::COLUMNS_SQL, params()),
                self.query(introspect::INDEXES_SQL, params()),
            );
            let mut columns = introspect::parse_columns(&columns?, self.flavor);
            let indexes = introspect::parse_indexes(&indexes?);
            // Task 18: UNIQUE from the indexes (the parse stays the TS's).
            seaquel_engine::introspect::apply_unique_indexes(&mut columns, &indexes);
            Ok((columns, indexes))
        }

        /// Index usage reads `mysql.innodb_index_stats`, which needs a grant
        /// on the `mysql` schema. Without one it is empty instead of failing
        /// the whole Statistics view.
        async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
            let (overview, table_sizes, index_usage) = futures::join!(
                self.query(introspect::OVERVIEW_SQL, vec![]),
                self.query(introspect::TABLE_SIZES_SQL, vec![]),
                self.query(introspect::INDEX_USAGE_SQL, vec![]),
            );
            let index_usage = match index_usage {
                Ok(r) => introspect::parse_index_usage(&r),
                Err(e) if is_permission_error(&e) => vec![],
                Err(e) => return Err(e),
            };
            Ok(DatabaseStatistics {
                overview: introspect::parse_overview(&overview?),
                table_sizes: introspect::parse_table_sizes(&table_sizes?),
                index_usage,
            })
        }

        async fn explain(
            &self,
            sql: &str,
            params: Vec<Value>,
            analyze: bool,
        ) -> Result<ExplainResult, DbError> {
            let r = self
                .query(&introspect::explain_sql(sql, analyze, self.flavor), params)
                .await?;
            introspect::parse_explain(&r, analyze, self.flavor)
        }
    },
    read_only = {
        /// A read-only session and transaction (AI safety plan, Decision 1)
        /// on a pooled connection that is closed afterwards, never returned:
        /// the setting, `SET SESSION` changes and `GET_LOCK` locks would
        /// survive a rollback.
        ///
        /// `START TRANSACTION READ ONLY` alone doesn't stop DDL, which commits
        /// the transaction and then runs; the session setting covers the
        /// statement after that commit (`SET SESSION TRANSACTION READ ONLY`
        /// sets the session's `transaction_read_only`, or `tx_read_only` on
        /// older servers). The session setting alone doesn't
        /// stop a procedure that switches it off before it writes, because
        /// `CALL` runs each statement of a procedure as its own; the
        /// transaction does, unless the procedure commits it first (a gap,
        /// see `tests/read_only.rs`). The user's SQL goes through
        /// `fetch_capped`, a prepared statement, so it is one statement.
        ///
        /// Taking the connection from the pool means a pool at its limit
        /// makes this wait, like any other query, instead of opening more.
        async fn query_read_only(
            &self,
            sql: &str,
            params: Vec<Value>,
        ) -> Result<QueryResult, DbError> {
            let mut conn = self.pool.acquire().await.map_err(DbError::query_error)?;
            // Before the first statement, so an error or a dropped future
            // closes it too.
            conn.close_on_drop();
            for setup in [
                // Not `SET SESSION transaction_read_only = 1`: MariaDB 10.x
                // and MySQL before 5.7.20 call that variable `tx_read_only`.
                // This form works on MySQL 5.6.5+ and MariaDB 10.0+.
                "SET SESSION TRANSACTION READ ONLY",
                "START TRANSACTION READ ONLY",
            ] {
                sqlx::Executor::execute(&mut *conn, setup)
                    .await
                    .map_err(DbError::query_error)?;
            }
            let result = fetch_capped(&mut *conn, sql, &params).await;
            // The server rolls the transaction back when the connection goes.
            // Closing now rather than on drop frees its locks as soon as it
            // does.
            let _ = conn.close().await;
            result.map_err(read_only_refusal)
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_errors() {
        let e = DbError::query_error(
            "error returned from database: 1142 (42000): SELECT command denied to user 'u'@'%' for table 'innodb_index_stats'",
        );
        assert!(is_permission_error(&e));
        assert!(!is_permission_error(&DbError::query_error(
            "error returned from database: 1054 (42S22): Unknown column 'x'"
        )));
        // Only the server's code counts, not the words.
        assert!(!is_permission_error(&DbError::query_error(
            "error returned from database: 1146 (42S02): Table 'access denied 1142' doesn't exist"
        )));
        assert!(!is_permission_error(&DbError::query_error(
            "pool timed out: access denied"
        )));
    }

    #[test]
    fn read_only_refusals() {
        let e = read_only_refusal(DbError::query_error(
            "error returned from database: 1792 (25006): Cannot execute statement in a READ ONLY transaction.",
        ));
        assert_eq!(e.code, "READ_ONLY");
        assert_eq!(
            e.message,
            "Cannot execute statement in a READ ONLY transaction."
        );
        let other = read_only_refusal(DbError::query_error(
            "error returned from database: 1064 (42000): You have an error in your SQL syntax",
        ));
        assert_eq!(other.code, "QUERY_ERROR");
    }
}
