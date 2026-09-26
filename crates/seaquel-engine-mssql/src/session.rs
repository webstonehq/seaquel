//! The held client and what runs on it: parameterised queries (sent as
//! `sp_executesql`), plain batches, and every result set a response carries.

use futures::{FutureExt, TryStreamExt};
use log::{debug, warn};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use tiberius::{Client, Query, QueryItem, QueryStream};
use tokio::net::TcpStream;
use tokio::sync::MutexGuard;
use tokio_util::compat::Compat;

use seaquel_engine::{DbError, Value};

use crate::bind::{bind_mssql_param, inline_nulls};
use crate::decode::row_to_values;
use crate::driver::Request;

pub(crate) type MssqlClient = Client<Compat<TcpStream>>;

/// The connection behind the driver's mutex.
pub(crate) struct Connection {
    /// `None` until the first connect, and after a failed reconnect.
    pub(crate) client: Option<MssqlClient>,
    /// Set before any I/O on `client`, cleared once the response has been
    /// read to its end, unless session state is held (an open transaction,
    /// `SET SHOWPLAN_XML ON`, …; see [`Session::hold_state`]). A caller
    /// dropped in between leaves it set: the client may hold half a request,
    /// an unread reply, session settings nobody will undo, or a transaction
    /// the server has opened but tiberius never saw (its transaction
    /// descriptor then stays 0 and every later request fails with error
    /// 3989).
    ///
    /// A dirty connection is never reused. Dropping the [`Session`] closes
    /// it at once (a synchronous socket close), so the server rolls back an
    /// open transaction, releases its locks and aborts a running statement
    /// without waiting for this driver's next call; that call connects
    /// again. A cancelled or abandoned query therefore closes the
    /// connection. A reconnect loses what outlives a call on the old
    /// session: `##global` temp tables, session context, app locks, and a
    /// transaction opened by hand, which is silently rolled back (its later
    /// COMMIT fails with 3902). User statements run through `sp_executesql`
    /// (see `Request`), so their `SET` options and `#temp` tables end with
    /// the call, and the driver switches back after a `USE`; only the
    /// driver's own batches change session state, under
    /// [`Session::hold_state`].
    pub(crate) dirty: bool,
}

/// One result set: its columns (also when it has no rows) and its rows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

/// Which result sets to keep. The others are read and dropped: the response
/// is always drained, so a trailing set's error is still reported.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Keep {
    First,
    All,
    /// Only execution plans (sets whose columns include
    /// [`crate::introspect::SHOWPLAN_COLUMN`]). The query's own rows under
    /// `STATISTICS XML` are dropped and don't count towards the row cap.
    Plans,
}

/// How a failure is reported: `query` and `execute` have their own error
/// codes and wording.
#[derive(Clone, Copy)]
pub(crate) enum Op {
    Query,
    Execute,
}

/// Why a request failed, before it becomes a `DbError`.
pub(crate) enum Failure {
    Server(tiberius::error::Error),
    TooLarge(usize),
    /// Over the cap, with the rest of the response read and dropped (see
    /// [`Session::run_query_drained`]): the connection is clean.
    TooLargeDrained(usize),
    Panicked,
}

impl Failure {
    /// The server's error number, for a server error.
    pub(crate) fn server_code(&self) -> Option<u32> {
        match self {
            Failure::Server(e) => e.code(),
            _ => None,
        }
    }

    /// Whether the connection is still in a known state afterwards. A
    /// server error is reported only once its response has been read to the
    /// end, so it leaves nothing on the wire, unless it says the client's
    /// transaction descriptor is wrong (3988/3989) or is fatal (class 20+).
    fn leaves_connection_clean(&self) -> bool {
        match self {
            // 3988/3989: the client's transaction descriptor is wrong.
            // Class 20 and above: the server closes the connection.
            Failure::Server(tiberius::error::Error::Server(e)) => {
                !matches!(e.code(), 3988 | 3989) && e.class() < 20
            }
            Failure::TooLargeDrained(_) => true,
            Failure::Server(_) | Failure::TooLarge(_) | Failure::Panicked => false,
        }
    }

