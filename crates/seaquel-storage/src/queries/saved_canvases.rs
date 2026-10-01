//! `saved_canvases`: saved workflows, one row each (phase 5d-2, Decision
//! 23). Until 5d-2 they were written only by `project_state::save`, which
//! replaced a project's whole list; that function keeps doing so for its
//! frozen fixtures. Core now writes them one at a time here.
//!
//! `data` is today's `SavedWorkflow` JSON, stored and read byte for byte;
//! storage never parses more than whether it is JSON, and its `name` and
//! times for `meta` (migration `0003`), which `workflowsList` answers
//! instead of the bodies ([`list_meta`], phase 5d-2 Task 7).

use seaquel_types::storage::PersistedWorkflowMeta;
use serde::Deserialize;
use serde_json::value::RawValue;
use sqlx::Row;

use super::codec::{stored_json, Result};
use crate::{Reader, WriteTx};

/// A stored workflow row. `data` is `None` when the stored text isn't
/// JSON, is `null` or isn't UTF-8: the row exists (an update can replace
/// it, a remove deletes it), but no list shows it, as today's load skipped
/// it.
#[derive(Debug, Clone)]
pub struct WorkflowRow {
    pub id: String,
    pub project_id: String,
    pub data: Option<Box<RawValue>>,
}

/// The list behind [`list`]: `idx_saved_canvases_project` (migration
/// `0002`), in rowid order (`?1` the project).
pub const LIST: &str = "SELECT CAST(data AS BLOB) FROM saved_canvases \
                        WHERE project_id = ?1 ORDER BY rowid";

/// A workflow's `meta` from its `data` column: `{name, createdAt,
/// updatedAt}` as JSON, each the stored JSON's field when it is text, else
/// `null`; `{}` for JSON that isn't an object; NULL for a row that doesn't
/// read (not text, not JSON, or `null`), which no list shows. [`insert`]
/// and [`update`] write it with the data, migration `0003` filled it for
/// the rows before it, and [`list_meta`] falls back to it for a row an
/// older release wrote (NULL `meta`). The `CASE`s make sure a JSON function
/// only ever sees text `json_valid` accepted.
pub const META_OF_DATA: &str = "CASE WHEN typeof(data) = 'text' AND json_valid(data) THEN \
    CASE json_type(data) WHEN 'null' THEN NULL WHEN 'object' THEN json_object( \
      'name', CASE WHEN json_type(data, '$.name') = 'text' THEN data ->> '$.name' END, \
      'createdAt', CASE WHEN json_type(data, '$.createdAt') = 'text' \
        THEN data ->> '$.createdAt' END, \
      'updatedAt', CASE WHEN json_type(data, '$.updatedAt') = 'text' \
        THEN data ->> '$.updatedAt' END) \
    ELSE '{}' END END";

/// `meta` of a row whose body doesn't read (not text, not JSON, `null`):
/// known, so no refill looks at it again, and listed by nothing.
pub const UNREADABLE: &str = "null";

/// Fills `meta` of every row an older release wrote without it (5d-2 Task 7
/// review); the ones that don't read get [`UNREADABLE`].
pub(crate) fn refill_sql() -> String {
    format!(
        "UPDATE saved_canvases SET meta = COALESCE({META_OF_DATA}, '{UNREADABLE}') \
         WHERE meta IS NULL"
    )
}

/// [`list_meta`]'s query (`?1` the project): `idx_saved_canvases_project`,
/// in rowid order, reading each body's size from its record header and the
/// body itself only for a row with no `meta`.
pub fn list_meta_sql() -> String {
    format!(
        "SELECT CAST(id AS BLOB), CAST(project_id AS BLOB), \
           CAST(COALESCE(meta, {META_OF_DATA}) AS BLOB), \
           COALESCE(octet_length(data), 0) \
         FROM saved_canvases WHERE project_id = ?1 ORDER BY rowid"
    )
}

