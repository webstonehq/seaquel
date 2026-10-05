//! The one-time move of secrets out of stored connection strings (phase 5d),
//! run when a writable workspace opens: the keychain first
//! and outside any write transaction, then the stripped string and the save
//! flags in one row update that reads the string again, and the notice.
//!
//! Every case uses a temp file and a `TestStore`; the live ones use the e2e
//! Docker Postgres and SSH containers when `SEAQUEL_TEST_POSTGRES` and
//! `SEAQUEL_TEST_SSH` are set.
#![cfg(all(feature = "storage", feature = "secrets", feature = "workspace"))]
// Native-only tests: tasks on the test runtime and the wall clock.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use common::{capture_logs, core, dump, file_bytes, insert_rows, logged, TestStore};
use seaquel_core::storage::{app_state, connections, Storage, StorageOptions};
use seaquel_core::{
    ConnectRequest, Core, HostKeyPolicy, Workspace, WorkspaceEvent, WorkspaceSpec,
    STRING_SECRETS_NOTICE_KEY, STRING_SECRETS_UPGRADED_KEY,
};
use serde_json::{json, Value};

const T0: &str = "2024-01-01T00:00:00.000Z";

fn row(id: &str, ty: &str, string: &str, extra: Value) -> Value {
    let mut r = json!({
        "id": id, "project_id": "p1", "name": format!("Name {id}"), "type": ty,
        "host": "db.example.com", "port": 5432, "database_name": "app", "username": "alice",
        "ssl_mode": null, "connection_string": string, "last_connected": T0,
        "ssh_tunnel": null, "save_password": 0, "save_ssh_password": 0,
        "save_ssh_key_passphrase": 0, "is_local_only": 1, "shared_connection_id": null,
        "ai_share_schema": null, "ai_share_data": null, "active_ai_provider_id": null,
        "active_ai_model": null,
    });
    for (k, v) in extra.as_object().unwrap() {
        r[k] = v.clone();
    }
    r
}

/// A file at `dir` holding project `p1` and `rows`, written before any
/// workspace opens it (as a pre-5a file would be).
async fn seed(dir: &Path, rows: &[Value]) -> PathBuf {
    let path = dir.join("seaquel.db");
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    insert_rows(
        &st,
        "projects",
        &[
            json!({"id": "p1", "name": "Main", "description": null, "created_at": T0,
                 "updated_at": T0, "git_repo_path": null}),
        ],
    )
    .await;
    insert_rows(&st, "connections", rows).await;
    st.close().await;
    path
}

async fn open(core: &Core, dir: &Path, store: Option<Arc<TestStore>>) -> Arc<Workspace> {
    let spec = WorkspaceSpec::new(dir);
    let spec = match store {
        Some(s) => spec.with_secrets(s),
        None => spec,
    };
    core.open_workspace(spec).await.unwrap()
}

async fn stored(ws: &Workspace, id: &str) -> Value {
    dump(ws.storage(), "connections", "id")
        .await
        .into_iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("no row {id}"))
}

async fn notice(ws: &Workspace) -> Vec<String> {
    app_state::get(ws.storage(), STRING_SECRETS_NOTICE_KEY)
        .await
        .unwrap()
        .map(|n| serde_json::from_str(&n).unwrap())
        .unwrap_or_default()
}

async fn upgraded(ws: &Workspace) -> bool {
    app_state::get(ws.storage(), STRING_SECRETS_UPGRADED_KEY)
        .await
        .unwrap()
        .is_some()
}

#[tokio::test]
async fn a_database_password_moves_to_db_then_the_string_is_stripped() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        &[row(
            "c1",
            "postgres",
            "postgres://alice:canary-db-pw@db.example.com/app",
            json!({}),
        )],
    )
    .await;
    let store = TestStore::new();
    let ws = open(&core(), dir.path(), Some(store.clone())).await;

    assert_eq!(store.entries()["db:c1"], "canary-db-pw");
    let r = stored(&ws, "c1").await;
    assert_eq!(
        r["connection_string"],
        "postgres://alice@db.example.com/app"
    );
    assert_eq!(r["save_password"], 1);
    assert_eq!(r["save_ssh_password"], 0);
    assert!(notice(&ws).await.is_empty());
    assert!(upgraded(&ws).await);
}

