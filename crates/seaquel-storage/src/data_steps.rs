//! Data steps: one-off rewrites of stored rows that need Rust, run by
//! `Storage::open` after the numbered SQL migrations. Each runs once per
//! file, recorded by name in `_seaquel_data_steps`. See
//! `migrations/README.md` for when to write one instead of a migration.

use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::connection_string::{is_legacy_built_string, username_from_string, StringFields};
use crate::queries::codec::{number, opt_text, text};
use crate::{strip_connection_string_password, StorageError};

/// Where applied steps are recorded, with the time they ran.
pub const DATA_STEPS_TABLE: &str = "_seaquel_data_steps";

/// One step: a name that's never reused, and what it does. It runs inside
/// the transaction that records it.
struct Step {
    name: &'static str,
    kind: StepKind,
}

enum StepKind {
    StripConnectionStringPasswords,
    DropLegacyBuiltConnectionStrings,
    BackfillNameKeys,
    BackfillDashboardNameKeys,
}

/// Every step, in the order they run. Append only; never rename or remove
/// one that has shipped.
const STEPS: &[Step] = &[
    Step {
        name: "strip_connection_string_passwords",
        kind: StepKind::StripConnectionStringPasswords,
    },
    Step {
        name: "drop_legacy_built_connection_strings",
        kind: StepKind::DropLegacyBuiltConnectionStrings,
    },
    Step {
        name: "backfill_name_keys",
        kind: StepKind::BackfillNameKeys,
    },
    Step {
        name: "backfill_dashboard_name_keys",
        kind: StepKind::BackfillDashboardNameKeys,
    },
];

/// Runs the steps this file hasn't had yet, each in its own `BEGIN
/// IMMEDIATE` transaction. A file that has them all only gets a read.
///
/// A step that fails is rolled back, logged (its name and the error's
/// category, never a value) and not recorded, so `open` carries on and the
/// next open tries it again. A cleanup mustn't make the app unusable on
/// every start.
pub(crate) async fn run(pool: &SqlitePool) {
    let read = match pool.acquire().await {
        Ok(mut conn) => pending(&mut conn).await,
        Err(e) => Err(e.into()),
    };
    let pending = match read {
        Ok(pending) => pending,
        Err(e) => {
            log::warn!(
                "data steps: couldn't read which ran ({}); retrying on the next open",
                category(&e)
            );
            return;
        }
    };
    for step in STEPS.iter().filter(|s| pending.contains(&s.name)) {
        if let Err(e) = run_step(pool, step).await {
            log::warn!(
                "data step {} failed ({}); rolled back, retrying on the next open",
                step.name,
                category(&e)
            );
        }
    }
}

/// An error's category for the log: sqlx's error kind and SQLite's code,
/// without the message, which can quote stored values.
fn category(e: &StorageError) -> String {
    match e {
        StorageError::Sqlx(sqlx::Error::Database(db)) => {
            format!(
                "database error {:?}, code {}",
                db.kind(),
                db.code().unwrap_or_default()
            )
        }
        StorageError::Sqlx(sqlx::Error::Decode(_)) => "decode error".to_string(),
        StorageError::Sqlx(_) => "sqlx error".to_string(),
        _ => e.code().to_string(),
    }
}

/// One step and its record, in one transaction: both or neither.
async fn run_step(pool: &SqlitePool, step: &Step) -> Result<(), StorageError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {DATA_STEPS_TABLE} (name TEXT PRIMARY KEY, applied_at TEXT)"
    ))
    .execute(&mut *tx)
    .await?;
    // Another process may have run it since `pending` looked.
    let done: Option<(String,)> = sqlx::query_as(&format!(
        "SELECT name FROM {DATA_STEPS_TABLE} WHERE name = ?"
    ))
    .bind(step.name)
    .fetch_optional(&mut *tx)
    .await?;
    if done.is_some() {
        return Ok(());
    }
    match step.kind {
        StepKind::StripConnectionStringPasswords => {
            strip_connection_string_passwords(&mut tx).await?
        }
        StepKind::DropLegacyBuiltConnectionStrings => {
            drop_legacy_built_connection_strings(&mut tx).await?
        }
        StepKind::BackfillNameKeys => backfill_name_keys(&mut tx).await?,
        StepKind::BackfillDashboardNameKeys => backfill_dashboard_name_keys(&mut tx).await?,
    }
    sqlx::query(&format!(
        "INSERT INTO {DATA_STEPS_TABLE} (name, applied_at) VALUES (?, datetime('now'))"
    ))
    .bind(step.name)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The steps not yet recorded (all of them when the table doesn't exist).
/// It only reads, so a read-only open uses it too.
pub(crate) async fn pending(
    conn: &mut SqliteConnection,
) -> Result<Vec<&'static str>, StorageError> {
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(DATA_STEPS_TABLE)
            .fetch_optional(&mut *conn)
            .await?;
    if exists.is_none() {
        return Ok(STEPS.iter().map(|s| s.name).collect());
    }
    let done: Vec<(String,)> = sqlx::query_as(&format!("SELECT name FROM {DATA_STEPS_TABLE}"))
        .fetch_all(&mut *conn)
        .await?;
    Ok(STEPS
        .iter()
        .map(|s| s.name)
        .filter(|n| !done.iter().any(|(d,)| d == n))
        .collect())
}

