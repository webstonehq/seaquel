//! `ai_messages.parts` (phase 6, migration `0007`): a
//! reply's tool calls, stored next to its text. Expand-only: older releases
//! never read the column, and a row they write reads back with no parts.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::path::Path;

use common::*;
use seaquel_storage::{ai_chats, Storage, StorageError, StorageOptions, STORAGE_NEEDS_UPGRADE};
use seaquel_types::storage::{PersistedAIChat, PersistedAIMessage};
use serde_json::json;

const RELEASES: &[&str] = &[
    "v2026.4.5-beta.1",
    "v2026.4.5",
    "v2026.4.8",
    "v2026.9.1",
    "v2026.9.2",
    "current",
];

fn read_only() -> StorageOptions {
    StorageOptions {
        read_only: true,
        ..StorageOptions::default()
    }
}

fn assert_needs_upgrade(err: &StorageError, reason: &str) {
    assert_eq!(err.code(), STORAGE_NEEDS_UPGRADE, "{err}");
    assert!(
        err.to_string().contains(reason),
        "{err} should mention {reason}"
    );
}

async fn columns_of(st: &Storage, table: &str) -> Vec<String> {
    sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .fetch_all(st.pool())
        .await
        .unwrap()
}

/// A project, a connection and a chat `a` on it, with plain SQL as any
/// release wrote them.
const SEED: &str =
    "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p', 'P', 'c', 'u'); \
     INSERT INTO connections (id, project_id, name, type, host, port, database_name, username) \
       VALUES ('c', 'p', 'C', 'postgres', 'h', 5432, 'd', 'u'); \
     INSERT INTO ai_chats (id, connection_id, title, created_at, updated_at) \
       VALUES ('a', 'c', 'T', '2026-10-01T00:00:00.000Z', '2026-10-01T00:00:00.000Z'); \
     INSERT INTO ai_messages (id, chat_id, role, content, timestamp) \
       VALUES ('old', 'a', 'user', 'from an older release', '2026-10-01T00:00:00.000Z');";

/// A reply's `parts`: a round's text and one tool call with its result.
fn parts() -> serde_json::Value {
    json!([
        {"round": 0, "type": "text", "text": "Checking. "},
        {"round": 0, "type": "tool", "callId": "call_1", "name": "run_query",
         "input": {"sql": "SELECT 1"}, "ok": true, "result": "{\"rows\":[[1]]}"},
        {"round": 1, "type": "text", "text": "Done."}
    ])
}

fn message(id: &str, role: &str, content: &str, ts: &str) -> PersistedAIMessage {
    PersistedAIMessage {
        id: id.into(),
        chat_id: "a".into(),
        role: role.into(),
        content: content.into(),
        timestamp: ts.into(),
        query: None,
        dashboard_id: None,
        parts: None,
    }
}

fn reply() -> PersistedAIMessage {
    PersistedAIMessage {
        parts: Some(parts()),
        dashboard_id: Some("dash-1".into()),
        ..message(
            "r",
            "assistant",
            "Checking. Done.",
            "2026-10-02T00:00:00.000Z",
        )
    }
}

/// On every release's file `0007` adds `parts` (beta.1's has no chat
/// tables before the baseline, so the rows are seeded after the open; the
/// file at `0006` below has rows from before it). A row an older release
/// writes (naming its own columns) reads back with none, and a reply Core
/// stores keeps them.
#[tokio::test]
async fn migration_0007_applies_on_every_release_schema() {
    for release in RELEASES {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        load_fixture(&path, &format!("schemas/{release}.sql")).await;
        let st = Storage::open(&path, StorageOptions::default())
            .await
            .unwrap_or_else(|e| panic!("{release}: {e}"));
        sqlx::raw_sql(SEED).execute(st.pool()).await.unwrap();
        assert!(
            columns_of(&st, "ai_messages")
                .await
                .contains(&"parts".to_string()),
            "{release}"
        );
        let mut tx = st.write().await.unwrap();
        let done = ai_chats::append_turn_in(&mut tx, "a", &[reply()], None)
            .await
            .unwrap();
        assert_eq!(done, ai_chats::PutMessages::Done, "{release}");
        tx.commit().await.unwrap();
        // An older release's put names its own columns.
        sqlx::query(
            "INSERT INTO ai_messages (id, chat_id, role, content, timestamp, query, dashboard_id) \
             VALUES ('older-app', 'a', 'assistant', 'older', '2026-10-03T00:00:00.000Z', NULL, NULL)",
        )
        .execute(st.pool())
        .await
        .unwrap_or_else(|e| panic!("{release}: {e}"));
        let got = ai_chats::load_messages(&st, "a").await.unwrap();
        let ids: Vec<&str> = got.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["old", "r", "older-app"], "{release}");
        assert_eq!(got[0].parts, None, "{release}");
        assert_eq!(got[1], reply(), "{release}");
        assert_eq!(got[2].parts, None, "{release}");
        st.close().await;
    }
}