    pub(crate) fn into_db(self, op: Op) -> DbError {
        match self {
            Failure::Server(e) => match op {
                Op::Query => DbError::query_error(e),
                Op::Execute => DbError::execute_error(e),
            },
            Failure::TooLarge(cap) | Failure::TooLargeDrained(cap) => {
                DbError::result_too_large(cap)
            }
            // tiberius 0.12.3 panics via `todo!()` on SQL_VARIANT / UDT
            // column metadata (token_col_metadata.rs:174/204). The panic is
            // caught so the command returns an error instead of hanging the
            // UI forever. TODO: drop the catch_unwind once tiberius >0.12.3
            // ships fixes for token_col_metadata SQL_VARIANT/UDT decoding.
            Failure::Panicked => DbError {
                message: match op {
                    Op::Query => "SQL Server driver does not support a column type in this result (SQL_VARIANT or user-defined type). CAST the column to NVARCHAR(MAX) in your query.",
                    Op::Execute => "SQL Server driver panicked. A column type in the result is unsupported (SQL_VARIANT or user-defined type).",
                }
                .to_string(),
                code: "UNSUPPORTED_TYPE".to_string(),
            },
        }
    }
}

/// Builds a `sp_executesql` query with `params` bound as `@P1…` (a NULL
/// one as the literal, see [`inline_nulls`]).
pub(crate) fn build_query(sql: &str, params: &[Value]) -> Result<Query<'static>, DbError> {
    let mut query = Query::new(inline_nulls(sql, params).into_owned());
    for p in params {
        bind_mssql_param(&mut query, p)?;
    }
    Ok(query)
}

/// Runs `fut`, turning a tiberius panic into a failure.
async fn guarded<T>(fut: impl Future<Output = Result<T, Failure>>) -> Result<T, Failure> {
    AssertUnwindSafe(fut)
        .catch_unwind()
        .await
        .unwrap_or(Err(Failure::Panicked))
}

/// Reads the whole response, keeping the result sets `keep` asks for and at
/// most `cap` rows in all. Going over the cap returns early, leaving the
/// rest of the response unread (the connection is then dirty), unless
/// `drain`: then the rest is read and dropped, and `TooLargeDrained` comes
/// back once the response has ended (a server error in it wins).
async fn read_results(
    mut stream: QueryStream<'_>,
    keep: Keep,
    cap: usize,
    drain: bool,
) -> Result<Vec<ResultSet>, Failure> {
    let mut sets: Vec<ResultSet> = Vec::new();
    let mut keeping = false;
    let mut total = 0usize;
    let mut over = false;
    while let Some(item) = stream.try_next().await.map_err(Failure::Server)? {
        if over {
            continue;
        }
        match item {
            QueryItem::Metadata(meta) => {
                keeping = !meta.columns().is_empty()
                    && match keep {
                        Keep::All => true,
                        Keep::First => sets.is_empty(),
                        Keep::Plans => meta
                            .columns()
                            .iter()
                            .any(|c| c.name() == crate::introspect::SHOWPLAN_COLUMN),
                    };
                if keeping {
                    sets.push(ResultSet {
                        columns: meta
                            .columns()
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect(),
                        rows: Vec::new(),
                    });
                }
            }
            QueryItem::Row(row) => {
                if !keeping {
                    continue;
                }
                if total >= cap {
                    if !drain {
                        return Err(Failure::TooLarge(cap));
                    }
                    over = true;
                    sets.clear();
                    continue;
                }
                total += 1;
                if let Some(set) = sets.last_mut() {
                    set.rows.push(row_to_values(&row));
                }
            }
        }
    }
    if over {
        return Err(Failure::TooLargeDrained(cap));
    }
    Ok(sets)
}

