//! `dashboardVersionsRepo`: `dashboard_versions`, full snapshots.
//!
//! Pruning is computed in TypeScript, like the query versions' (see
//! `query_versions`), and [`prune`] runs it. The TypeScript keeps today's
//! rule that keeping 0 deletes every version.
//!
//! From phase 5d-2, `dashboardVersionsList` answers the versions without
//! their snapshots ([`list_meta_by_project`], reading the `widget_count`
//! column migration `0003` added) and `dashboardVersionGet` one version
//! whole ([`get`]).
//!
//! From phase 5d-2, Core numbers a new version inside its write
//! transaction ([`append`]) and prunes from [`list_meta`] with
//! [`delete_ids`], by `dashboard_version_limit` (Decision 21).

use crate::db;
use crate::db::SqliteRow;
use seaquel_types::storage::{
    DashboardVersionsPrune, PersistedDashboardVersion, PersistedDashboardVersionMeta,
};

use super::codec::{begin, insert_sql, number, opt_number, text, Result};
use super::query_versions::VersionMeta;
use crate::{Reader, Storage, WriteTx};

const COLUMNS: [&str; 5] = ["id", "dashboard_id", "version", "snapshot", "created_at"];

/// The length of a version's snapshot's `widgets` list (`snapshot` the
/// column), or NULL when the snapshot isn't JSON or its `widgets` isn't a
/// list: migration `0003` fills `widget_count` with it, [`append`] writes
/// it, and [`list_meta_by_project`] falls back to it for a row whose
/// `widget_count` is NULL. The `CASE` makes sure a JSON function only ever
/// sees text `json_valid` accepted.
pub const WIDGET_COUNT_OF_SNAPSHOT: &str = "CASE WHEN typeof(snapshot) = 'text' \
    AND json_valid(snapshot) THEN CASE WHEN json_type(snapshot, '$.widgets') = 'array' \
    THEN json_array_length(snapshot, '$.widgets') END END";

/// Fills `widget_count` of every version an older release wrote without it
/// (5d-2 Task 7 review); one that can't be counted gets `-1`, which reads
/// as no count.
pub(crate) fn refill_sql() -> String {
    format!(
        "UPDATE dashboard_versions SET widget_count = COALESCE({WIDGET_COUNT_OF_SNAPSHOT}, -1) \
         WHERE widget_count IS NULL"
    )
}

/// A project's dashboard versions without their snapshots, by dashboard,
/// then oldest first (`?1` the project).
pub fn list_meta_by_project_sql() -> String {
    format!(
        "SELECT CAST(dv.id AS BLOB) AS id, CAST(dv.dashboard_id AS BLOB) AS dashboard_id, \
           dv.version, CAST(dv.created_at AS BLOB) AS created_at, \
           COALESCE(dv.widget_count, {count}) AS widget_count, \
           COALESCE(octet_length(dv.snapshot), 0) AS bytes \
         FROM dashboard_versions dv JOIN dashboards d ON d.id = dv.dashboard_id \
         WHERE d.project_id = ?1 ORDER BY dv.dashboard_id, dv.version ASC",
        // `snapshot` is only `dashboard_versions`' column.
        count = WIDGET_COUNT_OF_SNAPSHOT,
    )
}

/// A text column selected as bytes, read lossily: a hand-edited value that
/// isn't UTF-8 never fails the list (5d-2 Task 7 review).
fn lossy(row: &SqliteRow, col: &str) -> Result<String> {
    let bytes: Option<Vec<u8>> = db::Row::try_get_unchecked(row, col)?;
    Ok(bytes
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default())
}

fn map_meta(row: &SqliteRow) -> Result<PersistedDashboardVersionMeta> {
    let widgets: Option<i64> = db::Row::try_get(row, "widget_count").ok().flatten();
    let bytes: i64 = db::Row::try_get(row, "bytes")?;
    Ok(PersistedDashboardVersionMeta {
        id: lossy(row, "id")?,
        dashboard_id: lossy(row, "dashboard_id")?,
        version: number(row, "version")?,
        created_at: lossy(row, "created_at")?,
        widget_count: widgets.and_then(|n| u32::try_from(n).ok()),
        bytes: bytes.max(0) as u64,
    })
}

/// The versions of every dashboard in a project without their snapshots
/// (`dashboardVersionsList`, phase 5d-2 Task 7): each version's number,
/// time, widget count and snapshot size, by dashboard id, then oldest
/// first. Only a row an older release wrote has its widgets counted here
/// (its `widget_count` is NULL); the snapshots are otherwise never read.
pub async fn list_meta_by_project(
    r: impl Into<Reader<'_>>,
    project_id: &str,
) -> Result<Vec<PersistedDashboardVersionMeta>> {
    let mut conn = r.into().conn().await?;
    let rows = db::query(&list_meta_by_project_sql())
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?;
    rows.iter().map(map_meta).collect()
}

/// One version of `dashboard_id`, whole (`dashboardVersionGet`), or `None`
/// when that dashboard has no version with that id (another dashboard's
/// version included).
pub async fn get(
    r: impl Into<Reader<'_>>,
    dashboard_id: &str,
    id: &str,
) -> Result<Option<PersistedDashboardVersion>> {
    let mut conn = r.into().conn().await?;
    let row = db::query(
        "SELECT id, dashboard_id, version, snapshot, created_at FROM dashboard_versions \
         WHERE id = ? AND dashboard_id = ?",
    )
    .bind(id)
    .bind(dashboard_id)
    .fetch_optional(&mut *conn)
    .await?;
    row.as_ref().map(map_row).transpose()
}

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
        db::query("SELECT * FROM dashboard_versions WHERE dashboard_id = ? ORDER BY version ASC")
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
    list_by_project(st, project_id).await
}

