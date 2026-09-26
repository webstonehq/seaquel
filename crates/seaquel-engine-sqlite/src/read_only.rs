//! The read-only statement gate: SQLite's own parser decides whether a
//! query is exactly one statement that doesn't write, before the driver's
//! read-only path runs it (plan: AI safety, Decision 1 and open question 2).
//!
//! sqlx runs every statement in a string, so `SELECT 1; INSERT …` would
//! insert, and `PRAGMA query_only = OFF; INSERT …` would switch off the
//! connection's read-only pragma first. `VACUUM INTO 'file'` writes a new
//! file even on a read-only, `query_only` connection. SQLite's
//! `sqlite3_stmt_readonly` is false for all the writes, and preparing the
//! rest of the input finds any second statement.
//!
//! `sqlite3_stmt_readonly` is also true for statements that change no
//! database but do change the process or the connection: `PRAGMA
//! hard_heap_limit` (a low one crashed the process), `soft_heap_limit` and
//! `temp_store_directory` (every connection in the process), and `ATTACH`.
//! So the read-only connection also gets an authorizer ([`deny_settings`])
//! that refuses every PRAGMA outside a small allowlist of queries, and
//! `ATTACH`/`DETACH`. It holds for everything SQLite prepares on that
//! connection, including the `PRAGMA` a `pragma_*()` table-valued function
//! prepares when it runs.
//!
//! This is the only `unsafe` code in the crate. The FFI surface is
//! `sqlite3_prepare_v2`, `sqlite3_stmt_readonly`, `sqlite3_finalize`,
//! `sqlite3_errmsg`, `sqlite3_errcode` and `sqlite3_set_authorizer`, all on
//! the handle sqlx hands out under its lock. `libsqlite3-sys` is pinned to
//! the version sqlx links (`links = "sqlite3"` allows only one), so the
//! `sqlite3` type here is sqlx's.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::ptr::{self, NonNull};

use libsqlite3_sys::{
    sqlite3, sqlite3_errcode, sqlite3_errmsg, sqlite3_finalize, sqlite3_prepare_v2,
    sqlite3_set_authorizer, sqlite3_stmt, sqlite3_stmt_readonly, SQLITE_ATTACH, SQLITE_AUTH,
    SQLITE_DENY, SQLITE_DETACH, SQLITE_OK, SQLITE_PRAGMA,
};
use seaquel_engine::DbError;
use sqlx::sqlite::LockedSqliteHandle;

/// The refusal for input with more than one statement.
pub(crate) const ONE_STATEMENT: &str = "Read-only queries run one statement at a time";
/// The refusal for a statement SQLite says may write.
pub(crate) const NO_WRITES: &str = "Read-only queries can't write to the database";
/// The refusal for input with no statement (empty, or only comments).
pub(crate) const NO_STATEMENT: &str = "The query has no statement to run";
/// The start of the refusal for what the authorizer denies; see
/// [`not_allowed`].
pub(crate) const NOT_ALLOWED: &str = "Read-only queries can't run this statement";

/// The authorizer's refusal, with SQLite's message (a bare "not authorized"
/// at prepare, or e.g. "vtable constructor failed: f" when a virtual table's
/// own statement was refused). Generic on purpose: the denial can come from
/// a statement SQLite prepares internally, not only from the user's.
fn not_allowed(message: &str) -> DbError {
    let detail = if message.trim() == "not authorized" {
        "not authorized".to_string()
    } else {
        format!("not authorized: {message}")
    };
    DbError::read_only(format!(
        "{NOT_ALLOWED} ({detail}). PRAGMAs that change settings, ATTACH and DETACH are refused."
    ))
}

/// PRAGMAs that only report, and may take an argument (a table, an index, a
/// row limit), which is passed to the authorizer the same way as a value.
const QUERY_PRAGMAS_WITH_ARG: &[&str] = &[
    "table_info",
    "table_xinfo",
    "table_list",
    "index_list",
    "index_info",
    "index_xinfo",
    "foreign_key_list",
    "foreign_key_check",
    "integrity_check",
    "quick_check",
];

/// PRAGMAs that report only when given no value: `user_version = 7` sets it.
const QUERY_PRAGMAS_NO_ARG: &[&str] = &[
    "compile_options",
    "function_list",
    "module_list",
    "pragma_list",
    "database_list",
    "collation_list",
    "user_version",
    "schema_version",
    // Prepared internally when FTS5 opens a table (and so by
    // `integrity_check`/`quick_check` with one present); FTS3/4 reads
    // `page_size`. Denying them made every FTS5 table unreadable.
    "data_version",
    "page_size",
];