/// Exclusive use of a clean connection, from `MssqlDriver::session`.
/// Everything run through one `Session` runs back to back with no other
/// caller's statement in between, so session state (a transaction,
/// `SET SHOWPLAN_XML ON`, …) set by one call applies to the next. Don't call
/// the driver's own methods while holding one: they wait for the same lock.
pub(crate) struct Session<'a> {
    conn: MutexGuard<'a, Connection>,
    /// While set, a finished request leaves the connection dirty (see
    /// [`Session::hold_state`]).
    holding_state: bool,
    /// Whether the last request finished cleanly.
    last_io_clean: bool,
    /// See [`Session::disposable`].
    disposable: bool,
}

impl Drop for Session<'_> {
    /// Closes a dirty connection now rather than at the next call, so the
    /// server rolls back and frees locks at once. Dropping a tiberius client
    /// only drops its socket: the fd is closed synchronously, nothing is
    /// awaited or flushed, so this can't block. No TLS close_notify is
    /// sent; the server sees the connection reset, which is enough.
    fn drop(&mut self) {
        if self.conn.dirty && self.conn.client.take().is_some() {
            if self.disposable {
                debug!(activity = "db.connect", driver = "mssql"; "Dropping a read-only connection mid-call (cancelled)");
            } else {
                warn!(activity = "db.connect", driver = "mssql"; "Closing the connection: a call on it did not finish");
            }
        }
    }
}

impl<'a> Session<'a> {
    /// `conn` must hold a client.
    pub(crate) fn new(conn: MutexGuard<'a, Connection>) -> Self {
        debug_assert!(conn.client.is_some() && !conn.dirty);
        Session {
            conn,
            holding_state: false,
            last_io_clean: true,
            disposable: false,
        }
    }

    /// Marks the connection as one used for a single call and then dropped
    /// (`query_read_only`'s): a drop mid-call is an expected cancel, logged
    /// at debug instead of as a warning.
    pub(crate) fn disposable(&mut self) {
        self.disposable = true;
    }

    /// Marks the connection dirty and hands out the client for one request.
    fn start_io(&mut self) -> &mut MssqlClient {
        self.conn.dirty = true;
        self.last_io_clean = false;
        self.conn
            .client
            .as_mut()
            .expect("a session always holds a client")
    }

    /// Clears the dirty mark once a request has finished cleanly, unless
    /// session state is held.
    fn finish_io<T>(&mut self, result: Result<T, Failure>) -> Result<T, Failure> {
        self.last_io_clean = match &result {
            Ok(_) => true,
            Err(f) => f.leaves_connection_clean(),
        };
        if self.last_io_clean && !self.holding_state {
            self.conn.dirty = false;
        }
        result
    }

    /// Call before changing session state that must not outlive this
    /// session: `BEGIN TRANSACTION`, `SET SHOWPLAN_XML ON`, … Until
    /// [`Session::release_state`], the connection counts as dirty, so if
    /// the session is dropped first (an error path that forgot, a caller
    /// that went away), the connection is closed and the state goes with it.
    pub(crate) fn hold_state(&mut self) {
        self.holding_state = true;
        self.conn.dirty = true;
    }

    /// Whether session state is held (see [`Session::hold_state`]).
    pub(crate) fn holding_state(&self) -> bool {
        self.holding_state
    }

    /// Whether the last request finished cleanly: its response was read to
    /// the end and nothing about the connection is in doubt.
    pub(crate) fn last_request_clean(&self) -> bool {
        self.last_io_clean
    }

    /// Call once the state is undone (COMMIT/ROLLBACK, `SET … OFF`). The
    /// connection is clean again if the last request finished cleanly.
    pub(crate) fn release_state(&mut self) {
        self.holding_state = false;
        if self.last_io_clean {
            self.conn.dirty = false;
        }
    }

