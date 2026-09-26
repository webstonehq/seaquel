//! `dashboardVersionsRepo`: `dashboard_versions`, full snapshots.
//!
//! Pruning is computed in TypeScript, like the query versions' (see
//! `query_versions`), and [`prune`] runs it. The TypeScript keeps today's
//! rule that keeping 0 deletes every version.

use seaquel_types::storage::{DashboardVersionsPrune, PersistedDashboardVersion};
use sqlx::sqlite::SqliteRow;

use super::codec::{begin, insert_sql, number, text, Result};
use crate::Storage;

const COLUMNS: [&str; 5] = ["id", "dashboard_id", "version", "snapshot", "created_at"];

fn map_row(row: &SqliteRow) -> Result<PersistedDashboardVersion> {
    Ok(PersistedDashboardVersion {
        id: text(row, "id")?,
        dashboard_id: text(row, "dashboard_id")?,
        version: number(row, "version")?,
        snapshot: text(row, "snapshot")?,
        created_at: text(row, "created_at")?,
    })
}

/// A dashboard's versions, oldest first.
pub async fn load_by_dashboard(
    st: &Storage,
    dashboard_id: &str,
) -> Result<Vec<PersistedDashboardVersion>> {
    let rows =
        sqlx::query("SELECT * FROM dashboard_versions WHERE dashboard_id = ? ORDER BY version ASC")
            .bind(dashboard_id)
            .fetch_all(st.pool())
            .await?;
    rows.iter().map(map_row).collect()
}

/// The versions of every dashboard in a project, by dashboard id, then
/// oldest first.
pub async fn load_by_project(
    st: &Storage,
    project_id: &str,
) -> Result<Vec<PersistedDashboardVersion>> {
    let rows = sqlx::query(
        "SELECT dv.* FROM dashboard_versions dv \
         JOIN dashboards d ON d.id = dv.dashboard_id \
         WHERE d.project_id = ? \
         ORDER BY dv.dashboard_id, dv.version ASC",
    )
    .bind(project_id)
    .fetch_all(st.pool())
    .await?;
    rows.iter().map(map_row).collect()
}

/// Inserts a version; (dashboard, version) is unique.
pub async fn insert(st: &Storage, v: &PersistedDashboardVersion) -> Result<()> {
    sqlx::query(&insert_sql("dashboard_versions", &COLUMNS))
        .bind(&v.id)
        .bind(&v.dashboard_id)
        .bind(v.version)
        .bind(&v.snapshot)
        .bind(&v.created_at)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Deletes `delete_ids` from the dashboard's versions, in one transaction.
/// Only versions of `dashboard_id` are touched.
pub async fn prune(st: &Storage, p: &DashboardVersionsPrune) -> Result<()> {
    let mut tx = begin(st).await?;
    for id in &p.delete_ids {
        sqlx::query("DELETE FROM dashboard_versions WHERE dashboard_id = ? AND id = ?")
            .bind(&p.dashboard_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
