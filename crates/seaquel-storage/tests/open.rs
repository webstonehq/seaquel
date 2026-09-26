//! `Storage::open`: new files, version rows, the legacy JSON refusal, files
//! that aren't SQLite, a baseline that fails, and the connection pragmas.

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use seaquel_storage::{
    Storage, StorageError, StorageOptions, LEGACY_JSON_FILES, LEGACY_STORAGE, STORAGE_CORRUPT,
    STORAGE_ERROR,
};
use sqlx::Connection;

async fn open(path: &Path) -> Result<Storage, StorageError> {
    Storage::open(path, StorageOptions::default()).await
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn fresh_file_gets_version_4_and_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("users/u1/meta.db");
    let storage = open(&path).await.unwrap();
    assert_eq!(storage.path(), path);
    let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM schema_version")
        .fetch_all(storage.pool())
        .await
        .unwrap();
    assert_eq!(versions, vec![4]);
    storage.close().await;
    assert!(path.is_file());
}

#[tokio::test]
async fn empty_file_is_a_fresh_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    std::fs::write(&path, b"").unwrap();
    let storage = open(&path).await.unwrap();
    let latest: i64 = sqlx::query_scalar("SELECT MAX(version) FROM schema_version")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(latest, 4);
    storage.close().await;
}

#[tokio::test]
async fn v3_file_ends_at_4() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    load_fixture(&path, "schemas/v2026.4.5-beta.1.sql").await;
    let storage = open(&path).await.unwrap();
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version ORDER BY rowid")
            .fetch_all(storage.pool())
            .await
            .unwrap();
    assert_eq!(versions, vec![1, 3, 4]);
    storage.close().await;
}

#[tokio::test]
async fn a_version_above_4_gets_no_new_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    load_fixture(&path, "schemas/current.sql").await;
    exec_file(&path, "INSERT INTO schema_version (version) VALUES (5)").await;
    let storage = open(&path).await.unwrap();
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM schema_version ORDER BY rowid")
            .fetch_all(storage.pool())
            .await
            .unwrap();
    assert_eq!(versions, vec![4, 5]);
    storage.close().await;
}

#[tokio::test]
async fn legacy_json_without_a_database_is_refused() {
    for name in LEGACY_JSON_FILES {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(name), b"{}").unwrap();
        let before = entries(dir.path());

        let err = open(&dir.path().join("seaquel.db")).await.unwrap_err();
        assert_eq!(err.code(), LEGACY_STORAGE, "{name}");
        assert!(matches!(&err, StorageError::Legacy { files, .. } if files == &[name.to_string()]));
        let message = err.to_string();
        assert!(message.contains(name), "{message}");
        assert!(
            message.contains("2026.4.5 through 2026.9.x"),
            "the message names the fix: {message}"
        );
        // Nothing was created.
        assert_eq!(entries(dir.path()), before, "{name}");
    }
}

#[tokio::test]
async fn legacy_json_next_to_a_database_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    open(&path).await.unwrap().close().await;
    for name in LEGACY_JSON_FILES {
        std::fs::write(dir.path().join(name), b"{\"connections\": []}").unwrap();
    }
    open(&path).await.unwrap().close().await;
}

#[tokio::test]
async fn other_json_files_are_not_legacy() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("themes.json"), b"{}").unwrap();
    open(&dir.path().join("seaquel.db"))
        .await
        .unwrap()
        .close()
        .await;
}

#[tokio::test]
async fn a_file_that_isnt_sqlite_is_corrupt_and_left_untouched() {
    let cases: [(&str, &[u8]); 3] = [
        ("text", b"this is not a database, just some text\n"),
        ("short", b"SQLite"),
        ("json", b"{\"connections\": []}"),
    ];
    for (label, bytes) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        std::fs::write(&path, bytes).unwrap();

        let err = open(&path).await.unwrap_err();
        assert_eq!(err.code(), STORAGE_CORRUPT, "{label}: {err}");
        assert!(err.to_string().contains("wasn't changed"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "{label}");
        assert_eq!(entries(dir.path()), vec!["seaquel.db"], "{label}");
    }
}

