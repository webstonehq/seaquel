use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt, TryFutureExt, TryStreamExt};
use sqlx::pool::{PoolConnection, PoolOptions};
use sqlx::postgres::{
    PgConnectOptions, PgConnection, PgQueryResult, PgRow, PgStatement, PgTypeInfo,
};
use sqlx::{ConnectOptions, Either, Pool, Postgres};

use seaquel_engine::{
    CappedResult, ConnectConfig, DatabaseStatistics, DbError, Dialect, ExplainResult, OpenOptions,
    ReadOnlyOptions, RowCap, RunningStatement, SchemaColumn, SchemaIndex, SchemaTable, Value,
};

use crate::dialect::PostgresDialect;
use crate::introspect;

pub struct PostgresDriver {
    pool: Pool<Postgres>,
}

impl PostgresDriver {
    pub async fn connect(config: &ConnectConfig, open: OpenOptions) -> Result<Self, DbError> {
        let conn_str = config.connection_string.as_deref().ok_or_else(|| {
            DbError::connection_error("connection_string is required for PostgreSQL")
        })?;

        // sqlx logs every statement (and, at WARN, each one slower than a
        // second) with its whole SQL: never user SQL. The cancel connections
        // clone these options, so it's off there too.
        let options = PgConnectOptions::from_str(conn_str)
            .map_err(DbError::connection_error)?
            .disable_statement_logging();
        let pool = pool_options(open)
            .connect_with(options)
            .await
            .map_err(DbError::connection_error)?;

        Ok(Self { pool })
    }
}

/// The pool: sqlx's defaults, at most `open.max_pool_size` connections
/// when that is set.
fn pool_options(open: OpenOptions) -> PoolOptions<Postgres> {
    let options = PoolOptions::<Postgres>::new();
    match open.max_pool_size {
        Some(n) => options.max_connections(n.max(1)),
        None => options,
    }
}

seaquel_engine::impl_sqlx_driver!(
    PostgresDriver,
    Postgres,
    sqlx::postgres::PgArguments,
    decode_fn = crate::decode::to_value,
    bind_fn = crate::bind::bind_value,
    last_insert_id = |_: &_| None,
    stream_start = stream_start,
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
            // Bug fix 1: bound as $1 = table, $2 = schema.
            let params = || vec![Value::from(table), Value::from(schema)];
            let (columns, indexes, partial) = futures::join!(
                self.query(introspect::COLUMNS_SQL, params()),
                self.query(introspect::INDEXES_SQL, params()),
                self.query(introspect::PARTIAL_UNIQUE_SQL, params()),
            );
            let mut columns = introspect::parse_columns(&columns?);
            let indexes = introspect::parse_indexes(&indexes?);
            // Task 18: UNIQUE from the indexes (the parse stays the TS's),
            // without partial and INCLUDE unique indexes.
            let partial: Vec<Value> = partial?
                .rows
                .into_iter()
                .map(|mut r| r.swap_remove(0))
                .collect();
            let plain: Vec<SchemaIndex> = indexes
                .iter()
                .filter(|i| !partial.contains(&Value::Text(i.name.clone())))
                .cloned()
                .collect();
            seaquel_engine::introspect::apply_unique_indexes(&mut columns, &plain);
            Ok((columns, indexes))
        }

        async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
            let (overview, table_sizes, index_usage) = futures::join!(
                self.query(introspect::OVERVIEW_SQL, vec![]),
                self.query(introspect::TABLE_SIZES_SQL, vec![]),
                self.query(introspect::INDEX_USAGE_SQL, vec![]),
            );
            Ok(DatabaseStatistics {
                overview: introspect::parse_overview(&overview?),
                table_sizes: introspect::parse_table_sizes(&table_sizes?),
                index_usage: introspect::parse_index_usage(&index_usage?),
            })
        }

        async fn explain(
            &self,
            sql: &str,
            params: Vec<Value>,
            analyze: bool,
        ) -> Result<ExplainResult, DbError> {
            let r = self
                .query(&PostgresDialect.explain_sql(sql, analyze), params)
                .await?;
            introspect::parse_explain(&r, analyze)
        }
    },
    read_only = {
        async fn query_read_only(
            &self,
            sql: &str,
            params: Vec<Value>,
            max_rows: Option<usize>,
        ) -> Result<CappedResult, DbError> {
            read_only_call(
                &self.pool,
                sql,
                &params,
                RowCap::read_only(max_rows, None),
                None,
            )
            .await
        }

        async fn query_read_only_with(
            &self,
            sql: &str,
            params: Vec<Value>,
            options: ReadOnlyOptions,
        ) -> Result<CappedResult, DbError> {
            let cap = options.row_cap();
            read_only_call(&self.pool, sql, &params, cap, options.timeout).await
        }

        /// Planning runs user code: an IMMUTABLE or STABLE function is
        /// folded into a constant while the plan is made, and whatever it
        /// calls runs with it (a VOLATILE function's INSERT was committed
        /// by a plain EXPLAIN). So the EXPLAIN runs exactly like a read-only
        /// query, in [`read_only_call`]'s transaction. The extended protocol
        /// refuses a second statement ("cannot insert multiple commands
        /// into a prepared statement").
        async fn explain_read_only(
            &self,
            sql: &str,
            params: Vec<Value>,
            timeout: Option<Duration>,
        ) -> Result<ExplainResult, DbError> {
            let cap = RowCap::fail(seaquel_engine::max_query_rows());
            let explain = PostgresDialect.explain_sql(sql, false);
            let r = read_only_call(&self.pool, &explain, &params, cap, timeout).await?;
            introspect::parse_explain(&r.into(), false)
        }
    }
);

