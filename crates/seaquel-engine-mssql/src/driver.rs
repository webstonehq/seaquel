use log::{debug, error, info, warn};
use tiberius::{AuthMethod, Client, Config, Query};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Semaphore};
use tokio_util::compat::TokioAsyncWriteCompatExt;

use seaquel_engine::{
    BatchStatement, ConnectConfig, DbError, Driver, ExecuteResult, ExpectRows, ExplainResult,
    QueryResult, SchemaColumn, SchemaIndex, SchemaTable, Value,
};

use crate::introspect;
use crate::session::{build_query, Connection, Failure, Keep, MssqlClient, Op, ResultSet, Session};

/// How a statement goes to the server.
///
/// Every statement is an RPC call to `sp_executesql`, with or without
/// parameters, so what it changes about the session (`SET` options,
/// `#temp` tables) ends with the call and can't leak into the next one:
/// the editor's `SET NOCOUNT ON` or `SET ROWCOUNT 1` doesn't reach the
/// introspection or grid edits that share the connection. `USE` does
/// outlive an RPC call, so the driver switches back after it
/// ([`MssqlDriver::restore_database`]).
///
/// The exception is a parameterless statement that must start a batch
/// ([`must_start_batch`]: `CREATE VIEW`, `CREATE PROCEDURE`, `CREATE
/// SCHEMA`, …). tiberius always sends `sp_executesql` with a parameter
/// list, where such a statement is a syntax error, so it goes as a plain
/// batch. Those statements change no session state and affect no rows.
/// The driver's own session batches (`BEGIN`/`COMMIT`/`ROLLBACK` of
/// `transaction`, EXPLAIN's `SET … ON`/`OFF`) are plain batches too.
pub(crate) enum Request {
    Rpc(Query<'static>),
    Batch(String),
}

impl Request {
    /// `sp_executesql`, unless the statement has no parameters and must
    /// start a batch.
    pub(crate) fn new(sql: &str, params: &[Value]) -> Result<Self, DbError> {
        if params.is_empty() && must_start_batch(sql) {
            Ok(Request::Batch(sql.to_string()))
        } else {
            Ok(Request::Rpc(build_query(sql, params)?))
        }
    }
}

/// Whether `sql` has the keyword `USE` outside strings, quoted names and
/// comments. Unlike `SET` options, a `USE` inside an `sp_executesql` RPC
/// call stays in force after it, so the driver switches back to the
/// connection's database after such a statement (see
/// [`MssqlDriver::restore_database`]). `USE` is reserved, so the only other
/// matches are the `USE HINT` / `USE PLAN` query hints, which cost one
/// needless `USE`.
pub(crate) fn mentions_use(sql: &str) -> bool {
    let s: Vec<char> = sql.chars().collect();
    let n = s.len();
    let word = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '#' | '$');
    let mut i = 0;
    while i < n {
        let c = s[i];
        if c == '-' && s.get(i + 1) == Some(&'-') {
            while i < n && s[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && s.get(i + 1) == Some(&'*') {
            let mut level = 1;
            i += 2;
            while i < n && level > 0 {
                if s[i] == '/' && s.get(i + 1) == Some(&'*') {
                    level += 1;
                    i += 2;
                } else if s[i] == '*' && s.get(i + 1) == Some(&'/') {
                    level -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if matches!(c, '\'' | '"' | '[') {
            let close = if c == '[' { ']' } else { c };
            i += 1;
            while i < n {
                if s[i] == close {
                    if s.get(i + 1) == Some(&close) {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
        } else if word(c) {
            let start = i;
            while i < n && word(s[i]) {
                i += 1;
            }
            let w: String = s[start..i].iter().collect();
            if w.eq_ignore_ascii_case("USE") {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Whether `sql` begins (after whitespace and comments) with a statement
/// SQL Server only accepts first in a batch: `CREATE [OR ALTER]` or `ALTER`
/// of a VIEW, PROC[EDURE], FUNCTION or TRIGGER, or `CREATE SCHEMA`,
/// `CREATE DEFAULT` or `CREATE RULE`.
pub(crate) fn must_start_batch(sql: &str) -> bool {
    let mut rest = sql;
    let mut words: Vec<String> = Vec::new();
    while words.len() < 4 {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, r)| r);
        } else if rest.starts_with("/*") {
            let mut level = 0usize;
            let mut end = rest.len();
            let bytes = rest.as_bytes();
            let mut i = 0;
            while i + 1 < bytes.len() {
                match &bytes[i..i + 2] {
                    b"/*" => {
                        level += 1;
                        i += 2;
                    }
                    b"*/" => {
                        level -= 1;
                        i += 2;
                        if level == 0 {
                            end = i;
                            break;
                        }
                    }
                    _ => i += 1,
                }
            }
            rest = &rest[end..];
        } else {
            let len = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            if len == 0 {
                break;
            }
            words.push(rest[..len].to_ascii_uppercase());
            rest = &rest[len..];
        }
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let object = match words.as_slice() {
        ["CREATE", "OR", "ALTER", object, ..] | ["ALTER", object, ..] => *object,
        ["CREATE", object, ..] => {
            if matches!(*object, "SCHEMA" | "DEFAULT" | "RULE") {
                return true;
            }
            object
        }
        _ => return false,
    };
    matches!(
        object,
        "VIEW" | "PROC" | "PROCEDURE" | "FUNCTION" | "TRIGGER"
    )
}

/// Rolls back the transaction `Driver::transaction` opened. `@@TRANCOUNT`
/// is already 0 when the server rolled back itself (XACT_ABORT ON, or a
/// statement that ended the transaction), where a bare ROLLBACK fails (3903).
const ROLLBACK: &str = "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION";

/// The error number `BEGIN` throws when a transaction is already open.
const ALREADY_IN_TRANSACTION: u32 = 50000;

/// Refuses to nest: in a transaction someone opened by hand, our COMMIT
/// would only decrement `@@TRANCOUNT` and our rollback would undo theirs.
const BEGIN: &str =
    "IF @@TRANCOUNT > 0 THROW 50000, 'A transaction is already open on this connection.', 1; \
                     BEGIN TRANSACTION";

/// How many `query_read_only` calls may run at once, each on its own
/// connection: a dashboard refreshing a dozen widgets opens at most this
/// many connections, and the rest wait their turn.
const READ_ONLY_CONNECTIONS: usize = 4;

/// How long one `query_read_only` call may take in all, waiting for a slot
/// included. Past it the call is dropped (its connection with it), so a
/// query that never ends can't hold a slot for good.
const READ_ONLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Opens `query_read_only`'s transaction; returns its id and depth
/// (`@@TRANCOUNT`).
///
/// - `IMPLICIT_TRANSACTIONS ON`, so a statement the query runs after ending
///   the transaction itself opens a new one, which [`READ_ONLY_END`] rolls
///   back, instead of committing on its own.
/// - Two deep (with implicit transactions on, BEGIN already makes it two;
///   the second BEGIN is there in case it doesn't), so one COMMIT in the
///   query only unnests it and commits nothing.
/// - The marker table: a session `#temp` table created inside the
///   transaction, so it's gone once the transaction was rolled back
///   ([`read_only_query`]'s CATCH reads it for a deadlock victim).
/// - `XACT_ABORT OFF`, the default, whatever the server's `user options`
///   say.
/// - `LOCK_TIMEOUT 10000`: a query blocked on a lock fails with 1222 after
///   10 s (a statement error: the transaction stays intact) instead of
///   holding a connection slot until the call's own timeout.
const READ_ONLY_BEGIN: &str = "\
SET XACT_ABORT OFF; \
SET LOCK_TIMEOUT 10000; \
SET IMPLICIT_TRANSACTIONS ON; \
BEGIN TRANSACTION; \
IF @@TRANCOUNT < 2 BEGIN TRANSACTION; \
CREATE TABLE #seaquel_read_only (x INT); \
SELECT CURRENT_TRANSACTION_ID() AS tx, @@TRANCOUNT AS n";

/// Reads how `query_read_only`'s query left the transaction (depth and
/// id), then rolls back. The connection is dropped afterwards, so the
/// session settings and a committed marker table aren't undone here.
const READ_ONLY_END: &str = "\
SELECT @@TRANCOUNT AS n, CURRENT_TRANSACTION_ID() AS tx; \
IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION";

/// The error a module or `sp_executesql` raises when it returns with a
/// different `@@TRANCOUNT` than it started with. Not raised while
/// `IMPLICIT_TRANSACTIONS` is on (checked on SQL Server 2022), so the depth
/// is compared too.
const TRANCOUNT_MISMATCH: u32 = 266;

/// The error [`read_only_query`]'s CATCH throws when the query failed after
/// ending the read-only transaction. The query can raise it too (THROW
/// takes any number from 50000), which only makes its own error read as an
/// escape.
const ESCAPE_SIGNAL: u32 = 59173;

/// The query in `query_read_only`: the user's SQL (after [`inline_nulls`])
/// runs in a nested `sp_executesql`, passed as a parameter and never
/// spliced into this text, with the bound parameters forwarded under the
/// types tiberius declares for them ([`declared_type`]). It sits in TRY, so
/// every error the query raises lands in the CATCH, which tells the query's
/// own errors from escapes by the transaction:
///
/// - The same transaction (`CURRENT_TRANSACTION_ID()` as [`READ_ONLY_BEGIN`]
///   read it, still open): the query's own error, even when it doomed the
///   transaction (245, 241, 9400, 13609: XACT_STATE -1) or rolled it back
///   with XACT_ABORT on inside TRY (the transaction is doomed, not ended,
///   until the CATCH). `THROW;` re-raises it with its number and message.
/// - A deadlock victim (1205) whose transaction is gone with the marker:
///   the server rolled back and nothing was committed. Re-raised too.
/// - Anything else (another transaction, or none): the query ended the
///   transaction and then failed. [`ESCAPE_SIGNAL`], with the error's
///   number and message.
///
/// A query that ends the transaction and then succeeds never reaches the
/// CATCH: [`READ_ONLY_END`] catches that.
///
/// [`inline_nulls`]: crate::bind::inline_nulls
/// [`declared_type`]: crate::bind::declared_type
fn read_only_query(sql: &str, params: &[Value], tx: i64) -> Result<Query<'static>, DbError> {
    let n = params.len();
    let mut declared = Vec::with_capacity(n);
    let mut forwarded = Vec::with_capacity(n);
    for (i, p) in params.iter().enumerate() {
        let ty = crate::bind::declared_type(p)
            .ok_or_else(|| DbError::query_error("array parameters are not supported"))?;
        declared.push(format!("@P{} {ty}", i + 1));
        forwarded.push(format!(", @P{0} = @P{0}", i + 1));
    }
    let exec = if n == 0 {
        format!("EXEC sp_executesql @P{};", n + 1)
    } else {
        format!(
            "EXEC sp_executesql @P{}, @P{}{};",
            n + 1,
            n + 2,
            forwarded.concat()
        )
    };
    let text = format!(
        "BEGIN TRY {exec} END TRY \
         BEGIN CATCH \
         IF (@@TRANCOUNT > 0 AND CURRENT_TRANSACTION_ID() = {tx}) \
            OR (ERROR_NUMBER() = 1205 AND OBJECT_ID(N'tempdb..#seaquel_read_only') IS NULL) \
            THROW; \
         DECLARE @seaquel_message NVARCHAR(2048) = \
            CONCAT(N'Error ', ERROR_NUMBER(), N': ', ERROR_MESSAGE()); \
         THROW {ESCAPE_SIGNAL}, @seaquel_message, 1; \
         END CATCH"
    );
    let mut query = Query::new(text);
    for p in params {
        crate::bind::bind_mssql_param(&mut query, p)?;
    }
    query.bind(crate::bind::inline_nulls(sql, params).into_owned());
    if n > 0 {
        query.bind(declared.join(", "));
    }
    Ok(query)
}

/// `query_read_only`'s refusal when the query ended its transaction.
const ESCAPED: &str = "The query ended the read-only transaction, so what it changed may have \
                       been committed. SQL Server has no read-only mode; connect with a login \
                       that can only read to prevent this.";

/// `query_read_only`'s refusal when the query committed, rolled back or
/// opened a transaction without ending the read-only one.
const TRIED: &str = "The query tried to commit, roll back or open a transaction. It ran in a \
                     read-only transaction that was rolled back, and nothing was committed.";

/// The integer in `sets`' first row, column `col`.
fn cell(sets: &[ResultSet], col: usize) -> Option<i64> {
    sets.first()?.rows.first()?.get(col)?.as_i64()
}

/// What `query_read_only` returns, from the id and depth
/// [`READ_ONLY_BEGIN`] and [`READ_ONLY_END`] read around the query, and the
/// query's own result:
///
/// - **[`ESCAPE_SIGNAL`]** (the query failed after ending the transaction,
///   see [`read_only_query`]): `READ_ONLY` ([`ESCAPED`]), with its error.
/// - **Intact** (the same transaction, still open) **with another depth,
///   or a 266**: the query committed, rolled back or opened a transaction
///   without ending ours, and nothing was committed: `READ_ONLY` ([`TRIED`]).
/// - **Any other server error**: the query's own, as the CATCH decided;
///   the transaction may be gone by now (a doomed transaction is rolled back
///   when the request ends).
/// - **Intact**: the query's result, or `RESULT_TOO_LARGE`.
/// - **Ended** (another transaction, or none) after rows or
///   `RESULT_TOO_LARGE`: `READ_ONLY` ([`ESCAPED`]); with the error appended
///   for the latter.
fn read_only_outcome<E: QueryFailure>(
    begin: &[ResultSet],
    end: &[ResultSet],
    run: Result<Vec<ResultSet>, E>,
) -> Result<Vec<ResultSet>, DbError> {
    let (Some(tx), Some(depth), Some(n), Some(tx_after)) =
        (cell(begin, 0), cell(begin, 1), cell(end, 0), cell(end, 1))
    else {
        return Err(DbError::query_error(
            "The read-only transaction's state could not be read",
        ));
    };
    let code = run.as_ref().err().and_then(QueryFailure::server_code);
    let escaped_with = |f: E| {
        DbError::read_only(format!(
            "{ESCAPED} The query's error: {}",
            f.into_db().message
        ))
    };
    if code == Some(ESCAPE_SIGNAL) {
        return run.map_err(escaped_with);
    }
    let intact = n >= 1 && tx_after == tx;
    if intact && (n != depth || code == Some(TRANCOUNT_MISMATCH)) {
        return Err(DbError::read_only(TRIED));
    }
    if code.is_some() || intact {
        return run.map_err(QueryFailure::into_db);
    }
    match run {
        Err(f) => Err(escaped_with(f)),
        Ok(_) => Err(DbError::read_only(ESCAPED)),
    }
}

/// What [`read_only_outcome`] needs from the query's failure: a trait so
/// its tests can fake server errors, which tiberius can't construct.
trait QueryFailure {
    fn server_code(&self) -> Option<u32>;
    fn into_db(self) -> DbError;
}

impl QueryFailure for Failure {
    fn server_code(&self) -> Option<u32> {
        Failure::server_code(self)
    }

    fn into_db(self) -> DbError {
        Failure::into_db(self, Op::Query)
    }
}

/// rustls reports certificate verification failures as I/O errors rather
/// than `Error::Tls`, so match on the message for those.
fn is_tls_error(e: &tiberius::error::Error) -> bool {
    match e {
        tiberius::error::Error::Tls(_) => true,
        tiberius::error::Error::Io { message, .. } => message.contains("certificate"),
        _ => false,
    }
}

/// One SQL Server connection. Calls take turns on it (see [`Session`]).
///
/// A call that doesn't finish (a UI cancel, a request whose client went
/// away, RESULT_TOO_LARGE, a fatal error) closes the connection, and the
/// next call opens a new one with `config` (see [`Connection::dirty`]). A
/// reconnect loses what outlives a call on the old session: `##global`
/// temp tables, session context (`sp_set_session_context`), app locks, and
/// a transaction opened by hand, which is silently rolled back (its later
/// COMMIT fails with 3902). `SET` options and `#temp` tables end with each
/// call anyway (see [`Request`]), and the driver undoes a `USE`, so a
/// reconnect can't flip the database or settings under the next statement.
pub struct MssqlDriver {
    config: ConnectConfig,
    conn: Mutex<Connection>,
    /// See [`READ_ONLY_CONNECTIONS`].
    read_only_slots: Semaphore,
}

/// Sent as a batch when a connection opens (and on every reconnect), so it
/// holds for the session: `sp_executesql` calls see it, and a `SET` inside
/// one ends with that call. A server whose `user options` include NOCOUNT
/// (512) starts every session with NOCOUNT ON, which makes every UPDATE and
/// DELETE report 0 rows, so every grid edit would fail as matching no row.
/// Logs a failed read-only call at a level that matches what it means. The
/// model's own mistakes (syntax errors, unknown tables, a refused
/// statement, the row cap) are routine and go to `debug`; a detected escape,
/// where something may have been committed, is a `warn`; anything else
/// (timeouts, connection failures) is an `error`.
fn log_read_only_failure(e: &DbError) {
    let code = e.code.as_str();
    if code == "READ_ONLY" && e.message.starts_with(ESCAPED) {
        warn!(activity = "db.query", driver = "mssql", error_code = code; "Read-only query ended its transaction; what it changed after that may be committed");
    } else if matches!(code, "READ_ONLY" | "QUERY_ERROR" | "RESULT_TOO_LARGE") {
        debug!(activity = "db.query", driver = "mssql", error_code = code; "Read-only query failed");
    } else {
        error!(activity = "db.query", driver = "mssql", error_code = code; "Read-only query failed");
    }
}

pub(crate) const SESSION_DEFAULTS: &str = "SET NOCOUNT OFF";

/// Runs [`SESSION_DEFAULTS`] on a new connection.
pub(crate) async fn reset_session(client: &mut MssqlClient) -> Result<(), DbError> {
    client
        .simple_query(SESSION_DEFAULTS)
        .await
        .map_err(DbError::connection_error)?
        .into_results()
        .await
        .map_err(DbError::connection_error)?;
    Ok(())
}

async fn open_client(config: &ConnectConfig) -> Result<MssqlClient, DbError> {
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
    .map_err(DbError::connection_error)?;

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

    // TLS and login, bounded like the TCP connect.
    let mut client = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        Client::connect(tiberius_config, tcp.compat_write()),
    )
    .await
    .map_err(|_| DbError {
        message: "Connection timed out during TLS or login".to_string(),
        code: "TIMEOUT".to_string(),
    })?
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

    reset_session(&mut client).await?;
    Ok(client)
}

impl MssqlDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let client = open_client(config).await?;
        Ok(Self {
            config: config.clone(),
            conn: Mutex::new(Connection {
                client: Some(client),
                dirty: false,
            }),
            read_only_slots: Semaphore::new(READ_ONLY_CONNECTIONS),
        })
    }

    /// Waits for the connection and holds it until the `Session` is dropped.
    /// A dirty connection is closed and replaced first; if that fails, the
    /// error is returned and the next call tries again.
    pub(crate) async fn session(&self) -> Result<Session<'_>, DbError> {
        let mut conn = self.conn.lock().await;
        if conn.dirty || conn.client.is_none() {
            // Usually already closed by `Session`'s drop. Closing the TCP
            // connection makes the server roll back an open transaction and
            // abort a running statement.
            conn.client = None;
            conn.dirty = true;
            info!(activity = "db.connect", driver = "mssql"; "Reconnecting");
            conn.client = Some(open_client(&self.config).await?);
            conn.dirty = false;
        }
        Ok(Session::new(conn))
    }

    /// Makes the next call reconnect, as a failed call would.
    #[cfg(test)]
    pub(crate) async fn force_reconnect(&self) {
        let mut conn = self.conn.lock().await;
        conn.client = None;
        conn.dirty = true;
    }

    /// Runs [`reset_session`] on the open connection.
    #[cfg(test)]
    pub(crate) async fn reset_open_session(&self) -> Result<(), DbError> {
        let mut conn = self.conn.lock().await;
        reset_session(conn.client.as_mut().expect("an open connection")).await
    }

    /// The database the connection was opened in.
    fn database(&self) -> &str {
        self.config.database.as_deref().unwrap_or("master")
    }

    /// After a statement that may have run `USE` (see [`mentions_use`]),
    /// switches back to the connection's database, so the next call, and
    /// the introspection and grid edits sharing the connection, don't run
    /// in another one. If the switch can't be made (or the statement left
    /// the connection in doubt), the connection stays dirty: the session
    /// closes it and the next call reconnects into the right database.
    async fn restore_database(&self, session: &mut Session<'_>, sqls: &[&str]) {
        if !sqls.iter().any(|sql| mentions_use(sql)) {
            return;
        }
        // State someone else holds (EXPLAIN whose OFF failed) stays held.
        let held_before = session.holding_state();
        session.hold_state();
        if !session.last_request_clean() {
            return;
        }
        let sql = format!("USE {}", crate::dialect::qi(self.database()));
        match session.run_batch(&sql, Keep::All).await {
            Ok(_) if !held_before => session.release_state(),
            Ok(_) => {}
            Err(f) => {
                let e = f.into_db(Op::Query);
                warn!(activity = "db.query", driver = "mssql"; "Switching back to the connection's database failed, reconnecting: {}", e.message);
            }
        }
    }

    /// Runs a parameterised query and returns every result set, in order,
    /// each with its columns even when it has no rows. Statements without a
    /// result set (`INSERT`, `DECLARE`, …) add none. `Driver::query` returns
    /// the first of these.
    pub async fn query_results(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<Vec<ResultSet>, DbError> {
        let request = Request::new(sql, &params)?;
        let mut session = self.session().await?;
        let result = session.run(request, Keep::All).await;
        self.restore_database(&mut session, &[sql]).await;
        result
            .map_err(|f| f.into_db(Op::Query))
            .inspect_err(|e| {
                error!(activity = "db.query", driver = "mssql", error_code = e.code.as_str(); "Query failed");
            })
    }

    /// `query_read_only` without its overall timeout.
    async fn read_only_call(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<Vec<ResultSet>, DbError> {
        if params.iter().any(|p| matches!(p, Value::Array(_))) {
            return Err(DbError::query_error("array parameters are not supported"));
        }
        // The semaphore is never closed, so `acquire` can't fail.
        let _slot = self
            .read_only_slots
            .acquire()
            .await
            .expect("the read-only semaphore is never closed");
        let conn = Mutex::new(Connection {
            client: Some(open_client(&self.config).await?),
            dirty: false,
        });
        let mut session = Session::new(conn.lock().await);
        session.disposable();
        let begin = session
            .run_batch(READ_ONLY_BEGIN, Keep::All)
            .await
            .map_err(|f| f.into_db(Op::Query))?;
        let tx = cell(&begin, 0).ok_or_else(|| {
            DbError::query_error("The read-only transaction's id could not be read")
        })?;
        let query = read_only_query(sql, &params, tx)?;
        // Over the row cap the rest is read and dropped, so the end batch
        // can still run and an escape before that point is still reported.
        let run = session.run_query_drained(query, Keep::First).await;
        let outcome = if session.last_request_clean() {
            match session.run_batch(READ_ONLY_END, Keep::All).await {
                Ok(end) => read_only_outcome(&begin, &end, run),
                Err(f) => Err(f.into_db(Op::Query)),
            }
        } else {
            // Half a response on the wire (a fatal error, a panic): nothing
            // more can be sent. Dropping the connection rolls back. An
            // escape before that point goes unreported.
            run.map_err(|f| f.into_db(Op::Query))
        };
        session.close();
        outcome
    }

    /// Runs `queries` in one transaction on the held session: BEGIN, each
    /// statement as its own `sp_executesql`, COMMIT. BEGIN and COMMIT are
    /// batches, because a transaction begun inside `sp_executesql` must end
    /// there (error 266). On a failure after BEGIN it rolls back; if the
    /// rollback fails too, the connection stays dirty and the next call
    /// reconnects, which rolls back on the server.
    ///
    /// Each request comes with its [`ExpectRows`]: a statement that affected
    /// fewer rows (its DONE tokens summed, so rows its triggers touched
    /// count too) rolls back with `NO_ROWS_AFFECTED`.
    async fn run_transaction(
        session: &mut Session<'_>,
        requests: Vec<(Request, Option<ExpectRows>)>,
    ) -> Result<(), DbError> {
        if let Err(f) = session.run_batch(BEGIN, Keep::All).await {
            if f.server_code() == Some(ALREADY_IN_TRANSACTION) {
                return Err(DbError {
                    message: "Execute failed: a transaction is already open on this connection \
                              (BEGIN TRANSACTION run by hand). Commit or roll it back first."
                        .to_string(),
                    code: "EXECUTE_ERROR".to_string(),
                });
            }
            return Err(f.into_db(Op::Execute));
        }
        session.hold_state();

        let mut outcome = Ok(());
        for (index, (request, expect)) in requests.into_iter().enumerate() {
            outcome = match session.execute(request).await {
                Ok(affected) => expect.map_or(Ok(()), |e| e.check(index, affected)),
                Err(f) => Err(f.into_db(Op::Execute)),
            };
            if outcome.is_err() {
                break;
            }
        }
        if outcome.is_ok() {
            outcome = session
                .run_batch("COMMIT TRANSACTION", Keep::All)
                .await
                .map(|_| ())
                .map_err(|f| f.into_db(Op::Execute));
        }
        match outcome {
            Ok(()) => {
                session.release_state();
                Ok(())
            }
            Err(e) => {
                match session.run_batch(ROLLBACK, Keep::All).await {
                    Ok(_) => session.release_state(),
                    Err(rollback) => {
                        let rollback = rollback.into_db(Op::Execute);
                        warn!(activity = "db.transaction", driver = "mssql"; "Rollback failed, reconnecting on the next call: {}", rollback.message);
                    }
                }
                Err(e)
            }
        }
    }
}

#[seaquel_runtime::async_trait]
impl Driver for MssqlDriver {
    /// The first result set; later ones are read and dropped (see
    /// [`MssqlDriver::query_results`] for all of them).
    async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        let request = Request::new(sql, &params)?;
        let mut session = self.session().await?;
        let result = session.run(request, Keep::First).await;
        self.restore_database(&mut session, &[sql]).await;
        let sets = result
            .map_err(|f| f.into_db(Op::Query))
            .inspect_err(|e| {
            error!(activity = "db.query", driver = "mssql", error_code = e.code.as_str(); "Query failed");
        })?;
        let first = sets.into_iter().next().unwrap_or_default();
        Ok(QueryResult {
            columns: first.columns,
            rows: first.rows,
        })
    }

    async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        let request = Request::new(sql, &params)?;
        let mut session = self.session().await?;
        let result = session.execute(request).await;
        self.restore_database(&mut session, &[sql]).await;
        let rows_affected = result
            .map_err(|f| f.into_db(Op::Execute))
            .inspect_err(|e| {
            error!(activity = "db.execute", driver = "mssql", error_code = e.code.as_str(); "Execute failed");
        })?;
        Ok(ExecuteResult {
            rows_affected,
            last_insert_id: None,
        })
    }

    /// Like the sqlx drivers: every statement or none. The first failure
    /// rolls back and is returned (`EXECUTE_ERROR`, or the bind error,
    /// before anything runs).
    ///
    /// The connection is held throughout, so nothing else interleaves. The
    /// rollback is `IF @@TRANCOUNT > 0 ROLLBACK`: with XACT_ABORT ON (or a
    /// statement that rolled back itself) the server has already ended the
    /// transaction, and a bare ROLLBACK would fail. The session's XACT_ABORT
    /// is left alone: statements run until the first error either way.
    ///
    /// If the caller goes away mid-transaction, or the rollback fails, the
    /// connection is closed at once and the server rolls back; the next call
    /// reconnects. A caller that goes away after COMMIT was sent can't know
    /// the outcome: the transaction may already be committed (all of it).
    ///
    /// SQL Server transactions nest, so this refuses to start (with
    /// `EXECUTE_ERROR`) while one is already open on the connection, e.g. a
    /// `BEGIN TRANSACTION` run by hand: COMMIT would only decrement
    /// `@@TRANCOUNT`, and a rollback would undo the outer transaction too.
    async fn transaction(&self, statements: Vec<BatchStatement>) -> Result<(), DbError> {
        let requests = statements
            .iter()
            .map(|s| Ok((Request::new(&s.sql, &s.params)?, s.expect_rows)))
            .collect::<Result<Vec<_>, DbError>>()?;
        let mut session = self.session().await?;
        let result = Self::run_transaction(&mut session, requests).await;
        let sqls: Vec<&str> = statements.iter().map(|s| s.sql.as_str()).collect();
        self.restore_database(&mut session, &sqls).await;
        result
            .inspect_err(|e| {
                error!(activity = "db.transaction", driver = "mssql", error_code = e.code.as_str(); "Transaction failed");
            })
    }

    /// SQL Server has no read-only transaction or session, so the query
    /// runs in a transaction that is always rolled back, on a connection of
    /// its own: the user's held session is never touched, so its `##temp`
    /// tables, session context, app locks and any transaction opened by
    /// hand are left alone, and it doesn't wait behind a slow AI query.
    ///
    /// 1. At most [`READ_ONLY_CONNECTIONS`] calls run at once; the others
    ///    wait for a slot. The whole call, waiting included, is cancelled
    ///    after [`READ_ONLY_TIMEOUT`].
    /// 2. A new connection from the stored config (the same connect
    ///    timeouts and session defaults as the held one).
    /// 3. [`READ_ONLY_BEGIN`]: `XACT_ABORT` off, a 10 s lock timeout,
    ///    `IMPLICIT_TRANSACTIONS` on, the transaction two deep, the marker
    ///    table, and the transaction's id and depth.
    /// 4. The query in a nested `sp_executesql` inside TRY/CATCH
    ///    ([`read_only_query`]; always RPC: a plain batch would keep its
    ///    `SET` options), keeping the first result set. The CATCH tells the
    ///    query's own errors from escapes. Past the row cap the rest of the
    ///    response is read and dropped.
    /// 5. [`READ_ONLY_END`]: reads `@@TRANCOUNT` and the transaction id,
    ///    then rolls back.
    /// 6. The connection is dropped, whatever happened. ROLLBACK and the end
    ///    of `sp_executesql` don't undo everything a query can do to its
    ///    session: `SET CONTEXT_INFO`, `sp_set_session_context`, a session
    ///    app lock and a global cursor all outlive both.
    ///
    /// Dropping the future at any await drops the connection, and the
    /// server aborts the statement and rolls back.
    ///
    /// Running on its own connection, the query doesn't see the user's
    /// uncommitted changes, and under READ COMMITTED it waits for rows the
    /// user's open transaction has locked, for up to the lock timeout.
    ///
    /// **Escapes.** One COMMIT only unnests the transaction, and a write
    /// after the query ended it runs in an implicit transaction that is
    /// rolled back with the rest. But a query can still commit (`DELETE …;
    /// COMMIT; COMMIT`, a procedure that commits until `@@TRANCOUNT` is 0).
    /// That is detected afterwards and returned as `READ_ONLY` (see
    /// [`read_only_outcome`]); what it committed stays committed. A read-only
    /// login is the only full fix. Also not undone: `NEXT VALUE FOR`
    /// advances a sequence for good.
    async fn query_read_only(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        let call = tokio::time::timeout(READ_ONLY_TIMEOUT, self.read_only_call(sql, params));
        let sets = match call.await {
            Ok(result) => result,
            // `timeout` dropped the call: its slot and its connection.
            Err(_) => Err(DbError {
                message: format!(
                    "The read-only query didn't finish within {} seconds and was cancelled",
                    READ_ONLY_TIMEOUT.as_secs()
                ),
                code: "TIMEOUT".to_string(),
            }),
        }
        .inspect_err(log_read_only_failure)?;
        let first = sets.into_iter().next().unwrap_or_default();
        Ok(QueryResult {
            columns: first.columns,
            rows: first.rows,
        })
    }

    async fn close(&self) -> Result<(), DbError> {
        // tiberius Client doesn't have an explicit close method;
        // dropping the client closes the connection
        Ok(())
    }

    // ── Introspection ──
    //
    // Statistics stay unsupported (the default `NOT_SUPPORTED`): the
    // TypeScript adapter had none, and the UI shows its fallback.

    async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
        let r = self.query(introspect::SCHEMAS_SQL, vec![]).await?;
        Ok(introspect::parse_schemas(&r))
    }

    async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
        let r = self.query(introspect::SCHEMA_SQL, vec![]).await?;
        Ok(introspect::parse_schema(&r))
    }

    /// Bug fix 1: the table and schema are bound, so any name works.
    async fn table_metadata(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        let params = || vec![Value::from(table), Value::from(schema)];
        let columns = self.query(introspect::COLUMNS_SQL, params()).await?;
        let indexes = self.query(introspect::INDEXES_SQL, params()).await?;
        let filtered = self
            .query(introspect::FILTERED_UNIQUE_SQL, params())
            .await?;
        let mut columns = introspect::parse_columns(&columns);
        let indexes = introspect::parse_indexes(&indexes);
        // Task 18: UNIQUE from the indexes (the parse stays as recorded),
        // without filtered and INCLUDE unique indexes.
        let filtered: Vec<Value> = filtered
            .rows
            .into_iter()
            .map(|mut r| r.swap_remove(0))
            .collect();
        let plain: Vec<SchemaIndex> = indexes
            .iter()
            .filter(|i| !filtered.contains(&Value::Text(i.name.clone())))
            .cloned()
            .collect();
        seaquel_engine::introspect::apply_unique_indexes(&mut columns, &plain);
        Ok((columns, indexes))
    }

    /// Bug fix 9: the batches of [`introspect::explain_batches`] on the held
    /// connection. `SET … ON` stays in force for the session until the
    /// matching OFF, so the session holds state in between: if the call
    /// fails or is dropped before OFF, the connection is closed and the next
    /// call reconnects, instead of returning plans for ordinary queries.
    /// That reconnect also rolls back a transaction the user opened by hand
    /// on this connection, as any reconnect does.
    ///
    /// ANALYZE runs the query through `sp_executesql` (bound when it has
    /// parameters; a plain batch only if it must start one) and keeps only
    /// the plan's result set. A plain EXPLAIN runs the query as a batch,
    /// since under `SHOWPLAN_XML` an `sp_executesql` call returns no plan.
    /// Its parameters are declared without values
    /// ([`introspect::declare_params`]), which is enough to compile the
    /// plan, but the optimizer then can't sniff them: estimates are the
    /// generic ones for an unknown value, not those for the values given.
    async fn explain(
        &self,
        sql: &str,
        params: Vec<Value>,
        analyze: bool,
    ) -> Result<ExplainResult, DbError> {
        let [on, query, off] = introspect::explain_batches(sql, analyze);
        let request = if analyze {
            Request::new(&query, &params)?
        } else {
            let query = crate::bind::inline_nulls(&query, &params);
            Request::Batch(format!("{}{query}", introspect::declare_params(&params)?))
        };
        let mut session = self.session().await?;
        session.hold_state();
        session.batch(&on).await?;
        let run = session
            .run(request, Keep::Plans)
            .await
            .map_err(|f| f.into_db(Op::Query));
        // A request that didn't finish cleanly leaves the connection in
        // doubt: don't send more, the session closes it when dropped.
        if session.last_request_clean() {
            match session.batch(&off).await {
                Ok(_) => session.release_state(),
                Err(e) => {
                    warn!(activity = "db.explain", driver = "mssql"; "{off} failed, reconnecting on the next call: {}", e.message);
                }
            }
        }
        self.restore_database(&mut session, &[sql]).await;
        let sets = run.inspect_err(|e| {
            error!(activity = "db.explain", driver = "mssql", error_code = e.code.as_str(); "EXPLAIN failed");
        })?;
        Ok(introspect::parse_explain(
            &introspect::plan_result(&sets),
            analyze,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::must_start_batch;

    mod read_only_outcome {
        use super::super::{read_only_outcome, QueryFailure, ESCAPED, ESCAPE_SIGNAL, TRIED};
        use crate::session::ResultSet;
        use seaquel_engine::{DbError, Value};

        /// A failure: a server error with its number, or the row cap
        /// (`TooLargeDrained`, which isn't a server error).
        enum Fake {
            Server(u32),
            TooLarge,
        }

        impl QueryFailure for Fake {
            fn server_code(&self) -> Option<u32> {
                match self {
                    Fake::Server(code) => Some(*code),
                    Fake::TooLarge => None,
                }
            }

            fn into_db(self) -> DbError {
                match self {
                    Fake::Server(code) => DbError::query_error(format!("error {code}")),
                    Fake::TooLarge => DbError::result_too_large(5),
                }
            }
        }

        fn set(cells: &[i64]) -> Vec<ResultSet> {
            vec![ResultSet {
                columns: (0..cells.len()).map(|i| format!("c{i}")).collect(),
                rows: vec![cells.iter().map(|&n| Value::Int(n)).collect()],
            }]
        }

        fn rows() -> Result<Vec<ResultSet>, Fake> {
            Ok(set(&[42]))
        }

        fn fails(code: u32) -> Result<Vec<ResultSet>, Fake> {
            Err(Fake::Server(code))
        }

        /// `(n, tx)` after a transaction whose id was 7, two deep.
        fn outcome(
            end: (i64, i64),
            run: Result<Vec<ResultSet>, Fake>,
        ) -> Result<Vec<ResultSet>, DbError> {
            read_only_outcome(&set(&[7, 2]), &set(&[end.0, end.1]), run)
        }

        const INTACT: (i64, i64) = (2, 7);
        /// Ended: no transaction, or another one (an implicit one after it).
        const ENDED: [(i64, i64); 2] = [(0, 9), (1, 9)];

        fn assert_read_only(err: &DbError, message: &str) {
            assert_eq!(
                (err.code.as_str(), err.message.as_str()),
                ("READ_ONLY", message)
            );
        }

        #[test]
        fn an_intact_transaction_returns_the_query_result() {
            assert_eq!(outcome(INTACT, rows()).unwrap(), set(&[42]));
            let err = outcome(INTACT, Err(Fake::TooLarge)).unwrap_err();
            assert_eq!(err.code, "RESULT_TOO_LARGE");
        }

        #[test]
        fn the_querys_own_errors_are_returned_whatever_the_transaction() {
            // The CATCH re-raised them as the query's own: a doomed
            // transaction (245) is rolled back by the time the end batch
            // reads it, so the end state doesn't decide.
            for end in [INTACT, ENDED[0], ENDED[1]] {
                for code in [245, 241, 9400, 13609, 8134, 1222, 1205] {
                    let err = outcome(end, fails(code)).unwrap_err();
                    assert_eq!(
                        (err.code.as_str(), err.message.as_str()),
                        (
                            "QUERY_ERROR",
                            format!("Query failed: error {code}").as_str()
                        )
                    );
                }
            }
        }

        #[test]
        fn the_escape_signal_is_an_escape_with_the_error() {
            for end in [INTACT, ENDED[0], ENDED[1]] {
                let err = outcome(end, fails(ESCAPE_SIGNAL)).unwrap_err();
                assert_read_only(
                    &err,
                    &format!("{ESCAPED} The query's error: Query failed: error {ESCAPE_SIGNAL}"),
                );
            }
        }

        #[test]
        fn a_changed_depth_or_266_is_a_try_that_committed_nothing() {
            // One user COMMIT, or a nested BEGIN: still ours.
            for n in [1, 3] {
                for run in [rows(), fails(266), fails(8134), Err(Fake::TooLarge)] {
                    assert_read_only(&outcome((n, 7), run).unwrap_err(), TRIED);
                }
            }
            // Same id and depth with a 266.
            assert_read_only(&outcome(INTACT, fails(266)).unwrap_err(), TRIED);
        }

        #[test]
        fn an_ended_transaction_after_rows_or_the_row_cap_is_an_escape() {
            for end in ENDED {
                assert_read_only(&outcome(end, rows()).unwrap_err(), ESCAPED);
                let err = outcome(end, Err(Fake::TooLarge)).unwrap_err();
                assert_eq!(err.code, "READ_ONLY");
                assert!(err.message.starts_with(ESCAPED), "{}", err.message);
                assert!(err.message.contains("5-row cap"), "{}", err.message);
            }
        }

        #[test]
        fn unreadable_state_is_an_error() {
            let err = read_only_outcome(&set(&[7]), &set(&[2, 7]), rows()).unwrap_err();
            assert_eq!(err.code, "QUERY_ERROR");
            let err = read_only_outcome(&set(&[7, 2]), &set(&[1]), rows()).unwrap_err();
            assert_eq!(err.code, "QUERY_ERROR");
            assert_ne!(err.message, TRIED);
        }
    }

    #[test]
    fn use_is_found_outside_strings_and_comments() {
        for sql in [
            "USE master",
            "select 1; use [x]",
            "SELECT 1 OPTION (USE HINT ('X'))",
        ] {
            assert!(super::mentions_use(sql), "{sql}");
        }
        for sql in [
            "SELECT 'USE master'",
            "SELECT [use] FROM t",
            "SELECT \"USE\" FROM t",
            "-- USE master\nSELECT 1",
            "/* USE /* nested */ master */ SELECT 1",
            "SELECT user_id, USED FROM t",
            "",
        ] {
            assert!(!super::mentions_use(sql), "{sql}");
        }
    }

    #[test]
    fn statements_that_must_start_a_batch() {
        for sql in [
            "CREATE VIEW v AS SELECT 1 AS a",
            "create or alter procedure p as select 1",
            "  -- note\n/* a /* nested */ comment */ CREATE PROC p AS SELECT 1",
            "ALTER FUNCTION f() RETURNS INT AS BEGIN RETURN 1 END",
            "CREATE TRIGGER t ON x AFTER INSERT AS SELECT 1",
            "CREATE SCHEMA s",
            "CREATE DEFAULT d AS 0",
            "CREATE RULE r AS @v > 0",
        ] {
            assert!(must_start_batch(sql), "{sql}");
        }
        for sql in [
            "CREATE TABLE t (id INT)",
            "ALTER TABLE t ADD c INT",
            "ALTER SCHEMA s TRANSFER dbo.t",
            "SELECT 'CREATE VIEW'",
            "INSERT INTO v VALUES (1)",
            "-- CREATE VIEW v\nSELECT 1",
            "/* unclosed CREATE VIEW",
            "",
            "CREATE OR REPLACE VIEW v AS SELECT 1",
        ] {
            assert!(!must_start_batch(sql), "{sql}");
        }
    }
}