#[tokio::test]
async fn a_sqlite_header_over_garbage_is_corrupt_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend((0..4096u32).map(|i| (i * 31 % 251) as u8));
    std::fs::write(&path, &bytes).unwrap();

    let err = open(&path).await.unwrap_err();
    assert_eq!(err.code(), STORAGE_CORRUPT, "{err}");
    // The read-only probe caught it, before the pool could switch the file
    // to WAL.
    assert!(
        matches!(
            err,
            StorageError::Corrupt {
                untouched: true,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("wasn't changed"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(entries(dir.path()), vec!["seaquel.db"]);
}

/// The UI's storage gate (`src/lib/storage/storage-gate.svelte.ts`) reads
/// the path and whether the file was changed out of this text, so its shape
/// is pinned: `<path> isn't a readable Seaquel database (<reason>).`, plus
/// ` The file wasn't changed.` only when that's true.
#[tokio::test]
async fn the_corrupt_message_keeps_the_shape_the_ui_parses() {
    let untouched = StorageError::Corrupt {
        path: "/data/seaquel.db".into(),
        reason: "file is not a database".into(),
        untouched: true,
    };
    assert_eq!(
        untouched.to_string(),
        "/data/seaquel.db isn't a readable Seaquel database (file is not a database). \
         The file wasn't changed."
    );
    let changed = StorageError::Corrupt {
        path: "/data/seaquel.db".into(),
        reason: "database disk image is malformed".into(),
        untouched: false,
    };
    assert_eq!(
        changed.to_string(),
        "/data/seaquel.db isn't a readable Seaquel database (database disk image is malformed)."
    );

    // And a real refusal has that shape with the file's own path.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    std::fs::write(&path, b"not a database").unwrap();
    let message = open(&path).await.unwrap_err().to_string();
    let prefix = format!("{} isn't a readable Seaquel database (", path.display());
    assert!(message.starts_with(&prefix), "{message}");
    assert!(
        message.ends_with("). The file wasn't changed."),
        "{message}"
    );
}

/// The probe reads a WAL file another pool holds open, with its `-wal` and
/// `-shm` files present.
#[tokio::test]
async fn a_second_open_of_a_file_in_use_works() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let first = open(&path).await.unwrap();
    sqlx::query("INSERT INTO app_state (key, value) VALUES ('k', 'v')")
        .execute(first.pool())
        .await
        .unwrap();
    assert!(dir.path().join("seaquel.db-wal").exists());

    let second = open(&path).await.unwrap();
    let value: String = sqlx::query_scalar("SELECT value FROM app_state WHERE key = 'k'")
        .fetch_one(second.pool())
        .await
        .unwrap();
    assert_eq!(value, "v");
    second.close().await;
    first.close().await;
}

/// A v2026.4.5-beta.1 file with data, plus an `ai_messages` table whose
/// columns conflict with the baseline's (no `chat_id`, so its index can't be
/// made). The baseline gets as far as its last step before it fails: columns
/// added, `project_id` moved, orphans deleted, a column renamed. All of that
/// has to be rolled back.
#[tokio::test]
async fn a_failed_baseline_leaves_the_file_unchanged() {
    let data: serde_json::Value =
        serde_json::from_str(&fixture("upgrades/v2026.4.5-beta.1-data.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    load_fixture(&path, data["base"].as_str().unwrap()).await;
    for sql in data["seedSql"].as_array().unwrap() {
        exec_file(&path, sql.as_str().unwrap()).await;
    }
    exec_file(
        &path,
        "CREATE TABLE ai_messages (id TEXT PRIMARY KEY, content TEXT NOT NULL);
         INSERT INTO ai_messages (id, content) VALUES ('m1', 'hello');",
    )
    .await;
    let before = snapshot(&path).await;
    assert!(before
        .iter()
        .any(|l| l.starts_with("dashboards: ") && l.contains("dash-orphan")));

    let err = open(&path).await.unwrap_err();
    assert_eq!(err.code(), STORAGE_ERROR, "{err}");
    assert!(err.to_string().contains("chat_id"), "{err}");

    assert_eq!(snapshot(&path).await, before);
}

#[tokio::test]
async fn every_connection_gets_the_pragmas() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let storage = Storage::open(
        &path,
        StorageOptions {
            max_connections: 3,
            idle_timeout: Some(Duration::from_secs(60)),
        },
    )
    .await
    .unwrap();
    assert_eq!(storage.pool().options().get_max_connections(), 3);
    assert_eq!(
        storage.pool().options().get_idle_timeout(),
        Some(Duration::from_secs(60))
    );

    // Hold three at once, so each is its own connection.
    let mut held = Vec::new();
    for _ in 0..3 {
        held.push(storage.pool().acquire().await.unwrap());
    }
    for conn in &mut held {
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut **conn)
            .await
            .unwrap();
        let busy: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut **conn)
            .await
            .unwrap();
        let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut **conn)
            .await
            .unwrap();
        assert_eq!((fk, busy, journal.as_str()), (1, 5000, "wal"));
    }
    drop(held);
    storage.close().await;
}

#[tokio::test]
async fn foreign_keys_cascade() {
    let dir = tempfile::tempdir().unwrap();
    let storage = open(&dir.path().join("seaquel.db")).await.unwrap();
    let pool = storage.pool();
    sqlx::raw_sql(
        "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p', 'P', 'x', 'x');
         INSERT INTO saved_queries (id, project_id, name, query, created_at, updated_at)
           VALUES ('q', 'p', 'Q', 'SELECT 1', 'x', 'x');
         DELETE FROM projects WHERE id = 'p';",
    )
    .execute(pool)
    .await
    .unwrap();
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM saved_queries")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    storage.close().await;
}

#[tokio::test]
async fn a_file_with_a_newer_builds_migration_still_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    open(&path).await.unwrap().close().await;
    let mut conn = raw_connect(&path).await;
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (9999, 'from the future', 1, x'00', 0)",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();

    // An older build meeting a file a newer one migrated: it opens, and the
    // newer build's record stays.
    let storage = open(&path).await.unwrap();
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(storage.pool())
            .await
            .unwrap();
    // Every migration this build knows, then the newer build's.
    let mut expected: Vec<i64> = sqlx::migrate!("./migrations")
        .iter()
        .map(|m| m.version)
        .collect();
    expected.push(9999);
    assert_eq!(versions, expected);
    storage.close().await;
}