#[tokio::test]
async fn an_ssh_password_moves_to_ssh_then_the_string_is_stripped() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        &[row(
            "c1",
            "postgres",
            "postgresql+ssh://deploy:canary-ssh@bastion/alice@db/app",
            json!({}),
        )],
    )
    .await;
    let store = TestStore::new();
    let ws = open(&core(), dir.path(), Some(store.clone())).await;

    assert_eq!(store.entries()["ssh:c1"], "canary-ssh");
    assert!(!store.entries().contains_key("db:c1"));
    let r = stored(&ws, "c1").await;
    assert_eq!(
        r["connection_string"],
        "postgresql+ssh://deploy@bastion/alice@db/app"
    );
    assert_eq!(r["save_ssh_password"], 1);
    assert_eq!(r["save_password"], 0);
    assert!(notice(&ws).await.is_empty());
}

#[tokio::test]
async fn an_equal_keychain_entry_sets_the_flag_and_strips() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        &[row(
            "c1",
            "postgres",
            "postgres://alice:same-pw@db.example.com/app",
            json!({}),
        )],
    )
    .await;
    // A crash between an earlier keychain write and its row update.
    let store = TestStore::with(&[("db:c1", "same-pw")]);
    let ws = open(&core(), dir.path(), Some(store.clone())).await;

    assert_eq!(store.sets.load(Ordering::SeqCst), 0, "nothing to write");
    let r = stored(&ws, "c1").await;
    assert_eq!(
        r["connection_string"],
        "postgres://alice@db.example.com/app"
    );
    assert_eq!(r["save_password"], 1);
    assert!(notice(&ws).await.is_empty());
}