/// A file the cleanup pass's `0006` already reached: the CLI refuses it
/// until the app has opened it once, and the app's open adds `parts`.
#[tokio::test]
async fn a_file_at_0006_is_refused_read_only_and_upgraded_by_a_writable_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let migrations = dir.path().join("migrations");
    std::fs::create_dir_all(&migrations).unwrap();
    for file in [
        "0001_name_keys.sql",
        "0002_window_state.sql",
        "0003_window_order_and_list_meta.sql",
        "0004_shared_links.sql",
        "0005_shared_connection_origin.sql",
        "0006_history_params.sql",
    ] {
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("migrations")
                .join(file),
            migrations.join(file),
        )
        .unwrap();
    }
    let at_0006 = sqlx::migrate::Migrator::new(migrations).await.unwrap();
    Storage::open_with_migrator(&path, StorageOptions::default(), at_0006)
        .await
        .unwrap()
        .close()
        .await;
    exec_file(&path, SEED).await;
    let before = snapshot(&path).await;

    let err = Storage::open(&path, read_only()).await.unwrap_err();
    assert_needs_upgrade(&err, "migration 7");
    assert_eq!(snapshot(&path).await, before);

    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    assert!(columns_of(&st, "ai_messages")
        .await
        .contains(&"parts".to_string()));
    let got = ai_chats::load_messages(&st, "a").await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].parts, None);
    st.close().await;
    Storage::open(&path, read_only())
        .await
        .unwrap()
        .close()
        .await;
}

async fn fresh(dir: &Path) -> Storage {
    let st = Storage::open(dir.join("seaquel.db"), StorageOptions::default())
        .await
        .unwrap();
    sqlx::raw_sql(SEED).execute(st.pool()).await.unwrap();
    st
}

/// `parts` is stored as JSON text and read back as it was written; a value
/// that isn't a JSON list (a hand edit, a broken write) reads as none and
/// keeps the row.
#[tokio::test]
async fn parts_round_trip_and_unreadable_parts_read_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let st = fresh(dir.path()).await;
    let mut tx = st.write().await.unwrap();
    ai_chats::put_messages(&mut tx, "a", &[reply()])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let stored: Option<String> = sqlx::query_scalar("SELECT parts FROM ai_messages WHERE id = 'r'")
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stored.unwrap()).unwrap(),
        parts()
    );
    for (id, raw) in [("not-json", "not json"), ("not-a-list", r#"{"round":0}"#)] {
        sqlx::query(
            "INSERT INTO ai_messages (id, chat_id, role, content, timestamp, parts) \
             VALUES (?, 'a', 'assistant', 'x', '2026-10-05T00:00:00.000Z', ?)",
        )
        .bind(id)
        .bind(raw)
        .execute(st.pool())
        .await
        .unwrap();
    }
    let got = ai_chats::load_messages(&st, "a").await.unwrap();
    assert_eq!(got.len(), 4);
    let by_id = |id: &str| got.iter().find(|m| m.id == id).unwrap().clone();
    assert_eq!(by_id("r"), reply());
    assert_eq!(by_id("not-json").parts, None);
    assert_eq!(by_id("not-a-list").parts, None);
    // A message stored again without parts keeps them, as an older
    // release's put does; one with parts replaces them.
    let mut tx = st.write().await.unwrap();
    ai_chats::put_messages(&mut tx, "a", &[message("r", "assistant", "plain", "t")])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let kept = ai_chats::load_messages(&st, "a").await.unwrap();
    let r = kept.iter().find(|m| m.id == "r").unwrap();
    assert_eq!((r.content.as_str(), &r.parts), ("plain", &Some(parts())));
    let mut other = message("r", "assistant", "plain", "t");
    other.parts = Some(json!([]));
    let mut tx = st.write().await.unwrap();
    ai_chats::put_messages(&mut tx, "a", &[other])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let stored: Option<String> = sqlx::query_scalar("SELECT parts FROM ai_messages WHERE id = 'r'")
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(stored.as_deref(), Some("[]"));
}

