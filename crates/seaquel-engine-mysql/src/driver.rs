use std::time::Duration;

use sqlx::{MySql, Pool};

use seaquel_engine::{
    CappedResult, ConnectConfig, DatabaseStatistics, DbError, ExplainResult, ReadOnlyOptions,
    RowCap, SchemaColumn, SchemaIndex, SchemaTable, Value,
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
        async fn query_read_only(
            &self,
            sql: &str,
            params: Vec<Value>,
            max_rows: Option<usize>,
        ) -> Result<CappedResult, DbError> {
            let cap = RowCap::read_only(max_rows, None);
            read_only_call(&self.pool, self.flavor, sql, &params, cap, None).await
        }

        async fn query_read_only_with(
            &self,
            sql: &str,
            params: Vec<Value>,
            options: ReadOnlyOptions,
        ) -> Result<CappedResult, DbError> {
            let cap = options.row_cap();
            read_only_call(&self.pool, self.flavor, sql, &params, cap, options.timeout).await
        }

        /// MariaDB evaluates a constant subquery while it plans: a plain
        /// `EXPLAIN SELECT … WHERE n = (SELECT NEXTVAL(s))` advanced the
        /// sequence. So the EXPLAIN runs exactly like a read-only query, in
        /// [`read_only_call`]'s session and transaction, where the sequence
        /// write is refused (1792). MySQL 8 didn't write through EXPLAIN,
        /// but gets the same path. A prepared statement is one statement.
        async fn explain_read_only(
            &self,
            sql: &str,
            params: Vec<Value>,
            timeout: Option<Duration>,
        ) -> Result<ExplainResult, DbError> {
            let cap = RowCap::fail(seaquel_engine::max_query_rows());
            let explain = introspect::explain_sql(sql, false, self.flavor);
            let r =
                read_only_call(&self.pool, self.flavor, &explain, &params, cap, timeout).await?;
            introspect::parse_explain(&r.into(), false, self.flavor)
        }
    }
);

/// A read-only session and transaction (AI safety plan, Decision 1) on a
/// pooled connection that is closed afterwards, never returned: the setting,
/// `SET SESSION` changes and `GET_LOCK` locks would survive a rollback.
///
/// `START TRANSACTION READ ONLY` alone doesn't stop DDL, which commits the
/// transaction and then runs; the session setting covers the statement after
/// that commit (`SET SESSION TRANSACTION READ ONLY` sets the session's
/// `transaction_read_only`, or `tx_read_only` on older servers). The session
/// setting alone doesn't stop a procedure that switches it off before it
/// writes, because `CALL` runs each statement of a procedure as its own; the
/// transaction does, unless the procedure commits it first (a gap, see
/// `tests/read_only.rs`). The SQL goes through `fetch_capped`, a prepared
/// statement, so it is one statement.
///
/// With `timeout` the server stops the statement itself: MySQL's
/// `max_execution_time` (3024), which covers only SELECT, and MariaDB's
/// `max_statement_time` (1969), which covers every statement; both are
/// reported as `TIMEOUT`. Once the statement is running, dropping the call
/// or leaving its result unread sends `KILL QUERY` from a connection of its
/// own ([`KillOnDrop`]): closing the connection alone left the statement
/// running on the server.
///
/// Taking the connection from the pool means a pool at its limit makes this
/// wait, like any other query, instead of opening more.
async fn read_only_call(
    pool: &Pool<MySql>,
    flavor: Flavor,
    sql: &str,
    params: &[Value],
    cap: RowCap,
    timeout: Option<Duration>,
) -> Result<CappedResult, DbError> {
    let mut conn = pool.acquire().await.map_err(DbError::query_error)?;
    // Before the first statement, so an error or a dropped future closes it
    // too.
    conn.close_on_drop();
    for setup in setup_sql(flavor, timeout) {
        sqlx::Executor::execute(&mut *conn, setup.as_str())
            .await
            .map_err(DbError::query_error)?;
    }
    let id: Option<(u64,)> = sqlx::query_as("SELECT CONNECTION_ID()")
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::query_error)?;
    let mut kill = id.map(|(id,)| KillOnDrop::new(pool, id));
    let result = fetch_capped(&mut *conn, sql, params, cap).await;
    // The statement is over when it read all its rows or the server ended
    // it with an error; otherwise (the row cap, a truncation, a decode
    // error) it may still be running.
    let finished = match &result {
        Ok(r) => !r.truncated,
        Err(e) => server_error(e).is_some(),
    };
    if finished {
        if let Some(kill) = kill.as_mut() {
            kill.disarm();
        }
    }
    // The server rolls the transaction back when the connection goes.
    // Closing now rather than on drop frees its locks as soon as it does.
    let _ = conn.close().await;
    drop(kill);
    result.map_err(|e| match server_error(&e) {
        Some((ER_QUERY_TIMEOUT | ER_STATEMENT_TIMEOUT, _, message)) if timeout.is_some() => {
            seaquel_engine::timeout_error(message)
        }
        _ => read_only_refusal(e),
    })
}

