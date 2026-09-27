//! Data steps: one-off rewrites of stored rows that need Rust, run by
//! `Storage::open` after the numbered SQL migrations. Each runs once per
//! file, recorded by name in `_seaquel_data_steps`. See
//! `migrations/README.md` for when to write one instead of a migration.

use sqlx::{Row, SqliteConnection, SqlitePool};

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
}

/// Every step, in the order they run. Append only; never rename or remove
/// one that has shipped.
const STEPS: &[Step] = &[Step {
    name: "strip_connection_string_passwords",
    kind: StepKind::StripConnectionStringPasswords,
}];

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