/// Decision 13.1: connection strings the TypeScript stored with a password
/// (key=value strings, which it couldn't parse) lose it. Rows are addressed
/// by rowid, so a NULL id doesn't matter, and only rows that change are
/// written. Text that isn't valid UTF-8 is left alone.
async fn strip_connection_string_passwords(
    conn: &mut SqliteConnection,
) -> Result<(), StorageError> {
    let rows = sqlx::query(
        "SELECT rowid, connection_string FROM connections \
         WHERE typeof(connection_string) = 'text'",
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        let rowid: i64 = row.try_get(0)?;
        let bytes: Vec<u8> = row.try_get_unchecked(1)?;
        let Ok(stored) = String::from_utf8(bytes) else {
            continue;
        };
        let stripped = strip_connection_string_password(&stored);
        if stripped != stored {
            sqlx::query("UPDATE connections SET connection_string = ? WHERE rowid = ?")
                .bind(stripped)
                .bind(rowid)
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(())
}

/// Phase 5d Decision 12: the strings rows written before phase 5a hold,
/// which the old `buildConnectionString` rebuilt from the row's own fields
/// on every save, become NULL. Core connects with a string when there is one
/// and ignores the fields, so such a string went stale the moment a field
/// was edited; until now the TypeScript dropped them on every load
/// (`initializePersistedConnections`), but the MCP server reads rows the app
/// may not have loaded since.
///
/// It does exactly what that load did, row by row:
/// - fields are read as `connections::load_all` reads them;
/// - an empty `username` is taken from the string's URL user, as the load
///   did before comparing ([`username_from_string`]);
/// - a non-empty string that [`is_legacy_built_string`] says the old
///   builder made from those fields is set to NULL, and a user taken from
///   it is written to `username`, as the load's save wrote it back.
///
/// Rows are addressed by rowid, only rows that change are written, and a
/// row with a value that doesn't decode (text that isn't UTF-8) is left
/// alone, so the step can't fail on data a user can't fix.
async fn drop_legacy_built_connection_strings(
    conn: &mut SqliteConnection,
) -> Result<(), StorageError> {
    let rows = sqlx::query(
        "SELECT rowid, type, host, port, database_name, username, ssl_mode, connection_string \
         FROM connections WHERE typeof(connection_string) = 'text' AND connection_string <> ''",
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        let rowid: i64 = row.try_get(0)?;
        let Some(change) = legacy_row(&row) else {
            continue;
        };
        match change {
            LegacyRow::Drop => {
                sqlx::query("UPDATE connections SET connection_string = NULL WHERE rowid = ?")
                    .bind(rowid)
                    .execute(&mut *conn)
                    .await?;
            }
            LegacyRow::DropKeepingUser(username) => {
                sqlx::query(
                    "UPDATE connections SET connection_string = NULL, username = ? WHERE rowid = ?",
                )
                .bind(username)
                .bind(rowid)
                .execute(&mut *conn)
                .await?;
            }
        }
    }
    Ok(())
}

/// The tables with a `name_key` column from migration `0001_name_keys.sql`.
pub(crate) const NAME_KEY_TABLES: [&str; 3] = ["connections", "projects", "saved_queries"];

/// Every table with a `name_key` column: [`NAME_KEY_TABLES`] and
/// `dashboards` (migration `0002_window_state.sql`). `refill_name_keys`
/// covers them all.
pub(crate) const ALL_NAME_KEY_TABLES: [&str; 4] =
    ["connections", "projects", "saved_queries", "dashboards"];

/// Phase 5d-1 probe fix: fills `name_key` (added by `0001_name_keys.sql`)
/// for the rows written before it, with [`seaquel_types::names::name_key`],
/// the function storage's writes use, so stored keys and new ones agree by
/// construction. Only rows whose key is NULL and whose name is UTF-8 text
/// are written, by rowid; one pass, linear in rows. A row it leaves NULL
/// (no name, or a name that isn't UTF-8) is still found by the duplicate
/// check, which folds the names of NULL-key rows itself.
pub(crate) async fn backfill_name_keys(conn: &mut SqliteConnection) -> Result<(), StorageError> {
    fill_name_keys(conn, &NAME_KEY_TABLES).await
}

/// Whether any row of [`ALL_NAME_KEY_TABLES`] has a NULL key and a name
/// the fill would give one (UTF-8 text). Rows with a NULL key are only
/// those an older release wrote or renamed, so they are few; a name that
/// isn't UTF-8 never gets a key and so never counts, or every open would
/// report work it can't do.
pub(crate) async fn name_keys_pending(pool: &SqlitePool) -> Result<bool, StorageError> {
    let mut conn = pool.acquire().await?;
    for table in ALL_NAME_KEY_TABLES {
        let names: Vec<(Vec<u8>,)> = sqlx::query_as(&format!(
            "SELECT CAST(name AS BLOB) FROM {table} \
             WHERE name_key IS NULL AND typeof(name) = 'text'"
        ))
        .fetch_all(&mut *conn)
        .await?;
        if names.iter().any(|(b,)| std::str::from_utf8(b).is_ok()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Phase 5d-2 (Decision 21): [`backfill_name_keys`] for `dashboards`, whose
/// `name_key` came with `0002_window_state.sql`. A dashboard with a NULL
/// `project_id` (beta-era files) gets its key too; it's in no project's
/// lookup either way.
pub(crate) async fn backfill_dashboard_name_keys(
    conn: &mut SqliteConnection,
) -> Result<(), StorageError> {
    fill_name_keys(conn, &["dashboards"]).await
}

/// Sets `name_key` on every row of `tables` whose key is NULL and whose
/// name is UTF-8 text, by rowid, in one pass per table.
pub(crate) async fn fill_name_keys(
    conn: &mut SqliteConnection,
    tables: &[&str],
) -> Result<(), StorageError> {
    for table in tables {
        let rows = sqlx::query(&format!(
            "SELECT rowid, name FROM {table} WHERE name_key IS NULL AND typeof(name) = 'text'"
        ))
        .fetch_all(&mut *conn)
        .await?;
        let update = format!("UPDATE {table} SET name_key = ? WHERE rowid = ?");
        for row in rows {
            let rowid: i64 = row.try_get(0)?;
            let bytes: Vec<u8> = row.try_get_unchecked(1)?;
            let Ok(name) = String::from_utf8(bytes) else {
                continue;
            };
            sqlx::query(&update)
                .bind(seaquel_types::names::name_key(&name))
                .bind(rowid)
                .execute(&mut *conn)
                .await?;
        }
    }
    Ok(())
}

enum LegacyRow {
    Drop,
    /// The row's `username` was empty and the string supplied this one.
    DropKeepingUser(String),
}

/// What the step does to one row: `None` leaves it alone, including when a
/// value doesn't decode.
fn legacy_row(row: &sqlx::sqlite::SqliteRow) -> Option<LegacyRow> {
    let string = opt_text(row, "connection_string").ok()??;
    let ty = text(row, "type").ok()?;
    let host = text(row, "host").ok()?;
    let port = number(row, "port").ok()?;
    let database_name = text(row, "database_name").ok()?;
    let stored_user = text(row, "username").ok()?;
    let ssl_mode = opt_text(row, "ssl_mode").ok()?;
    let username = if stored_user.is_empty() {
        username_from_string(&string)
    } else {
        stored_user.clone()
    };
    let fields = StringFields {
        ty: &ty,
        host: &host,
        port,
        database_name: &database_name,
        username: &username,
        ssl_mode: ssl_mode.as_deref(),
    };
    if !is_legacy_built_string(&string, &fields) {
        return None;
    }
    Some(if username == stored_user {
        LegacyRow::Drop
    } else {
        LegacyRow::DropKeepingUser(username)
    })
}
