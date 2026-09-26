//! `projectsRepo`: `projects` and their `project_labels`.

use seaquel_types::storage::{ConnectionLabel, PersistedProject};
use sqlx::SqliteConnection;

use super::codec::{begin, bit, flag, opt_text, select_sql, text, upsert_sql, Result};
use crate::Storage;

const TABLE: &str = "projects";
const COLUMNS: [&str; 6] = [
    "id",
    "name",
    "description",
    "created_at",
    "updated_at",
    "git_repo_path",
];

/// Every project with its labels, in rowid order.
pub async fn load_all(st: &Storage) -> Result<Vec<PersistedProject>> {
    let rows = sqlx::query(&select_sql(TABLE, &COLUMNS, ""))
        .fetch_all(st.pool())
        .await?;
    let mut projects = Vec::with_capacity(rows.len());
    for row in &rows {
        let id = text(row, "id")?;
        let labels = sqlx::query(
            "SELECT id, name, is_predefined, color FROM project_labels WHERE project_id = ?",
        )
        .bind(&id)
        .fetch_all(st.pool())
        .await?;
        let custom_labels = labels
            .iter()
            .map(|l| {
                Ok(ConnectionLabel {
                    id: text(l, "id")?,
                    name: text(l, "name")?,
                    is_predefined: flag(l, "is_predefined")?,
                    color: text(l, "color")?,
                })
            })
            .collect::<Result<_>>()?;
        projects.push(PersistedProject {
            id,
            name: text(row, "name")?,
            description: opt_text(row, "description")?,
            created_at: text(row, "created_at")?,
            updated_at: text(row, "updated_at")?,
            custom_labels,
            git_repo_path: opt_text(row, "git_repo_path")?,
        });
    }
    Ok(projects)
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
    sqlx::query(&upsert_sql(TABLE, &COLUMNS, "id"))
        .bind(&p.id)
        .bind(&p.name)
        .bind(&p.description)
        .bind(&p.created_at)
        .bind(&p.updated_at)
        .bind(&p.git_repo_path)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM project_labels WHERE project_id = ?")
        .bind(&p.id)
        .execute(&mut *conn)
        .await?;
    for label in &p.custom_labels {
        sqlx::query(
            "INSERT INTO project_labels (id, project_id, name, is_predefined, color) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&label.id)
        .bind(&p.id)
        .bind(&label.name)
        .bind(bit(label.is_predefined))
        .bind(&label.color)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Deletes the project. Its labels, connections, state, tabs, canvases,
/// saved queries and dashboards cascade (except on files that started on
/// v2026.4.5-beta.1, whose `saved_queries` and `dashboards` have no foreign
/// key).
pub async fn remove(st: &Storage, project_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM projects WHERE id = ?")
        .bind(project_id)
        .execute(st.pool())
        .await?;
    Ok(())
}