/// A read-only transaction (AI safety plan, Decision 1) on a pooled
/// connection that is closed afterwards, never returned: `ROLLBACK` doesn't
/// undo session state such as advisory locks or `PREPARE`d statements. The
/// SQL goes through `fetch_capped`, which uses the extended protocol, so a
/// second statement is refused; the simple protocol would let `SET
/// transaction_read_only = off; DELETE …` through as the first statement of
/// the transaction.
///
/// With `timeout`, `SET LOCAL statement_timeout` makes the server stop the
/// statement itself (SQLSTATE 57014, reported as `TIMEOUT`). And once the
/// statement is running, dropping the call (a cancel, the caller's
/// deadline) or leaving its result unread (the row cap, a truncation) sends
/// `pg_cancel_backend` from a connection of its own ([`CancelOnDrop`]):
/// closing the connection alone left the backend running until it next
/// wrote to the socket.
///
/// Taking the connection from the pool means a pool at its limit makes this
/// wait, like any other query, instead of opening more.
async fn read_only_call(
    pool: &Pool<Postgres>,
    sql: &str,
    params: &[Value],
    cap: RowCap,
    timeout: Option<Duration>,
) -> Result<CappedResult, DbError> {
    let mut conn = pool.acquire().await.map_err(DbError::query_error)?;
    // Before the first statement, so an error or a dropped future closes it
    // too.
    conn.close_on_drop();
    // One simple-protocol round trip. The numbers are formatted here, never
    // taken from the caller's text.
    let setup = setup_sql(timeout);
    let rows = sqlx::Executor::fetch_all(&mut *conn, setup.as_str())
        .await
        .map_err(DbError::query_error)?;
    let mut cancel = rows
        .first()
        .and_then(|row| CancelOnDrop::new(pool, row, "db.query_read_only", CancelCheck::None));
    let noted = OnceLock::new();
    let executor = NoteError {
        conn: &mut conn,
        noted: &noted,
    };
    let result = fetch_capped(executor, sql, params, cap).await;
    let finished = result.as_ref().is_ok_and(|r| !r.truncated);
    // The statement is over when it read all its rows or the server ended
    // it with an error. Otherwise (the row cap, a truncation, a decode
    // error) the backend may still be computing rows nobody reads.
    if finished || noted.get().is_some() {
        if let Some(cancel) = cancel.as_mut() {
            cancel.disarm();
        }
    }
    // Only after a query that read all its rows. After an error, the row cap
    // or a truncation, the connection may still be receiving the rest of the
    // result, and `ROLLBACK` would read every remaining row before its own
    // reply; closing makes the server abort the transaction instead.
    if finished {
        if let Err(e) = sqlx::Executor::execute(&mut *conn, "ROLLBACK").await {
            seaquel_engine::__private::log::warn!(
                activity = "db.query_read_only";
                "ROLLBACK failed; closing the connection anyway: {e}"
            );
        }
    }
    // Closing now rather than on drop frees its session state (advisory
    // locks) as soon as the server sees it go.
    let _ = conn.close().await;
    drop(cancel);
    result.map_err(|e| match noted.into_inner() {
        Some((code, message)) if code == READ_ONLY_SQLSTATE => DbError::read_only(message),
        Some((code, message)) if code == QUERY_CANCELED_SQLSTATE && timeout.is_some() => {
            seaquel_engine::timeout_error(message)
        }
        _ => e,
    })
}

