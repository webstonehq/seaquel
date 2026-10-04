//! `Storage::external_version` and the writer connection (phase 7a
//! Decisions 5 and 6): every write of a `Storage` goes through one
//! connection, so `PRAGMA data_version` on it changes only when another
//! connection, in this process or another, committed.

#![cfg(not(target_arch = "wasm32"))]
// A test binary that re-runs itself as the second process, and times it.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use seaquel_storage::{app_state, projects, query_history, SchemaPolicy, Storage, StorageOptions};

async fn open(path: &Path) -> Storage {
    Storage::open(path, StorageOptions::default())
        .await
        .unwrap()
}

async fn put(st: &Storage, key: &str) {
    let mut tx = st.write().await.unwrap();
    app_state::set_in(&mut tx, key, Some("v")).await.unwrap();
    tx.commit().await.unwrap();
}

async fn version(st: &Storage) -> i64 {
    st.external_version()
        .await
        .unwrap()
        .expect("no write holds the lock")
}

#[tokio::test]
async fn own_writes_dont_move_it_and_anothers_do() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let a = open(&path).await;
    let b = open(&path).await;
    let base = version(&a).await;

    for i in 0..5 {
        put(&a, &format!("a{i}")).await;
    }
    // Also the storage-group writes that used to go straight to the pool.
    app_state::set(&a, "direct", Some("v")).await.unwrap();
    query_history::set_favorite(&a, "none", true).await.unwrap();
    assert_eq!(version(&a).await, base, "A's own writes");

    put(&b, "b").await;
    let after_b = version(&a).await;
    assert_ne!(after_b, base, "B's write");
    // Seen once: the next poll is unchanged.
    assert_eq!(version(&a).await, after_b);

    // A's write doesn't show up in A, and B sees it.
    let b_base = version(&b).await;
    put(&a, "a-again").await;
    assert_eq!(version(&a).await, after_b);
    assert_ne!(version(&b).await, b_base);
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn reads_and_its_own_checkpoints_dont_move_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let a = open(&path).await;
    let b = open(&path).await;
    put(&a, "k").await;
    let base = version(&a).await;

    assert_eq!(projects::count(&b).await.unwrap(), 0);
    assert_eq!(projects::count(&a).await.unwrap(), 0);
    assert_eq!(app_state::get(&b, "k").await.unwrap().as_deref(), Some("v"));
    assert_eq!(version(&a).await, base, "reads");
    assert!(a.checkpoint().await.unwrap());
    a.vacuum().await.unwrap();
    assert!(a.checkpoint().await.unwrap());
    assert_eq!(version(&a).await, base, "A's own checkpoints and VACUUM");

    // Another connection's `wal_checkpoint(TRUNCATE)` restarts the WAL,
    // which SQLite reports as a change: one reload too many, never one too
    // few (the S4 spike saw it coalesce with the commits before it). Core
    // checkpoints only in the app's open, so a second process rarely sees
    // one.
    assert!(b.checkpoint().await.unwrap());
    let after = version(&a).await;
    assert_eq!(after, base + 1);
    assert_eq!(version(&a).await, after);
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn it_is_none_while_a_write_holds_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let a = open(&path).await;
    let base = version(&a).await;
    let tx = a.write().await.unwrap();
    assert_eq!(a.external_version().await.unwrap(), None);
    drop(tx);
    // The dropped write's rollback runs on a task; the next poll waits for
    // nothing and answers again.
    let mut answered = None;
    for _ in 0..100 {
        answered = a.external_version().await.unwrap();
        if answered.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(answered, Some(base));
    a.close().await;
}

