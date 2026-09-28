//! `query_history::append` and `set_favorite`: the targeted history writes
//! that replace the TypeScript's whole-list `replaceAll` (phase 5b Task 3).
//!
//! The cap must be exactly the rule `serializeQueryHistory` applied to the
//! in-memory list before every save, so an existing user's first append
//! removes what their next `replaceAll` would have, and nothing else.

use std::collections::BTreeSet;
use std::path::Path;

use seaquel_storage::{
    connections, projects, query_history, query_history::HISTORY_KEEP, Storage, StorageOptions,
    STORAGE_ERROR,
};
use seaquel_types::storage::{PersistedConnection, PersistedProject, PersistedQueryHistoryItem};
use serde_json::json;
use serde_json::value::RawValue;

async fn open(dir: &Path) -> Storage {
    Storage::open(dir.join("seaquel.db"), StorageOptions::default())
        .await
        .unwrap()
}

/// A storage with project `p` and saved connections `c1` and `c2`.
async fn setup(dir: &Path) -> Storage {
    let st = open(dir).await;
    let project: PersistedProject = serde_json::from_value(json!({
        "id": "p", "name": "P", "createdAt": "2026-01-01T00:00:00.000Z",
        "updatedAt": "2026-01-01T00:00:00.000Z", "customLabels": []
    }))
    .unwrap();
    projects::save(&st, &project).await.unwrap();
    for id in ["c1", "c2"] {
        let c: PersistedConnection = serde_json::from_value(json!({
            "id": id, "projectId": "p", "name": id, "type": "postgres", "host": "h",
            "port": 5432, "databaseName": "d", "username": "u", "labelIds": []
        }))
        .unwrap();
        connections::save(&st, &c).await.unwrap();
    }
    st
}

/// The `n`th timestamp, one second apart, as `toISOString()` writes them.
fn ts(n: u32) -> String {
    format!(
        "2026-01-{:02}T{:02}:{:02}:{:02}.000Z",
        1 + n / 86_400,
        (n / 3600) % 24,
        (n / 60) % 60,
        n % 60
    )
}

fn item(id: &str, connection_id: &str, n: u32, favorite: bool) -> PersistedQueryHistoryItem {
    PersistedQueryHistoryItem {
        id: id.into(),
        query: format!("SELECT {n}"),
        timestamp: ts(n),
        execution_time: 1.5,
        row_count: 1.0,
        connection_id: connection_id.into(),
        favorite,
        connection_labels_snapshot: Some(RawValue::from_string("[]".into()).unwrap()),
        connection_name_snapshot: connection_id.into(),
    }
}

fn ids(items: &[PersistedQueryHistoryItem]) -> Vec<String> {
    items.iter().map(|h| h.id.clone()).collect()
}

async fn load(st: &Storage, connection_id: &str) -> Vec<PersistedQueryHistoryItem> {
    query_history::load_by_connection(st, connection_id)
        .await
        .unwrap()
}

/// `serializeQueryHistory` (`persistence-manager.svelte.ts`), ported: the
/// first `HISTORY_KEEP` of the in-memory list (newest first), then the
/// favourites past them.
fn old_serializer(list: &[PersistedQueryHistoryItem]) -> BTreeSet<String> {
    let keep = HISTORY_KEEP.min(list.len());
    let mut out: BTreeSet<String> = list[..keep].iter().map(|h| h.id.clone()).collect();
    out.extend(
        list[keep..]
            .iter()
            .filter(|h| h.favorite)
            .map(|h| h.id.clone()),
    );
    out
}

#[test]
fn the_cap_is_500() {
    assert_eq!(HISTORY_KEEP, 500);
}

#[tokio::test]
async fn append_inserts_one_row() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    let h = item("hist-1", "c1", 1, false);
    query_history::append(&st, &h).await.unwrap();
    let rows = load(&st, "c1").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        serde_json::to_value(&rows[0]).unwrap(),
        serde_json::to_value(&h).unwrap()
    );
    assert!(load(&st, "c2").await.is_empty());
}

