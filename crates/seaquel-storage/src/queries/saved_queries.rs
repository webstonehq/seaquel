//! `savedQueriesRepo`: `saved_queries`.

use crate::db;
use crate::db::SqliteRow;
use seaquel_types::storage::PersistedSavedQuery;

use super::codec::{
    begin, bit, flag, insert_sql, json, json_text, opt_text, select_sql, text, upsert_sql, Result,
    SqliteQuery,
};
use super::{IdName, RowLink, SharedLink};
use crate::{Reader, Storage, WriteTx};

const TABLE: &str = "saved_queries";
const COLUMNS: [&str; 13] = [
    "id",
    "project_id",
    "name",
    "query",
    "parameters",
    "starred",
    "shared",
    "description",
    "database_type",
    "tags",
    "folder",
    "created_at",
    "updated_at",
];

/// [`COLUMNS`] and the link path (migration `0004`), which the reads carry
/// and only [`set_link`] writes.
const READ_COLUMNS: [&str; 14] = [
    "id",
    "project_id",
    "name",
    "query",
    "parameters",
    "starred",
    "shared",
    "description",
    "database_type",
    "tags",
    "folder",
    "created_at",
    "updated_at",
    "shared_path",
];

fn map_row(row: &SqliteRow) -> Result<PersistedSavedQuery> {
    Ok(PersistedSavedQuery {
        id: text(row, "id")?,
        project_id: text(row, "project_id")?,
        name: text(row, "name")?,
        query: text(row, "query")?,
        parameters: json(row, "parameters")?,
        starred: flag(row, "starred")?,
        shared: flag(row, "shared")?,
        description: opt_text(row, "description")?,
        database_type: opt_text(row, "database_type")?,
        tags: json(row, "tags")?,
        folder: opt_text(row, "folder")?,
        created_at: text(row, "created_at")?,
        updated_at: text(row, "updated_at")?,
        shared_path: opt_text(row, "shared_path")?,
    })
}

/// A project's saved queries, in rowid order.
pub async fn load_by_project(st: &Storage, project_id: &str) -> Result<Vec<PersistedSavedQuery>> {
    let rows = db::query(&select_sql(TABLE, &READ_COLUMNS, "project_id = ?"))
        .bind(project_id)
        .fetch_all(st.pool())
        .await?;
    rows.iter().map(map_row).collect()
}

/// A project's saved queries, in rowid order, read through `r` (inside a
/// write, the transaction): the rows a shared project's sync plans over.
pub async fn list(r: impl Into<Reader<'_>>, project_id: &str) -> Result<Vec<PersistedSavedQuery>> {
    let mut conn = r.into().conn().await?;
    let rows = db::query(&select_sql(TABLE, &READ_COLUMNS, "project_id = ?"))
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?;
    rows.iter().map(map_row).collect()
}

