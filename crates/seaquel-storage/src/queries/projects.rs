//! `projectsRepo`: `projects` and their `project_labels`.

use crate::db;
use crate::db::SqliteRow;
use db::SqliteConnection;
use seaquel_types::storage::{ConnectionLabel, PersistedProject};

use super::codec::{begin, insert_sql, opt_text, select_sql, text, upsert_sql, Result};
use super::{connections, project_labels};
use crate::{Reader, Storage, WriteTx};

const TABLE: &str = "projects";
const COLUMNS: [&str; 6] = [
    "id",
    "name",
    "description",
    "created_at",
    "updated_at",
    "git_repo_path",
];

fn map_row(
    row: &SqliteRow,
    id: String,
    custom_labels: Vec<ConnectionLabel>,
) -> Result<PersistedProject> {
    Ok(PersistedProject {
        id,
        name: text(row, "name")?,
        description: opt_text(row, "description")?,
        created_at: text(row, "created_at")?,
        updated_at: text(row, "updated_at")?,
        custom_labels,
        git_repo_path: opt_text(row, "git_repo_path")?,
    })
}

/// Every project with its labels, in rowid order.
pub async fn load_all(st: &Storage) -> Result<Vec<PersistedProject>> {
    let rows = db::query(&select_sql(TABLE, &COLUMNS, ""))
        .fetch_all(st.pool())
        .await?;
    let mut conn = st.pool().acquire().await?;
    let mut projects = Vec::with_capacity(rows.len());
    for row in &rows {
        let id = text(row, "id")?;
        let labels = project_labels::of_project(&mut conn, &id).await?;
        projects.push(map_row(row, id, labels)?);
    }
    Ok(projects)
}