/// `BEGIN READ ONLY`, the statement timeout when there is one, and the
/// backend's pid and start time for [`CancelOnDrop`].
fn setup_sql(timeout: Option<Duration>) -> String {
    let mut sql = String::from("BEGIN READ ONLY; ");
    if let Some(t) = timeout {
        // 0 would turn the timeout off; the setting's maximum is INT_MAX ms.
        let ms = t.as_millis().clamp(1, i32::MAX as u128);
        sql.push_str(&format!("SET LOCAL statement_timeout = {ms}; "));
    }
    sql.push_str(BACKEND_SQL);
    sql
}

/// The connection's backend pid and start time (microseconds since the
/// epoch), for [`CancelOnDrop`].
const BACKEND_SQL: &str = "SELECT pid, (extract(epoch FROM backend_start) * 1000000)::int8 \
     FROM pg_stat_activity WHERE pid = pg_backend_pid()";

/// The most of a streamed statement's text [`CancelOnDrop`] compares with
/// what the backend runs (`pg_stat_activity.query` is cut at
/// `track_activity_query_size`, 1 kB by default).
const STATEMENT_PREFIX_CHARS: usize = 4096;

/// What [`CancelOnDrop`] checks before it cancels, besides the backend's
/// pid and start time.
enum CancelCheck {
    /// Nothing more: the read-only path's statement runs in a transaction
    /// on a connection that is closed, never reused.
    None,
    /// The backend is running a statement (`state = 'active'`): a streamed
    /// statement with too little plain text to compare.
    Active,
    /// The backend runs a statement whose text, after leading whitespace,
    /// starts like this (as far as `pg_stat_activity` keeps it).
    Prefix(String),
}

/// `query_stream`'s connection (`impl_sqlx_driver!`'s `stream_start`).
/// Dropped before [`RunningStatement::finish`], it sends `pg_cancel_backend`
/// for the statement ([`CancelOnDrop`]) and closes the connection instead
/// of handing it back: returned, the pool would first wait for the
/// statement to end, and a later cancel could reach the next caller's.
pub(crate) struct RunningStream {
    conn: PoolConnection<Postgres>,
    cancel: Option<CancelOnDrop>,
    finished: bool,
}

/// Looks up the connection's backend before the statement runs: one round
/// trip. When the lookup fails (a server without `pg_stat_activity`), the
/// stream runs as before, with nothing to cancel.
async fn stream_start(
    pool: &Pool<Postgres>,
    mut conn: PoolConnection<Postgres>,
    sql: &str,
) -> RunningStream {
    let backend = sqlx::query(BACKEND_SQL)
        .persistent(false)
        .fetch_optional(&mut *conn)
        .await;
    // Postgres reports the text as sent, `$1` included; the prefix still
    // skips leading whitespace and stops before a character outside the
    // BMP, like MySQL's, so a server encoding other than UTF8 can't fail
    // the comparison. Too short a prefix checks the backend alone.
    let check = match seaquel_engine::statement_prefix(sql, None, STATEMENT_PREFIX_CHARS) {
        Some(prefix) => CancelCheck::Prefix(prefix),
        None => CancelCheck::Active,
    };
    let cancel = match backend {
        Ok(Some(row)) => CancelOnDrop::new(pool, &row, "db.query_stream", check),
        _ => None,
    };
    RunningStream {
        conn,
        cancel,
        finished: false,
    }
}

