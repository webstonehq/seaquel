//! Phase 5d-1 storage: `WriteTx` and the targeted library queries
//! (connections, projects, custom labels, saved queries and their
//! versions) Core's `library` methods run inside one write transaction, and
//! the `drop_legacy_built_connection_strings` data step.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use seaquel_storage::{
    connections, project_labels, projects, query_versions, saved_queries, user_credentials, IdName,
    Storage, StorageError, StorageOptions, DATA_STEPS_TABLE, STORAGE_NEEDS_UPGRADE,
    STORAGE_READ_ONLY,
};
use seaquel_types::storage::{
    ConnectionLabel, PersistedConnection, PersistedCredential, PersistedProject,
    PersistedSavedQuery,
};
use serde_json::value::RawValue;

// ── Helpers ──

async fn fresh(dir: &Path) -> (Storage, PathBuf) {
    let path = dir.join("seaquel.db");
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    (st, path)
}

fn project(id: &str, name: &str) -> PersistedProject {
    PersistedProject {
        id: id.into(),
        name: name.into(),
        description: None,
        created_at: "2026-10-04T00:00:00.000Z".into(),
        updated_at: "2026-10-04T00:00:00.000Z".into(),
        custom_labels: vec![],
        git_repo_path: None,
    }
}

fn label(id: &str, name: &str, color: &str) -> ConnectionLabel {
    ConnectionLabel {
        id: id.into(),
        name: name.into(),
        is_predefined: false,
        color: color.into(),
    }
}

fn connection(id: &str, project_id: &str, name: &str) -> PersistedConnection {
    PersistedConnection {
        id: id.into(),
        project_id: project_id.into(),
        name: name.into(),
        ty: "postgres".into(),
        host: "localhost".into(),
        port: 5432.0,
        database_name: "app".into(),
        username: "me".into(),
        ssl_mode: None,
        connection_string: None,
        last_connected: None,
        ssh_tunnel: None,
        save_password: false,
        save_ssh_password: false,
        save_ssh_key_passphrase: false,
        label_ids: vec![],
        is_local_only: None,
        shared_connection_id: None,
        ai_share_schema: None,
        ai_share_data: None,
        active_ai_provider_id: None,
        active_ai_model: None,
        shared_origin: None,
    }
}

fn saved_query(
    id: &str,
    project_id: &str,
    name: &str,
    folder: Option<&str>,
) -> PersistedSavedQuery {
    PersistedSavedQuery {
        id: id.into(),
        name: name.into(),
        query: "SELECT 1".into(),
        project_id: project_id.into(),
        created_at: "c".into(),
        updated_at: "u".into(),
        parameters: None,
        starred: false,
        shared: false,
        description: None,
        database_type: None,
        tags: None,
        folder: folder.map(Into::into),
        shared_path: None,
    }
}

fn json(s: &str) -> Option<Box<RawValue>> {
    Some(RawValue::from_string(s.into()).unwrap())
}

fn conn_json(c: &PersistedConnection) -> serde_json::Value {
    serde_json::to_value(c).unwrap()
}

async fn count(st: &Storage, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(st.pool()).await.unwrap()
}

/// A project `p` holding one connection `c`.
async fn seeded(st: &Storage) {
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p", "P")).await.unwrap();
    connections::insert(&mut tx, &connection("c", "p", "C"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

// ── WriteTx ──

/// Two pools on one file (the web server's evicted workspace next to a
/// fresh one): the second writer waits for the first's lock and then sees
/// its commit, so a read-check-write inside one `WriteTx` can't interleave.
// A native test that needs a second task; the wasm32 rule behind
// `disallowed_methods` doesn't apply.
#[allow(clippy::disallowed_methods)]
#[tokio::test]
async fn a_write_transaction_serialises_two_writers() {
    let dir = tempfile::tempdir().unwrap();
    let (a, path) = fresh(dir.path()).await;
    let b = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();

    let mut first = a.write().await.unwrap();
    assert_eq!(projects::count(&mut first).await.unwrap(), 0);
    projects::insert(&mut first, &project("p1", "One"))
        .await
        .unwrap();

    let second = tokio::spawn(async move {
        let mut tx = b.write().await.unwrap();
        // Read, then write on what was read: it must see the first commit.
        let seen = projects::count(&mut tx).await.unwrap();
        projects::insert(&mut tx, &project("p2", "Two"))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        b.close().await;
        seen
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!second.is_finished(), "the second writer didn't wait");
    first.commit().await.unwrap();

    assert_eq!(second.await.unwrap(), 1);
    assert_eq!(projects::count(&a).await.unwrap(), 2);
    a.close().await;
}

fn pool_of(max_connections: u32) -> StorageOptions {
    StorageOptions {
        max_connections,
        ..StorageOptions::default()
    }
}

/// With one pool connection, two writers and a pool read issued together
/// all finish: a writer waiting its turn holds no connection.
#[tokio::test]
async fn two_writes_and_a_read_finish_on_a_pool_of_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let st = Storage::open(&path, pool_of(1)).await.unwrap();
    let write = |id: &'static str| {
        let st = &st;
        async move {
            let mut tx = st.write().await.unwrap();
            let before = projects::count(&mut tx).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            projects::insert(&mut tx, &project(id, id)).await.unwrap();
            tx.commit().await.unwrap();
            before
        }
    };
    let all = async {
        tokio::join!(write("a"), write("b"), async {
            projects::count(&st).await.unwrap()
        })
    };
    let (a, b, _) = tokio::time::timeout(Duration::from_secs(4), all)
        .await
        .expect("a write or the read stalled");
    // They ran one after the other: one saw the other's commit.
    let mut seen = [a, b];
    seen.sort();
    assert_eq!(seen, [0, 1]);
    assert_eq!(projects::count(&st).await.unwrap(), 2);
    st.close().await;
}

/// A writer queued behind an open write doesn't take the pool's other
/// connection, so a read gets it at once (before, it held that connection
/// busy-waiting for SQLite's lock and the read waited, up to 5 s).
#[tokio::test]
async fn a_queued_writer_leaves_the_pool_to_readers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let st = Storage::open(&path, pool_of(2)).await.unwrap();
    let mut first = st.write().await.unwrap();
    projects::insert(&mut first, &project("a", "A"))
        .await
        .unwrap();

    let queued = async {
        let mut tx = st.write().await.unwrap();
        projects::insert(&mut tx, &project("b", "B")).await.unwrap();
        tx.commit().await.unwrap();
    };
    let read_while_open = async {
        // Let the second writer queue first.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let n = tokio::time::timeout(Duration::from_secs(1), projects::count(&st))
            .await
            .expect("the read waited for a writer's connection")
            .unwrap();
        first.commit().await.unwrap();
        n
    };
    let ((), n) = tokio::join!(queued, read_while_open);
    assert_eq!(n, 0, "the read saw the committed file only");
    assert_eq!(projects::count(&st).await.unwrap(), 2);
    st.close().await;
}

/// A write begun while this task holds one (directly, or through a
/// replace-all save's `codec::begin`) can never get the mutex: it fails
/// after the wait instead of hanging, and the held one still commits.
#[tokio::test]
async fn a_nested_write_times_out_instead_of_hanging() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let st = st.with_write_wait(Duration::from_millis(200));
    assert_eq!(seaquel_storage::WRITE_WAIT, Duration::from_secs(30));
    let mut held = st.write().await.unwrap();
    projects::insert(&mut held, &project("p", "P"))
        .await
        .unwrap();

    let err = st.write().await.unwrap_err();
    assert!(
        matches!(err, StorageError::WriteLockTimeout { .. }),
        "{err:?}"
    );
    assert_eq!(err.code(), "STORAGE_ERROR");
    let err = projects::save(&st, &project("q", "Q")).await.unwrap_err();
    assert!(
        matches!(err, StorageError::WriteLockTimeout { .. }),
        "{err:?}"
    );

    held.commit().await.unwrap();
    assert_eq!(projects::count(&st).await.unwrap(), 1);
    st.write().await.unwrap().rollback().await.unwrap();
    st.close().await;
}

