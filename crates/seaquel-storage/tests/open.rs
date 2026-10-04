//! `Storage::open`: new files, version rows, the legacy JSON refusal, files
//! that aren't SQLite, a baseline that fails, the connection pragmas, the
//! read-only open and the migration lock.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use seaquel_storage::{
    Storage, StorageError, StorageOptions, DATA_STEPS_TABLE, LEGACY_JSON_FILES, LEGACY_STORAGE,
    STORAGE_CORRUPT, STORAGE_ERROR, STORAGE_NEEDS_UPGRADE, STORAGE_NOT_FOUND,
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
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap();
    // Three connections: two in the pool for reads, and the writer
    // connection outside it (phase 7a Decision 5).
    assert_eq!(storage.pool().options().get_max_connections(), 2);
    assert_eq!(
        storage.pool().options().get_idle_timeout(),
        Some(Duration::from_secs(60))
    );
    let mut tx = storage.write().await.unwrap();
    let writer: (i64, i64, String) = (
        sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
    );
    assert_eq!(writer, (1, 5000, "wal".to_string()));
    tx.commit().await.unwrap();

    // Hold both at once, so each is its own connection.
    let mut held = Vec::new();
    for _ in 0..2 {
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

// ── Read-only opens ──

fn read_only() -> StorageOptions {
    StorageOptions {
        read_only: true,
        ..StorageOptions::default()
    }
}

async fn open_read_only(path: &Path) -> Result<Storage, StorageError> {
    Storage::open(path, read_only()).await
}

/// The migrations in `tests/test_migrations`: one table that fails if it's
/// created twice, and enough rows to keep its migrator busy for a moment.
async fn test_migrator() -> sqlx::migrate::Migrator {
    sqlx::migrate::Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/test_migrations"),
    )
    .await
    .unwrap()
}

/// The file's bytes and the directory's entries, to show a refused
/// read-only open changed nothing (no `-wal` or `-shm` either).
fn disk_state(path: &Path) -> (Vec<u8>, Vec<String>) {
    (
        std::fs::read(path).unwrap(),
        entries(path.parent().unwrap()),
    )
}