#[tokio::test]
async fn a_different_keychain_entry_is_kept_the_flag_left_and_the_row_listed() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        &[
            row(
                "c1",
                "postgres",
                "postgres://alice:in-string@db.example.com/app",
                json!({}),
            ),
            row(
                "c2",
                "postgres",
                "postgresql+ssh://deploy:ssh-in-string@bastion/alice@db/app",
                json!({}),
            ),
        ],
    )
    .await;
    let store = TestStore::with(&[("db:c1", "typed-later"), ("ssh:c2", "typed-later")]);
    let ws = open(&core(), dir.path(), Some(store.clone())).await;

    assert_eq!(store.entries()["db:c1"], "typed-later");
    assert_eq!(store.entries()["ssh:c2"], "typed-later");
    assert_eq!(
        store.sets.load(Ordering::SeqCst),
        0,
        "no entry is overwritten"
    );
    let (r1, r2) = (stored(&ws, "c1").await, stored(&ws, "c2").await);
    assert_eq!(
        r1["connection_string"],
        "postgres://alice@db.example.com/app"
    );
    assert_eq!(r1["save_password"], 0, "the flag is left as it was");
    assert_eq!(
        r2["connection_string"],
        "postgresql+ssh://deploy@bastion/alice@db/app"
    );
    assert_eq!(r2["save_ssh_password"], 0);
    assert_eq!(notice(&ws).await, vec!["c1", "c2"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn no_keychain_call_holds_the_write_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed(
        dir.path(),
        &[row(
            "c1",
            "postgres",
            "postgres://alice:pw-1@db.example.com/app",
            json!({}),
        )],
    )
    .await;
    let store = TestStore::new();
    store.block_set.store(true, Ordering::SeqCst);
    let entered = store.entered_set.notified();
    let opening = tokio::spawn({
        let (dir, store) = (dir.path().to_path_buf(), store.clone());
        async move { open(&core(), &dir, Some(store)).await }
    });
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("the upgrade reaches the keychain");

    // While the keychain write waits (a prompt on screen), another writer
    // still commits: the upgrade holds no write lock.
    let other = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let write = async {
        let mut tx = other.write().await?;
        app_state::set_in(&mut tx, "other-writer", Some("1")).await?;
        tx.commit().await
    };
    tokio::time::timeout(Duration::from_secs(5), write)
        .await
        .expect("another writer isn't blocked by a keychain call")
        .unwrap();
    other.close().await;

    store.release();
    let ws = opening.await.unwrap();
    assert_eq!(
        stored(&ws, "c1").await["connection_string"],
        "postgres://alice@db.example.com/app"
    );
    assert!(upgraded(&ws).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_changed_between_read_and_write_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed(
        dir.path(),
        &[row(
            "c1",
            "postgres",
            "postgres://alice:pw-1@db.example.com/app",
            json!({}),
        )],
    )
    .await;
    let store = TestStore::new();
    store.block_set.store(true, Ordering::SeqCst);
    let entered = store.entered_set.notified();
    let opening = tokio::spawn({
        let (dir, store) = (dir.path().to_path_buf(), store.clone());
        async move { open(&core(), &dir, Some(store)).await }
    });
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap();

    // Another window saves the row meanwhile (Core's writes strip it).
    let other = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let mut c = connections::get(&other, "c1").await.unwrap().unwrap();
    c.connection_string = Some("postgres://alice@db.example.com/renamed".into());
    let mut tx = other.write().await.unwrap();
    connections::update(&mut tx, &c).await.unwrap();
    tx.commit().await.unwrap();
    other.close().await;

    store.release();
    let ws = opening.await.unwrap();
    let r = stored(&ws, "c1").await;
    assert_eq!(
        r["connection_string"], "postgres://alice@db.example.com/renamed",
        "the other save stands"
    );
    assert_eq!(
        r["save_password"], 0,
        "the skipped row's flags aren't touched"
    );
    assert!(!upgraded(&ws).await, "the next open lists again");
    assert!(
        !store.entries().contains_key("db:c1"),
        "the entry this pass wrote for the skipped row is taken back"
    );
}

#[tokio::test]
async fn a_keychain_failure_leaves_the_row_and_the_next_open_retries_it() {
    let dir = tempfile::tempdir().unwrap();
    let original = "postgres://alice:pw-1@db.example.com/app";
    seed(dir.path(), &[row("c1", "postgres", original, json!({}))]).await;
    let store = TestStore::new();
    store.fail_set.store(true, Ordering::SeqCst);
    let core = core();
    let ws = open(&core, dir.path(), Some(store.clone())).await;
    let r = stored(&ws, "c1").await;
    assert_eq!(
        r["connection_string"], original,
        "not stripped before the keychain has it"
    );
    assert_eq!(r["save_password"], 0);
    assert!(!upgraded(&ws).await);
    ws.close().await;

    store.fail_set.store(false, Ordering::SeqCst);
    let ws = open(&core, dir.path(), Some(store.clone())).await;
    assert_eq!(store.entries()["db:c1"], "pw-1");
    let r = stored(&ws, "c1").await;
    assert_eq!(
        r["connection_string"],
        "postgres://alice@db.example.com/app"
    );
    assert_eq!(r["save_password"], 1);
    assert!(upgraded(&ws).await);
}

#[tokio::test]
async fn an_unmovable_secret_is_stripped_and_listed_in_the_notice() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        &[
            row(
                "c1",
                "postgres",
                "host=db.example.com password=canary-libpq dbname=app",
                json!({}),
            ),
            row(
                "c2",
                "duckdb",
                "duckdb:///data/app.duckdb?s3_secret_access_key=canary-s3",
                json!({}),
            ),
        ],
    )
    .await;
    let store = TestStore::new();
    let ws = open(&core(), dir.path(), Some(store.clone())).await;

    assert!(
        store.entries().is_empty(),
        "nothing the driver doesn't read moves"
    );
    for id in ["c1", "c2"] {
        let s = stored(&ws, id).await["connection_string"].to_string();
        assert!(!s.contains("canary"), "{id}: {s}");
    }
    assert_eq!(notice(&ws).await, vec!["c1", "c2"]);
    assert!(upgraded(&ws).await);
}