#[tokio::test]
async fn a_dropped_write_transaction_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    {
        let mut tx = st.write().await.unwrap();
        projects::insert(&mut tx, &project("p", "P")).await.unwrap();
        // Reads inside the transaction see its own writes.
        assert!(projects::get(&mut tx, "p").await.unwrap().is_some());
        assert!(projects::get(&st, "p").await.unwrap().is_none());
    }
    assert_eq!(projects::count(&st).await.unwrap(), 0);

    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p", "P")).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(projects::count(&st).await.unwrap(), 0);
    st.close().await;
}

#[tokio::test]
async fn a_read_only_storage_refuses_a_write_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let (st, path) = fresh(dir.path()).await;
    st.close().await;
    let ro = Storage::open(
        &path,
        StorageOptions {
            read_only: true,
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(projects::count(&ro).await.unwrap(), 0);
    let err = match ro.write().await {
        Ok(mut tx) => projects::insert(&mut tx, &project("p", "P"))
            .await
            .unwrap_err(),
        Err(e) => e,
    };
    assert_eq!(err.code(), STORAGE_READ_ONLY, "{err}");
    assert!(matches!(err, StorageError::ReadOnly { .. }), "{err:?}");
    assert!(ro.is_read_only());
    ro.close().await;
}

// ── Connections ──

#[tokio::test]
async fn insert_then_get_round_trips_and_strips_the_password() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let mut c = connection("c", "p", "C");
    c.connection_string = Some("postgres://me:hunter2@h/db".into());
    c.label_ids = vec!["dev".into(), "l1".into(), "dev".into()];
    c.ssh_tunnel = json(r#"{"enabled":true}"#);
    c.ai_share_data = Some(false);
    c.last_connected = Some("2026-10-04T00:00:00.000Z".into());
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p", "P")).await.unwrap();
    connections::insert(&mut tx, &c).await.unwrap();
    // An id that exists is refused, not overwritten.
    assert!(connections::insert(&mut tx, &connection("c", "p", "Other"))
        .await
        .is_err());
    tx.commit().await.unwrap();

    let got = connections::get(&st, "c").await.unwrap().unwrap();
    let mut want = c.clone();
    // The TypeScript's strip: the scheme as typed.
    want.connection_string = Some("postgres://me@h/db".into());
    want.label_ids = vec!["dev".into(), "l1".into()];
    assert_eq!(conn_json(&got), conn_json(&want));
    // The same row `load_all` gives.
    let all = connections::load_all(&st).await.unwrap();
    assert_eq!(conn_json(&all[0]), conn_json(&got));
    assert!(connections::get(&st, "missing").await.unwrap().is_none());
    st.close().await;
}

#[tokio::test]
async fn update_changes_one_row_and_its_labels() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p", "P")).await.unwrap();
    projects::insert(&mut tx, &project("q", "Q")).await.unwrap();
    let mut a = connection("a", "p", "A");
    a.label_ids = vec!["dev".into()];
    let mut b = connection("b", "p", "B");
    b.label_ids = vec!["dev".into()];
    connections::insert(&mut tx, &a).await.unwrap();
    connections::insert(&mut tx, &b).await.unwrap();
    tx.commit().await.unwrap();

    let mut changed = a.clone();
    changed.name = "A2".into();
    changed.host = "db".into();
    changed.connection_string = Some("Server=h;Password=x".into());
    changed.label_ids = vec!["prod".into(), "l9".into()];
    changed.ai_share_schema = Some(true);
    // Storage never moves a connection to another project.
    changed.project_id = "q".into();
    let mut tx = st.write().await.unwrap();
    assert!(connections::update(&mut tx, &changed).await.unwrap());
    assert!(!connections::update(&mut tx, &connection("nope", "p", "N"))
        .await
        .unwrap());
    tx.commit().await.unwrap();

    let got = connections::get(&st, "a").await.unwrap().unwrap();
    let mut want = changed.clone();
    want.project_id = "p".into();
    want.connection_string = Some("Server=h;".into());
    want.label_ids = vec!["l9".into(), "prod".into()];
    assert_eq!(conn_json(&got), conn_json(&want));
    let other = connections::get(&st, "b").await.unwrap().unwrap();
    assert_eq!(conn_json(&other), conn_json(&b));
    assert_eq!(count(&st, "SELECT COUNT(*) FROM connections").await, 2);
    assert!(connections::get(&st, "nope").await.unwrap().is_none());
    st.close().await;
}

#[tokio::test]
async fn delete_cascades_history_chats_and_labels() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    connections::insert(&mut tx, &connection("keep", "p", "K"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for id in ["c", "keep"] {
        for sql in [
            format!("INSERT INTO connection_labels (connection_id, label_id) VALUES ('{id}', 'dev')"),
            format!(
                "INSERT INTO query_history (id, connection_id, query, timestamp, execution_time, row_count) \
                 VALUES ('h-{id}', '{id}', 'SELECT 1', 't', 1, 1)"
            ),
            format!(
                "INSERT INTO ai_chats (id, connection_id, title, created_at, updated_at) \
                 VALUES ('chat-{id}', '{id}', 't', 'c', 'u')"
            ),
            format!(
                "INSERT INTO ai_messages (id, chat_id, role, content, timestamp) \
                 VALUES ('m-{id}', 'chat-{id}', 'user', 'hi', 't')"
            ),
        ] {
            sqlx::query(&sql).execute(st.pool()).await.unwrap();
        }
    }

    let mut tx = st.write().await.unwrap();
    assert!(connections::delete(&mut tx, "c").await.unwrap());
    assert!(!connections::delete(&mut tx, "c").await.unwrap());
    tx.commit().await.unwrap();

    for table in [
        "connection_labels",
        "query_history",
        "ai_chats",
        "ai_messages",
    ] {
        assert_eq!(
            count(&st, &format!("SELECT COUNT(*) FROM {table}")).await,
            1,
            "{table}"
        );
    }
    assert_eq!(
        count(
            &st,
            "SELECT COUNT(*) FROM query_history WHERE connection_id = 'keep'"
        )
        .await,
        1
    );
    st.close().await;
}

/// `insert` and `update` store strings by the TypeScript's rules
/// (`strip_connection_string_secrets`), which catch what the phase 3 strip
/// `save` still uses doesn't; `save` is unchanged.
#[tokio::test]
async fn insert_and_update_store_no_secret() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let cases: [(&str, Option<&str>); 6] = [
        (
            "host=db password=hunter2 dbname=app",
            Some("host=db dbname=app"),
        ),
        (
            "duckdb:///x.duckdb?s3_secret_access_key=hunter2&threads=4",
            Some("duckdb:///x.duckdb?threads=4"),
        ),
        (
            "postgresql+ssh://s@b/dbu:hunter2@h/db",
            Some("postgresql+ssh://s@b/dbu@h/db"),
        ),
        ("postgres://u:hunter2#x@h/db", Some("")),
        ("", None),
        ("postgres://u@h/db", Some("postgres://u@h/db")),
    ];
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p", "P")).await.unwrap();
    for (i, (input, want)) in cases.iter().enumerate() {
        let mut c = connection(&format!("i{i}"), "p", "C");
        c.connection_string = Some(input.to_string());
        connections::insert(&mut tx, &c).await.unwrap();
        let mut u = connection(&format!("u{i}"), "p", "C");
        connections::insert(&mut tx, &u).await.unwrap();
        u.connection_string = Some(input.to_string());
        assert!(connections::update(&mut tx, &u).await.unwrap());
        for id in [c.id, u.id] {
            let got = connections::get(&mut tx, &id).await.unwrap().unwrap();
            assert_eq!(got.connection_string.as_deref(), *want, "{input:?}");
        }
    }
    tx.commit().await.unwrap();

    // `save` keeps the phase 3 strip, which the repo tests pin.
    let mut c = connection("s", "p", "C");
    c.connection_string = Some("host=db password=hunter2".into());
    connections::save(&st, &c).await.unwrap();
    let got = connections::get(&st, "s").await.unwrap().unwrap();
    assert_eq!(
        got.connection_string.as_deref(),
        Some("host=db password=hunter2")
    );
    st.close().await;
}