/// Each write's connection, as a TEMP table on it (TEMP tables live and die
/// with their connection).
async fn mark_writer(st: &Storage) {
    let mut tx = st.write().await.unwrap();
    sqlx::query("CREATE TEMP TABLE writer_mark (x)")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn writer_is_marked(st: &Storage) -> bool {
    let mut tx = st.write().await.unwrap();
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM temp.sqlite_master WHERE name = 'writer_mark'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    n == 1
}

#[tokio::test]
async fn every_write_uses_the_same_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let a = Storage::open(
        &path,
        StorageOptions {
            max_connections: 4,
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap();
    mark_writer(&a).await;
    for _ in 0..8 {
        assert!(writer_is_marked(&a).await);
    }
    // No pool connection is the writer.
    let mut held = Vec::new();
    for _ in 0..a.pool().options().get_max_connections() {
        held.push(a.pool().acquire().await.unwrap());
    }
    for conn in &mut held {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM temp.sqlite_master WHERE name = 'writer_mark'",
        )
        .fetch_one(&mut **conn)
        .await
        .unwrap();
        assert_eq!(n, 0);
    }
    drop(held);
    a.close().await;
}

#[tokio::test]
async fn after_a_failed_rollback_the_next_write_gets_a_fresh_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let a = open(&path).await;
    mark_writer(&a).await;
    let base = version(&a).await;

    let mut tx = a.write().await.unwrap();
    app_state::set_in(&mut tx, "uncommitted", Some("v"))
        .await
        .unwrap();
    tx.fail_next_rollback_for_tests();
    assert!(tx.rollback().await.is_err());

    assert!(!writer_is_marked(&a).await, "a fresh writer connection");
    assert_eq!(app_state::get(&a, "uncommitted").await.unwrap(), None);
    // A replaced connection can't compare its data_version with the old
    // one's, so the version moves once: a reload too many, never one too few.
    let after = version(&a).await;
    assert_ne!(after, base);
    assert_eq!(version(&a).await, after);
    a.close().await;
}

#[tokio::test]
async fn a_dropped_write_whose_rollback_fails_replaces_the_connection_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let a = open(&path).await;
    mark_writer(&a).await;
    let mut tx = a.write().await.unwrap();
    tx.fail_next_rollback_for_tests();
    drop(tx);
    assert!(!writer_is_marked(&a).await);
    a.close().await;
}

