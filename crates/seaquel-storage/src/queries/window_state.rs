//! `window_state` (migration `0002_window_state.sql`, phase 5d-2 Decision
//! 22): one window's view of one project (its open tabs with their text,
//! pane layout and active ids) as one JSON blob, stored and read byte for
//! byte, with `rev`, the page's save counter.
//!
//! Every row needs its window's `windows` row first (a foreign key):
//! Core calls `windows::touch` before [`put_if_newer`]. Rows go with their
//! window and with their project (both cascade).

use crate::db;
use crate::db::Row;
use crate::db::SqliteRow;
use serde_json::value::RawValue;

use super::codec::{encode_error, stored_json, Result};
use super::windows::{spare_json, PRUNE_BATCH};
use crate::{Reader, WriteTx};

/// A stored view state.
#[derive(Debug, Clone)]
pub struct WindowStateRow {
    pub window_id: String,
    pub project_id: String,
    /// `None` when the stored text doesn't read (not UTF-8, not JSON,
    /// `null`); Core then falls back as for a window with no row.
    pub state: Option<Box<RawValue>>,
    pub rev: u64,
    pub updated_at: String,
}

const COLUMNS: &str = "CAST(window_id AS BLOB), CAST(project_id AS BLOB), \
                       CAST(state AS BLOB), rev, CAST(updated_at AS BLOB)";

fn lossy(bytes: Option<Vec<u8>>) -> String {
    bytes
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

fn map_row(row: &SqliteRow) -> Result<WindowStateRow> {
    // A rev that isn't a whole number (hand-edited) reads as 0.
    let rev: Option<i64> = row.try_get(3).ok().flatten();
    Ok(WindowStateRow {
        window_id: lossy(row.try_get_unchecked(0)?),
        project_id: lossy(row.try_get_unchecked(1)?),
        state: stored_json(row.try_get_unchecked(2)?),
        rev: rev.unwrap_or(0).max(0) as u64,
        updated_at: lossy(row.try_get_unchecked(4)?),
    })
}

/// The window's view state of the project, or `None`.
pub async fn get(
    r: impl Into<Reader<'_>>,
    window_id: &str,
    project_id: &str,
) -> Result<Option<WindowStateRow>> {
    let mut conn = r.into().conn().await?;
    let row = db::query(&format!(
        "SELECT {COLUMNS} FROM window_state WHERE window_id = ? AND project_id = ?"
    ))
    .bind(window_id)
    .bind(project_id)
    .fetch_optional(&mut *conn)
    .await?;
    row.as_ref().map(map_row).transpose()
}

/// The stored state's size in bytes (`octet_length`, from the record
/// header, without reading the text), or `None` when the window has no row
/// for the project. Core compares a save past the web's size limit with it
/// before parsing the body.
pub async fn state_bytes(
    r: impl Into<Reader<'_>>,
    window_id: &str,
    project_id: &str,
) -> Result<Option<u64>> {
    let mut conn = r.into().conn().await?;
    let n: Option<Option<i64>> = db::query_scalar(
        "SELECT octet_length(state) FROM window_state WHERE window_id = ? AND project_id = ?",
    )
    .bind(window_id)
    .bind(project_id)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(n.map(|n| n.unwrap_or(0).max(0) as u64))
}

/// The project's most recently saved view state, whichever window saved
/// it: the last write committed (`write_seq`, one past the project's
/// highest on every write, then `rowid`), so it is what the legacy mirror
/// shows, even for saves in one millisecond (5d-2 Task 7). One step down
/// `idx_window_state_project_seq` (`?1` the project), no sort.
pub const MOST_RECENT: &str = "\
    SELECT CAST(window_id AS BLOB), CAST(project_id AS BLOB), CAST(state AS BLOB), rev, \
           CAST(updated_at AS BLOB) \
    FROM window_state WHERE project_id = ?1 ORDER BY write_seq DESC, rowid DESC LIMIT 1";

/// What a new window starts with (Decision 22): the project's most recently
/// used window's row, or `None` when no window has saved this project.
pub async fn most_recent(
    r: impl Into<Reader<'_>>,
    project_id: &str,
) -> Result<Option<WindowStateRow>> {
    let mut conn = r.into().conn().await?;
    let row = db::query(MOST_RECENT)
        .bind(project_id)
        .fetch_optional(&mut *conn)
        .await?;
    row.as_ref().map(map_row).transpose()
}

/// What [`put_if_newer`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Put {
    /// The state was stored. `false` when a save with this `rev` or a
    /// higher one was already stored: the caller answers `stale`, and
    /// writes nothing else for it (no legacy mirror).
    pub written: bool,
    /// The stored `rev` after the call: the one given when written, else
    /// the newer one that's kept, so the page can move its counter past it
    /// (or reload).
    pub rev: u64,
}