/// Installs [`authorize`] on the read-only connection for the rest of its
/// life (the call). Every statement prepared afterwards, by the gate, by
/// sqlx, or inside a `pragma_*()` function, goes through it.
pub(crate) fn deny_settings(handle: &mut LockedSqliteHandle<'_>) {
    let db = handle.as_raw_handle();
    // SAFETY: `db` is the open connection behind the lock `handle` holds,
    // so sqlx's worker isn't using it. `authorize` is a plain function with
    // no user data (null), so there is nothing to outlive; it replaces any
    // earlier authorizer, and sqlx sets none.
    unsafe {
        sqlite3_set_authorizer(db.as_ptr(), Some(authorize), ptr::null_mut());
    }
}

/// The authorizer: `SQLITE_DENY` for `ATTACH`, `DETACH` and any PRAGMA but
/// the query PRAGMAs above, `SQLITE_OK` for everything else. It can't panic
/// (no allocation, no indexing, no unwrap), so nothing unwinds into C.
unsafe extern "C" fn authorize(
    _user_data: *mut c_void,
    action: c_int,
    arg1: *const c_char,
    arg2: *const c_char,
    _database: *const c_char,
    _trigger: *const c_char,
) -> c_int {
    match action {
        SQLITE_ATTACH | SQLITE_DETACH => SQLITE_DENY,
        SQLITE_PRAGMA => {
            if arg1.is_null() {
                return SQLITE_DENY;
            }
            // SAFETY: for SQLITE_PRAGMA, SQLite passes the pragma's name as
            // a NUL-terminated string valid for this call.
            let name = unsafe { CStr::from_ptr(arg1) }.to_bytes();
            let is = |list: &[&str]| list.iter().any(|p| p.as_bytes().eq_ignore_ascii_case(name));
            if is(QUERY_PRAGMAS_WITH_ARG) || (arg2.is_null() && is(QUERY_PRAGMAS_NO_ARG)) {
                SQLITE_OK
            } else {
                SQLITE_DENY
            }
        }
        _ => SQLITE_OK,
    }
}

/// `Ok` if `sql` is exactly one statement and `sqlite3_stmt_readonly` is
/// true for it, with any number of empty statements, comments and
/// whitespace around it. Refusals are `READ_ONLY`; a statement SQLite can't
/// prepare returns its error as `QUERY_ERROR`, as running it would.
///
/// It walks the input the way sqlx's `VirtualStatement` does (prepare,
/// advance by the tail, until nothing is left), so the gate and the
/// execution split the text the same way. Nothing is stepped: preparing
/// doesn't run anything. Every prepared statement is finalized before this
/// returns.
pub(crate) fn check(handle: &mut LockedSqliteHandle<'_>, sql: &str) -> Result<(), DbError> {
    // SQLite stops reading at a NUL byte and returns the NUL as the tail,
    // so everything after it would go unchecked (sqlx itself then loops on
    // the zero-length advance).
    // The driver refuses it before this runs; checked again because the
    // gate is unsound without it.
    if sql.contains('\0') {
        return Err(DbError::query_error(crate::driver::NUL_BYTE));
    }
    let db = handle.as_raw_handle();
    let mut rest = sql.as_bytes();
    let mut statement: Option<Statement> = None;
    while !rest.is_empty() {
        let (next, consumed) = prepare(db, rest)?;
        rest = &rest[consumed..];
        match next {
            Some(_) if statement.is_some() => return Err(DbError::read_only(ONE_STATEMENT)),
            Some(next) if !next.read_only() => return Err(DbError::read_only(NO_WRITES)),
            Some(next) => statement = Some(next),
            // Nothing prepared and nothing consumed would loop forever.
            // Can't happen without a NUL byte, refused above.
            None if consumed == 0 => {
                return Err(DbError::query_error(
                    "SQLite stopped reading the query before its end",
                ))
            }
            None => {}
        }
    }
    match statement {
        Some(_) => Ok(()),
        None => Err(DbError::read_only(NO_STATEMENT)),
    }
}

/// A prepared statement, finalized on drop.
struct Statement(NonNull<sqlite3_stmt>);

impl Statement {
    fn read_only(&self) -> bool {
        // SAFETY: `self.0` is a live statement from `sqlite3_prepare_v2`,
        // not yet finalized (only `Drop` finalizes it), and the connection
        // lock `check` holds keeps sqlx's worker off the connection.
        unsafe { sqlite3_stmt_readonly(self.0.as_ptr()) != 0 }
    }
}

impl Drop for Statement {
    fn drop(&mut self) {
        // SAFETY: as in `read_only`; this is the one place the statement is
        // finalized, and nothing uses it afterwards.
        unsafe {
            sqlite3_finalize(self.0.as_ptr());
        }
    }
}