#[tokio::test]
async fn append_keeps_the_newest_500_non_favourites() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    for n in 0..510 {
        query_history::append(&st, &item(&format!("h{n}"), "c1", n, false))
            .await
            .unwrap();
    }
    let rows = load(&st, "c1").await;
    assert_eq!(rows.len(), 500);
    assert_eq!(rows[0].id, "h509");
    assert_eq!(rows[499].id, "h10");
}

#[tokio::test]
async fn append_keeps_favourites_past_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    // The two oldest are favourites.
    for n in 0..505 {
        query_history::append(&st, &item(&format!("h{n}"), "c1", n, n < 2))
            .await
            .unwrap();
    }
    let rows = load(&st, "c1").await;
    let kept: BTreeSet<_> = ids(&rows).into_iter().collect();
    // 500 newest (h5..h504) plus the two favourites past them.
    assert_eq!(rows.len(), 502);
    assert!(kept.contains("h0") && kept.contains("h1"));
    assert!(!kept.contains("h2") && !kept.contains("h4"));
    assert!(kept.contains("h5") && kept.contains("h504"));
}

#[tokio::test]
async fn append_ranks_equal_timestamps_by_insertion() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    // Every row in the same millisecond: the last one appended is the newest.
    for n in 0..501 {
        let mut h = item(&format!("h{n}"), "c1", 0, false);
        h.query = format!("SELECT {n}");
        query_history::append(&st, &h).await.unwrap();
    }
    let kept: BTreeSet<_> = ids(&load(&st, "c1").await).into_iter().collect();
    assert_eq!(kept.len(), 500);
    assert!(!kept.contains("h0"));
    assert!(kept.contains("h500"));
}

#[tokio::test]
async fn append_prunes_only_its_own_connection() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    let other: Vec<_> = (0..600)
        .map(|n| item(&format!("o{n}"), "c2", n, false))
        .collect();
    // An over-cap list for c2, as a file could hold; c1's appends leave it.
    query_history::replace_all(&st, "c2", &other).await.unwrap();
    for n in 0..501 {
        query_history::append(&st, &item(&format!("h{n}"), "c1", n, false))
            .await
            .unwrap();
    }
    assert_eq!(load(&st, "c1").await.len(), 500);
    assert_eq!(load(&st, "c2").await.len(), 600);
}

#[tokio::test]
async fn append_for_an_unsaved_connection_fails() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    let err = query_history::append(&st, &item("h", "unsaved", 1, false))
        .await
        .unwrap_err();
    assert_eq!(err.code(), STORAGE_ERROR);
    assert!(
        matches!(
            &err,
            seaquel_storage::StorageError::Sqlx(sqlx::Error::Database(db))
                if db.kind() == sqlx::error::ErrorKind::ForeignKeyViolation
        ),
        "{err:?}"
    );
    assert!(load(&st, "unsaved").await.is_empty());
}

#[tokio::test]
async fn append_with_a_duplicate_id_fails_and_prunes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    let list: Vec<_> = (0..600)
        .map(|n| item(&format!("h{n}"), "c1", n, false))
        .collect();
    query_history::replace_all(&st, "c1", &list).await.unwrap();
    query_history::append(&st, &item("h3", "c1", 700, false))
        .await
        .unwrap_err();
    // The insert failed, so the transaction's prune never ran.
    assert_eq!(load(&st, "c1").await.len(), 600);
}

#[tokio::test]
async fn append_matches_the_old_serializer() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    // 700 rows, favourites scattered (a file an older build may have left:
    // its cap kept favourites, and more rows than the cap are possible).
    let stored: Vec<_> = (0..700)
        .map(|n| item(&format!("h{n}"), "c1", n, n % 7 == 3 || n % 97 == 0))
        .collect();
    query_history::replace_all(&st, "c1", &stored)
        .await
        .unwrap();

    // What the old build did: prepend the new item to the loaded list
    // (newest first) and save `serializeQueryHistory`'s result.
    let new = item("new", "c1", 1000, false);
    let mut list = vec![new.clone()];
    list.extend(load(&st, "c1").await);
    let expected = old_serializer(&list);

    query_history::append(&st, &new).await.unwrap();
    let actual: BTreeSet<_> = ids(&load(&st, "c1").await).into_iter().collect();
    assert_eq!(actual, expected);
    assert!(actual.len() > 500, "favourites past the cap stay");
}