/// A file the app has opened and closed: up to date, in WAL mode, with no
/// `-wal` or `-shm` left.
async fn current_file(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("seaquel.db");
    // One connection: when several close at once, each can see another
    // still open and leave the `-wal` and `-shm` behind.
    let storage = Storage::open(
        &path,
        StorageOptions {
            max_connections: 1,
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO app_state (key, value) VALUES ('k', 'v')")
        .execute(storage.pool())
        .await
        .unwrap();
    storage.close().await;
    // sqlx's workers finish closing just after `close` returns, and the
    // last one removes the `-wal` and `-shm`.
    for _ in 0..100 {
        if entries(dir) == ["seaquel.db"] {
            return path;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the app's -wal and -shm stayed: {:?}", entries(dir));
}

fn assert_needs_upgrade(err: &StorageError, reason: &str) {
    assert_eq!(err.code(), STORAGE_NEEDS_UPGRADE, "{err}");
    let message = err.to_string();
    assert!(
        message.ends_with("Open the Seaquel app once to update your data."),
        "{message}"
    );
    assert!(
        message.contains(reason),
        "{message} should mention {reason}"
    );
}

#[tokio::test]
async fn a_read_only_open_of_a_current_file_reads_and_cant_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = current_file(dir.path()).await;
    let before = std::fs::read(&path).unwrap();

    let storage = open_read_only(&path).await.unwrap();
    let value: String = sqlx::query_scalar("SELECT value FROM app_state WHERE key = 'k'")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(value, "v");

    for sql in [
        "INSERT INTO app_state (key, value) VALUES ('k2', 'v2')",
        "UPDATE app_state SET value = 'x'",
        "CREATE TABLE sneaky (x)",
    ] {
        let err = sqlx::query(sql)
            .execute(storage.pool())
            .await
            .expect_err(sql);
        let sqlx::Error::Database(db) = &err else {
            panic!("{sql}: {err:?}");
        };
        // SQLITE_READONLY
        assert_eq!(db.code().as_deref(), Some("8"), "{sql}: {err}");
    }
    // The journal mode is the file's own, left as the app set it.
    let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(journal, "wal");
    storage.close().await;
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// The desktop app has the file open and is writing: the read-only pool
/// still reads, and sees only what's committed.
#[tokio::test]
async fn a_read_only_open_reads_while_another_pool_holds_a_write_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let path = current_file(dir.path()).await;
    let app = open(&path).await.unwrap();
    let mut tx = app.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::query("UPDATE app_state SET value = 'uncommitted' WHERE key = 'k'")
        .execute(&mut *tx)
        .await
        .unwrap();

    let reader = tokio::time::timeout(Duration::from_secs(2), open_read_only(&path))
        .await
        .expect("the read-only open doesn't wait for the writer")
        .unwrap();
    let value: String = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query_scalar("SELECT value FROM app_state WHERE key = 'k'").fetch_one(reader.pool()),
    )
    .await
    .expect("the read doesn't wait for the writer")
    .unwrap();
    assert_eq!(value, "v");

    tx.commit().await.unwrap();
    let value: String = sqlx::query_scalar("SELECT value FROM app_state WHERE key = 'k'")
        .fetch_one(reader.pool())
        .await
        .unwrap();
    assert_eq!(value, "uncommitted");
    reader.close().await;
    app.close().await;
}

#[tokio::test]
async fn a_read_only_open_of_a_pre_baseline_file_needs_an_upgrade() {
    for fixture in ["schemas/v2026.4.5-beta.1.sql", "schemas/v2026.4.5.sql"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        load_fixture(&path, fixture).await;
        let before = disk_state(&path);

        let err = open_read_only(&path).await.unwrap_err();
        assert_needs_upgrade(&err, "schema");
        assert_eq!(disk_state(&path), before, "{fixture}");
    }
}

/// The baseline has run on this file, but something it would change again
/// is back: each one alone makes the file need the app.
#[tokio::test]
async fn a_read_only_open_needs_an_upgrade_when_the_baseline_would_change_anything() {
    let cases = [
        "ALTER TABLE connections DROP COLUMN active_ai_model",
        "DROP INDEX idx_ai_messages_chat",
        "DROP TABLE user_themes",
        "DELETE FROM schema_version",
        "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p', 'P', 'x', 'x');
         INSERT INTO project_state (project_id, active_view) VALUES ('p', 'canvas')",
        "ALTER TABLE saved_queries ADD COLUMN connection_id TEXT",
        "ALTER TABLE project_state RENAME COLUMN active_workflow_tab_id TO active_canvas_tab_id",
    ];
    for sql in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = current_file(dir.path()).await;
        exec_file(&path, sql).await;
        let before = disk_state(&path);

        let err = open_read_only(&path).await.unwrap_err();
        assert_needs_upgrade(&err, "schema");
        assert_eq!(disk_state(&path), before, "{sql}");
    }
}

#[tokio::test]
async fn a_read_only_open_with_an_unapplied_migration_needs_an_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = current_file(dir.path()).await;
    let before = disk_state(&path);

    let err = Storage::open_with_migrator(&path, read_only(), test_migrator().await)
        .await
        .unwrap_err();
    assert_needs_upgrade(&err, "migration 9001");
    assert_eq!(disk_state(&path), before);
}

/// A file the app opened before `0002_window_state.sql` shipped (only
/// `0001` applied): the CLI refuses it until the app has run `0002`, and
/// leaves it as it was.
#[tokio::test]
async fn a_read_only_open_refuses_a_file_with_0002_pending() {
    let dir = tempfile::tempdir().unwrap();
    let only_0001 = dir.path().join("migrations");
    std::fs::create_dir(&only_0001).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations/0001_name_keys.sql"),
        only_0001.join("0001_name_keys.sql"),
    )
    .unwrap();
    let path = dir.path().join("seaquel.db");
    let older = sqlx::migrate::Migrator::new(only_0001).await.unwrap();
    Storage::open_with_migrator(&path, StorageOptions::default(), older)
        .await
        .unwrap()
        .close()
        .await;
    let before = snapshot(&path).await;

    let err = open_read_only(&path).await.unwrap_err();
    assert_needs_upgrade(&err, "migration 2");
    assert_eq!(snapshot(&path).await, before);

    // The app's open applies it; then the CLI's works.
    open(&path).await.unwrap().close().await;
    open_read_only(&path).await.unwrap().close().await;
}