/// Prepares the first statement in `sql`: the statement, if the text up to
/// the tail had one (it may be only whitespace, comments or a `;`), and how
/// many bytes SQLite consumed.
fn prepare(db: NonNull<sqlite3>, sql: &[u8]) -> Result<(Option<Statement>, usize), DbError> {
    let len = c_int::try_from(sql.len())
        .map_err(|_| DbError::query_error("The query is too long for SQLite"))?;
    let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
    let mut tail: *const c_char = ptr::null();
    // SAFETY: `db` is the open connection behind the `LockedSqliteHandle`
    // `check` borrows, so no other thread uses it until we return. `sql`
    // is valid for `len` bytes (SQLite reads no further, and doesn't need a
    // terminator when given the length). `stmt` and `tail` are valid out
    // pointers.
    let rc = unsafe {
        sqlite3_prepare_v2(
            db.as_ptr(),
            sql.as_ptr().cast::<c_char>(),
            len,
            &mut stmt,
            &mut tail,
        )
    };
    // Owned before anything can return, so it's finalized on every path.
    let stmt = NonNull::new(stmt).map(Statement);
    if rc != SQLITE_OK {
        let message = error_message(db);
        // SAFETY: as above; the error code of the prepare that just failed.
        return Err(if unsafe { sqlite3_errcode(db.as_ptr()) } == SQLITE_AUTH {
            not_allowed(&message)
        } else {
            DbError::query_error(message)
        });
    }
    // SQLite sets `tail` to a point inside `sql` (at most its end). Checked
    // rather than trusted, so a bad pointer can't slice out of bounds.
    let consumed = (tail as usize)
        .checked_sub(sql.as_ptr() as usize)
        .filter(|n| *n <= sql.len())
        .ok_or_else(|| DbError::query_error("SQLite returned a tail outside the query"))?;
    Ok((stmt, consumed))
}

/// The connection's last error message.
fn error_message(db: NonNull<sqlite3>) -> String {
    // SAFETY: `db` is the locked, open connection. `sqlite3_errmsg` returns
    // a NUL-terminated string SQLite owns, valid until the next call on the
    // connection; it's copied before anything else touches it.
    unsafe { CStr::from_ptr(sqlite3_errmsg(db.as_ptr())) }
        .to_string_lossy()
        .into_owned()
}