/// ER_NO_SUCH_THREAD: `KILL` of a connection that is gone.
fn is_no_such_thread(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>())
        .is_some_and(|d| d.number() == 1094)
}

/// MySQL: "maximum statement execution time exceeded".
const ER_QUERY_TIMEOUT: u16 = 3024;
/// MariaDB: "Query execution was interrupted (max_statement_time exceeded)".
const ER_STATEMENT_TIMEOUT: u16 = 1969;

/// The session setup: the timeout for this server, then the read-only
/// session and transaction. The numbers are formatted here, never taken
/// from the caller's text.
fn setup_sql(flavor: Flavor, timeout: Option<Duration>) -> Vec<String> {
    let mut sql = Vec::new();
    if let Some(t) = timeout {
        // 0 turns either setting off. MySQL's is milliseconds up to 2^32-1;
        // MariaDB's is seconds with microseconds.
        let ms = t.as_millis().clamp(1, u128::from(u32::MAX));
        sql.push(match flavor {
            Flavor::Mysql => format!("SET SESSION max_execution_time = {ms}"),
            Flavor::Mariadb => format!(
                "SET SESSION max_statement_time = {}.{:03}",
                ms / 1000,
                ms % 1000
            ),
        });
    }
    // Not `SET SESSION transaction_read_only = 1`: MariaDB 10.x and MySQL
    // before 5.7.20 call that variable `tx_read_only`. This form works on
    // MySQL 5.6.5+ and MariaDB 10.0+.
    sql.push("SET SESSION TRANSACTION READ ONLY".to_string());
    sql.push("START TRANSACTION READ ONLY".to_string());
    sql
}

/// Sends `KILL QUERY` for the read-only call's connection when dropped
/// while armed, from a new connection (not the pool's: a full pool would
/// make it wait) on a task of its own. Connection ids aren't reused while
/// the server runs, and the call's connection is closed, never reused, so
/// the kill can't reach another statement.
///
/// Engine crates are native only, so the task runs on the ambient tokio
/// runtime (sqlx needs one anyway). Without one nothing is sent.
struct KillOnDrop {
    options: sqlx::mysql::MySqlConnectOptions,
    id: u64,
    armed: bool,
}

impl KillOnDrop {
    fn new(pool: &Pool<MySql>, id: u64) -> Self {
        Self {
            options: (*pool.connect_options()).clone(),
            id,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (options, id) = (self.options.clone(), self.id);
        runtime.spawn(async move {
            use sqlx::Connection;
            let sent = async {
                let mut conn = sqlx::MySqlConnection::connect_with(&options).await?;
                // A number, formatted here. Over the text protocol: KILL
                // isn't preparable on every server.
                let r =
                    sqlx::Executor::execute(&mut conn, format!("KILL QUERY {id}").as_str()).await;
                let _ = conn.close().await;
                r
            };
            match sent.await {
                Ok(_) => seaquel_engine::__private::log::debug!(
                    activity = "db.query_read_only";
                    "Sent KILL QUERY for an unfinished read-only query"
                ),
                // The connection already ended, which is the goal.
                Err(e) if is_no_such_thread(&e) => {}
                Err(e) => seaquel_engine::__private::log::warn!(
                    activity = "db.query_read_only";
                    "KILL QUERY for an unfinished read-only query failed: {e}"
                ),
            }
        });
    }
}

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
    fn setup_sets_each_servers_timeout() {
        let tail = [
            "SET SESSION TRANSACTION READ ONLY".to_string(),
            "START TRANSACTION READ ONLY".to_string(),
        ];
        assert_eq!(setup_sql(Flavor::Mysql, None), tail);
        assert_eq!(setup_sql(Flavor::Mariadb, None), tail);
        let t = Some(Duration::from_millis(1500));
        assert_eq!(
            setup_sql(Flavor::Mysql, t)[0],
            "SET SESSION max_execution_time = 1500"
        );
        assert_eq!(
            setup_sql(Flavor::Mariadb, t)[0],
            "SET SESSION max_statement_time = 1.500"
        );
        // 0 would mean no limit.
        let tiny = Some(Duration::from_micros(10));
        assert_eq!(
            setup_sql(Flavor::Mysql, tiny)[0],
            "SET SESSION max_execution_time = 1"
        );
        assert_eq!(
            setup_sql(Flavor::Mariadb, tiny)[0],
            "SET SESSION max_statement_time = 0.001"
        );
        assert_eq!(
            setup_sql(Flavor::Mysql, Some(Duration::from_secs(u64::MAX)))[0],
            format!("SET SESSION max_execution_time = {}", u32::MAX)
        );
        assert_eq!(&setup_sql(Flavor::Mariadb, t)[1..], tail);
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