#[tokio::test]
async fn web_strips_every_listed_row_and_lists_it() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        &[
            row(
                "c1",
                "postgres",
                "postgres://alice:canary-web@db.example.com/app",
                json!({}),
            ),
            row(
                "c2",
                "postgres",
                "host=db.example.com password=canary-web2",
                json!({}),
            ),
            row(
                "c3",
                "postgres",
                "postgres://alice@db.example.com/app",
                json!({}),
            ),
        ],
    )
    .await;
    let ws = open(&core(), dir.path(), None).await;
    assert_eq!(
        stored(&ws, "c1").await["connection_string"],
        "postgres://alice@db.example.com/app"
    );
    assert_eq!(stored(&ws, "c1").await["save_password"], 0);
    assert!(!stored(&ws, "c2").await["connection_string"]
        .to_string()
        .contains("canary"));
    assert_eq!(
        stored(&ws, "c3").await["connection_string"],
        "postgres://alice@db.example.com/app",
        "a row with no secret isn't touched"
    );
    assert_eq!(notice(&ws).await, vec!["c1", "c2"]);
    assert!(upgraded(&ws).await);
}

/// Every string form the string-secrets upgrade handles, each with a canary.
fn canary_rows() -> Vec<Value> {
    vec![
        row(
            "u1",
            "postgres",
            "postgres://alice:canary-url@db.example.com/app",
            json!({}),
        ),
        row(
            "u2",
            "postgres",
            "postgres://db.example.com/app?password=canary-param",
            json!({}),
        ),
        row(
            "u3",
            "postgres",
            "host=db.example.com password=canary-libpq",
            json!({}),
        ),
        row(
            "u4",
            "postgres",
            "host=h sslpassword=canary-sslpw",
            json!({}),
        ),
        row(
            "u5",
            "mssql",
            "Server=h;Database=app;User Id=sa;Password=canary-ado;",
            json!({}),
        ),
        row(
            "u6",
            "duckdb",
            "duckdb:///f.duckdb?s3_secret_access_key=canary-duck",
            json!({}),
        ),
        row(
            "u7",
            "postgres",
            "postgresql+ssh://deploy:canary-sshpw@bastion/alice:canary-path@db/app",
            json!({}),
        ),
        row(
            "u8",
            "postgres",
            "postgres://alice:canary#frag@db.example.com/app",
            json!({}),
        ),
        row(
            "u9",
            "mysql",
            "mysql://root:canary-mysql@db.example.com/app",
            json!({}),
        ),
    ]
}

const CANARY: &[u8] = b"canary";

#[tokio::test]
async fn no_plaintext_secret_is_left() {
    for with_store in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), &canary_rows()).await;
        assert!(file_bytes(dir.path())
            .windows(CANARY.len())
            .any(|w| w == CANARY));
        let store = TestStore::new();
        let ws = open(&core(), dir.path(), with_store.then(|| store.clone())).await;
        assert!(
            connections::with_secret_in_string(ws.storage())
                .await
                .unwrap()
                .is_empty(),
            "store: {with_store}"
        );
        ws.close().await;
        drop(ws);
        let bytes = file_bytes(dir.path());
        assert!(
            !bytes.windows(CANARY.len()).any(|w| w == CANARY),
            "a canary is left in the file (store: {with_store})"
        );
    }
}

#[tokio::test]
async fn the_upgrade_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), &canary_rows()).await;
    let store = TestStore::new();
    let core = core();
    let ws = open(&core, dir.path(), Some(store.clone())).await;
    let rows = dump(ws.storage(), "connections", "id").await;
    let secrets = store.entries();
    let notice_before = notice(&ws).await;
    ws.close().await;
    let (sets, gets) = (
        store.sets.load(Ordering::SeqCst),
        store.gets.load(Ordering::SeqCst),
    );

    let ws = open(&core, dir.path(), Some(store.clone())).await;
    assert_eq!(dump(ws.storage(), "connections", "id").await, rows);
    assert_eq!(store.entries(), secrets);
    assert_eq!(notice(&ws).await, notice_before);
    assert_eq!(
        store.sets.load(Ordering::SeqCst),
        sets,
        "no keychain write on the second open"
    );
    assert_eq!(
        store.gets.load(Ordering::SeqCst),
        gets,
        "no keychain read either"
    );
}

