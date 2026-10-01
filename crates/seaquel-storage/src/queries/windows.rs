//! `windows` (migration `0002_window_state.sql`, phase 5d-2 Decision 22):
//! one row per desktop window (its webview label) or web browser tab
//! (`win-<uuid>`), with the project it shows and when it was last used.
//! A window's view state per project is in `window_state`, which cascades
//! from here.
//!
//! Times are the ISO strings Core's executor makes
//! (`2026-10-04T00:00:00.000Z`), which order as text. The 30-day prune
//! goes by them; "most recently used" goes by `write_seq` (migration
//! `0003`), one past the table's highest on every write, so it is the last
//! write committed even when two land in one millisecond (5d-2 Task 7).

use sqlx::Row;

use super::codec::{encode_error, Result};
use crate::{Reader, WriteTx};

/// The most rows one prune statement deletes, so a save's cleanup is
/// bounded whatever was left behind. What's past it goes on later saves.
pub const PRUNE_BATCH: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRow {
    pub window_id: String,
    /// `None` before the window first activates a project. Not a foreign
    /// key: it can name a project removed since.
    pub active_project_id: Option<String>,
    pub updated_at: String,
}

/// A text column read as bytes, so a hand-edited value that isn't UTF-8
/// never fails the read.
fn lossy(bytes: Option<Vec<u8>>) -> Option<String> {
    bytes.map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// One window, or `None`.
pub async fn get(r: impl Into<Reader<'_>>, window_id: &str) -> Result<Option<WindowRow>> {
    let mut conn = r.into().conn().await?;
    let row = sqlx::query(
        "SELECT CAST(active_project_id AS BLOB), CAST(updated_at AS BLOB) \
         FROM windows WHERE window_id = ?",
    )
    .bind(window_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(WindowRow {
        window_id: window_id.to_string(),
        active_project_id: lossy(row.try_get_unchecked(0)?),
        updated_at: lossy(row.try_get_unchecked(1)?).unwrap_or_default(),
    }))
}

/// Marks the window used at `now`, adding its row if it has none. Its
/// active project is kept.
pub async fn touch(tx: &mut WriteTx, window_id: &str, now: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO windows (window_id, active_project_id, updated_at, write_seq) \
         VALUES (?, NULL, ?, (SELECT COALESCE(MAX(write_seq), 0) + 1 FROM windows)) \
         ON CONFLICT(window_id) DO UPDATE SET \
           updated_at = excluded.updated_at, write_seq = excluded.write_seq",
    )
    .bind(window_id)
    .bind(now)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Sets the window's active project and marks it used at `now`, adding its
/// row if it has none.
pub async fn set_active_project(
    tx: &mut WriteTx,
    window_id: &str,
    project_id: &str,
    now: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO windows (window_id, active_project_id, updated_at, write_seq) \
         VALUES (?, ?, ?, (SELECT COALESCE(MAX(write_seq), 0) + 1 FROM windows)) \
         ON CONFLICT(window_id) DO UPDATE SET \
           active_project_id = excluded.active_project_id, updated_at = excluded.updated_at, \
           write_seq = excluded.write_seq",
    )
    .bind(window_id)
    .bind(project_id)
    .bind(now)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The most recently used window that has an active project (the last
/// write committed: `write_seq`, then `rowid`, descending): a walk down
/// `idx_windows_write_seq`, newest first, past the windows without one
/// (at most the user's window count, which the prunes bound).
pub const MOST_RECENT_ACTIVE: &str = "\
    SELECT CAST(window_id AS BLOB), CAST(active_project_id AS BLOB), CAST(updated_at AS BLOB) \
    FROM windows WHERE active_project_id IS NOT NULL \
    ORDER BY write_seq DESC, rowid DESC LIMIT 1";

/// `windowGet`'s fallback for a window with no active project of its own
/// (Decision 22): the most recently used window that has one, or `None`.
pub async fn most_recent_active(r: impl Into<Reader<'_>>) -> Result<Option<WindowRow>> {
    let mut conn = r.into().conn().await?;
    let row = sqlx::query(MOST_RECENT_ACTIVE)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(WindowRow {
        window_id: lossy(row.try_get_unchecked(0)?).unwrap_or_default(),
        active_project_id: lossy(row.try_get_unchecked(1)?),
        updated_at: lossy(row.try_get_unchecked(2)?).unwrap_or_default(),
    }))
}

/// Windows unused since before `?1`, oldest first, at most `?3` of them,
/// none of those named in `?2` (a JSON array): one range search of
/// `idx_windows_updated`.
pub const PRUNE_UNUSED: &str = "\
    DELETE FROM windows WHERE rowid IN ( \
      SELECT rowid FROM windows \
      WHERE updated_at < ?1 AND window_id NOT IN (SELECT value FROM json_each(?2)) \
      ORDER BY updated_at, rowid LIMIT ?3)";

/// The windows past the `?1` most recent (equal times ordered as
/// [`MOST_RECENT_ACTIVE`] orders them), at most `?3` of them, except
/// those named in `?2`: a walk down `idx_windows_write_seq` of `?1 + ?3`
/// entries at most.
pub const PRUNE_OVER_COUNT: &str = "\
    DELETE FROM windows WHERE rowid IN ( \
      SELECT rowid FROM windows ORDER BY write_seq DESC, rowid DESC LIMIT ?3 OFFSET ?1) \
    AND window_id NOT IN (SELECT value FROM json_each(?2))";

/// The window ids a prune must never delete (the saving window, and `main`
/// on desktop), as the JSON array the prune statements read.
pub(crate) fn spare_json(spare: &[&str]) -> Result<String> {
    serde_json::to_string(spare).map_err(|e| encode_error(e.to_string()))
}

/// The bounded cleanup a view-state save runs (Decision 22), each step one
/// indexed `DELETE` of at most [`PRUNE_BATCH`] windows (their view states
/// cascade):
/// - windows last used before `unused_before` (Core passes 30 days ago);
/// - then, with `max_windows`, the windows past the most recent
///   `max_windows`.
///
/// Neither deletes a window named in `spare`. Returns how many windows
/// went.
pub async fn prune(
    tx: &mut WriteTx,
    unused_before: &str,
    max_windows: Option<u32>,
    spare: &[&str],
) -> Result<u64> {
    let spare = spare_json(spare)?;
    let conn = tx.conn();
    let mut deleted = sqlx::query(PRUNE_UNUSED)
        .bind(unused_before)
        .bind(&spare)
        .bind(i64::from(PRUNE_BATCH))
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if let Some(max) = max_windows {
        deleted += sqlx::query(PRUNE_OVER_COUNT)
            .bind(i64::from(max))
            .bind(&spare)
            .bind(i64::from(PRUNE_BATCH))
            .execute(&mut *conn)
            .await?
            .rows_affected();
    }
    Ok(deleted)
}
