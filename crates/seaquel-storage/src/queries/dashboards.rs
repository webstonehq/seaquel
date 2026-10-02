//! `dashboardsRepo`: `dashboards`.

use crate::db;
use crate::db::SqliteRow;
use seaquel_types::storage::PersistedDashboard;

use super::codec::{bit, flag, insert_sql, opt_text, select_sql, text, upsert_sql, Result};
use super::{IdName, RowLink, SharedLink};
use crate::{Reader, Storage, WriteTx};

const TABLE: &str = "dashboards";
const COLUMNS: [&str; 11] = [
    "id",
    "project_id",
    "name",
    "viewport",
    "widgets",
    "date_filter",
    "starred",
    "shared",
    "description",
    "created_at",
    "updated_at",
];

/// [`COLUMNS`] and the link path (migration `0004`), which the reads carry
/// and only [`set_link`] writes.
const READ_COLUMNS: [&str; 12] = [
    "id",
    "project_id",
    "name",
    "viewport",
    "widgets",
    "date_filter",
    "starred",
    "shared",
    "description",
    "created_at",
    "updated_at",
    "shared_path",
];

fn map_row(row: &SqliteRow) -> Result<PersistedDashboard> {
    Ok(PersistedDashboard {
        id: text(row, "id")?,
        project_id: text(row, "project_id")?,
        name: text(row, "name")?,
        viewport: text(row, "viewport")?,
        widgets: text(row, "widgets")?,
        date_filter: opt_text(row, "date_filter")?,
        starred: flag(row, "starred")?,
        shared: flag(row, "shared")?,
        description: opt_text(row, "description")?,
        created_at: text(row, "created_at")?,
        updated_at: text(row, "updated_at")?,
        shared_path: opt_text(row, "shared_path")?,
    })
}

/// A project's dashboards, in rowid order.
pub async fn load_by_project(st: &Storage, project_id: &str) -> Result<Vec<PersistedDashboard>> {
    list(st, project_id).await
}

/// [`load_by_project`] on the pool or inside a write. A NULL `starred`
/// (every file can hold one) reads as false; on a beta-era file a NULL
/// `project_id` reads as `""`, so it's in no project's list. A row with a
/// value that doesn't decode (text that isn't UTF-8, from a hand-edited
/// file) is skipped rather than failing the list.
pub async fn list(r: impl Into<Reader<'_>>, project_id: &str) -> Result<Vec<PersistedDashboard>> {
    let mut conn = r.into().conn().await?;
    let rows = db::query(&select_sql(
        TABLE,
        &READ_COLUMNS,
        "project_id = ? ORDER BY rowid",
    ))
    .bind(project_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.iter().filter_map(|row| map_row(row).ok()).collect())
}

/// One dashboard, as [`list`] gives it, or `None` (also for a row with a
/// value that doesn't decode, which no list shows either).
pub async fn get(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<PersistedDashboard>> {
    let mut conn = r.into().conn().await?;
    let row = db::query(&select_sql(TABLE, &READ_COLUMNS, "id = ?"))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.as_ref().and_then(|row| map_row(row).ok()))
}

/// Inserts a new dashboard with its name key. An id that exists fails (the
/// primary key) rather than overwriting.
pub async fn insert(tx: &mut WriteTx, d: &PersistedDashboard) -> Result<()> {
    let conn = tx.conn();
    db::query(&insert_sql(TABLE, &COLUMNS))
        .bind(&d.id)
        .bind(&d.project_id)
        .bind(&d.name)
        .bind(&d.viewport)
        .bind(&d.widgets)
        .bind(&d.date_filter)
        .bind(bit(d.starred))
        .bind(bit(d.shared))
        .bind(&d.description)
        .bind(&d.created_at)
        .bind(&d.updated_at)
        .execute(&mut *conn)
        .await?;
    super::set_name_key(conn, TABLE, &d.id, &d.name).await
}