/// [`list_meta_sql`]'s row: id, project and the `meta` JSON as bytes, the
/// body's size. `meta` is bytes because it can hold text that isn't UTF-8:
/// `->>` returns an escaped lone surrogate as CESU-8 (`"\ud800"` gives
/// `ED A0 80`) and `json_object` copies a stored non-UTF-8 byte through, so
/// reading it as a string would fail the whole list (5d-2 Task 7 review).
type MetaRow = (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>, i64);

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Meta {
    name: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

/// A project's saved workflows without their bodies (`workflowsList`,
/// phase 5d-2 Task 7): id, project, name, times and size, in rowid order,
/// the rows that don't read left out (as [`list`] leaves them out).
pub async fn list_meta(
    r: impl Into<Reader<'_>>,
    project_id: &str,
) -> Result<Vec<PersistedWorkflowMeta>> {
    let mut conn = r.into().conn().await?;
    let rows: Vec<MetaRow> = sqlx::query_as(&list_meta_sql())
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, project, meta, bytes)| {
            let meta = meta?;
            if meta == UNREADABLE.as_bytes() {
                return None;
            }
            // Lossy: a bad byte becomes U+FFFD, which stays valid JSON.
            let meta: Meta =
                serde_json::from_str(&String::from_utf8_lossy(&meta)).unwrap_or_default();
            Some(PersistedWorkflowMeta {
                id: lossy(id),
                project_id: lossy(project),
                name: meta.name.unwrap_or_default(),
                created_at: meta.created_at,
                updated_at: meta.updated_at,
                bytes: bytes.max(0) as u64,
            })
        })
        .collect())
}

/// A project's saved workflows as their stored JSON, in rowid order,
/// without the rows that don't read (see [`WorkflowRow`]).
pub async fn list(r: impl Into<Reader<'_>>, project_id: &str) -> Result<Vec<Box<RawValue>>> {
    let mut conn = r.into().conn().await?;
    let rows: Vec<(Option<Vec<u8>>,)> = sqlx::query_as(LIST)
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(data,)| stored_json(data))
        .collect())
}

/// One saved workflow, or `None` when there's no row with that id.
pub async fn get(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<WorkflowRow>> {
    let mut conn = r.into().conn().await?;
    let row = sqlx::query(
        "SELECT id, project_id, CAST(data AS BLOB) AS data FROM saved_canvases WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(WorkflowRow {
        id: id.to_string(),
        project_id: lossy(row.try_get_unchecked(1)?),
        data: stored_json(row.try_get_unchecked(2)?),
    }))
}

/// A text column read as bytes, for a value only compared or echoed back.
fn lossy(bytes: Option<Vec<u8>>) -> String {
    bytes
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// Inserts a new saved workflow. An id that exists fails (the primary
/// key: `saved_canvases.id` is global) rather than overwriting.
pub async fn insert(tx: &mut WriteTx, id: &str, project_id: &str, data: &str) -> Result<()> {
    let conn = tx.conn();
    sqlx::query("INSERT INTO saved_canvases (id, project_id, data) VALUES (?, ?, ?)")
        .bind(id)
        .bind(project_id)
        .bind(data)
        .execute(&mut *conn)
        .await?;
    set_meta(conn, id).await
}

/// Writes the row's `meta` from its data, in a statement of its own (as
/// `0001`'s name keys are), so `saved_canvases_meta_stale` never clears it.
async fn set_meta(conn: &mut sqlx::SqliteConnection, id: &str) -> Result<()> {
    sqlx::query(&format!(
        "UPDATE saved_canvases SET meta = COALESCE({META_OF_DATA}, '{UNREADABLE}') WHERE id = ?"
    ))
    .bind(id)
    .execute(conn)
    .await?;
    Ok(())
}

/// Replaces an existing workflow's data; it never moves to another
/// project. `false` when there's no row with that id.
pub async fn update(tx: &mut WriteTx, id: &str, data: &str) -> Result<bool> {
    let conn = tx.conn();
    let done = sqlx::query("UPDATE saved_canvases SET data = ? WHERE id = ?")
        .bind(data)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    set_meta(conn, id).await?;
    Ok(true)
}

/// Deletes one saved workflow. `false` when there was none.
pub async fn delete(tx: &mut WriteTx, id: &str) -> Result<bool> {
    let done = sqlx::query("DELETE FROM saved_canvases WHERE id = ?")
        .bind(id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// How many saved workflows the file holds, in every project, the rows
/// that don't read included.
pub async fn count(r: impl Into<Reader<'_>>) -> Result<u64> {
    let mut conn = r.into().conn().await?;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM saved_canvases")
        .fetch_one(&mut *conn)
        .await?;
    Ok(n.max(0) as u64)
}