/// The rows the Core upgrade (Decision 12a) takes secrets out of: ids and
/// engines of strings that still hold one, and no others.
#[tokio::test]
async fn with_secret_in_string_lists_only_rows_holding_a_secret() {
    let dir = tempfile::tempdir().unwrap();
    let (st, path) = fresh(dir.path()).await;
    seeded(&st).await;
    st.close().await;
    // Written by hand, as a pre-5a build left them (the queries strip).
    exec_file(
        &path,
        "INSERT INTO connections (id, project_id, name, type, host, port, database_name, username, connection_string) VALUES \
         ('libpq', 'p', 'n', 'postgres', 'h', 5432, 'd', 'u', 'host=db password=x'), \
         ('plain', 'p', 'n', 'postgres', 'h', 5432, 'd', 'u', 'postgres://u@h/d'), \
         ('duck', 'p', 'n', 'duckdb', '', 0, '/x', '', 'duckdb:///x?s3_secret_access_key=k'), \
         ('null', 'p', 'n', 'postgres', 'h', 5432, 'd', 'u', NULL), \
         ('empty', 'p', 'n', 'postgres', 'h', 5432, 'd', 'u', ''), \
         ('ssh', 'p', 'n', 'postgres', 'h', 5432, 'd', 'u', 'postgresql+ssh://s@b/dbu:pw@h/db'), \
         (NULL, 'p', 'n', 'postgres', 'h', 5432, 'd', 'u', 'host=db password=x'), \
         ('bytes', 'p', 'n', 'postgres', 'h', 5432, 'd', 'u', CAST(x'ff' AS TEXT));",
    )
    .await;
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let got = connections::with_secret_in_string(&st).await.unwrap();
    let got: Vec<(String, String)> = got.into_iter().map(|r| (r.id, r.ty)).collect();
    assert_eq!(
        got,
        [
            ("libpq".to_string(), "postgres".to_string()),
            ("duck".to_string(), "duckdb".to_string()),
            ("ssh".to_string(), "postgres".to_string()),
        ]
    );
    st.close().await;
}

#[tokio::test]
async fn names_ids_and_counts_are_per_project() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p", "P")).await.unwrap();
    projects::insert(&mut tx, &project("q", "Q")).await.unwrap();
    connections::insert(&mut tx, &connection("a", "p", "Alpha"))
        .await
        .unwrap();
    connections::insert(&mut tx, &connection("b", "p", "Beta"))
        .await
        .unwrap();
    connections::insert(&mut tx, &connection("c", "q", "Alpha"))
        .await
        .unwrap();
    assert_eq!(
        connections::names_in_project(&mut tx, "p").await.unwrap(),
        vec![
            IdName {
                id: "a".into(),
                name: "Alpha".into()
            },
            IdName {
                id: "b".into(),
                name: "Beta".into()
            },
        ]
    );
    tx.commit().await.unwrap();
    assert_eq!(
        connections::ids_in_project(&st, "q").await.unwrap(),
        vec!["c".to_string()]
    );
    assert!(connections::ids_in_project(&st, "none")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(connections::count(&st).await.unwrap(), 3);
    st.close().await;
}

// ── Projects ──

#[tokio::test]
async fn projects_insert_get_update_and_insert_if_missing() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let mut p = project("p", "P");
    p.custom_labels = vec![label("l1", "Mine", "#112233")];
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &p).await.unwrap();
    assert!(projects::insert(&mut tx, &project("p", "Again"))
        .await
        .is_err());
    // The default project: created once, never overwritten.
    assert!(
        projects::insert_if_missing(&mut tx, &project("default-seaquel", "Seaquel"))
            .await
            .unwrap()
    );
    assert!(
        !projects::insert_if_missing(&mut tx, &project("default-seaquel", "Other"))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert_eq!(
        projects::get(&st, "default-seaquel")
            .await
            .unwrap()
            .unwrap()
            .name,
        "Seaquel"
    );

    // Update writes the project's own columns; labels and created_at stay.
    let mut changed = project("p", "Renamed");
    changed.description = Some("d".into());
    changed.git_repo_path = Some("/repo".into());
    changed.created_at = "other".into();
    changed.updated_at = "later".into();
    let mut tx = st.write().await.unwrap();
    assert!(projects::update(&mut tx, &changed).await.unwrap());
    assert!(!projects::update(&mut tx, &project("nope", "N"))
        .await
        .unwrap());
    tx.commit().await.unwrap();

    let got = projects::get(&st, "p").await.unwrap().unwrap();
    let mut want = changed.clone();
    want.created_at = p.created_at.clone();
    want.custom_labels = p.custom_labels.clone();
    assert_eq!(
        serde_json::to_value(&got).unwrap(),
        serde_json::to_value(&want).unwrap()
    );
    assert_eq!(projects::count(&st).await.unwrap(), 2);
    let all = projects::load_all(&st).await.unwrap();
    assert_eq!(
        serde_json::to_value(&all[0]).unwrap(),
        serde_json::to_value(&got).unwrap()
    );
    st.close().await;
}