#[tokio::test]
async fn a_read_only_storage_polls_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let app = open(&path).await;
    let ro = Storage::open(
        &path,
        StorageOptions {
            read_only: true,
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap();
    let base = version(&ro).await;
    assert_eq!(projects::count(&ro).await.unwrap(), 0);
    assert_eq!(version(&ro).await, base);
    put(&app, "k").await;
    assert_ne!(version(&ro).await, base);
    ro.close().await;
    app.close().await;
}

/// The web's two connections per user are now one reader and one writer:
/// two readers holding read transactions don't keep a writer out.
#[tokio::test]
async fn two_readers_dont_starve_a_writer_on_the_webs_pool() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.db");
    let st = Storage::open(
        &path,
        StorageOptions {
            max_connections: 2,
            idle_timeout: Some(Duration::from_secs(60)),
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(st.pool().options().get_max_connections(), 1);
    let reader = st.pool().begin().await.unwrap();
    let second = {
        let st = st.clone();
        tokio::spawn(async move { projects::count(&st).await.unwrap() })
    };
    tokio::time::timeout(Duration::from_secs(5), put(&st, "k"))
        .await
        .expect("the write isn't kept out by the readers");
    drop(reader);
    assert_eq!(second.await.unwrap(), 0);
    st.close().await;
}

// ── Across processes ──

/// Set in the child: the file it writes to.
const CHILD_FILE: &str = "SEAQUEL_TEST_SECOND_WRITER_FILE";
const CHILD_COMMITS: usize = 10;
const CHILD_GAP: Duration = Duration::from_millis(120);

/// The second process. Run by [`another_process_s_commits_are_seen`] as a
/// child of this test binary; on its own (no [`CHILD_FILE`]) it does
/// nothing.
#[tokio::test]
async fn second_writer_child() {
    let Ok(file) = std::env::var(CHILD_FILE) else {
        return;
    };
    let st = Storage::open(
        PathBuf::from(file),
        StorageOptions {
            schema: SchemaPolicy::RequireCurrent,
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap();
    for i in 0..CHILD_COMMITS {
        tokio::time::sleep(CHILD_GAP).await;
        put(&st, &format!("child-{i}")).await;
        // The parent reads the line to time the commit.
        println!(
            "COMMITTED {i} {}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros()
        );
    }
    st.close().await;
}

#[tokio::test]
async fn another_process_s_commits_are_seen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let a = open(&path).await;
    let mut last = version(&a).await;

    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "second_writer_child", "--nocapture", "--quiet"])
        .env(CHILD_FILE, &path)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let mut seen = Vec::new();
    // Poll every 20 ms; A also writes now and then, which must not count.
    let mut own_writes = 0;
    while started.elapsed() < Duration::from_secs(20) && seen.len() < CHILD_COMMITS {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if own_writes < 5 {
            put(&a, &format!("parent-{own_writes}")).await;
            own_writes += 1;
        }
        let Some(v) = a.external_version().await.unwrap() else {
            continue;
        };
        if v != last {
            last = v;
            seen.push(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_micros(),
            );
        }
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "the child failed");
    let commits: Vec<u128> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("COMMITTED "))
        .map(|l| l.split(' ').nth(1).unwrap().parse().unwrap())
        .collect();
    assert_eq!(
        commits.len(),
        CHILD_COMMITS,
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    // Every commit was seen (120 ms apart, polled every 20 ms), and nothing
    // else was: the parent's own writes never counted.
    assert_eq!(
        seen.len(),
        CHILD_COMMITS,
        "seen {seen:?}, commits {commits:?}"
    );
    let mut lags: Vec<u128> = commits
        .iter()
        .zip(&seen)
        .map(|(c, s)| s.saturating_sub(*c))
        .collect();
    lags.sort_unstable();
    eprintln!(
        "cross-process detection at 20 ms polling: p50 {} µs, max {} µs ({} commits)",
        lags[lags.len() / 2],
        lags[lags.len() - 1],
        lags.len()
    );
    assert!(lags[lags.len() - 1] < 1_000_000, "{lags:?}");
    let all: Vec<String> = sqlx::query_scalar("SELECT key FROM app_state WHERE key LIKE 'child-%'")
        .fetch_all(a.pool())
        .await
        .unwrap();
    assert_eq!(all.len(), CHILD_COMMITS);
    a.close().await;
}

// ── Task 1 review fixes ──

/// The web's writer (with `idle_timeout`) stays open between writes and
/// closes only once it has been idle that long, so a burst of writes costs
/// one open, and an idle user still holds no file handle.
#[tokio::test]
async fn the_writer_closes_only_after_its_idle_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.db");
    let idle = Duration::from_millis(300);
    let st = Storage::open(
        &path,
        StorageOptions {
            max_connections: 2,
            idle_timeout: Some(idle),
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap();
    mark_writer(&st).await;
    // Writes inside the window use the same connection, and each one
    // starts the window again.
    for _ in 0..4 {
        tokio::time::sleep(idle / 3).await;
        assert!(writer_is_marked(&st).await, "within the idle window");
    }
    // Idle past the window: closed, so the next write gets a fresh one.
    tokio::time::sleep(idle * 2).await;
    assert!(!writer_is_marked(&st).await, "closed after the idle window");
    st.close().await;
}

async fn secure_delete_of_next_write(st: &Storage) -> i64 {
    let mut tx = st.write().await.unwrap();
    let on: i64 = sqlx::query_scalar("PRAGMA secure_delete")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    on
}

/// A write that turned `secure_delete` on and then failed doesn't leave it
/// on for every later write on the writer connection.
#[tokio::test]
async fn secure_delete_is_off_again_after_a_failed_or_dropped_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let st = open(&path).await;
    assert_eq!(secure_delete_of_next_write(&st).await, 0);

    let mut tx = st.write().await.unwrap();
    tx.secure_delete(true).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        secure_delete_of_next_write(&st).await,
        0,
        "after a rollback"
    );

    let mut tx = st.write().await.unwrap();
    tx.secure_delete(true).await.unwrap();
    drop(tx);
    assert_eq!(secure_delete_of_next_write(&st).await, 0, "after a drop");

    let mut tx = st.write().await.unwrap();
    tx.secure_delete(true).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(secure_delete_of_next_write(&st).await, 0, "after a commit");
    st.close().await;
}