#[tokio::test]
async fn it_is_recorded_once_nothing_is_left() {
    // A file with nothing to move is recorded on its first open.
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        &[row("c1", "postgres", "postgres://a@h/db", json!({}))],
    )
    .await;
    let ws = open(&core(), dir.path(), Some(TestStore::new())).await;
    assert!(upgraded(&ws).await);
    assert!(notice(&ws).await.is_empty());

    // After it's recorded, a row that somehow holds a secret isn't looked
    // at again (Core's writes strip every string they store).
    ws.close().await;
    let st = Storage::open(dir.path().join("seaquel.db"), StorageOptions::default())
        .await
        .unwrap();
    insert_rows(
        &st,
        "connections",
        &[row("c2", "postgres", "postgres://a:late@h/db", json!({}))],
    )
    .await;
    st.close().await;
    let store = TestStore::new();
    let ws = open(&core(), dir.path(), Some(store.clone())).await;
    assert_eq!(store.gets.load(Ordering::SeqCst), 0);
    assert_eq!(
        stored(&ws, "c2").await["connection_string"],
        "postgres://a:late@h/db"
    );
}

#[tokio::test]
async fn a_read_only_workspace_never_runs_it() {
    let dir = tempfile::tempdir().unwrap();
    let original = "postgres://alice:pw-ro@db.example.com/app";
    seed(dir.path(), &[row("c1", "postgres", original, json!({}))]).await;
    let store = TestStore::new();
    let ws = core()
        .open_workspace(
            WorkspaceSpec::new(dir.path())
                .with_storage_options(StorageOptions {
                    read_only: true,
                    ..Default::default()
                })
                .with_secrets(store.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        store.gets.load(Ordering::SeqCst) + store.sets.load(Ordering::SeqCst),
        0
    );
    assert_eq!(stored(&ws, "c1").await["connection_string"], original);
    assert!(!upgraded(&ws).await);
}

/// The config the builder makes for a saved row, with the store's secrets.
async fn config_of(ws: &Workspace, store: &TestStore, id: &str) -> seaquel_types::ConnectConfig {
    use seaquel_core::domain::connections::{plan, Target};
    let row = connections::get(ws.storage(), id).await.unwrap().unwrap();
    let plan = plan(
        Target::Saved(&row),
        &seaquel_core::SuppliedSecrets::none(),
        |key: String| {
            let value = store.entries().get(&key).cloned();
            async move { Ok::<_, String>(value) }
        },
    )
    .await
    .unwrap();
    plan.config(plan.tunnel(None).map(|_| 1), false).unwrap()
}

#[tokio::test]
async fn a_split_row_still_connects() {
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        &[
            // Moved to db:<id>.
            row(
                "url",
                "postgres",
                "postgres://alice:Pw-url1@db.example.com/app",
                json!({}),
            ),
            // Unmovable, but saved in the keychain too (the TS saved both).
            row(
                "ado",
                "mssql",
                "Server=db.example.com;Database=app;User Id=sa;Password=Pw-ado1;",
                json!({"save_password": 1}),
            ),
            row(
                "libpq",
                "postgres",
                "host=db.example.com dbname=app user=alice password=Pw-lib1",
                json!({"save_password": 1}),
            ),
            // A `+ssh` URL's path password is the database's.
            row(
                "path",
                "postgres",
                "postgresql+ssh://deploy:Ssh-pw1@bastion/alice:Pw-path1@db/app",
                json!({}),
            ),
        ],
    )
    .await;
    let store = TestStore::with(&[("db:ado", "Pw-ado1"), ("db:libpq", "Pw-lib1")]);
    let ws = open(&core(), dir.path(), Some(store.clone())).await;
    assert_eq!(store.entries()["db:url"], "Pw-url1");
    assert_eq!(store.entries()["db:path"], "Pw-path1");
    for (id, pw) in [
        ("url", "Pw-url1"),
        ("ado", "Pw-ado1"),
        ("libpq", "Pw-lib1"),
        ("path", "Pw-path1"),
    ] {
        let stored_string = stored(&ws, id).await["connection_string"].to_string();
        assert!(
            !stored_string.contains(pw),
            "{id} is stripped: {stored_string}"
        );
        let config = config_of(&ws, &store, id).await;
        let carried = config
            .connection_string
            .as_deref()
            .is_some_and(|s| s.contains(pw))
            || config.password.as_deref() == Some(pw);
        assert!(
            carried,
            "{id}: the builder puts db:<id> back into what it connects with"
        );
    }
}