/// A project `id` with a row in every table that belongs to it.
async fn project_contents(st: &Storage, id: &str) {
    let mut p = project(id, id);
    p.custom_labels = vec![label(&format!("l-{id}"), "L", "#000000")];
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &p).await.unwrap();
    let mut c = connection(&format!("c-{id}"), id, "C");
    c.label_ids = vec![format!("l-{id}")];
    connections::insert(&mut tx, &c).await.unwrap();
    saved_queries::insert(&mut tx, &saved_query(&format!("s-{id}"), id, "S", None))
        .await
        .unwrap();
    query_versions::append_keyframe(
        &mut tx,
        &format!("v-{id}"),
        &format!("s-{id}"),
        "SELECT 0",
        "t",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    for sql in [
        format!(
            "INSERT INTO dashboards (id, project_id, name, created_at, updated_at) \
             VALUES ('d-{id}', '{id}', 'D', 'c', 'u')"
        ),
        format!(
            "INSERT INTO dashboard_versions (id, dashboard_id, version, snapshot, created_at) \
             VALUES ('dv-{id}', 'd-{id}', 1, '{{}}', 't')"
        ),
        format!("INSERT INTO saved_canvases (id, project_id, data) VALUES ('w-{id}', '{id}', '{{}}')"),
        format!("INSERT INTO project_state (project_id) VALUES ('{id}')"),
        format!(
            "INSERT INTO tabs (id, project_id, tab_type, name) VALUES ('t-{id}', '{id}', 'query', 'T')"
        ),
        format!(
            "INSERT INTO query_history (id, connection_id, query, timestamp, execution_time, row_count) \
             VALUES ('h-{id}', 'c-{id}', 'SELECT 1', 't', 1, 1)"
        ),
    ] {
        sqlx::query(&sql).execute(st.pool()).await.unwrap();
    }
}

const PROJECT_TABLES: &[&str] = &[
    "projects",
    "project_labels",
    "connections",
    "connection_labels",
    "saved_queries",
    "query_versions",
    "dashboards",
    "dashboard_versions",
    "saved_canvases",
    "project_state",
    "tabs",
    "query_history",
];

async fn counts(st: &Storage) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    for t in PROJECT_TABLES {
        out.push((
            t.to_string(),
            count(st, &format!("SELECT COUNT(*) FROM {t}")).await,
        ));
    }
    out
}

/// On a file that started on v2026.4.5-beta.1, `saved_queries` and
/// `dashboards` have no foreign key to `projects`, so the plain delete
/// leaves them behind; `delete_with_orphans` doesn't.
#[tokio::test]
async fn delete_with_orphans_removes_saved_queries_dashboards_and_workflows_on_a_beta_file() {
    for fixture in ["schemas/v2026.4.5-beta.1.sql", "schemas/current.sql"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        load_fixture(&path, fixture).await;
        let st = Storage::open(&path, StorageOptions::default())
            .await
            .unwrap();
        project_contents(&st, "gone").await;
        project_contents(&st, "kept").await;
        let kept_only: Vec<(String, i64)> = counts(&st)
            .await
            .into_iter()
            .map(|(t, n)| (t, n / 2))
            .collect();

        if fixture.contains("beta") {
            // The fixture really is beta-shaped: on a copy, the plain delete
            // leaves the saved query and the dashboard behind.
            let probe_path = dir.path().join("probe.db");
            load_fixture(&probe_path, fixture).await;
            let probe = Storage::open(&probe_path, StorageOptions::default())
                .await
                .unwrap();
            project_contents(&probe, "gone").await;
            projects::remove(&probe, "gone").await.unwrap();
            for table in ["saved_queries", "dashboards"] {
                assert_eq!(
                    count(&probe, &format!("SELECT COUNT(*) FROM {table}")).await,
                    1,
                    "beta file: {table} outlive a plain project delete"
                );
            }
            probe.close().await;
        }

        let mut tx = st.write().await.unwrap();
        let removed = projects::delete_with_orphans(&mut tx, "gone")
            .await
            .unwrap();
        assert_eq!(removed, Some(vec!["c-gone".to_string()]), "{fixture}");
        assert_eq!(
            projects::delete_with_orphans(&mut tx, "gone")
                .await
                .unwrap(),
            None
        );
        tx.commit().await.unwrap();
        assert_eq!(counts(&st).await, kept_only, "{fixture}");
        assert!(projects::get(&st, "kept").await.unwrap().is_some());
        st.close().await;
    }
}

// ── Custom labels ──

#[tokio::test]
async fn labels_list_insert_update_and_delete_within_their_project() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p", "P")).await.unwrap();
    projects::insert(&mut tx, &project("q", "Q")).await.unwrap();
    project_labels::insert(&mut tx, "p", &label("l1", "One", "#111111"))
        .await
        .unwrap();
    project_labels::insert(&mut tx, "p", &label("l2", "Two", "#222222"))
        .await
        .unwrap();
    project_labels::insert(&mut tx, "q", &label("l3", "Three", "#333333"))
        .await
        .unwrap();
    // Another project's label can't be changed or removed through this one.
    assert!(
        !project_labels::update(&mut tx, "p", &label("l3", "X", "#000000"))
            .await
            .unwrap()
    );
    assert!(!project_labels::delete(&mut tx, "p", "l3").await.unwrap());
    assert!(
        project_labels::update(&mut tx, "p", &label("l1", "Uno", "#abcdef"))
            .await
            .unwrap()
    );
    assert!(project_labels::delete(&mut tx, "p", "l2").await.unwrap());
    assert!(!project_labels::delete(&mut tx, "p", "l2").await.unwrap());
    tx.commit().await.unwrap();

    assert_eq!(
        project_labels::list(&st, "p").await.unwrap(),
        vec![label("l1", "Uno", "#abcdef")]
    );
    assert_eq!(
        project_labels::list(&st, "q").await.unwrap(),
        vec![label("l3", "Three", "#333333")]
    );
    // What projects::load_all reads is the same list.
    let loaded = projects::load_all(&st).await.unwrap();
    assert_eq!(loaded[0].custom_labels, vec![label("l1", "Uno", "#abcdef")]);
    st.close().await;
}

/// Decision 10: the label's id goes from every connection that has it,
/// whatever its project, and only that label's rows go.
#[tokio::test]
async fn strip_from_connections_removes_the_label_from_every_connection() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p", "P")).await.unwrap();
    projects::insert(&mut tx, &project("q", "Q")).await.unwrap();
    for (id, project_id, labels) in [
        ("a", "p", vec!["x", "dev"]),
        ("b", "p", vec!["dev"]),
        ("c", "p", vec!["x"]),
        ("d", "q", vec!["x", "y"]),
        ("e", "q", vec!["y"]),
    ] {
        let mut c = connection(id, project_id, id);
        c.label_ids = labels.into_iter().map(Into::into).collect();
        connections::insert(&mut tx, &c).await.unwrap();
    }
    let stripped = project_labels::strip_from_connections(&mut tx, "x")
        .await
        .unwrap();
    assert_eq!(stripped, ["a", "c", "d"]);
    assert!(project_labels::strip_from_connections(&mut tx, "x")
        .await
        .unwrap()
        .is_empty());
    tx.commit().await.unwrap();

    let labels = |id: &'static str| {
        let st = &st;
        async move { connections::get(st, id).await.unwrap().unwrap().label_ids }
    };
    assert_eq!(labels("a").await, ["dev"]);
    assert_eq!(labels("b").await, ["dev"]);
    assert!(labels("c").await.is_empty());
    assert_eq!(labels("d").await, ["y"]);
    assert_eq!(labels("e").await, ["y"]);
    st.close().await;
}