/// One saved query, as [`load_by_project`] gives it, or `None`.
pub async fn get(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<PersistedSavedQuery>> {
    let mut conn = r.into().conn().await?;
    let row = db::query(&select_sql(TABLE, &READ_COLUMNS, "id = ?"))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    row.as_ref().map(map_row).transpose()
}

/// Binds `name` through `updated_at` ([`COLUMNS`] after `id` and
/// `project_id`), in order.
fn bind_fields<'q>(q: SqliteQuery<'q>, sq: &'q PersistedSavedQuery) -> SqliteQuery<'q> {
    q.bind(&sq.name)
        .bind(&sq.query)
        .bind(json_text(&sq.parameters))
        .bind(bit(sq.starred))
        .bind(bit(sq.shared))
        .bind(&sq.description)
        .bind(&sq.database_type)
        .bind(json_text(&sq.tags))
        .bind(&sq.folder)
        .bind(&sq.created_at)
        .bind(&sq.updated_at)
}

/// Inserts a new saved query. An id that exists fails (the primary key)
/// rather than overwriting.
pub async fn insert(tx: &mut WriteTx, q: &PersistedSavedQuery) -> Result<()> {
    let sql = insert_sql(TABLE, &COLUMNS);
    let conn = tx.conn();
    bind_fields(db::query(&sql).bind(&q.id).bind(&q.project_id), q)
        .execute(&mut *conn)
        .await?;
    super::set_name_key(conn, TABLE, &q.id, &q.name).await
}

/// Writes every field of an existing saved query but `project_id`,
/// `created_at` and its link: a saved query never moves, keeps when it was
/// made, and its link changes only through [`set_link`].
/// `false` when there's no saved query with that id.
pub async fn update(tx: &mut WriteTx, q: &PersistedSavedQuery) -> Result<bool> {
    let conn = tx.conn();
    let done = db::query(
        "UPDATE saved_queries SET name = ?, query = ?, parameters = ?, starred = ?, shared = ?, \
         description = ?, database_type = ?, tags = ?, folder = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&q.name)
    .bind(&q.query)
    .bind(json_text(&q.parameters))
    .bind(bit(q.starred))
    .bind(bit(q.shared))
    .bind(&q.description)
    .bind(&q.database_type)
    .bind(json_text(&q.tags))
    .bind(&q.folder)
    .bind(&q.updated_at)
    .bind(&q.id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    super::set_name_key(conn, TABLE, &q.id, &q.name).await?;
    Ok(true)
}

/// Deletes one saved query; its versions cascade. `false` when there was no
/// such query.
pub async fn delete(tx: &mut WriteTx, id: &str) -> Result<bool> {
    let done = db::query("DELETE FROM saved_queries WHERE id = ?")
        .bind(id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// The ids and names of a project's saved queries in `folder`, in rowid
/// order. No folder (NULL) and an empty one (`""`) are the same folder.
/// Names come as stored: comparing them (trimmed, case-folded) is Core's.
pub async fn names_in_folder(
    r: impl Into<Reader<'_>>,
    project_id: &str,
    folder: Option<&str>,
) -> Result<Vec<IdName>> {
    let mut conn = r.into().conn().await?;
    let rows: Vec<(Option<String>, Option<String>)> = db::query_as(
        "SELECT id, name FROM saved_queries \
         WHERE project_id = ? AND COALESCE(folder, '') = COALESCE(?, '') ORDER BY rowid",
    )
    .bind(project_id)
    .bind(folder)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| IdName {
            id: id.unwrap_or_default(),
            name: name.unwrap_or_default(),
        })
        .collect())
}

/// The `name_key` lookup behind [`with_name_key_in_folder`]: two searches
/// of `idx_saved_queries_name_key` (`?1` the project, `?2` the folder, `?3`
/// the key).
pub const NAME_KEY_LOOKUP: &str = "\
    SELECT rowid, id, name, name_key FROM saved_queries \
    WHERE project_id = ?1 AND COALESCE(folder, '') = COALESCE(?2, '') AND name_key = ?3 \
    UNION ALL \
    SELECT rowid, id, name, name_key FROM saved_queries \
    WHERE project_id = ?1 AND COALESCE(folder, '') = COALESCE(?2, '') AND name_key IS NULL \
    ORDER BY 1";

/// The ids and names of a project's saved queries in `folder` whose name
/// has `key`, in rowid order (see `connections::with_name_key`). No folder
/// (NULL) and an empty one (`""`) are the same folder.
pub async fn with_name_key_in_folder(
    r: impl Into<Reader<'_>>,
    project_id: &str,
    folder: Option<&str>,
    key: &str,
) -> Result<Vec<IdName>> {
    let mut conn = r.into().conn().await?;
    let rows = db::query_as(NAME_KEY_LOOKUP)
        .bind(project_id)
        .bind(folder)
        .bind(key)
        .fetch_all(&mut *conn)
        .await?;
    Ok(super::matching_key(rows, key))
}

/// [`set_link`]'s statement: by primary key.
pub const SET_LINK: &str = "UPDATE saved_queries SET shared_path = ?1, shared_base = ?2, \
     shared_file_id = ?3 WHERE id = ?4";

/// Stores a saved query's [`SharedLink`] (all three columns; `None`
/// clears one), and nothing else. `false` when there's no saved query with
/// that id.
pub async fn set_link(tx: &mut WriteTx, id: &str, link: &SharedLink) -> Result<bool> {
    super::set_link(tx.conn(), SET_LINK, id, link).await
}

/// [`link`]'s query: by primary key.
pub const LINK: &str =
    "SELECT shared_path, shared_base, shared_file_id FROM saved_queries WHERE id = ?1";

/// One saved query's [`SharedLink`], or `None` when there's no such row.
pub async fn link(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<SharedLink>> {
    let mut conn = r.into().conn().await?;
    super::link_of(&mut conn, LINK, id).await
}

/// [`links`]' query: `idx_saved_queries_project` (`?1` the project).
pub const LINKS: &str = "SELECT id, shared_path, shared_base, shared_file_id \
     FROM saved_queries WHERE project_id = ?1 ORDER BY rowid";

/// The link of every saved query in a project, shared or not, in rowid
/// order.
pub async fn links(r: impl Into<Reader<'_>>, project_id: &str) -> Result<Vec<RowLink>> {
    let mut conn = r.into().conn().await?;
    let rows = db::query_as(LINKS)
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?;
    Ok(super::row_links(rows))
}

/// [`by_shared_path`]'s query: one search of
/// `idx_saved_queries_shared_path` (`?1` the project, `?2` the path).
pub const BY_SHARED_PATH: &str = "SELECT id FROM saved_queries \
     WHERE project_id = ?1 AND shared_path = ?2 AND id IS NOT NULL ORDER BY rowid";

/// The ids of a project's saved queries whose file is `path`, compared
/// exactly, in rowid order. Usually one; nothing stops a hand-edited file
/// from holding two.
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

/// How many saved queries the file holds, in every project.
pub async fn count(r: impl Into<Reader<'_>>) -> Result<u64> {
    let mut conn = r.into().conn().await?;
    let n: i64 = db::query_scalar("SELECT COUNT(*) FROM saved_queries")
        .fetch_one(&mut *conn)
        .await?;
    Ok(n.max(0) as u64)
}

/// Makes `queries` the project's saved queries, in one transaction: the
/// project's other queries are deleted (their versions cascade), and each
/// of `queries` is upserted by id, so a kept query keeps its versions. An id
/// that belongs to another project moves to this one.
pub async fn save_all(
    st: &Storage,
    project_id: &str,
    queries: &[PersistedSavedQuery],
) -> Result<()> {
    let mut tx = begin(st).await?;
    if queries.is_empty() {
        db::query("DELETE FROM saved_queries WHERE project_id = ?")
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
    } else {
        let placeholders = vec!["?"; queries.len()].join(",");
        let sql = format!(
            "DELETE FROM saved_queries WHERE project_id = ? AND id NOT IN ({placeholders})"
        );
        let mut delete = db::query(&sql).bind(project_id);
        for q in queries {
            delete = delete.bind(&q.id);
        }
        delete.execute(&mut *tx).await?;
    }
    let upsert = upsert_sql(TABLE, &COLUMNS, "id");
    for q in queries {
        bind_fields(db::query(&upsert).bind(&q.id).bind(&q.project_id), q)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Deletes a project's saved queries. Their versions cascade.
pub async fn remove_by_project(st: &Storage, project_id: &str) -> Result<()> {
    db::query("DELETE FROM saved_queries WHERE project_id = ?")
        .bind(project_id)
        .execute(st.pool())
        .await?;
    Ok(())
}
