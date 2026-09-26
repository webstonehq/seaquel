//! The AI's read-only check (Task 3, fix 14). Port of `validateReadOnlyQuery`
//! (`src/lib/services/ai/context.ts`, whose body phase 2b deleted), on tokens,
//! with the rules of `readOnlyModel` in the recorder's `statements-model.ts`
//! (`docs/plans/artifacts/2026-09-27-sql-recorder-statements-model.ts.txt`).
//!
//! This check gates which statements the AI may run; it isn't a sandbox (a
//! user-defined function can do anything, and no list can cover those).
//! DuckDB's `read_csv('https://…' || …)` can send data out through httpfs and
//! stays allowed.

use crate::scan::{significant, ScanOptions, Token, TokenKind};
use crate::statements::{is_name, unquote_name, Tokens};
use crate::SqlEngine;

/// The message users see, word for word the TS text.
pub const READ_ONLY_MESSAGE: &str = "Only read-only SELECT queries are permitted";

/// The TS list plus fix 14's additions. `SELECT … INTO` creates a table; FOR
/// UPDATE is caught by UPDATE.
const BLOCKED: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "TRUNCATE", "CREATE", "GRANT", "REVOKE", "EXEC",
    "MERGE", "COPY", "CALL", "DO", "EXECUTE", "SET", "LOAD", "INSTALL", "ATTACH", "DETACH",
    "PRAGMA", "VACUUM", "REPLACE", "LOCK", "INTO",
];

/// SQL Server runs statements that follow each other without a `;`, so every
/// T-SQL statement word is blocked there. FETCH stays allowed (OFFSET …
/// FETCH); RAISERROR and PRINT are harmless.
const BLOCKED_MSSQL: &[&str] = &[
    "KILL",
    "SHUTDOWN",
    "DBCC",
    "BACKUP",
    "RESTORE",
    "DENY",
    "RECONFIGURE",
    "WAITFOR",
    "RECEIVE",
    "WRITETEXT",
    "UPDATETEXT",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "SAVE",
    "DECLARE",
    "IF",
    "WHILE",
    "USE",
    "SETUSER",
    "REVERT",
    "ADD",
    "OPEN",
    "CLOSE",
    "DEALLOCATE",
    "CHECKPOINT",
    "ENABLE",
    "DISABLE",
];

/// T-SQL statement words that are blocked only before CONVERSATION.
const BLOCKED_MSSQL_BEFORE_CONVERSATION: &[&str] = &["END", "MOVE"];

/// Blocked words that are also string functions: `REPLACE(`, MySQL's `INSERT(`.
const FUNCTIONS: &[&str] = &["REPLACE", "INSERT"];

/// Functions with side effects outside the query, blocked as a name followed
/// by `(` (compared lower-cased).
const BLOCKED_FUNCTIONS: &[&str] = &[
    // Postgres: other connections, files, large objects, sequences, settings
    "dblink",
    "lo_import",
    "lo_export",
    "lo_unlink",
    "lo_put",
    "lo_from_bytea",
    "lo_create",
    "pg_terminate_backend",
    "pg_cancel_backend",
    "set_config",
    "setval",
    "nextval",
    "pg_reload_conf",
    "pg_read_file",
    "pg_read_binary_file",
    "pg_ls_dir",
    "pg_file_write",
    "pg_file_unlink",
    "pg_file_rename",
    // Postgres: replication, notifications, locks, admin
    "pg_create_physical_replication_slot",
    "pg_create_logical_replication_slot",
    "pg_drop_replication_slot",
    "pg_logical_slot_get_changes",
    "pg_logical_slot_peek_changes",
    "pg_notify",
    "pg_advisory_lock",
    "pg_advisory_xact_lock",
    "pg_switch_wal",
    "pg_promote",
    "pg_rotate_logfile",
    "pg_import_system_collations",
    // Sleeping and locks
    "pg_sleep",
    "sleep",
    "benchmark",
    "get_lock",
    // SQL Server: other servers and files
    "openquery",
    "openrowset",
    "opendatasource",
    // SQLite, MySQL: files
    "load_extension",
    "load_file",
];

const BLOCKED_FUNCTION_PREFIXES: &[&str] = &["dblink_", "pg_stat_reset"];

fn blocked_function(name: &str) -> bool {
    let name = name.to_lowercase();
    BLOCKED_FUNCTIONS.contains(&name.as_str())
        || BLOCKED_FUNCTION_PREFIXES
            .iter()
            .any(|p| name.starts_with(p))
}