/// `e` with code `READ_ONLY` if SQLite refused a write (`SQLITE_READONLY`,
/// primary code 8, e.g. from `PRAGMA query_only`) or the authorizer refused
/// a statement (`SQLITE_AUTH`, 23), keeping its message.
///
/// `fetch_capped` (in `impl_sqlx_driver!`) has already turned the
/// `sqlx::Error` into a `DbError`, so the code is read back from the
/// message, where sqlx's `SqliteError` puts the extended code as
/// `(code: N)`.
pub(crate) fn map_read_only_error(e: DbError) -> DbError {
    let code = e
        .message
        .split_once("(code: ")
        .and_then(|(_, rest)| rest.split_once(')'))
        .and_then(|(n, _)| n.parse::<i32>().ok());
    match code {
        Some(n) if n & 0xff == 8 => DbError::read_only(e.message),
        Some(SQLITE_AUTH) => not_allowed(&e.message),
        _ => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{Connection, SqliteConnection};

    /// The gate, with the authorizer, on a private in-memory connection: no
    /// database file, no tables. Nothing is stepped, so a statement that
    /// slips through can't do anything here.
    async fn gate(sql: &str) -> Result<(), DbError> {
        let mut conn = SqliteConnection::connect("sqlite::memory:").await.unwrap();
        let result = {
            let mut handle = conn.lock_handle().await.unwrap();
            deny_settings(&mut handle);
            check(&mut handle, sql)
        };
        conn.close().await.unwrap();
        result
    }

    async fn refused(sql: &str) -> DbError {
        gate(sql)
            .await
            .err()
            .unwrap_or_else(|| panic!("{sql:?} passed"))
    }

    #[tokio::test]
    async fn one_read_only_statement_passes() {
        for sql in [
            "SELECT 1",
            "SELECT 1;",
            "SELECT 1; -- note",
            "  /* a */ ;; SELECT 1 ; ; /* b */ -- c\n",
            "WITH c AS (SELECT 1 AS x) SELECT x FROM c",
            "SELECT 'é;ü' AS \"ß\"; -- ✓",
            "EXPLAIN QUERY PLAN SELECT 1",
            "VALUES (1), (2)",
        ] {
            assert!(gate(sql).await.is_ok(), "{sql:?}: {:?}", gate(sql).await);
        }
    }

    #[tokio::test]
    async fn no_statement_is_refused() {
        for sql in ["", "   \n", "-- only a comment", "/* c */", ";", " ; ; "] {
            let e = refused(sql).await;
            assert_eq!(
                (e.code.as_str(), e.message.as_str()),
                ("READ_ONLY", NO_STATEMENT),
                "{sql:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_second_statement_is_refused() {
        for sql in [
            "SELECT 1; SELECT 2",
            "SELECT 1; CREATE TABLE t (a)",
            "SELECT 1; /* x */ ; SELECT 2",
        ] {
            let e = refused(sql).await;
            assert_eq!(
                (e.code.as_str(), e.message.as_str()),
                ("READ_ONLY", ONE_STATEMENT),
                "{sql:?}"
            );
        }
    }

    #[tokio::test]
    async fn writes_are_refused() {
        for sql in [
            "CREATE TABLE t (a)",
            "CREATE TEMP TABLE t (a)",
            "VACUUM INTO '/nonexistent/x.sqlite'",
            "PRAGMA query_only = OFF; CREATE TABLE t (a)",
        ] {
            let e = refused(sql).await;
            assert_eq!(e.code, "READ_ONLY", "{sql:?}: {e:?}");
        }
        let e = refused("CREATE TABLE t (a)").await;
        assert_eq!(e.message, NO_WRITES);
    }

    #[tokio::test]
    async fn a_statement_that_does_not_prepare_returns_sqlites_error() {
        let e = refused("SELEC 1").await;
        assert_eq!(e.code, "QUERY_ERROR");
        assert!(e.message.contains("syntax error"), "{e:?}");
        // In the tail too: the whole input is checked.
        let e = refused("SELECT 1; SELEC 2").await;
        assert!(e.message.contains("syntax error"), "{e:?}");
        let e = refused("SELECT * FROM no_such_table").await;
        assert!(e.message.contains("no such table"), "{e:?}");
    }

    #[tokio::test]
    async fn a_nul_byte_is_refused() {
        let e = refused("SELECT 1\0; DELETE FROM t").await;
        assert_eq!(e.code, "QUERY_ERROR");
        assert!(e.message.contains(crate::driver::NUL_BYTE), "{e:?}");
    }

    #[tokio::test]
    async fn the_authorizer_refuses_settings_and_attach() {
        for sql in [
            "PRAGMA hard_heap_limit = 200000",
            "PRAGMA soft_heap_limit = 200000",
            "PRAGMA temp_store_directory = '/tmp'",
            "PRAGMA hard_heap_limit",
            "PRAGMA query_only = OFF",
            "PRAGMA user_version = 7",
            "PRAGMA USER_VERSION(7)",
            "PRAGMA main.journal_mode = WAL",
            "PRAGMA writable_schema = ON",
            "ATTACH ':memory:' AS m",
            "ATTACH 'file:x?mode=memory&cache=shared' AS m",
            "DETACH m",
        ] {
            let e = refused(sql).await;
            assert_eq!(e.code, "READ_ONLY", "{sql:?}: {e:?}");
            assert!(e.message.starts_with(NOT_ALLOWED), "{sql:?}: {e:?}");
        }
    }

    #[tokio::test]
    async fn query_pragmas_pass_the_authorizer() {
        for sql in [
            "PRAGMA table_info(sqlite_master)",
            "PRAGMA main.TABLE_INFO('sqlite_master')",
            "PRAGMA table_list",
            "PRAGMA integrity_check",
            "PRAGMA quick_check(1)",
            "PRAGMA user_version",
            "PRAGMA schema_version",
            "PRAGMA database_list",
            "PRAGMA compile_options",
            "SELECT * FROM pragma_table_info('sqlite_master')",
            "SELECT * FROM pragma_hard_heap_limit",
        ] {
            assert!(gate(sql).await.is_ok(), "{sql:?}: {:?}", gate(sql).await);
        }
    }

    #[tokio::test]
    async fn a_one_megabyte_select_passes() {
        let sql = format!("SELECT '{}' AS big", "x".repeat(1024 * 1024));
        assert!(gate(&sql).await.is_ok());
        let sql = format!("{sql}; CREATE TABLE t (a)");
        assert_eq!(refused(&sql).await.message, ONE_STATEMENT);
    }

    #[test]
    fn sqlite_readonly_errors_map_to_read_only() {
        let e = map_read_only_error(DbError::query_error(
            "error returned from database: (code: 8) attempt to write a readonly database",
        ));
        assert_eq!(e.code, "READ_ONLY");
        assert!(e.message.contains("attempt to write a readonly database"));
        // An extended code (SQLITE_READONLY_DBMOVED = 8 | 4 << 8).
        let e = map_read_only_error(DbError::query_error("(code: 1032) x"));
        assert_eq!(e.code, "READ_ONLY");
        for other in [
            "(code: 1) near \"x\": syntax error",
            "(code: 264",
            "no code",
        ] {
            let e = map_read_only_error(DbError::query_error(other));
            assert_eq!(e.code, "QUERY_ERROR", "{other}");
        }
    }
}