/// One project with its labels, as [`load_all`] gives it, or `None`.
pub async fn get(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<PersistedProject>> {
    let mut conn = r.into().conn().await?;
    let Some(row) = db::query(&select_sql(TABLE, &COLUMNS, "id = ?"))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
    else {
        return Ok(None);
    };
    let labels = project_labels::of_project(&mut conn, id).await?;
    map_row(&row, id.to_string(), labels).map(Some)
}

/// Upserts the project and replaces its labels.
pub async fn save(st: &Storage, project: &PersistedProject) -> Result<()> {
    let mut tx = begin(st).await?;
    save_one(&mut tx, project).await?;
    tx.commit().await?;
    Ok(())
}

/// [`save`] for each project, in one transaction.
pub async fn save_all(st: &Storage, projects: &[PersistedProject]) -> Result<()> {
    let mut tx = begin(st).await?;
    for project in projects {
        save_one(&mut tx, project).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn save_one(conn: &mut SqliteConnection, p: &PersistedProject) -> Result<()> {
    db::query(&upsert_sql(TABLE, &COLUMNS, "id"))
        .bind(&p.id)
        .bind(&p.name)
        .bind(&p.description)
        .bind(&p.created_at)
        .bind(&p.updated_at)
        .bind(&p.git_repo_path)
        .execute(&mut *conn)
        .await?;
    project_labels::replace_all(conn, &p.id, &p.custom_labels).await
}

/// Inserts a new project and its labels. An id that exists fails (the
/// primary key) rather than overwriting.
pub async fn insert(tx: &mut WriteTx, p: &PersistedProject) -> Result<()> {
    let conn = tx.conn();
    insert_row(conn, p, "").await?;
    super::set_name_key(conn, TABLE, &p.id, &p.name).await?;
    for label in &p.custom_labels {
        project_labels::insert_row(conn, &p.id, label).await?;
    }
    Ok(())
}

/// [`insert`], unless a project with that id exists (the default project,
/// `default-seaquel`): then nothing is written. Whether it inserted.
pub async fn insert_if_missing(tx: &mut WriteTx, p: &PersistedProject) -> Result<bool> {
    let conn = tx.conn();
    if !insert_row(conn, p, " ON CONFLICT(id) DO NOTHING").await? {
        return Ok(false);
    }
    super::set_name_key(conn, TABLE, &p.id, &p.name).await?;
    for label in &p.custom_labels {
        project_labels::insert_row(conn, &p.id, label).await?;
    }
    Ok(true)
}

async fn insert_row(conn: &mut SqliteConnection, p: &PersistedProject, tail: &str) -> Result<bool> {
    let done = db::query(&format!("{}{tail}", insert_sql(TABLE, &COLUMNS)))
        .bind(&p.id)
        .bind(&p.name)
        .bind(&p.description)
        .bind(&p.created_at)
        .bind(&p.updated_at)
        .bind(&p.git_repo_path)
        .execute(&mut *conn)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Writes the project's own columns (`name`, `description`, `updated_at`,
/// `git_repo_path`). Its labels (`project_labels`) and `created_at` are left
/// alone. `false` when there's no project with that id.
pub async fn update(tx: &mut WriteTx, p: &PersistedProject) -> Result<bool> {
    let conn = tx.conn();
    let done = db::query(
        "UPDATE projects SET name = ?, description = ?, updated_at = ?, git_repo_path = ? \
         WHERE id = ?",
    )
    .bind(&p.name)
    .bind(&p.description)
    .bind(&p.updated_at)
    .bind(&p.git_repo_path)
    .bind(&p.id)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    super::set_name_key(conn, TABLE, &p.id, &p.name).await?;
    Ok(true)
}

/// Deletes the project and everything in it, in the caller's transaction:
/// first its saved queries, dashboards and saved workflows by `project_id`
/// (on files that started on v2026.4.5-beta.1, `saved_queries` and
/// `dashboards` have no foreign key, so the project's cascade misses them),
/// then the project, whose cascade takes its labels, connections (with
/// their history, chats and labels), state and tabs. The versions of the
/// saved queries and dashboards cascade from them.
///
/// Returns the ids of the connections it removed, so Core can delete their
/// secrets after the commit, or `None` when there's no such project
/// (nothing is written).
pub async fn delete_with_orphans(tx: &mut WriteTx, id: &str) -> Result<Option<Vec<String>>> {
    let conn = tx.conn();
    let exists: Option<(i64,)> = db::query_as("SELECT 1 FROM projects WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    if exists.is_none() {
        return Ok(None);
    }
    let connection_ids = connections::ids_of_project(conn, id).await?;
    for table in ["saved_queries", "dashboards", "saved_canvases"] {
        db::query(&format!("DELETE FROM {table} WHERE project_id = ?"))
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }
    db::query("DELETE FROM projects WHERE id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(Some(connection_ids))
}

/// The ids and names of every project, in rowid order.
pub async fn names(r: impl Into<Reader<'_>>) -> Result<Vec<super::IdName>> {
    let mut conn = r.into().conn().await?;
    let rows: Vec<(Option<String>, Option<String>)> =
        db::query_as("SELECT id, name FROM projects ORDER BY rowid")
            .fetch_all(&mut *conn)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| super::IdName {
            id: id.unwrap_or_default(),
            name: name.unwrap_or_default(),
        })
        .collect())
}

/// The `name_key` lookup behind [`with_name_key`]: two searches of
/// `idx_projects_name_key` (`?1` the key).
pub const NAME_KEY_LOOKUP: &str = "\
    SELECT rowid, id, name, name_key FROM projects WHERE name_key = ?1 \
    UNION ALL \
    SELECT rowid, id, name, name_key FROM projects WHERE name_key IS NULL \
    ORDER BY 1";

/// The ids and names of the projects whose name has `key`, in rowid order
/// (see `connections::with_name_key`).
pub async fn with_name_key(r: impl Into<Reader<'_>>, key: &str) -> Result<Vec<super::IdName>> {
    let mut conn = r.into().conn().await?;
    let rows = db::query_as(NAME_KEY_LOOKUP)
        .bind(key)
        .fetch_all(&mut *conn)
        .await?;
    Ok(super::matching_key(rows, key))
}

/// [`set_shared_dir`]'s statement: by primary key.
pub const SET_SHARED_DIR: &str = "UPDATE projects SET shared_dir = ?1 WHERE id = ?2";

/// Stores the project's directory under `.seaquel/projects/` in its repo
/// (migration `0004`; `None` clears it), and nothing else, so a rename
/// never moves it (Q25). `false` when there's no project with that id.
pub async fn set_shared_dir(tx: &mut WriteTx, id: &str, dir: Option<&str>) -> Result<bool> {
    let done = db::query(SET_SHARED_DIR)
        .bind(dir)
        .bind(id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// The project's stored directory in its repo. `None` when it has none
/// (the slug of its name then) or there's no such project.
pub async fn shared_dir(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<String>> {
    let mut conn = r.into().conn().await?;
    let dir: Option<Option<String>> =
        db::query_scalar("SELECT shared_dir FROM projects WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(dir.flatten())
}

/// The ids of the projects linked to the repo at `path` (`git_repo_path`,
/// compared exactly), in rowid order. Few rows, so a scan.
pub async fn ids_with_repo_path(r: impl Into<Reader<'_>>, path: &str) -> Result<Vec<String>> {
    let mut conn = r.into().conn().await?;
    Ok(db::query_scalar(
        "SELECT id FROM projects WHERE git_repo_path = ? AND id IS NOT NULL ORDER BY rowid",
    )
    .bind(path)
    .fetch_all(&mut *conn)
    .await?)
}

/// How many projects the file holds.
pub async fn count(r: impl Into<Reader<'_>>) -> Result<u64> {
    let mut conn = r.into().conn().await?;
    let n: i64 = db::query_scalar("SELECT COUNT(*) FROM projects")
        .fetch_one(&mut *conn)
        .await?;
    Ok(n.max(0) as u64)
}

/// Deletes the project. Its labels, connections, state, tabs, canvases,
/// saved queries and dashboards cascade (except on files that started on
/// v2026.4.5-beta.1, whose `saved_queries` and `dashboards` have no foreign
/// key).
pub async fn remove(st: &Storage, project_id: &str) -> Result<()> {
    db::query("DELETE FROM projects WHERE id = ?")
        .bind(project_id)
        .execute(st.pool())
        .await?;
    Ok(())
}
