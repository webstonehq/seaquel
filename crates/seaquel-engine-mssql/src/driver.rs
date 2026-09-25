use log::{error, info, warn};
use tiberius::{AuthMethod, Client, Config, Query};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::compat::TokioAsyncWriteCompatExt;

use seaquel_engine::{
    BatchStatement, ConnectConfig, DbError, Driver, ExecuteResult, ExpectRows, ExplainResult,
    QueryResult, SchemaColumn, SchemaIndex, SchemaTable, Value,
};

use crate::introspect;
use crate::session::{build_query, Connection, Keep, MssqlClient, Op, ResultSet, Session};

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
}

/// Sent as a batch when a connection opens (and on every reconnect), so it
/// holds for the session: `sp_executesql` calls see it, and a `SET` inside
/// one ends with that call. A server whose `user options` include NOCOUNT
/// (512) starts every session with NOCOUNT ON, which makes every UPDATE and
/// DELETE report 0 rows, so every grid edit would fail as matching no row.
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
