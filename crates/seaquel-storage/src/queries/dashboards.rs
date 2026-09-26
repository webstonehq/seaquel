//! `dashboardsRepo`: `dashboards`.

use seaquel_types::storage::PersistedDashboard;

use super::codec::{bit, flag, opt_text, select_sql, text, upsert_sql, Result};
use crate::Storage;

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

/// A project's dashboards, in rowid order.
pub async fn load_by_project(st: &Storage, project_id: &str) -> Result<Vec<PersistedDashboard>> {
    let rows = sqlx::query(&select_sql(TABLE, &COLUMNS, "project_id = ?"))
        .bind(project_id)
        .fetch_all(st.pool())
        .await?;
    rows.iter()
        .map(|row| {
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
            })
        })
        .collect()
}

/// Upserts a dashboard.
pub async fn save(st: &Storage, d: &PersistedDashboard) -> Result<()> {
    sqlx::query(&upsert_sql(TABLE, &COLUMNS, "id"))
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
    sqlx::query("DELETE FROM dashboards WHERE id = ?")
        .bind(id)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Deletes a project's dashboards.
pub async fn remove_by_project(st: &Storage, project_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM dashboards WHERE project_id = ?")
        .bind(project_id)
        .execute(st.pool())
        .await?;
    Ok(())
}