impl std::ops::Deref for RunningStream {
    type Target = PoolConnection<Postgres>;
    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl std::ops::DerefMut for RunningStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

impl RunningStatement for RunningStream {
    fn finish(&mut self) {
        self.finished = true;
        if let Some(cancel) = self.cancel.as_mut() {
            cancel.disarm();
        }
    }
}

impl Drop for RunningStream {
    fn drop(&mut self) {
        // Then the fields drop: the connection closes, and the armed
        // cancel is sent.
        if !self.finished {
            self.conn.close_on_drop();
        }
    }
}

/// How long a cancel may take (connecting included) before it's given up.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);

/// Sends `pg_cancel_backend` for a statement's backend when dropped while
/// armed, from a new connection (not the pool's: a full pool would make it
/// wait) on a task of its own, given up after [`CANCEL_TIMEOUT`]. Dropping
/// never waits for it. The backend's start time guards against a pid the
/// server has since given to another backend. For a streamed statement,
/// which runs outside a transaction, the backend must also still be
/// running that statement ([`CancelCheck`]).
///
/// That check is by text, not identity. Behind a transaction-pooling proxy
/// (PgBouncer in transaction mode) the looked-up backend needn't be the one
/// that ran the statement: usually nothing is cancelled then, but if that
/// backend is running a statement that starts the same way for another
/// client (the same text from the same app, say), that statement is
/// cancelled. With `track_activities = off` the server doesn't report
/// statements (`state` is never `active` for other sessions), so no cancel
/// is sent and a stopped stream's statement runs to its end.
///
/// Engine crates are native only, so the task runs on the ambient tokio
/// runtime (sqlx needs one anyway). Without one nothing is sent. Nothing
/// it logs holds SQL or a server message: only an error's kind and
/// SQLSTATE ([`error_kind`]).
struct CancelOnDrop {
    options: sqlx::postgres::PgConnectOptions,
    pid: i32,
    started: i64,
    check: CancelCheck,
    activity: &'static str,
    armed: bool,
}

impl CancelOnDrop {
    fn new(
        pool: &Pool<Postgres>,
        row: &PgRow,
        activity: &'static str,
        check: CancelCheck,
    ) -> Option<Self> {
        use sqlx::Row;
        let pid = row.try_get::<i32, _>(0).ok()?;
        let started = row.try_get::<i64, _>(1).ok()?;
        Some(Self {
            options: (*pool.connect_options()).clone(),
            pid,
            started,
            check,
            activity,
            armed: true,
        })
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

/// Cancels the statement of backend `$1` if it is still the backend that
/// started at `$2` (microseconds since the epoch).
const CANCEL_SQL: &str = "SELECT pg_cancel_backend(pid) FROM pg_stat_activity \
     WHERE pid = $1 AND (extract(epoch FROM backend_start) * 1000000)::int8 = $2";

/// [`CANCEL_SQL`], only while that backend runs a statement.
const CANCEL_ACTIVE_SQL: &str = "SELECT pg_cancel_backend(pid) FROM pg_stat_activity \
     WHERE pid = $1 AND (extract(epoch FROM backend_start) * 1000000)::int8 = $2 \
     AND state = 'active'";

/// [`CANCEL_ACTIVE_SQL`], only while that statement's text, after leading
/// whitespace, starts like `$3` (as far as either is known: the server cuts
/// the text it keeps).
const CANCEL_STATEMENT_SQL: &str = "SELECT pg_cancel_backend(pid) FROM ( \
       SELECT pid, ltrim(query, E' \\t\\n\\r\\x0B\\x0C') AS q FROM pg_stat_activity \
       WHERE pid = $1 AND (extract(epoch FROM backend_start) * 1000000)::int8 = $2 \
       AND state = 'active') a \
     WHERE length(q) > 0 \
     AND left(q, least(length(q), length($3))) = left($3, least(length(q), length($3)))";

/// An error's kind, and its SQLSTATE, for a log line: never its message,
/// which can quote the statement.
fn error_kind(e: &sqlx::Error) -> String {
    match e {
        sqlx::Error::Database(d) => match d.code() {
            Some(code) => format!("server error {code}"),
            None => "server error".to_string(),
        },
        sqlx::Error::Io(io) => format!("I/O error ({:?})", io.kind()),
        sqlx::Error::Tls(_) => "TLS error".to_string(),
        sqlx::Error::PoolTimedOut => "pool timed out".to_string(),
        sqlx::Error::Protocol(_) => "protocol error".to_string(),
        sqlx::Error::Configuration(_) => "configuration error".to_string(),
        _ => "error".to_string(),
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (options, pid, started, activity) =
            (self.options.clone(), self.pid, self.started, self.activity);
        let check = std::mem::replace(&mut self.check, CancelCheck::None);
        runtime.spawn(async move {
            use sqlx::Connection;
            let sent = async {
                let mut conn = sqlx::PgConnection::connect_with(&options).await?;
                let query = match &check {
                    CancelCheck::Prefix(prefix) => sqlx::query(CANCEL_STATEMENT_SQL)
                        .bind(pid)
                        .bind(started)
                        .bind(prefix.as_str()),
                    CancelCheck::Active => sqlx::query(CANCEL_ACTIVE_SQL).bind(pid).bind(started),
                    CancelCheck::None => sqlx::query(CANCEL_SQL).bind(pid).bind(started),
                };
                let r = query.persistent(false).execute(&mut conn).await;
                let _ = conn.close().await;
                r
            };
            match tokio::time::timeout(CANCEL_TIMEOUT, sent).await {
                Ok(Ok(_)) => seaquel_engine::__private::log::debug!(
                    activity = activity;
                    "Sent pg_cancel_backend for an unfinished statement"
                ),
                Ok(Err(e)) => seaquel_engine::__private::log::warn!(
                    activity = activity;
                    "pg_cancel_backend for an unfinished statement failed: {}", error_kind(&e)
                ),
                Err(_) => seaquel_engine::__private::log::warn!(
                    activity = activity;
                    "pg_cancel_backend for an unfinished statement timed out"
                ),
            }
        });
    }
}

/// SQLSTATE `read_only_sql_transaction`: a write refused in a read-only
/// transaction.
const READ_ONLY_SQLSTATE: &str = "25006";

/// SQLSTATE `query_canceled`: the statement timeout (or a cancel request).
const QUERY_CANCELED_SQLSTATE: &str = "57014";

/// One connection, as an executor that notes the SQLSTATE and message of
/// the database error its query fails with. `fetch_capped` maps a failure
/// to its text, and Postgres's text has no SQLSTATE in it.
#[derive(Debug)]
struct NoteError<'c> {
    conn: &'c mut PgConnection,
    noted: &'c OnceLock<(String, String)>,
}

fn note_error(noted: &OnceLock<(String, String)>, e: &sqlx::Error) {
    if let sqlx::Error::Database(db) = e {
        let code = db.code().map(|c| c.to_string()).unwrap_or_default();
        let _ = noted.set((code, db.message().to_string()));
    }
}

impl<'c> sqlx::Executor<'c> for NoteError<'c> {
    type Database = Postgres;