    /// Closes the connection now, on purpose, without the warning a drop
    /// logs: `query_read_only`'s own connection, whose session state can't
    /// be trusted afterwards however the call ended.
    pub(crate) fn close(mut self) {
        self.conn.client = None;
        self.conn.dirty = true;
    }

    /// Runs a parameterised query (`@P1…`) and returns every result set.
    /// It runs as `sp_executesql`, so a `SET` inside it lasts only until it
    /// ends; use [`Session::batch`] for session settings.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn query(
        &mut self,
        sql: &str,
        params: &[Value],
    ) -> Result<Vec<ResultSet>, DbError> {
        let query = build_query(sql, params)?;
        self.run_query(query, Keep::All)
            .await
            .map_err(|f| f.into_db(Op::Query))
    }

    pub(crate) async fn run_query(
        &mut self,
        query: Query<'_>,
        keep: Keep,
    ) -> Result<Vec<ResultSet>, Failure> {
        self.run_query_with(query, keep, false).await
    }

    /// [`Session::run_query`], but over the row cap the rest of the
    /// response is read and dropped (`TooLargeDrained`), so the connection
    /// stays usable for another request.
    pub(crate) async fn run_query_drained(
        &mut self,
        query: Query<'_>,
        keep: Keep,
    ) -> Result<Vec<ResultSet>, Failure> {
        self.run_query_with(query, keep, true).await
    }

    async fn run_query_with(
        &mut self,
        query: Query<'_>,
        keep: Keep,
        drain: bool,
    ) -> Result<Vec<ResultSet>, Failure> {
        let cap = seaquel_engine::max_query_rows();
        let client = self.start_io();
        let result = guarded(async move {
            let stream = query.query(client).await.map_err(Failure::Server)?;
            read_results(stream, keep, cap, drain).await
        })
        .await;
        self.finish_io(result)
    }

    /// Runs `sql` as a plain batch (no parameters, no `sp_executesql`) and
    /// returns every result set. Session settings made here last, e.g.
    /// `SET SHOWPLAN_XML ON`, which must be alone in its batch. Never pass
    /// user input: nothing is bound.
    pub(crate) async fn batch(&mut self, sql: &str) -> Result<Vec<ResultSet>, DbError> {
        self.run_batch(sql, Keep::All)
            .await
            .map_err(|f| f.into_db(Op::Query))
    }

    pub(crate) async fn run_batch(
        &mut self,
        sql: &str,
        keep: Keep,
    ) -> Result<Vec<ResultSet>, Failure> {
        let cap = seaquel_engine::max_query_rows();
        let client = self.start_io();
        let result = guarded(async move {
            let stream = client.simple_query(sql).await.map_err(Failure::Server)?;
            read_results(stream, keep, cap, false).await
        })
        .await;
        self.finish_io(result)
    }

    /// Runs `request` (see [`Request`]) and returns the result sets `keep`
    /// asks for.
    pub(crate) async fn run(
        &mut self,
        request: Request,
        keep: Keep,
    ) -> Result<Vec<ResultSet>, Failure> {
        match request {
            Request::Rpc(query) => self.run_query(query, keep).await,
            Request::Batch(sql) => self.run_batch(&sql, keep).await,
        }
    }

    /// Runs a statement and returns the rows it affected, summed over its
    /// statements. A batch (a statement that must start one) reports 0:
    /// tiberius counts rows only for RPC calls.
    pub(crate) async fn execute(&mut self, request: Request) -> Result<u64, Failure> {
        let query = match request {
            Request::Rpc(query) => query,
            Request::Batch(sql) => return self.run_batch(&sql, Keep::All).await.map(|_| 0),
        };
        let client = self.start_io();
        let result = guarded(async move {
            let result = query.execute(client).await.map_err(Failure::Server)?;
            Ok(result.rows_affected().iter().sum())
        })
        .await;
        self.finish_io(result)
    }
}