/// Stores `state` as the window's view of the project at `rev` when the
/// stored `rev` is lower, or when there's no row yet (the first save, or a
/// copy written on load, lands whatever its `rev`).
pub async fn put_if_newer(
    tx: &mut WriteTx,
    window_id: &str,
    project_id: &str,
    rev: u64,
    state: &str,
    now: &str,
) -> Result<Put> {
    let rev = i64::try_from(rev).map_err(|_| encode_error("rev is past 2^63 - 1"))?;
    let done = db::query(
        "INSERT INTO window_state (window_id, project_id, state, rev, updated_at, write_seq) \
         VALUES (?1, ?2, ?3, ?4, ?5, \
           (SELECT COALESCE(MAX(write_seq), 0) + 1 FROM window_state WHERE project_id = ?2)) \
         ON CONFLICT(window_id, project_id) DO UPDATE SET \
           state = excluded.state, rev = excluded.rev, updated_at = excluded.updated_at, \
           write_seq = excluded.write_seq \
         WHERE excluded.rev > window_state.rev",
    )
    .bind(window_id)
    .bind(project_id)
    .bind(state)
    .bind(rev)
    .bind(now)
    .execute(tx.conn())
    .await?;
    if done.rows_affected() > 0 {
        return Ok(Put {
            written: true,
            rev: rev as u64,
        });
    }
    let stored: Option<Option<i64>> = db::query_scalar(
        "SELECT CAST(rev AS INTEGER) FROM window_state WHERE window_id = ? AND project_id = ?",
    )
    .bind(window_id)
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?;
    let stored = stored.flatten();
    Ok(Put {
        written: false,
        rev: stored.unwrap_or(0).max(0) as u64,
    })
}

/// The project's view states past the `?2` most recent, at most `?4` of
/// them, except those of windows named in `?3` (a JSON array), in
/// [`MOST_RECENT`]'s order: a walk down `idx_window_state_project_seq` of
/// `?2 + ?4` entries at most.
pub const PRUNE_FOR_PROJECT: &str = "\
    DELETE FROM window_state WHERE rowid IN ( \
      SELECT rowid FROM window_state WHERE project_id = ?1 \
      ORDER BY write_seq DESC, rowid DESC LIMIT ?4 OFFSET ?2) \
    AND window_id NOT IN (SELECT value FROM json_each(?3))";

/// Keeps at most `max_states` view states of the project (the most recent),
/// never deleting one of a window named in `spare`, in one indexed
/// `DELETE` of at most [`PRUNE_BATCH`] rows (Decision 22). Returns how many
/// went. The legacy `project_state` and `tabs` rows are never pruned.
pub async fn prune_for_project(
    tx: &mut WriteTx,
    project_id: &str,
    max_states: u32,
    spare: &[&str],
) -> Result<u64> {
    let done = db::query(PRUNE_FOR_PROJECT)
        .bind(project_id)
        .bind(i64::from(max_states))
        .bind(spare_json(spare)?)
        .bind(i64::from(PRUNE_BATCH))
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}