#[tokio::test]
async fn a_read_only_open_with_a_pending_data_step_needs_an_upgrade() {
    for sql in [
        format!("DELETE FROM {DATA_STEPS_TABLE}"),
        format!("DROP TABLE {DATA_STEPS_TABLE}"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = current_file(dir.path()).await;
        exec_file(&path, &sql).await;
        let before = disk_state(&path);

        let err = open_read_only(&path).await.unwrap_err();
        assert_eq!(err.code(), STORAGE_NEEDS_UPGRADE, "{err}");
        assert!(
            matches!(&err, StorageError::DataStepPending { step, .. }
                if step == "strip_connection_string_passwords"),
            "{err:?}"
        );
        // It can't only say "open the app": the app may have, and failed.
        let message = err.to_string();
        assert!(
            message.contains("open it once to update your data"),
            "{message}"
        );
        assert!(
            message.contains(
                "check the app's log for \"data step strip_connection_string_passwords failed\""
            ),
            "{message}"
        );
        assert_eq!(disk_state(&path), before, "{sql}");
    }
}

#[tokio::test]
async fn a_read_only_open_of_an_empty_file_needs_an_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    std::fs::write(&path, b"").unwrap();
    let err = open_read_only(&path).await.unwrap_err();
    assert_needs_upgrade(&err, "empty");
    assert_eq!(disk_state(&path), (Vec::new(), vec!["seaquel.db".into()]));
}

#[tokio::test]
async fn a_read_only_open_never_creates_the_file_or_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("users/u1/seaquel.db");
    let err = open_read_only(&path).await.unwrap_err();
    assert_eq!(err.code(), STORAGE_NOT_FOUND, "{err}");
    let message = err.to_string();
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert!(message.contains("Open the Seaquel app once"), "{message}");
    assert_eq!(entries(dir.path()), Vec::<String>::new());

    let path = dir.path().join("seaquel.db");
    assert_eq!(
        open_read_only(&path).await.unwrap_err().code(),
        STORAGE_NOT_FOUND
    );
    assert_eq!(entries(dir.path()), Vec::<String>::new());

    // Legacy JSON still says what it is.
    std::fs::write(dir.path().join("projects.json"), b"{}").unwrap();
    let err = open_read_only(&path).await.unwrap_err();
    assert_eq!(err.code(), LEGACY_STORAGE, "{err}");
    assert_eq!(entries(dir.path()), vec!["projects.json"]);
}

#[tokio::test]
async fn a_read_only_open_of_a_corrupt_file_is_corrupt_and_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend((0..4096u32).map(|i| (i * 31 % 251) as u8));
    std::fs::write(&path, &bytes).unwrap();
    let err = open_read_only(&path).await.unwrap_err();
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
    assert_eq!(disk_state(&path), (bytes, vec!["seaquel.db".into()]));
}

// ── The migration lock ──

/// Two pools open one file with a migration pending (the web server's
/// evicted workspace next to a fresh one, or the app next to a second
/// process). The migrator is serialised: exactly one applies it, and both
/// opens succeed. Without the lock the loser ran it again and failed on
/// `CREATE TABLE race_marker`.
#[tokio::test]
async fn two_pools_racing_a_pending_migration_both_open_and_apply_it_once() {
    for round in 0..8 {
        let dir = tempfile::tempdir().unwrap();
        let path = current_file(dir.path()).await;

        // Both opens are polled together; each pool's SQLite work runs on
        // its own sqlx worker threads, so they really do overlap.
        let (a, b) = (test_migrator().await, test_migrator().await);
        let (a, b) = tokio::join!(
            Storage::open_with_migrator(&path, StorageOptions::default(), a),
            Storage::open_with_migrator(&path, StorageOptions::default(), b),
        );
        let opened: Vec<Storage> = [a, b]
            .into_iter()
            .map(|r| r.unwrap_or_else(|e| panic!("round {round}: {} {e}", e.code())))
            .collect();

        let pool = opened[0].pool();
        let applied: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT version, success FROM _sqlx_migrations WHERE version > 9000 \
                 ORDER BY version",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        assert_eq!(applied, vec![(9001, true)], "round {round}");
        let markers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM race_marker")
            .fetch_one(pool)
            .await
            .unwrap();
        let filler: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM race_filler")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!((markers, filler), (1, 200_000), "round {round}");
        for storage in opened {
            storage.close().await;
        }

        // And the file now opens read-only with that migrator.
        Storage::open_with_migrator(&path, read_only(), test_migrator().await)
            .await
            .unwrap()
            .close()
            .await;
    }
}