// ── Live ──

fn live(name: &str) -> Option<Value> {
    let var = format!("SEAQUEL_TEST_{name}");
    match std::env::var(&var) {
        Ok(raw) => Some(serde_json::from_str(&raw).unwrap()),
        Err(_) if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("{var} is not set, and SEAQUEL_TEST_REQUIRE_ENGINES requires it")
        }
        Err(_) => {
            eprintln!("skipping: {var} is not set");
            None
        }
    }
}

#[tokio::test]
async fn a_split_url_row_connects_live() {
    let Some(pg) = live("POSTGRES") else { return };
    let base = pg["connection_string"].as_str().unwrap();
    // `postgres://postgres@127.0.0.1:5432/seaquel_test` with a password the
    // trusting test server ignores.
    let with_password = base.replacen("postgres@", "postgres:Live-pw1@", 1);
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        &[row(
            "live",
            "postgres",
            &with_password,
            json!({"host": "127.0.0.1", "database_name": "seaquel_test", "username": "postgres"}),
        )],
    )
    .await;
    let store = TestStore::new();
    let core = core();
    let ws = open(&core, dir.path(), Some(store.clone())).await;
    assert_eq!(store.entries()["db:live"], "Live-pw1");
    let id = ws
        .connect(&core, ConnectRequest::saved("live"))
        .await
        .expect("connects");
    let result = ws.query(&core, &id, "SELECT 1", vec![]).await.unwrap();
    assert_eq!(result.rows.len(), 1);
    ws.disconnect(&core, &id).await.unwrap();
}

#[tokio::test]
async fn an_ssh_password_row_still_connects_through_its_tunnel() {
    let Some(ssh) = live("SSH") else { return };
    let (host, port) = (ssh["host"].as_str().unwrap(), ssh["port"].as_u64().unwrap());
    let (remote, remote_port) = (
        ssh["remote_host"].as_str().unwrap(),
        ssh["remote_port"].as_u64().unwrap(),
    );
    let string = format!(
        "postgresql+ssh://seaquel:seaquel-test-password@{host}:{port}/postgres:Pg-pw1@{remote}:{remote_port}/postgres"
    );
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), &[row("tun", "postgres", &string, json!({"host": remote, "port": remote_port, "database_name": "postgres", "username": "postgres"}))]).await;
    let store = TestStore::new();
    let core = seaquel_core::with_default_plugins()
        .ssh_known_hosts(dir.path().join("known_hosts"))
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build();
    let ws = open(&core, dir.path(), Some(store.clone())).await;
    assert_eq!(store.entries()["ssh:tun"], "seaquel-test-password");
    assert_eq!(store.entries()["db:tun"], "Pg-pw1");
    let r = stored(&ws, "tun").await;
    assert_eq!(r["save_ssh_password"], 1);
    assert!(!r["connection_string"]
        .to_string()
        .contains("seaquel-test-password"));

    let err = ws
        .connect(&core, ConnectRequest::saved("tun"))
        .await
        .expect_err("an unknown host first");
    assert_eq!(err.code, "UNKNOWN_HOST_KEY", "{err}");
    let at = err.message.find("SHA256:").unwrap();
    let fingerprint: String = err.message[at..]
        .split(|c: char| c.is_whitespace() || c == ')')
        .next()
        .unwrap()
        .to_string();
    let id = ws
        .connect(
            &core,
            ConnectRequest::saved("tun").with_host_key(HostKeyPolicy::Trust(fingerprint)),
        )
        .await
        .expect("the tunnel authenticates with ssh:<id>");
    ws.query(&core, &id, "SELECT 1", vec![]).await.unwrap();
    ws.disconnect(&core, &id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn no_secret_in_logs_errors_events_or_debug() {
    let _ = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), &canary_rows()).await;
    // The first open's keychain reads fail: nothing moves, and the rows
    // wait for a pass this test watches.
    let store = TestStore::new();
    store.fail_get.store(true, Ordering::SeqCst);
    let ws = open(&core(), dir.path(), Some(store.clone())).await;
    let mut events = ws.events();
    store.fail_get.store(false, Ordering::SeqCst);
    store.fail_set.store(true, Ordering::SeqCst);
    ws.upgrade_string_secrets(None).await; // the keychain now refuses writes
    store.fail_set.store(false, Ordering::SeqCst);
    ws.upgrade_string_secrets(None).await;

    let mut seen = Vec::new();
    while let Ok(Some(e)) = tokio::time::timeout(
        Duration::from_millis(50),
        futures::StreamExt::next(&mut events),
    )
    .await
    {
        seen.push(e);
    }
    assert!(
        seen.iter().any(|e| matches!(e, WorkspaceEvent::StorageChanged(c) if c.kind == seaquel_core::StoredKind::Connection)),
        "{seen:?}"
    );
    let text = format!("{seen:?} {ws:?}\n{}", logged());
    assert!(!text.contains("canary"), "{text}");
    assert!(connections::with_secret_in_string(ws.storage())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_later_keychain_failure_takes_back_what_the_row_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let original = "postgresql+ssh://deploy:ssh-pw1@bastion/alice:db-pw1@db/app";
    seed(dir.path(), &[row("c1", "postgres", original, json!({}))]).await;
    let store = TestStore::new();
    *store.fail_set_key.lock().unwrap() = Some("ssh:c1".into());
    let ws = open(&core(), dir.path(), Some(store.clone())).await;
    assert!(
        store.entries().is_empty(),
        "db:c1 is taken back: {:?}",
        store.entries().keys()
    );
    assert_eq!(stored(&ws, "c1").await["connection_string"], original);
    assert!(!upgraded(&ws).await);
}

