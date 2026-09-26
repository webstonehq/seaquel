//! `savedQueriesRepo`: `saved_queries`.

use seaquel_types::storage::PersistedSavedQuery;

use super::codec::{
    begin, bit, flag, json, json_text, opt_text, select_sql, text, upsert_sql, Result,
};
use crate::Storage;

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

/// A project's saved queries, in rowid order.
pub async fn load_by_project(st: &Storage, project_id: &str) -> Result<Vec<PersistedSavedQuery>> {
    let rows = sqlx::query(&select_sql(TABLE, &COLUMNS, "project_id = ?"))
        .bind(project_id)
        .fetch_all(st.pool())
        .await?;
    rows.iter()
        .map(|row| {
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
            })
        })
        .collect()
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
        sqlx::query("DELETE FROM saved_queries WHERE project_id = ?")
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
    } else {
        let placeholders = vec!["?"; queries.len()].join(",");
        let sql = format!(
            "DELETE FROM saved_queries WHERE project_id = ? AND id NOT IN ({placeholders})"
        );
        let mut delete = sqlx::query(&sql).bind(project_id);
        for q in queries {
            delete = delete.bind(&q.id);
        }
        delete.execute(&mut *tx).await?;
    }
    let upsert = upsert_sql(TABLE, &COLUMNS, "id");
    for q in queries {
        sqlx::query(&upsert)
            .bind(&q.id)
            .bind(&q.project_id)
            .bind(&q.name)
            .bind(&q.query)
            .bind(json_text(&q.parameters))
            .bind(bit(q.starred))
            .bind(bit(q.shared))
            .bind(&q.description)
            .bind(&q.database_type)
            .bind(json_text(&q.tags))
            .bind(&q.folder)
            .bind(&q.created_at)
            .bind(&q.updated_at)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Deletes a project's saved queries. Their versions cascade.
pub async fn remove_by_project(st: &Storage, project_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM saved_queries WHERE project_id = ?")
        .bind(project_id)
        .execute(st.pool())
        .await?;
    Ok(())
}