/// The baseline renames `active_canvas_tab_id` only when
/// `active_workflow_tab_id` is missing, so a file with both is current.
#[tokio::test]
async fn a_leftover_canvas_column_next_to_the_workflow_one_is_current() {
    let dir = tempfile::tempdir().unwrap();
    let path = current_file(dir.path()).await;
    exec_file(
        &path,
        "ALTER TABLE project_state ADD COLUMN active_canvas_tab_id TEXT",
    )
    .await;
    open_read_only(&path).await.unwrap().close().await;

    // And the baseline agrees: it changes nothing on that file.
    let before = snapshot(&path).await;
    let mut conn = raw_connect(&path).await;
    let mut tx = conn.begin().await.unwrap();
    seaquel_storage::schema::baseline(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    conn.close().await.unwrap();
    assert_eq!(snapshot(&path).await, before);
}

/// Under the lock, a failing migration takes the ones this open applied
/// before it down too: 9001 succeeds, 9002 fails, and neither leaves a
/// table or a `_sqlx_migrations` row. The next open works.
#[tokio::test]
async fn a_failed_migration_rolls_back_everything_under_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = current_file(dir.path()).await;
    let migrator = sqlx::migrate::Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/test_migrations_failing"),
    )
    .await
    .unwrap();
    assert_eq!(migrator.iter().count(), 2);

    let err = Storage::open_with_migrator(&path, StorageOptions::default(), migrator)
        .await
        .unwrap_err();
    assert_eq!(err.code(), STORAGE_ERROR, "{err}");
    assert!(err.to_string().contains("migration 9002"), "{err}");

    let mut conn = raw_connect(&path).await;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE name IN ('fail_first', 'fail_second')",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    assert!(tables.is_empty(), "{tables:?}");
    let recorded: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE version > 9000")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(recorded, 0);
    conn.close().await.unwrap();

    let storage = open(&path).await.unwrap();
    let value: String = sqlx::query_scalar("SELECT value FROM app_state WHERE key = 'k'")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(value, "v");
    storage.close().await;
    open_read_only(&path).await.unwrap().close().await;
}

/// 5d-2 Task 7 review: a migration can hold the lock past the 5 s busy
/// timeout (`0003`'s fill takes about 3 s per GB). A second pool opening the
/// file meanwhile (the web server's evicted workspace next to a fresh one)
/// keeps waiting for it, up to `MIGRATION_WAIT`, instead of failing with
/// `STORAGE_ERROR`.
#[tokio::test]
async fn a_second_opener_waits_out_a_migration_longer_than_the_busy_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = current_file(dir.path()).await;
    // Another pool's migrator, holding the write lock for 6 s.
    let holder = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    let lock = holder.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let release = async move {
        tokio::time::sleep(Duration::from_secs(6)).await;
        lock.commit().await.unwrap();
    };
    let migrator = test_migrator().await;
    let (opened, ()) = tokio::join!(
        Storage::open_with_migrator(&path, StorageOptions::default(), migrator),
        release,
    );
    let st = opened.unwrap_or_else(|e| panic!("{} {e}", e.code()));
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE version > 9000")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(applied, [9001]);
    st.close().await;
    holder.close().await;
}

/// 5d-2 Task 7 re-review: the long wait is only for an open with work to
/// do. An up-to-date file whose write lock another process holds (a second
/// app instance, an outside tool) fails after about one busy timeout, as
/// before, rather than waiting a minute at startup.
#[tokio::test]
async fn an_up_to_date_file_waits_one_busy_timeout_for_a_held_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = current_file(dir.path()).await;
    let holder = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    let lock = holder.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let started = tokio::time::Instant::now();
    let opened = tokio::time::timeout(
        Duration::from_secs(20),
        Storage::open(&path, StorageOptions::default()),
    )
    .await
    .expect("gave up within 20 s");
    let waited = started.elapsed();
    let Err(err) = opened else {
        panic!("the held lock fails the open");
    };
    assert_eq!(err.code(), "STORAGE_ERROR", "{err}");
    assert!(
        waited >= Duration::from_secs(4) && waited < Duration::from_secs(9),
        "{waited:?}"
    );
    lock.rollback().await.unwrap();
    holder.close().await;
}