/// Core's turn write: the messages upserted by id, and the
/// chat's `updated_at` set when asked, in the caller's transaction. A
/// message id of another chat writes nothing.
#[tokio::test]
async fn append_turn_in_upserts_and_touches_the_chat() {
    let dir = tempfile::tempdir().unwrap();
    let st = fresh(dir.path()).await;
    sqlx::raw_sql(
        "INSERT INTO ai_chats (id, connection_id, title, created_at, updated_at) \
           VALUES ('b', 'c', 'T', '2026-10-01T00:00:00.000Z', '2026-10-01T00:00:00.000Z'); \
         INSERT INTO ai_messages (id, chat_id, role, content, timestamp) \
           VALUES ('b-msg', 'b', 'user', 'other chat', '2026-10-01T00:00:00.000Z');",
    )
    .execute(st.pool())
    .await
    .unwrap();

    let user = message("u", "user", "question", "2026-10-02T00:00:00.000Z");
    let mut tx = st.write().await.unwrap();
    let done = ai_chats::append_turn_in(&mut tx, "a", std::slice::from_ref(&user), None)
        .await
        .unwrap();
    assert_eq!(done, ai_chats::PutMessages::Done);
    let done = ai_chats::append_turn_in(&mut tx, "a", &[reply()], Some("2026-10-09T00:00:00.000Z"))
        .await
        .unwrap();
    assert_eq!(done, ai_chats::PutMessages::Done);
    // The reply again, as a later write of the same turn would.
    let mut again = reply();
    again.content = "Checking. Done again.".into();
    ai_chats::append_turn_in(&mut tx, "a", &[again.clone()], None)
        .await
        .unwrap();
    // Another chat's id: nothing written.
    let mut stolen = message("b-msg", "user", "stolen", "t");
    stolen.chat_id = "a".into();
    let refused =
        ai_chats::append_turn_in(&mut tx, "a", &[stolen], Some("2030-01-01T00:00:00.000Z"))
            .await
            .unwrap();
    assert_eq!(
        refused,
        ai_chats::PutMessages::OtherChat { id: "b-msg".into() }
    );
    tx.commit().await.unwrap();

    let got = ai_chats::load_messages(&st, "a").await.unwrap();
    let ids: Vec<&str> = got.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["old", "u", "r"]);
    assert_eq!(got[2], again);
    let chat: PersistedAIChat = ai_chats::get(&st, "a").await.unwrap().unwrap();
    assert_eq!(chat.updated_at, "2026-10-09T00:00:00.000Z");
    let other = ai_chats::load_messages(&st, "b").await.unwrap();
    assert_eq!(other[0].content, "other chat");
}

/// The chat's stored bytes (the web budget) count a message's parts with
/// its content, whole and for the messages a put replaces.
#[tokio::test]
async fn content_bytes_count_parts() {
    let dir = tempfile::tempdir().unwrap();
    let st = fresh(dir.path()).await;
    let before = ai_chats::content_bytes(&st, "a").await.unwrap();
    let mut tx = st.write().await.unwrap();
    ai_chats::put_messages(&mut tx, "a", &[reply()])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let r = reply();
    let own = (r.content.len() + parts().to_string().len()) as u64;
    assert_eq!(
        ai_chats::content_bytes(&st, "a").await.unwrap(),
        before + own
    );
    assert_eq!(
        ai_chats::content_bytes_of(&st, "a", &["r".to_string()])
            .await
            .unwrap(),
        own
    );
    assert_eq!(
        ai_chats::parts_bytes_of(&st, "a", &["r".to_string(), "old".to_string()])
            .await
            .unwrap(),
        parts().to_string().len() as u64
    );
}