/// `projects::save_all`, the frozen repo's replace-all save, still
/// replaces a project's labels (through `project_labels`).
#[tokio::test]
async fn save_all_still_replaces_labels() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let mut p = project("p", "P");
    p.custom_labels = vec![label("l1", "A", "#000000"), label("l2", "B", "#000000")];
    projects::save_all(&st, std::slice::from_ref(&p))
        .await
        .unwrap();
    p.custom_labels = vec![label("l2", "B2", "#ffffff")];
    projects::save_all(&st, std::slice::from_ref(&p))
        .await
        .unwrap();
    assert_eq!(
        project_labels::list(&st, "p").await.unwrap(),
        vec![label("l2", "B2", "#ffffff")]
    );
    st.close().await;
}

// ── Saved queries ──

#[tokio::test]
async fn saved_queries_insert_get_update_delete() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut q = saved_query("s", "p", "Q", Some("f"));
    q.parameters = json(r#"[{"name":"a","type":"text"}]"#);
    q.tags = json(r#"["x"]"#);
    q.starred = true;
    let mut tx = st.write().await.unwrap();
    saved_queries::insert(&mut tx, &q).await.unwrap();
    assert!(saved_queries::insert(&mut tx, &q).await.is_err());
    tx.commit().await.unwrap();
    let got = saved_queries::get(&st, "s").await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&got).unwrap(),
        serde_json::to_value(&q).unwrap()
    );

    let mut changed = q.clone();
    changed.name = "Q2".into();
    changed.query = "SELECT 2".into();
    changed.parameters = None;
    changed.shared = true;
    changed.updated_at = "later".into();
    // Neither moves nor gets a new creation time.
    changed.project_id = "elsewhere".into();
    changed.created_at = "other".into();
    let mut tx = st.write().await.unwrap();
    assert!(saved_queries::update(&mut tx, &changed).await.unwrap());
    assert!(
        !saved_queries::update(&mut tx, &saved_query("nope", "p", "N", None))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let got = saved_queries::get(&st, "s").await.unwrap().unwrap();
    let mut want = changed.clone();
    want.project_id = "p".into();
    want.created_at = "c".into();
    assert_eq!(
        serde_json::to_value(&got).unwrap(),
        serde_json::to_value(&want).unwrap()
    );

    let mut tx = st.write().await.unwrap();
    query_versions::append_keyframe(&mut tx, "v1", "s", "SELECT 1", "t")
        .await
        .unwrap();
    assert!(saved_queries::delete(&mut tx, "s").await.unwrap());
    assert!(!saved_queries::delete(&mut tx, "s").await.unwrap());
    tx.commit().await.unwrap();
    assert!(saved_queries::get(&st, "s").await.unwrap().is_none());
    assert_eq!(count(&st, "SELECT COUNT(*) FROM query_versions").await, 0);
    st.close().await;
}

#[tokio::test]
async fn names_in_folder_and_count() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("q", "Q")).await.unwrap();
    for (id, project_id, name, folder) in [
        ("a", "p", "A", None),
        ("b", "p", "B", Some("f")),
        ("c", "p", "C", None),
        ("d", "q", "D", None),
        ("e", "p", "E", Some("")),
    ] {
        saved_queries::insert(&mut tx, &saved_query(id, project_id, name, folder))
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let names = |v: Vec<IdName>| v.into_iter().map(|n| n.id).collect::<Vec<_>>();
    assert_eq!(
        names(
            saved_queries::names_in_folder(&st, "p", None)
                .await
                .unwrap()
        ),
        ["a", "c", "e"],
        "no folder and an empty one are one folder"
    );
    assert_eq!(
        names(
            saved_queries::names_in_folder(&st, "p", Some("f"))
                .await
                .unwrap()
        ),
        ["b"]
    );
    assert_eq!(
        names(
            saved_queries::names_in_folder(&st, "p", Some(""))
                .await
                .unwrap()
        ),
        ["a", "c", "e"]
    );
    assert_eq!(saved_queries::count(&st).await.unwrap(), 5);
    st.close().await;
}

// ── Query versions ──

#[tokio::test]
async fn append_keyframe_numbers_after_the_highest() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    saved_queries::insert(&mut tx, &saved_query("s", "p", "S", None))
        .await
        .unwrap();
    saved_queries::insert(&mut tx, &saved_query("t", "p", "T", None))
        .await
        .unwrap();
    let first = query_versions::append_keyframe(&mut tx, "v1", "s", "one", "t1")
        .await
        .unwrap();
    assert_eq!(first.version, 1.0);
    tx.commit().await.unwrap();
    // Versions written by the TypeScript, with a gap and diffs.
    for (id, version) in [("v2", 2), ("v5", 5)] {
        sqlx::query(
            "INSERT INTO query_versions (id, saved_query_id, version, diff, created_at) \
             VALUES (?, 's', ?, '@@ -1 +1 @@', 't')",
        )
        .bind(id)
        .bind(version)
        .execute(st.pool())
        .await
        .unwrap();
    }
    let mut tx = st.write().await.unwrap();
    let next = query_versions::append_keyframe(&mut tx, "v6", "s", "SELECT six", "t6")
        .await
        .unwrap();
    // Another query's numbering is its own.
    let other = query_versions::append_keyframe(&mut tx, "w1", "t", "x", "t")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(next.version, 6.0);
    assert_eq!(next.snapshot.as_deref(), Some("SELECT six"));
    assert_eq!(next.diff, None);
    assert_eq!((next.id.as_str(), next.query_id.as_str()), ("v6", "s"));
    assert_eq!(next.created_at, "t6");
    assert_eq!(other.version, 1.0);
    let stored = query_versions::load_by_query(&st, "s").await.unwrap();
    assert_eq!(stored.last(), Some(&next));
    st.close().await;
}

#[tokio::test]
async fn list_meta_reads_no_text() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    saved_queries::insert(&mut tx, &saved_query("s", "p", "S", None))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    // Texts that don't decode: a read of them would fail.
    for (id, version, snapshot, diff) in [
        ("v3", 3, Some(b"\xff\xfe" as &[u8]), None),
        ("v1", 1, Some(b"ok" as &[u8]), None),
        ("v2", 2, None, Some(b"\xff" as &[u8])),
    ] {
        sqlx::query(
            "INSERT INTO query_versions (id, saved_query_id, version, snapshot, diff, created_at) \
             VALUES (?, 's', ?, CAST(? AS TEXT), CAST(? AS TEXT), 't')",
        )
        .bind(id)
        .bind(version)
        .bind(snapshot)
        .bind(diff)
        .execute(st.pool())
        .await
        .unwrap();
    }
    assert!(query_versions::load_by_query(&st, "s").await.is_err());
    let meta = query_versions::list_meta(&st, "s").await.unwrap();
    let got: Vec<(String, f64, bool)> = meta
        .into_iter()
        .map(|m| (m.id, m.version, m.keyframe))
        .collect();
    assert_eq!(
        got,
        vec![
            ("v1".to_string(), 1.0, true),
            ("v2".to_string(), 2.0, false),
            ("v3".to_string(), 3.0, true),
        ]
    );
    st.close().await;
}