/// [`load_by_project`] on the pool or inside a write.
pub async fn list_by_project(
    r: impl Into<Reader<'_>>,
    project_id: &str,
) -> Result<Vec<PersistedDashboardVersion>> {
    let mut conn = r.into().conn().await?;
    let rows = db::query(
        "SELECT dv.* FROM dashboard_versions dv \
         JOIN dashboards d ON d.id = dv.dashboard_id \
         WHERE d.project_id = ? \
         ORDER BY dv.dashboard_id, dv.version ASC",
    )
    .bind(project_id)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter().map(map_row).collect()
}

/// Inserts a version; (dashboard, version) is unique.
pub async fn insert(st: &Storage, v: &PersistedDashboardVersion) -> Result<()> {
    let mut tx = begin(st).await?;
    db::query(&insert_sql("dashboard_versions", &COLUMNS))
        .bind(&v.id)
        .bind(&v.dashboard_id)
        .bind(v.version)
        .bind(&v.snapshot)
        .bind(&v.created_at)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Deletes `delete_ids` from the dashboard's versions, in one transaction.
/// Only versions of `dashboard_id` are touched.
pub async fn prune(st: &Storage, p: &DashboardVersionsPrune) -> Result<()> {
    let mut tx = begin(st).await?;
    for id in &p.delete_ids {
        db::query("DELETE FROM dashboard_versions WHERE dashboard_id = ? AND id = ?")
            .bind(&p.dashboard_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Appends a version of `dashboard_id` holding `snapshot`, numbered one
/// past its highest version (1 for the first), read through
/// `idx_dashboard_versions_dashboard`, with its widget count
/// ([`WIDGET_COUNT_OF_SNAPSHOT`]). Run inside the caller's write
/// transaction, the number can't collide with another writer's. Returns
/// the row as stored, without its snapshot (the caller has it).
pub async fn append(
    tx: &mut WriteTx,
    id: &str,
    dashboard_id: &str,
    snapshot: &str,
    created_at: &str,
) -> Result<PersistedDashboardVersionMeta> {
    let conn = tx.conn();
    let highest: Option<f64> = {
        let row = db::query(
            "SELECT MAX(version) AS highest FROM dashboard_versions WHERE dashboard_id = ?",
        )
        .bind(dashboard_id)
        .fetch_one(&mut *conn)
        .await?;
        opt_number(&row, "highest")?
    };
    let version = highest.map_or(1.0, |v| v.floor() + 1.0);
    db::query(&insert_sql("dashboard_versions", &COLUMNS))
        .bind(id)
        .bind(dashboard_id)
        .bind(version as i64)
        .bind(snapshot)
        .bind(created_at)
        .execute(&mut *conn)
        .await?;
    let row = db::query(&format!(
        "UPDATE dashboard_versions SET widget_count = COALESCE({WIDGET_COUNT_OF_SNAPSHOT}, -1) \
         WHERE id = ?1 AND dashboard_id = ?2 \
         RETURNING CAST(id AS BLOB) AS id, CAST(dashboard_id AS BLOB) AS dashboard_id, version, \
           CAST(created_at AS BLOB) AS created_at, widget_count, \
           COALESCE(octet_length(snapshot), 0) AS bytes"
    ))
    .bind(id)
    .bind(dashboard_id)
    .fetch_one(&mut *conn)
    .await?;
    map_meta(&row)
}

/// A dashboard's versions, oldest first, without reading their snapshots:
/// each is a keyframe (a whole snapshot), with its snapshot's bytes
/// (`octet_length`, read from the record header). The shape
/// `query_versions::list_meta` gives, so one prune plans both.
pub async fn list_meta(r: impl Into<Reader<'_>>, dashboard_id: &str) -> Result<Vec<VersionMeta>> {
    let mut conn = r.into().conn().await?;
    let rows = db::query(
        "SELECT id, version, created_at, COALESCE(octet_length(snapshot), 0) AS bytes \
         FROM dashboard_versions WHERE dashboard_id = ? ORDER BY version ASC",
    )
    .bind(dashboard_id)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter()
        .map(|row| {
            let bytes: i64 = db::Row::try_get(row, "bytes")?;
            Ok(VersionMeta {
                id: text(row, "id")?,
                version: number(row, "version")?,
                keyframe: true,
                created_at: text(row, "created_at")?,
                bytes: bytes.max(0) as u64,
            })
        })
        .collect()
}

/// Deletes those of `ids` that are versions of `dashboard_id` (another
/// dashboard's are left alone), and returns how many it deleted.
pub async fn delete_ids(tx: &mut WriteTx, dashboard_id: &str, ids: &[String]) -> Result<u64> {
    let conn = tx.conn();
    let mut deleted = 0;
    for id in ids {
        deleted += db::query("DELETE FROM dashboard_versions WHERE dashboard_id = ? AND id = ?")
            .bind(dashboard_id)
            .bind(id)
            .execute(&mut *conn)
            .await?
            .rows_affected();
    }
    Ok(deleted)
}