async fn key(ws: &Workspace, key: &str) -> bool {
    app_state::get(ws.storage(), key).await.unwrap().is_some()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_checkpoint_is_retried_and_doesnt_hold_the_upgrade() {
    use seaquel_core::{STRING_SECRETS_CHECKPOINT_KEY, STRING_SECRETS_VACUUM_KEY};
    let dir = tempfile::tempdir().unwrap();
    let path = seed(
        dir.path(),
        &[row(
            "c1",
            "postgres",
            "postgres://alice:pw-1@db.example.com/app",
            json!({}),
        )],
    )
    .await;
    // Another connection holds a read snapshot through the whole open, so
    // the checkpoint can't empty the WAL.
    let other = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let mut reader = other.pool().begin().await.unwrap();
    let _: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM connections")
        .fetch_one(&mut *reader)
        .await
        .unwrap();
    let core = core();
    let ws = open(&core, dir.path(), Some(TestStore::new())).await;
    assert_eq!(
        stored(&ws, "c1").await["connection_string"],
        "postgres://alice@db.example.com/app"
    );
    assert!(
        upgraded(&ws).await,
        "the upgraded flag doesn't wait for the scrub"
    );
    assert!(
        !key(&ws, STRING_SECRETS_VACUUM_KEY).await,
        "the rebuild ran"
    );
    assert!(
        key(&ws, STRING_SECRETS_CHECKPOINT_KEY).await,
        "the busy checkpoint is left"
    );
    ws.close().await;
    drop(reader);
    other.close().await;

    let ws = open(&core, dir.path(), Some(TestStore::new())).await;
    assert!(
        !key(&ws, STRING_SECRETS_CHECKPOINT_KEY).await,
        "the retry ran"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_vacuum_still_opens_and_the_next_open_retries_it() {
    use seaquel_core::{STRING_SECRETS_CHECKPOINT_KEY, STRING_SECRETS_VACUUM_KEY};
    let dir = tempfile::tempdir().unwrap();
    let path = seed(
        dir.path(),
        &[row(
            "c1",
            "postgres",
            "postgres://alice:pw-1@db.example.com/app",
            json!({}),
        )],
    )
    .await;
    // An earlier open stripped something and its scrub didn't finish.
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    for k in [STRING_SECRETS_VACUUM_KEY, STRING_SECRETS_CHECKPOINT_KEY] {
        app_state::set(&st, k, Some("1")).await.unwrap();
    }
    // While this open waits on the keychain, another process takes the
    // write lock and keeps it: the row, the rebuild and the checkpoint all
    // fail on it.
    let store = TestStore::new();
    store.block_set.store(true, Ordering::SeqCst);
    let entered = store.entered_set.notified();
    let core = Arc::new(core());
    let opening = tokio::spawn({
        let (core, dir, store) = (core.clone(), dir.path().to_path_buf(), store.clone());
        async move { open(&core, &dir, Some(store)).await }
    });
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .unwrap();
    let mut holder = st.write().await.unwrap();
    app_state::set_in(&mut holder, "holder", Some("1"))
        .await
        .unwrap();
    store.release();
    let ws = tokio::time::timeout(Duration::from_secs(60), opening)
        .await
        .expect("the app opens even though the scrub can't run")
        .unwrap();
    holder.rollback().await.unwrap();
    st.close().await;
    assert_eq!(
        app_state::get(ws.storage(), STRING_SECRETS_VACUUM_KEY)
            .await
            .unwrap()
            .as_deref(),
        Some("1"),
        "left for the next open; a rebuild that couldn't even be recorded isn't counted"
    );
    assert!(key(&ws, STRING_SECRETS_CHECKPOINT_KEY).await);
    assert!(!upgraded(&ws).await);
    ws.close().await;

    let ws = open(&core, dir.path(), Some(store.clone())).await;
    assert_eq!(
        stored(&ws, "c1").await["connection_string"],
        "postgres://alice@db.example.com/app"
    );
    assert!(upgraded(&ws).await);
    assert!(!key(&ws, STRING_SECRETS_VACUUM_KEY).await, "the retry ran");
    assert!(!key(&ws, STRING_SECRETS_CHECKPOINT_KEY).await);
}

#[tokio::test]
async fn a_strip_records_its_scrub_and_the_scrub_clears_it() {
    use seaquel_core::{STRING_SECRETS_CHECKPOINT_KEY, STRING_SECRETS_VACUUM_KEY};
    let _ = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    seed(
        dir.path(),
        &[row(
            "c1",
            "postgres",
            "postgres://alice:pw-1@db.example.com/app",
            json!({}),
        )],
    )
    .await;
    let ws = open(&core(), dir.path(), Some(TestStore::new())).await;
    assert!(!key(&ws, STRING_SECRETS_VACUUM_KEY).await);
    assert!(!key(&ws, STRING_SECRETS_CHECKPOINT_KEY).await);
    let log = logged();
    let line = log
        .lines()
        .find(|l| l.contains("Scrubbed the file"))
        .expect("the scrub is logged");
    for field in [
        "duration_ms=",
        "bytes_before=",
        "bytes_after=",
        "vacuumed=true",
        "checkpointed=true",
    ] {
        assert!(line.contains(field), "{line}");
    }
    assert!(!line.contains("pw-1"));
}

#[tokio::test]
async fn vacuum_stops_after_three_failed_attempts_and_the_checkpoint_keeps_going() {
    use seaquel_core::{
        MAX_VACUUM_ATTEMPTS, STRING_SECRETS_CHECKPOINT_KEY, STRING_SECRETS_VACUUM_KEY,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = seed(dir.path(), &[]).await;
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let cap = MAX_VACUUM_ATTEMPTS.to_string();
    app_state::set(&st, STRING_SECRETS_UPGRADED_KEY, Some("1"))
        .await
        .unwrap();
    app_state::set(&st, STRING_SECRETS_VACUUM_KEY, Some(&cap))
        .await
        .unwrap();
    app_state::set(&st, STRING_SECRETS_CHECKPOINT_KEY, Some("1"))
        .await
        .unwrap();
    st.close().await;

    let ws = open(&core(), dir.path(), Some(TestStore::new())).await;
    assert_eq!(
        app_state::get(ws.storage(), STRING_SECRETS_VACUUM_KEY)
            .await
            .unwrap(),
        Some(cap),
        "no fourth attempt: a failed one would count up, a good one clear it"
    );
    assert!(
        !key(&ws, STRING_SECRETS_CHECKPOINT_KEY).await,
        "the checkpoint still ran"
    );
}