#[tokio::test]
async fn delete_ids_touches_only_that_querys_versions() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    for q in ["s", "t"] {
        saved_queries::insert(&mut tx, &saved_query(q, "p", q, None))
            .await
            .unwrap();
    }
    for (id, q) in [("s1", "s"), ("s2", "s"), ("s3", "s"), ("t1", "t")] {
        query_versions::append_keyframe(&mut tx, id, q, "x", "t")
            .await
            .unwrap();
    }
    let ids: Vec<String> = ["s1", "s3", "t1", "missing"]
        .into_iter()
        .map(Into::into)
        .collect();
    assert_eq!(
        query_versions::delete_ids(&mut tx, "s", &ids)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        query_versions::delete_ids(&mut tx, "s", &[]).await.unwrap(),
        0
    );
    tx.commit().await.unwrap();
    let left: Vec<String> = sqlx::query_scalar("SELECT id FROM query_versions ORDER BY id")
        .fetch_all(st.pool())
        .await
        .unwrap();
    assert_eq!(left, ["s2", "t1"]);
    st.close().await;
}

// ── Vault rows ──

#[tokio::test]
async fn remove_all_for_key_in_a_write_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    for (scope, key) in [
        ("db", "c"),
        ("ssh", "c"),
        ("ssh-key", "c"),
        ("db", "d"),
        // Not a connection's: kept even though the id matches.
        ("ai-api-key-provider", "c"),
        ("license", "c"),
    ] {
        user_credentials::save(
            &st,
            &PersistedCredential {
                scope: scope.into(),
                key: key.into(),
                nonce: "n".into(),
                ciphertext: "x".into(),
                updated_at: "t".into(),
            },
        )
        .await
        .unwrap();
    }
    let mut tx = st.write().await.unwrap();
    assert_eq!(
        user_credentials::remove_all_for_key_in(&mut tx, "c")
            .await
            .unwrap(),
        3
    );
    tx.commit().await.unwrap();
    let left: Vec<(String, String)> =
        sqlx::query_as("SELECT scope, key FROM user_credentials ORDER BY scope, key")
            .fetch_all(st.pool())
            .await
            .unwrap();
    let left: Vec<(&str, &str)> = left.iter().map(|(s, k)| (s.as_str(), k.as_str())).collect();
    assert_eq!(
        left,
        [("ai-api-key-provider", "c"), ("db", "d"), ("license", "c")]
    );
    st.close().await;
}

// ── The legacy-string data step ──

const LEGACY_STEP: &str = "drop_legacy_built_connection_strings";

/// A current-schema file the app hasn't opened yet (no data steps run),
/// holding `rows`: (type, host, port, database, user, ssl mode, string).
type Row<'a> = (
    &'a str,
    &'a str,
    f64,
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
);

async fn file_with(path: &Path, rows: &[Row<'_>]) {
    load_fixture(path, "schemas/current.sql").await;
    let mut conn = raw_connect(path).await;
    sqlx::query(
        "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p', 'P', 'c', 'u')",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    for (i, (ty, host, port, db, user, ssl, cs)) in rows.iter().enumerate() {
        sqlx::query(
            "INSERT INTO connections (id, project_id, name, type, host, port, database_name, \
             username, ssl_mode, connection_string) VALUES (?, 'p', 'n', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(format!("c{i:03}"))
        .bind(ty)
        .bind(host)
        .bind(port)
        .bind(db)
        .bind(user)
        .bind(ssl)
        .bind(cs)
        .execute(&mut conn)
        .await
        .unwrap();
    }
    sqlx::Connection::close(conn).await.unwrap();
}

async fn strings_and_users(st: &Storage) -> Vec<(Option<String>, String)> {
    sqlx::query_as("SELECT connection_string, username FROM connections ORDER BY rowid")
        .fetch_all(st.pool())
        .await
        .unwrap()
}

/// Exactly the strings `isLegacyBuiltString` called the old builder's (the
/// TypeScript's own cases, as rows) become NULL; every other string keeps
/// its text. A row whose user came only from its string keeps that user,
/// as the TypeScript's load saved it.
#[tokio::test]
async fn the_data_step_drops_exactly_the_legacy_strings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let pg = (
        "postgres",
        "prod.example.com",
        5432.0,
        "app",
        "alice",
        Some("disable"),
    );
    let with = |(ty, host, port, db, user, ssl): (
        &'static str,
        &'static str,
        f64,
        &'static str,
        &'static str,
        Option<&'static str>,
    ),
                cs: &'static str|
     -> Row<'static> { (ty, host, port, db, user, ssl, Some(cs)) };
    let legacy: Vec<Row> = vec![
        with(
            pg,
            "postgresql://alice@prod.example.com/app?sslmode=disable",
        ),
        with(
            pg,
            "postgres://alice:secret@prod.example.com/app?sslmode=disable",
        ),
        with(
            (
                "postgres",
                "prod.example.com",
                6543.0,
                "app",
                "alice",
                Some("require"),
            ),
            "postgresql://alice@prod.example.com:6543/app?sslmode=require",
        ),
        with(
            ("mysql", "db", 3307.0, "shop", "root", Some("disable")),
            "mysql://root@db:3307/shop?ssl-mode=DISABLED",
        ),
        with(
            ("mssql", "sql", 1433.0, "app", "sa", Some("disable")),
            "mssql://sa@sql/app",
        ),
        with(
            ("sqlite", "", 0.0, "/data/a.db", "", None),
            "sqlite:///data/a.db",
        ),
        with(("duckdb", "", 0.0, "", "", None), "duckdb://:memory:"),
        with(
            (
                "postgres",
                "prod.example.com",
                5432.0,
                "app",
                "al@ice",
                Some("disable"),
            ),
            "postgresql://al%40ice@prod.example.com/app?sslmode=disable",
        ),
        // A pre-5a row with no user of its own: the user came from the
        // string, which the old builder made from that user.
        with(
            ("mysql", "db", 3306.0, "shop", "", None),
            "mysql://root@db/shop",
        ),
    ];
    let kept: Vec<Row> = vec![
        with(
            (
                "postgres",
                "staging",
                5432.0,
                "app",
                "alice",
                Some("disable"),
            ),
            "postgresql://alice@prod.example.com/app?sslmode=disable",
        ),
        with(
            pg,
            "postgresql://alice@prod.example.com/app?application_name=seaquel",
        ),
        with(
            ("sqlite", "", 0.0, "/data/a.db", "", None),
            "sqlite:///data/a.db?mode=ro",
        ),
        with(pg, ""),
        with(
            ("mssql", "sql", 1433.0, "app", "sa", None),
            "Server=sql;Database=app;User Id=sa",
        ),
        ("postgres", "h", 5432.0, "d", "u", None, None),
    ];
    let rows: Vec<Row> = legacy.iter().chain(&kept).copied().collect();
    file_with(&path, &rows).await;

    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let got = strings_and_users(&st).await;
    for (i, row) in legacy.iter().enumerate() {
        assert_eq!(got[i].0, None, "{:?} should be dropped", row.6);
    }
    assert_eq!(got[legacy.len() - 1].1, "root", "the user the string held");
    assert_eq!(got[0].1, "alice");
    for (j, row) in kept.iter().enumerate() {
        let (cs, user) = &got[legacy.len() + j];
        assert_eq!(cs.as_deref(), row.6, "kept");
        assert_eq!(user, row.4);
    }
    let steps: Vec<String> = sqlx::query_scalar(&format!("SELECT name FROM {DATA_STEPS_TABLE}"))
        .fetch_all(st.pool())
        .await
        .unwrap();
    assert!(steps.contains(&LEGACY_STEP.to_string()), "{steps:?}");
    st.close().await;
}

/// Text that isn't UTF-8, in the string or a field, leaves the row alone
/// (and doesn't fail the step), as do NULL ids.
#[tokio::test]
async fn the_data_step_skips_rows_it_cant_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    file_with(
        &path,
        &[(
            "mssql",
            "sql",
            1433.0,
            "app",
            "sa",
            None,
            Some("mssql://sa@sql/app"),
        )],
    )
    .await;
    exec_file(
        &path,
        "INSERT INTO connections (id, project_id, name, type, host, port, database_name, username, connection_string) \
         VALUES (NULL, 'p', 'n', 'mssql', CAST(x'ff' AS TEXT), 1433, 'app', 'sa', 'mssql://sa@sql/app'); \
         INSERT INTO connections (id, project_id, name, type, host, port, database_name, username, connection_string) \
         VALUES (NULL, 'p', 'n', 'mssql', 'sql', 1433, 'app', 'sa', CAST(x'ff' AS TEXT)); \
         INSERT INTO connections (id, project_id, name, type, host, port, database_name, username, connection_string) \
         VALUES (NULL, 'p', 'n', 'mssql', 'sql', 1433, 'app', 'sa', 'mssql://sa@sql/app');",
    )
    .await;
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let got: Vec<(Option<Vec<u8>>,)> =
        sqlx::query_as("SELECT CAST(connection_string AS BLOB) FROM connections ORDER BY rowid")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(
        got,
        vec![
            (None,),
            (Some(b"mssql://sa@sql/app".to_vec()),),
            (Some(vec![0xff]),),
            (None,),
        ]
    );
    st.close().await;
}

/// Linear in rows and string length: 10,000 rows and one 200 KB string.
// A native test measuring its own wall time; the wasm32 rule behind
// `disallowed_types` doesn't apply.
#[allow(clippy::disallowed_types)]
#[tokio::test]
async fn the_data_step_is_linear() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let long = format!("mssql://sa@sql/{}", "x".repeat(200_000));
    let hosts: Vec<String> = (0..10_000).map(|i| format!("h{i}")).collect();
    let strings: Vec<String> = hosts
        .iter()
        .map(|h| format!("mssql://sa@{h}/app"))
        .collect();
    let mut rows: Vec<Row> = hosts
        .iter()
        .zip(&strings)
        .map(|(h, s)| {
            (
                "mssql",
                h.as_str(),
                1433.0,
                "app",
                "sa",
                None,
                Some(s.as_str()),
            )
        })
        .collect();
    let long_db = "x".repeat(200_000);
    rows.push(("mssql", "sql", 1433.0, &long_db, "sa", None, Some(&long)));
    file_with(&path, &rows).await;

    let started = std::time::Instant::now();
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert!(strings_and_users(&st)
        .await
        .iter()
        .all(|(s, _)| s.is_none()));
    assert!(elapsed.as_secs() < 10, "open took {elapsed:?}");
    st.close().await;
}

