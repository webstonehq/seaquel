//! `queryVersionsRepo`: `query_versions`, the saved queries' history.
//!
//! Pruning is split (phase 3 plan, "Task 2 findings"): the TypeScript
//! resolves the diff-match-patch deltas, which count UTF-16 code units, and
//! decides what to delete and which survivor becomes a keyframe; [`prune`]
//! runs that.

use seaquel_types::storage::{PersistedQueryVersion, QueryVersionsPrune};
use sqlx::sqlite::SqliteRow;

use super::codec::{begin, insert_sql, number, opt_text, text, Result};
use crate::Storage;

const COLUMNS: [&str; 6] = [
    "id",
    "saved_query_id",
    "version",
    "snapshot",
    "diff",
    "created_at",
];

fn map_row(row: &SqliteRow) -> Result<PersistedQueryVersion> {
    Ok(PersistedQueryVersion {
        id: text(row, "id")?,
        query_id: text(row, "saved_query_id")?,
        version: number(row, "version")?,
        snapshot: opt_text(row, "snapshot")?,
        diff: opt_text(row, "diff")?,
        created_at: text(row, "created_at")?,
    })
}

/// A saved query's versions, oldest first.
pub async fn load_by_query(st: &Storage, query_id: &str) -> Result<Vec<PersistedQueryVersion>> {
    let rows =
        sqlx::query("SELECT * FROM query_versions WHERE saved_query_id = ? ORDER BY version ASC")
            .bind(query_id)
            .fetch_all(st.pool())
            .await?;
    rows.iter().map(map_row).collect()
}

/// The versions of every saved query in a project, by query id, then
/// oldest first.
pub async fn load_by_project(st: &Storage, project_id: &str) -> Result<Vec<PersistedQueryVersion>> {
    let rows = sqlx::query(
        "SELECT qv.* FROM query_versions qv \
         JOIN saved_queries sq ON sq.id = qv.saved_query_id \
         WHERE sq.project_id = ? \
         ORDER BY qv.saved_query_id, qv.version ASC",
    )
    .bind(project_id)
    .fetch_all(st.pool())
    .await?;
    rows.iter().map(map_row).collect()
}

/// Inserts a version. The table's CHECK wants exactly one of `snapshot`
/// and `diff`, and (query, version) is unique.
pub async fn insert(st: &Storage, v: &PersistedQueryVersion) -> Result<()> {
    sqlx::query(&insert_sql("query_versions", &COLUMNS))
        .bind(&v.id)
        .bind(&v.query_id)
        .bind(v.version)
        .bind(&v.snapshot)
        .bind(&v.diff)
        .bind(&v.created_at)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Runs a prune the TypeScript computed, in one transaction: deletes
/// `delete_ids`, then makes `promote` a keyframe holding its resolved text.
/// Only versions of `saved_query_id` are touched.
pub async fn prune(st: &Storage, p: &QueryVersionsPrune) -> Result<()> {
    let mut tx = begin(st).await?;
    for id in &p.delete_ids {
        sqlx::query("DELETE FROM query_versions WHERE saved_query_id = ? AND id = ?")
            .bind(&p.saved_query_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(promote) = &p.promote {
        sqlx::query(
            "UPDATE query_versions SET snapshot = ?, diff = NULL \
             WHERE id = ? AND saved_query_id = ?",
        )
        .bind(&promote.snapshot)
        .bind(&promote.id)
        .bind(&p.saved_query_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