/// Writes every field of an existing dashboard but `project_id`,
/// `created_at` and its link (a dashboard never moves, keeps when it was
/// made, and its link changes only through [`set_link`]), and its name
/// key. `false` when there's no dashboard with that id: an update never
/// re-inserts a deleted one.
pub async fn update(tx: &mut WriteTx, d: &PersistedDashboard) -> Result<bool> {
    let conn = tx.conn();
    let done = db::query(
        "UPDATE dashboards SET name = ?, viewport = ?, widgets = ?, date_filter = ?, starred = ?, \
         shared = ?, description = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&d.name)
    .bind(&d.viewport)
    .bind(&d.widgets)
    .bind(&d.date_filter)
    .bind(bit(d.starred))
    .bind(bit(d.shared))
    .bind(&d.description)
    .bind(&d.updated_at)
    .bind(&d.id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    super::set_name_key(conn, TABLE, &d.id, &d.name).await?;
    Ok(true)
}

/// Deletes one dashboard and its versions. The versions are deleted
/// explicitly, not left to the foreign key's cascade, so they go on every
/// file shape.
/// `false` when there was no such dashboard.
pub async fn delete(tx: &mut WriteTx, id: &str) -> Result<bool> {
    let conn = tx.conn();
    db::query("DELETE FROM dashboard_versions WHERE dashboard_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let done = db::query("DELETE FROM dashboards WHERE id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// The `name_key` lookup behind [`with_name_key`]: two searches of
/// `idx_dashboards_name_key` (`?1` the project, `?2` the key).
pub const NAME_KEY_LOOKUP: &str = "\
    SELECT rowid, id, name, name_key FROM dashboards WHERE project_id = ?1 AND name_key = ?2 \
    UNION ALL \
    SELECT rowid, id, name, name_key FROM dashboards WHERE project_id = ?1 AND name_key IS NULL \
    ORDER BY 1";

/// The ids and names of a project's dashboards whose name has `key`
/// (`seaquel_types::names::name_key`), in rowid order: Core's duplicate
/// check (Decision 21). Rows with no stored key (an older release wrote or
/// renamed them) are read too and compared by their name; a name that
/// isn't UTF-8 matches nothing.
pub async fn with_name_key(
    r: impl Into<Reader<'_>>,
    project_id: &str,
    key: &str,
) -> Result<Vec<IdName>> {
    let mut conn = r.into().conn().await?;
    let rows = db::query_as(NAME_KEY_LOOKUP)
        .bind(project_id)
        .bind(key)
        .fetch_all(&mut *conn)
        .await?;
    Ok(super::matching_key(rows, key))
}

/// [`set_link`]'s statement: by primary key.
pub const SET_LINK: &str = "UPDATE dashboards SET shared_path = ?1, shared_base = ?2, \
     shared_file_id = ?3 WHERE id = ?4";

/// Stores a dashboard's [`SharedLink`] (all three columns; `None` clears
/// one), and nothing else. `false` when there's no dashboard with that id.
pub async fn set_link(tx: &mut WriteTx, id: &str, link: &SharedLink) -> Result<bool> {
    super::set_link(tx.conn(), SET_LINK, id, link).await
}

/// [`link`]'s query: by primary key.
pub const LINK: &str =
    "SELECT shared_path, shared_base, shared_file_id FROM dashboards WHERE id = ?1";

/// One dashboard's [`SharedLink`], or `None` when there's no such row.
pub async fn link(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<SharedLink>> {
    let mut conn = r.into().conn().await?;
    super::link_of(&mut conn, LINK, id).await
}

/// [`links`]' query (`?1` the project), through the dashboards'
/// `project_id` indexes.
pub const LINKS: &str = "SELECT id, shared_path, shared_base, shared_file_id \
     FROM dashboards WHERE project_id = ?1 ORDER BY rowid";

/// The link of every dashboard in a project, shared or not, in rowid
/// order. On a beta-era file a dashboard with a NULL `project_id` is in no
/// project's list, as in [`list`].
pub async fn links(r: impl Into<Reader<'_>>, project_id: &str) -> Result<Vec<RowLink>> {
    let mut conn = r.into().conn().await?;
    let rows = db::query_as(LINKS)
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?;
    Ok(super::row_links(rows))
}

/// [`by_shared_path`]'s query: one search of `idx_dashboards_shared_path`
/// (`?1` the project, `?2` the path).
pub const BY_SHARED_PATH: &str = "SELECT id FROM dashboards \
     WHERE project_id = ?1 AND shared_path = ?2 AND id IS NOT NULL ORDER BY rowid";

/// The ids of a project's dashboards whose file is `path`, compared
/// exactly, in rowid order.
pub async fn by_shared_path(
    r: impl Into<Reader<'_>>,
    project_id: &str,
    path: &str,
) -> Result<Vec<String>> {
    let mut conn = r.into().conn().await?;
    Ok(db::query_scalar(BY_SHARED_PATH)
        .bind(project_id)
        .bind(path)
        .fetch_all(&mut *conn)
        .await?)
}

/// How many dashboards the file holds, in every project.
pub async fn count(r: impl Into<Reader<'_>>) -> Result<u64> {
    let mut conn = r.into().conn().await?;
    let n: i64 = db::query_scalar("SELECT COUNT(*) FROM dashboards")
        .fetch_one(&mut *conn)
        .await?;
    Ok(n.max(0) as u64)
}

/// Upserts a dashboard.
pub async fn save(st: &Storage, d: &PersistedDashboard) -> Result<()> {
    db::query(&upsert_sql(TABLE, &COLUMNS, "id"))
        .bind(&d.id)
        .bind(&d.project_id)
        .bind(&d.name)
        .bind(&d.viewport)
        .bind(&d.widgets)
        .bind(&d.date_filter)
        .bind(bit(d.starred))
        .bind(bit(d.shared))
        .bind(&d.description)
        .bind(&d.created_at)
        .bind(&d.updated_at)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Deletes a dashboard. Its versions cascade.
pub async fn remove(st: &Storage, id: &str) -> Result<()> {
    db::query("DELETE FROM dashboards WHERE id = ?")
        .bind(id)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Deletes a project's dashboards.
pub async fn remove_by_project(st: &Storage, project_id: &str) -> Result<()> {
    db::query("DELETE FROM dashboards WHERE project_id = ?")
        .bind(project_id)
        .execute(st.pool())
        .await?;
    Ok(())
}