/// The CLI opens read-only and never runs a step, so a file the app hasn't
/// opened since this step shipped is refused, and left as it was.
#[tokio::test]
async fn a_read_only_open_refuses_a_file_with_the_step_pending() {
    let dir = tempfile::tempdir().unwrap();
    let (st, path) = fresh(dir.path()).await;
    sqlx::query(&format!("DELETE FROM {DATA_STEPS_TABLE} WHERE name = ?"))
        .bind(LEGACY_STEP)
        .execute(st.pool())
        .await
        .unwrap();
    st.close().await;
    let before = snapshot(&path).await;

    let err = Storage::open(
        &path,
        StorageOptions {
            read_only: true,
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), STORAGE_NEEDS_UPGRADE, "{err}");
    assert!(
        matches!(&err, StorageError::DataStepPending { step, .. } if step == LEGACY_STEP),
        "{err:?}"
    );
    assert_eq!(snapshot(&path).await, before);

    // The app's open runs it, and then the CLI's succeeds.
    Storage::open(&path, StorageOptions::default())
        .await
        .unwrap()
        .close()
        .await;
    Storage::open(
        &path,
        StorageOptions {
            read_only: true,
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap()
    .close()
    .await;
}

// ── Name keys (phase 5d-1 probe fix) ──

use seaquel_types::names::name_key;

async fn stored_keys(st: &Storage, table: &str) -> Vec<(String, Option<String>)> {
    sqlx::query_as(&format!("SELECT id, name_key FROM {table} ORDER BY rowid"))
        .fetch_all(st.pool())
        .await
        .unwrap()
}

fn ids(v: Vec<IdName>) -> Vec<String> {
    v.into_iter().map(|n| n.id).collect()
}

/// Every write that stores a name stores its key with it, a rename
/// included (a case-only one too, which the stale-key trigger NULLs and
/// the write sets again).
#[tokio::test]
async fn writes_store_the_name_key() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p", " Ärger "))
        .await
        .unwrap();
    projects::insert_if_missing(&mut tx, &project("q", "Q"))
        .await
        .unwrap();
    connections::insert(&mut tx, &connection("c", "p", "STRASSE"))
        .await
        .unwrap();
    saved_queries::insert(&mut tx, &saved_query("s", "p", "Cafe\u{301}", None))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        stored_keys(&st, "projects").await,
        [
            ("p".to_string(), Some(name_key("ärger"))),
            ("q".to_string(), Some(name_key("q")))
        ]
    );
    assert_eq!(
        stored_keys(&st, "connections").await,
        [("c".to_string(), Some(name_key("straße")))]
    );
    assert_eq!(
        stored_keys(&st, "saved_queries").await,
        [("s".to_string(), Some(name_key("café")))]
    );

    let mut tx = st.write().await.unwrap();
    projects::update(&mut tx, &project("p", "Other"))
        .await
        .unwrap();
    connections::update(&mut tx, &connection("c", "p", "strasse"))
        .await
        .unwrap();
    saved_queries::update(&mut tx, &saved_query("s", "p", "New", None))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        stored_keys(&st, "projects").await[0].1,
        Some(name_key("other"))
    );
    assert_eq!(
        stored_keys(&st, "connections").await[0].1,
        Some(name_key("straße")),
        "a case-only rename keeps its key"
    );
    assert_eq!(
        stored_keys(&st, "saved_queries").await[0].1,
        Some(name_key("new"))
    );
    st.close().await;
}