    fn fetch_many<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxStream<'e, Result<Either<PgQueryResult, PgRow>, sqlx::Error>>
    where
        'c: 'e,
        E: 'q + sqlx::Execute<'q, Postgres>,
    {
        let noted = self.noted;
        self.conn
            .fetch_many(query)
            .inspect_err(move |e| note_error(noted, e))
            .boxed()
    }

    fn fetch_optional<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<PgRow>, sqlx::Error>>
    where
        'c: 'e,
        E: 'q + sqlx::Execute<'q, Postgres>,
    {
        let noted = self.noted;
        self.conn
            .fetch_optional(query)
            .inspect_err(move |e| note_error(noted, e))
            .boxed()
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        parameters: &'e [PgTypeInfo],
    ) -> BoxFuture<'e, Result<PgStatement<'q>, sqlx::Error>>
    where
        'c: 'e,
    {
        self.conn.prepare_with(sql, parameters)
    }

    fn describe<'e, 'q: 'e>(
        self,
        sql: &'q str,
    ) -> BoxFuture<'e, Result<sqlx::Describe<Postgres>, sqlx::Error>>
    where
        'c: 'e,
    {
        self.conn.describe(sql)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_size_follows_the_open_options() {
        assert_eq!(
            pool_options(OpenOptions::default()).get_max_connections(),
            10
        );
        let web = OpenOptions {
            max_pool_size: Some(4),
        };
        assert_eq!(pool_options(web).get_max_connections(), 4);
        let zero = OpenOptions {
            max_pool_size: Some(0),
        };
        assert_eq!(pool_options(zero).get_max_connections(), 1);
    }

    #[test]
    fn setup_sets_the_timeout_in_whole_milliseconds() {
        assert!(!setup_sql(None).contains("statement_timeout"));
        assert!(setup_sql(Some(Duration::from_millis(1500)))
            .contains("SET LOCAL statement_timeout = 1500; "));
        // 0 would mean no timeout.
        assert!(setup_sql(Some(Duration::from_micros(10)))
            .contains("SET LOCAL statement_timeout = 1; "));
        assert!(setup_sql(Some(Duration::from_secs(u64::MAX)))
            .contains(&format!("statement_timeout = {}; ", i32::MAX)));
        assert!(setup_sql(None).starts_with("BEGIN READ ONLY; SELECT pid"));
    }
}
