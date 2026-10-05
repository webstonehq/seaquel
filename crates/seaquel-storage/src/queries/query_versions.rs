//! `queryVersionsRepo`: `query_versions`, the saved queries' history.
//!
//! Pruning is split: the TypeScript
//! resolves the diff-match-patch deltas, which count UTF-16 code units, and
//! decides what to delete and which survivor becomes a keyframe; [`prune`]
//! runs that.
//!
//! From phase 5d, Core writes versions as keyframes only
//! ([`append_keyframe`], numbered inside its write transaction) and prunes
//! back to a keyframe from [`list_meta`] with [`delete_ids`], so no diff is
//! ever resolved in Rust.

use crate::db;
use crate::db::SqliteRow;
use seaquel_types::storage::{PersistedQueryVersion, QueryVersionsPrune};

use super::codec::{begin, insert_sql, number, opt_number, opt_text, text, Result};
use crate::{Reader, Storage, WriteTx};

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
        db::query("SELECT * FROM query_versions WHERE saved_query_id = ? ORDER BY version ASC")
            .bind(query_id)
            .fetch_all(st.pool())
            .await?;
    rows.iter().map(map_row).collect()
}

/// The versions of every saved query in a project, by query id, then
/// oldest first.
pub async fn load_by_project(st: &Storage, project_id: &str) -> Result<Vec<PersistedQueryVersion>> {
    let rows = db::query(
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
    let mut tx = begin(st).await?;
    db::query(&insert_sql("query_versions", &COLUMNS))
        .bind(&v.id)
        .bind(&v.query_id)
        .bind(v.version)
        .bind(&v.snapshot)
        .bind(&v.diff)
        .bind(&v.created_at)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Runs a prune the TypeScript computed, in one transaction: deletes
/// `delete_ids`, then makes `promote` a keyframe holding its resolved text.
/// Only versions of `saved_query_id` are touched.
pub async fn prune(st: &Storage, p: &QueryVersionsPrune) -> Result<()> {
    let mut tx = begin(st).await?;
    for id in &p.delete_ids {
        db::query("DELETE FROM query_versions WHERE saved_query_id = ? AND id = ?")
            .bind(&p.saved_query_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(promote) = &p.promote {
        db::query(
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

/// Appends a keyframe (`snapshot`, no `diff`) to a saved query's versions,
/// numbered one past its highest version (1 for the first). Run inside the
/// caller's write transaction, the number can't collide with another
/// writer's. Returns the row as stored.
pub async fn append_keyframe(
    tx: &mut WriteTx,
    id: &str,
    saved_query_id: &str,
    snapshot: &str,
    created_at: &str,
) -> Result<PersistedQueryVersion> {
    let conn = tx.conn();
    let highest: Option<f64> = {
        let row = db::query(
            "SELECT MAX(version) AS highest FROM query_versions WHERE saved_query_id = ?",
        )
        .bind(saved_query_id)
        .fetch_one(&mut *conn)
        .await?;
        opt_number(&row, "highest")?
    };
    let version = highest.map_or(1.0, |v| v.floor() + 1.0);
    db::query(&insert_sql("query_versions", &COLUMNS))
        .bind(id)
        .bind(saved_query_id)
        .bind(version as i64)
        .bind(snapshot)
        .bind(None::<&str>)
        .bind(created_at)
        .execute(&mut *conn)
        .await?;
    Ok(PersistedQueryVersion {
        id: id.to_string(),
        query_id: saved_query_id.to_string(),
        version,
        snapshot: Some(snapshot.to_string()),
        diff: None,
        created_at: created_at.to_string(),
    })
}

/// A version without its text, for planning a prune.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionMeta {
    pub id: String,
    pub version: f64,
    /// It holds a `snapshot` (whole text), not a `diff`.
    pub keyframe: bool,
    pub created_at: String,
    /// The bytes of its stored text (`octet_length` of the snapshot or the
    /// diff, which SQLite reads from the record header, not the text).
    pub bytes: u64,
}

/// A saved query's versions, oldest first, without reading their text.
pub async fn list_meta(r: impl Into<Reader<'_>>, saved_query_id: &str) -> Result<Vec<VersionMeta>> {
    let mut conn = r.into().conn().await?;
    let rows = db::query(
        "SELECT id, version, snapshot IS NOT NULL AS keyframe, created_at, \
         COALESCE(octet_length(snapshot), octet_length(diff), 0) AS bytes FROM query_versions \
         WHERE saved_query_id = ? ORDER BY version ASC",
    )
    .bind(saved_query_id)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter()
        .map(|row| {
            let keyframe: i64 = db::Row::try_get(row, "keyframe")?;
            let bytes: i64 = db::Row::try_get(row, "bytes")?;
            Ok(VersionMeta {
                id: text(row, "id")?,
                version: number(row, "version")?,
                keyframe: keyframe == 1,
                created_at: text(row, "created_at")?,
                bytes: bytes.max(0) as u64,
            })
        })
        .collect()
}

/// Deletes those of `ids` that are versions of `saved_query_id` (another
/// query's are left alone), and returns how many it deleted.
pub async fn delete_ids(tx: &mut WriteTx, saved_query_id: &str, ids: &[String]) -> Result<u64> {
    let conn = tx.conn();
    let mut deleted = 0;
    for id in ids {
        deleted += db::query("DELETE FROM query_versions WHERE saved_query_id = ? AND id = ?")
            .bind(saved_query_id)
            .bind(id)
            .execute(&mut *conn)
            .await?
            .rows_affected();
    }
    Ok(deleted)
}