/// The lookups return the rows whose key matches, in rowid order, within
/// the project (and folder), plus rows with no stored key whose name has
/// it: those an older release wrote or renamed.
#[tokio::test]
async fn lookups_by_key_include_rows_with_no_stored_key() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("q", "Other"))
        .await
        .unwrap();
    connections::insert(&mut tx, &connection("c2", "p", "c"))
        .await
        .unwrap();
    connections::insert(&mut tx, &connection("c3", "q", "C"))
        .await
        .unwrap();
    connections::insert(&mut tx, &connection("c4", "p", "D"))
        .await
        .unwrap();
    for (id, name, folder) in [
        ("s1", "A", None),
        ("s2", "a", Some("")),
        ("s3", "A", Some("f")),
    ] {
        saved_queries::insert(&mut tx, &saved_query(id, "p", name, folder))
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    // An older release renames `c4` to `C` (no key written) and inserts a
    // row with none.
    let mut conn = st.pool().acquire().await.unwrap();
    sqlx::query("UPDATE connections SET name = ' c ' WHERE id = 'c4'")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO connections (id, project_id, name, type, host, port, database_name, \
         username) VALUES ('c5', 'p', 'C', 'postgres', 'h', 1, 'd', 'u')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("UPDATE projects SET name = 'p' WHERE id = 'q'")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let keys = stored_keys(&st, "connections").await;
    assert_eq!(
        keys[3],
        ("c4".to_string(), None),
        "the stale key was NULLed"
    );
    assert_eq!(keys[4], ("c5".to_string(), None));

    assert_eq!(
        ids(connections::with_name_key(&st, "p", &name_key("C"))
            .await
            .unwrap()),
        ["c", "c2", "c4", "c5"]
    );
    assert_eq!(
        ids(connections::with_name_key(&st, "p", &name_key("D"))
            .await
            .unwrap()),
        Vec::<String>::new(),
        "the old key no longer matches"
    );
    assert_eq!(
        ids(connections::with_name_key(&st, "q", &name_key("c"))
            .await
            .unwrap()),
        ["c3"]
    );
    assert_eq!(
        ids(projects::with_name_key(&st, &name_key("P")).await.unwrap()),
        ["p", "q"]
    );
    assert_eq!(
        ids(
            saved_queries::with_name_key_in_folder(&st, "p", None, &name_key("a"))
                .await
                .unwrap()
        ),
        ["s1", "s2"],
        "no folder and an empty one are one folder"
    );
    assert_eq!(
        ids(
            saved_queries::with_name_key_in_folder(&st, "p", Some("f"), &name_key("a"))
                .await
                .unwrap()
        ),
        ["s3"]
    );
    st.close().await;
}

/// The lookups are index searches, not scans of the project.
#[tokio::test]
async fn lookups_by_key_use_the_indexes() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let plan = |sql: &'static str| {
        let st = &st;
        async move {
            let rows: Vec<(i64, i64, i64, String)> =
                sqlx::query_as(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .fetch_all(st.pool())
                    .await
                    .unwrap();
            rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n")
        }
    };
    for (sql, index) in [
        (connections::NAME_KEY_LOOKUP, "idx_connections_name_key"),
        (projects::NAME_KEY_LOOKUP, "idx_projects_name_key"),
        (saved_queries::NAME_KEY_LOOKUP, "idx_saved_queries_name_key"),
    ] {
        let plan = plan(sql).await;
        assert!(!plan.contains("SCAN"), "{sql}:\n{plan}");
        assert_eq!(plan.matches(index).count(), 2, "{sql}:\n{plan}");
    }
    st.close().await;
}

const NAME_KEY_STEP: &str = "backfill_name_keys";

/// A file from before the migration: the migration adds the columns and
/// the data step fills every key with `name_key`, leaving a name that
/// isn't UTF-8 NULL. It runs once.
#[tokio::test]
async fn the_name_key_step_backfills_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    load_fixture(&path, "schemas/current.sql").await;
    let mut conn = raw_connect(&path).await;
    for sql in [
        "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p', ' Straße ', 'c', 'u')",
        "INSERT INTO connections (id, project_id, name, type, host, port, database_name, username) \
         VALUES ('c', 'p', 'ÄRGER', 'postgres', 'h', 1, 'd', 'u')",
        "INSERT INTO connections (id, project_id, name, type, host, port, database_name, username) \
         VALUES ('bad', 'p', CAST(X'FF' AS TEXT), 'postgres', 'h', 1, 'd', 'u')",
        "INSERT INTO saved_queries (id, project_id, name, query, created_at, updated_at) \
         VALUES ('s', 'p', 'Q', 'SELECT 1', 'c', 'u')",
    ] {
        sqlx::query(sql).execute(&mut conn).await.unwrap();
    }
    sqlx::Connection::close(conn).await.unwrap();

    // The read-only open refuses the file until the app has opened it.
    let refused = Storage::open(
        &path,
        StorageOptions {
            read_only: true,
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code(), STORAGE_NEEDS_UPGRADE);

    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    assert_eq!(
        stored_keys(&st, "projects").await,
        [("p".to_string(), Some(name_key("strasse")))]
    );
    assert_eq!(
        stored_keys(&st, "connections").await,
        [
            ("c".to_string(), Some(name_key("ärger"))),
            ("bad".to_string(), None)
        ]
    );
    assert_eq!(
        stored_keys(&st, "saved_queries").await,
        [("s".to_string(), Some(name_key("q")))]
    );
    // The name that isn't UTF-8 matches nothing and doesn't fail the
    // lookup, which reads it (its key is NULL).
    assert_eq!(
        ids(connections::with_name_key(&st, "p", &name_key("ärger"))
            .await
            .unwrap()),
        ["c"]
    );
    assert!(connections::with_name_key(&st, "p", &name_key("\u{fffd}"))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        count(
            &st,
            &format!("SELECT COUNT(*) FROM {DATA_STEPS_TABLE} WHERE name = '{NAME_KEY_STEP}'")
        )
        .await,
        1
    );
    st.close().await;
    // Now the read-only open works.
    Storage::open(
        &path,
        StorageOptions {
            read_only: true,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .close()
    .await;
}

/// After the step ran, an older release (a downgrade, then the upgrade
/// again) can leave rows with no key. `refill_name_keys` fills them each
/// time it finds one, is never recorded, and only reads otherwise.
#[tokio::test]
async fn refill_name_keys_fills_keys_an_older_release_left_null() {
    let dir = tempfile::tempdir().unwrap();
    let (st, path) = fresh(dir.path()).await;
    seeded(&st).await;
    assert!(!seaquel_storage::refill_name_keys(&st).await.unwrap());
    // What an older release writes: no key, and a rename that drops it.
    sqlx::query(
        "INSERT INTO connections (id, project_id, name, type, host, port, database_name, \
         username) VALUES ('old', 'p', 'Older', 'postgres', 'h', 1, 'd', 'u')",
    )
    .execute(st.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE projects SET name = 'Renamed' WHERE id = 'p'")
        .execute(st.pool())
        .await
        .unwrap();
    assert_eq!(stored_keys(&st, "projects").await[0].1, None);
    assert!(seaquel_storage::refill_name_keys(&st).await.unwrap());
    assert_eq!(
        stored_keys(&st, "connections").await,
        [
            ("c".to_string(), Some(name_key("c"))),
            ("old".to_string(), Some(name_key("older")))
        ]
    );
    assert_eq!(
        stored_keys(&st, "projects").await[0].1,
        Some(name_key("renamed"))
    );
    assert!(!seaquel_storage::refill_name_keys(&st).await.unwrap());
    st.close().await;
    // Read-only storage is left alone.
    let ro = Storage::open(
        &path,
        StorageOptions {
            read_only: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!seaquel_storage::refill_name_keys(&ro).await.unwrap());
    ro.close().await;
}