#[tokio::test]
async fn set_favorite_sets_and_clears() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    query_history::append(&st, &item("h1", "c1", 1, false))
        .await
        .unwrap();
    query_history::append(&st, &item("h2", "c1", 2, false))
        .await
        .unwrap();

    query_history::set_favorite(&st, "h1", true).await.unwrap();
    // Setting, not toggling: a repeat leaves it set.
    query_history::set_favorite(&st, "h1", true).await.unwrap();
    let rows = load(&st, "c1").await;
    assert!(rows.iter().find(|h| h.id == "h1").unwrap().favorite);
    assert!(!rows.iter().find(|h| h.id == "h2").unwrap().favorite);

    query_history::set_favorite(&st, "h1", false).await.unwrap();
    let rows = load(&st, "c1").await;
    assert!(rows.iter().all(|h| !h.favorite));
}

#[tokio::test]
async fn set_favorite_on_an_unknown_id_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    query_history::append(&st, &item("h1", "c1", 1, false))
        .await
        .unwrap();
    let before = serde_json::to_value(load(&st, "c1").await).unwrap();
    query_history::set_favorite(&st, "nope", true)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(load(&st, "c1").await).unwrap(), before);
}

#[tokio::test]
async fn a_favourite_set_after_the_cap_passed_it_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    for n in 0..500 {
        query_history::append(&st, &item(&format!("h{n}"), "c1", n, false))
            .await
            .unwrap();
    }
    query_history::set_favorite(&st, "h0", true).await.unwrap();
    query_history::append(&st, &item("h500", "c1", 500, false))
        .await
        .unwrap();
    let kept: BTreeSet<_> = ids(&load(&st, "c1").await).into_iter().collect();
    assert_eq!(kept.len(), 501);
    assert!(kept.contains("h0"));
}

/// Decision 11's accepted difference: a legacy file (written newest first by
/// `replace_all`) with two rows sharing a timestamp across the cap. The old
/// serializer, working on the load order it had (the lower rowid first),
/// kept the newer row; `append` ranks ties by `rowid DESC` and keeps the
/// other one. Everything else matches.
#[tokio::test]
async fn append_on_a_tie_at_the_cap_keeps_the_other_row() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    // 501 rows, newest first as `replace_all` wrote them; rows 498 and 499
    // (0-based, newest first) share a timestamp.
    let stored: Vec<_> = (0..501u32)
        .map(|i| {
            let n = if i == 499 { 1000 - 498 } else { 1000 - i };
            item(&format!("h{i}"), "c1", n, false)
        })
        .collect();
    query_history::replace_all(&st, "c1", &stored)
        .await
        .unwrap();

    // The old build's in-memory list: the new item, then what it had loaded
    // (ties in rowid order, as the index handed them over).
    let new = item("new", "c1", 2000, false);
    let mut old_list = vec![new.clone()];
    old_list.extend(stored.iter().cloned());
    let old = old_serializer(&old_list);
    assert!(old.contains("h498") && !old.contains("h499"));

    query_history::append(&st, &new).await.unwrap();
    let actual: BTreeSet<_> = ids(&load(&st, "c1").await).into_iter().collect();
    let mut expected = old.clone();
    expected.remove("h498");
    expected.insert("h499".into());
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn load_breaks_timestamp_ties_by_the_row_appended_last() {
    let dir = tempfile::tempdir().unwrap();
    let st = setup(dir.path()).await;
    for id in ["a", "b", "c"] {
        query_history::append(&st, &item(id, "c1", 5, false))
            .await
            .unwrap();
    }
    query_history::append(&st, &item("older", "c1", 1, false))
        .await
        .unwrap();
    assert_eq!(ids(&load(&st, "c1").await), ["c", "b", "a", "older"]);
}