/// Whether one reading of the input holds anything but read-only statements.
fn refused(sql: &str, toks: Vec<Token>, engine: SqlEngine, ansi_mysql: bool) -> bool {
    let t = Tokens::new(sql, toks);
    let mssql = engine == SqlEngine::Mssql;
    // The statements, split on `;`, without the empty ones.
    let mut statements: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for k in 0..=t.len() {
        if k == t.len() || t.is(t.get(k), b';') {
            if k > start {
                statements.push((start, k));
            }
            start = k + 1;
        }
    }
    if statements.is_empty() {
        return true;
    }
    for (from, to) in statements {
        // Inside one statement: `at(k)` is `None` outside it.
        let at = |k: Option<usize>| k.filter(|&k| k >= from && k < to).and_then(|k| t.get(k));
        let word = |k: Option<usize>| k.filter(|&k| k >= from && k < to).and_then(|k| t.word(k));
        if !matches!(word(Some(from)), Some("SELECT" | "WITH")) {
            return true;
        }
        for k in from..to {
            let tok = &t.toks[k];
            let next = at(Some(k + 1));
            let next_is_call = t.is(next, b'(');
            let prev = at(k.checked_sub(1));
            let prev2 = at(k.checked_sub(2));
            if tok.kind == TokenKind::Quoted && next_is_call {
                // Postgres `U&"\0073etval"(`: a Unicode-escaped name called as a function.
                if t.is(prev, b'&') && prev2.is_some_and(|p| matches!(t.text(p), "U" | "u")) {
                    return true;
                }
                // A quoted function name: `"setval"(`, `` `load_file`( ``.
                if unquote_name(sql, tok, engine, ansi_mysql).is_some_and(|n| blocked_function(&n))
                {
                    return true;
                }
            }
            if tok.kind != TokenKind::Word {
                continue;
            }
            // A function with side effects, qualified or not.
            if next_is_call && blocked_function(t.text(tok)) {
                return true;
            }
            let w = t.word(k).unwrap_or("");
            if mssql
                && BLOCKED_MSSQL_BEFORE_CONVERSATION.contains(&w)
                && word(Some(k + 1)) == Some("CONVERSATION")
            {
                return true;
            }
            let blocked = BLOCKED.contains(&w) || (mssql && BLOCKED_MSSQL.contains(&w));
            if !blocked {
                continue;
            }
            // `t.set` is a column: a word after a `.` that follows a name.
            // After a number (`1. INTO t`) it's still a keyword.
            if t.is(prev, b'.') && is_name(sql, prev2, engine) {
                continue;
            }
            if FUNCTIONS.contains(&w) && next_is_call {
                continue;
            }
            return true;
        }
    }
    false
}

/// [`READ_ONLY_MESSAGE`] if `sql` isn't read-only, `None` if it is (fix 14):
///
/// - Every statement's first token is SELECT or WITH; an input with no
///   statement is refused.
/// - No statement holds a blocked word outside strings, comments and quoted
///   names (on SQL Server every T-SQL statement word too), or calls a
///   function with outside effects, qualified or not, quoted or not.
/// - MySQL/MariaDB: any executable comment is refused, and the input is read
///   with the default sql_mode and with `NO_BACKSLASH_ESCAPES` +
///   `ANSI_QUOTES`; Postgres with `standard_conforming_strings` on and off.
///   Every reading is scanned with fix 19's word rule and the TS's. Any
///   reading refused refuses.
pub fn read_only_error(sql: &str, engine: SqlEngine) -> Option<&'static str> {
    if engine.is_mysql() && (sql.contains("/*!") || sql.contains("/*M!")) {
        return Some(READ_ONLY_MESSAGE);
    }
    let base = ScanOptions {
        exec_comments: true,
        ..ScanOptions::default()
    };
    let mut readings = vec![base];
    if engine.is_mysql() {
        readings.push(ScanOptions {
            ansi_mysql: true,
            ..base
        });
    } else if engine == SqlEngine::Postgres {
        readings.push(ScanOptions {
            pg_backslash: true,
            ..base
        });
    }
    // Fix 19: every reading also with the TS word rule, so a mark glued to a
    // keyword (`INTÓ` on SQL Server) can't hide it from both word rules.
    readings
        .into_iter()
        .flat_map(|r| {
            [
                r,
                ScanOptions {
                    ts_words: true,
                    ..r
                },
            ]
        })
        .any(|r| refused(sql, significant(sql, engine, r), engine, r.ansi_mysql))
        .then_some(READ_ONLY_MESSAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PG: SqlEngine = SqlEngine::Postgres;

    #[test]
    fn gates_statements_on_tokens() {
        assert_eq!(read_only_error("SELECT 'DROP TABLE' AS s", PG), None);
        assert_eq!(
            read_only_error("SELECT 1; COPY t TO PROGRAM 'x'", PG),
            Some(READ_ONLY_MESSAGE)
        );
        assert_eq!(read_only_error("SELECT t.set FROM t", PG), None);
        assert_eq!(
            read_only_error("SELECT 1. INTO t", PG),
            Some(READ_ONLY_MESSAGE)
        );
        assert_eq!(
            read_only_error("SELECT REPLACE(a, 'x', 'y') FROM t", PG),
            None
        );
        assert_eq!(
            read_only_error("SELECT \"setval\"('s', 1)", PG),
            Some(READ_ONLY_MESSAGE)
        );
        assert_eq!(
            read_only_error("SELECT U&\"x\"(1)", PG),
            Some(READ_ONLY_MESSAGE)
        );
        assert_eq!(
            read_only_error("SELECT pg_stat_reset_shared('x')", PG),
            Some(READ_ONLY_MESSAGE)
        );
        assert_eq!(
            read_only_error("SELECT 1 AS k KILL 9999", SqlEngine::Mssql),
            Some(READ_ONLY_MESSAGE)
        );
        assert_eq!(
            read_only_error("SELECT 1 /*! */", SqlEngine::Mysql),
            Some(READ_ONLY_MESSAGE)
        );
        assert_eq!(read_only_error("", PG), Some(READ_ONLY_MESSAGE));
        assert_eq!(read_only_error(";;", PG), Some(READ_ONLY_MESSAGE));
    }

    #[test]
    fn both_readings() {
        // Standard strings: `'\'` ends, and the DELETE is code.
        let sql = "SELECT '\\'; DELETE FROM t; SELECT '1'";
        assert_eq!(read_only_error(sql, PG), Some(READ_ONLY_MESSAGE));
        // Default sql_mode: one long string. ANSI_QUOTES: a name, then code.
        let my = "SELECT \"a\\\"; DELETE FROM t; SELECT \"\"";
        assert_eq!(
            read_only_error(my, SqlEngine::Mysql),
            Some(READ_ONLY_MESSAGE)
        );
    }
}
