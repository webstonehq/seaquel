//! The browser's storage (phase 8 Task 2), run in wasm32 under Node:
//!
//! ```sh
//! CC_wasm32_unknown_unknown=<a clang with the wasm32 backend> \
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//! cargo test --target wasm32-unknown-unknown -p seaquel-storage --test wasm
//! ```
//!
//! With `SEAQUEL_RECORD_WASM_FIXTURE=1` in the environment, the last test
//! also writes its snapshot to `tests/fixtures/wasm-made/meta.db`, which
//! `tests/wasm_made.rs` opens natively.

#![cfg(target_arch = "wasm32")]

use seaquel_storage::{
    app_state, connections, dashboards, onboarding, projects, query_history, Storage,
    StorageOptions,
};
use seaquel_types::storage::PersistedProject;
use wasm_bindgen_test::wasm_bindgen_test;

/// Every migration this build knows, as sqlx records them.
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "name keys",
        include_str!("../migrations/0001_name_keys.sql"),
    ),
    (
        2,
        "window state",
        include_str!("../migrations/0002_window_state.sql"),
    ),
    (
        3,
        "window order and list meta",
        include_str!("../migrations/0003_window_order_and_list_meta.sql"),
    ),
    (
        4,
        "shared links",
        include_str!("../migrations/0004_shared_links.sql"),
    ),
    (
        5,
        "shared connection origin",
        include_str!("../migrations/0005_shared_connection_origin.sql"),
    ),
    (
        6,
        "history params",
        include_str!("../migrations/0006_history_params.sql"),
    ),
];

const DATA_STEPS: &[&str] = &[
    "backfill_dashboard_name_keys",
    "backfill_name_keys",
    "drop_legacy_built_connection_strings",
    "strip_connection_string_passwords",
];

async fn open(image: Option<Vec<u8>>) -> Storage {
    Storage::open("meta.db", StorageOptions::in_memory(image))
        .await
        .expect("the in-memory storage opens")
}

/// Reads through the storage's own pool with plain SQL, as the native
/// tests do with sqlx.
async fn rows(st: &Storage, sql: &str) -> Vec<Vec<String>> {
    st.debug_rows(sql).await.expect("a debug read")
}

async fn assert_current(st: &Storage) {
    let recorded = rows(
        st,
        "SELECT version, description, hex(checksum), success, execution_time >= 0 \
         FROM _sqlx_migrations ORDER BY version",
    )
    .await;
    let expected: Vec<Vec<String>> = MIGRATIONS
        .iter()
        .map(|(v, d, sql)| {
            use sha2::Digest;
            let sum: String = sha2::Sha384::digest(sql.as_bytes())
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect();
            vec![v.to_string(), d.to_string(), sum, "1".into(), "1".into()]
        })
        .collect();
    assert_eq!(recorded, expected);
    let steps = rows(st, "SELECT name FROM _seaquel_data_steps ORDER BY name").await;
    assert_eq!(
        steps,
        DATA_STEPS
            .iter()
            .map(|s| vec![s.to_string()])
            .collect::<Vec<_>>()
    );
    let version = rows(st, "SELECT MAX(version) FROM schema_version").await;
    assert_eq!(version, vec![vec!["4".to_string()]]);
}

#[wasm_bindgen_test]
async fn migrations_record_sqlx_checksums() {
    let st = open(None).await;
    assert_current(&st).await;
    // The table is sqlx's own, column for column.
    let table = rows(
        &st,
        "SELECT sql FROM sqlite_master WHERE name = '_sqlx_migrations'",
    )
    .await;
    assert_eq!(
        table,
        vec![vec![
            "CREATE TABLE _sqlx_migrations (\n    version BIGINT PRIMARY KEY,\n    \
                   description TEXT NOT NULL,\n    installed_on TIMESTAMP NOT NULL DEFAULT \
                   CURRENT_TIMESTAMP,\n    success BOOLEAN NOT NULL,\n    checksum BLOB NOT \
                   NULL,\n    execution_time BIGINT NOT NULL\n)"
                .to_string()
        ]]
    );
    // Foreign keys are on, and a new file has 4096-byte pages.
    assert_eq!(
        rows(&st, "PRAGMA foreign_keys").await,
        vec![vec!["1".to_string()]]
    );
    assert_eq!(
        rows(&st, "PRAGMA page_size").await,
        vec![vec!["4096".to_string()]]
    );
}

#[wasm_bindgen_test]
async fn the_s6_files_open_in_the_module() {
    // Today's demo build after a cold load and a reload: its connection,
    // with the label the visitor gave it, survives the upgrade.
    let st = open(Some(
        include_bytes!("fixtures/sqljs/demo-2026-10-01.db").to_vec(),
    ))
    .await;
    assert_current(&st).await;
    let conns = connections::load_all(&st).await.unwrap();
    assert_eq!(
        conns.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["demo-connection"]
    );
    assert_eq!(conns[0].label_ids, ["prod"]);

    // The live demo's file (2026-09-23, before phase 1), whose header the
    // spike's native open had switched to WAL: no connection, two sample
    // dashboards with one name, both kept.
    let st = open(Some(
        include_bytes!("fixtures/sqljs/live-demo-2026-09-23.db").to_vec(),
    ))
    .await;
    assert_current(&st).await;
    assert!(connections::load_all(&st).await.unwrap().is_empty());
    let boards = dashboards::list(&st, "default-seaquel").await.unwrap();
    assert_eq!(boards.len(), 2);
    assert!(boards.iter().all(|d| d.name == "E-Commerce Overview"));
}

#[wasm_bindgen_test]
async fn bytes_that_arent_a_database_are_corrupt() {
    let e = Storage::open(
        "meta.db",
        StorageOptions::in_memory(Some(b"{\"json\": true}".to_vec())),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code(), "STORAGE_CORRUPT");
    let mut junk = b"SQLite format 3\0".to_vec();
    junk.resize(4096, 7);
    let e = Storage::open("meta.db", StorageOptions::in_memory(Some(junk)))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "STORAGE_CORRUPT");
    assert!(e.to_string().contains("meta.db"), "{e}");
}

#[wasm_bindgen_test]
async fn writes_commit_count_and_roll_back_as_on_desktop() {
    let st = open(None).await;
    let before = st.commits();
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p1", "One"))
        .await
        .unwrap();
    drop(tx);
    assert_eq!(st.commits(), before, "a dropped write commits nothing");
    assert!(projects::get(&st, "p1").await.unwrap().is_none());

    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p1", "One"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(st.commits(), before + 1);
    app_state::set(&st, "k", Some("v")).await.unwrap();
    assert_eq!(st.commits(), before + 2);

    // A second writer waits for the first.
    let first = st.write().await.unwrap();
    let mut second = Box::pin(st.write());
    assert!(futures::poll!(second.as_mut()).is_pending());
    drop(first);
    assert!(futures::poll!(second.as_mut()).is_ready());

    // SQLITE_FULL reads as STORAGE_FULL, and the storage carries on.
    drop(second);
    st.debug_rows("PRAGMA max_page_count = 1").await.unwrap();
    let e = app_state::set(&st, "big", Some(&"x".repeat(100_000)))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "STORAGE_FULL");
    st.debug_rows("PRAGMA max_page_count = 1073741823")
        .await
        .unwrap();
    app_state::set(&st, "big", Some("small")).await.unwrap();

    // No snapshot while a write is open.
    let tx = st.write().await.unwrap();
    assert!(st.snapshot().is_err());
    drop(tx);
    assert!(st.snapshot().is_ok());
}

#[wasm_bindgen_test]
async fn json_reads_back_byte_for_byte() {
    // 5d's rule: stored JSON comes back as the bytes stored (spacing,
    // escapes, key order), not re-serialized.
    let st = open(None).await;
    let text = "{ \"b\":1,  \"a\":\"\\u00e9 Stra\u{df}e\", \"n\": 1.50 }";
    let raw = serde_json::value::RawValue::from_string(text.to_string()).unwrap();
    onboarding::save(&st, &raw).await.unwrap();
    let back = onboarding::load(&st).await.unwrap().unwrap();
    assert_eq!(back.get(), text);
}

#[wasm_bindgen_test]
async fn history_keeps_an_applied_changes_values() {
    // Cleanup pass B: the demo records a grid edit's values like desktop.
    let st = open(None).await;
    let project: PersistedProject = serde_json::from_value(serde_json::json!({
        "id": "p", "name": "P", "createdAt": "c", "updatedAt": "u", "customLabels": []
    }))
    .unwrap();
    projects::save(&st, &project).await.unwrap();
    let c: seaquel_types::storage::PersistedConnection =
        serde_json::from_value(serde_json::json!({
            "id": "c1", "projectId": "p", "name": "C", "type": "duckdb", "host": "",
            "port": 0, "databaseName": "", "username": "", "labelIds": []
        }))
        .unwrap();
    connections::save(&st, &c).await.unwrap();
    let item: seaquel_types::storage::PersistedQueryHistoryItem =
        serde_json::from_value(serde_json::json!({
            "id": "h", "query": "UPDATE t SET a = ? WHERE id = ?", "timestamp": "t",
            "executionTime": 1, "rowCount": 1, "connectionId": "c1", "favorite": false,
            "connectionNameSnapshot": "C",
            "params": ["Jonson", {"$sq": "bigint", "v": "9007199254740993"}]
        }))
        .unwrap();
    query_history::append_many(&st, &[item]).await.unwrap();
    let rows = query_history::load_by_connection(&st, "c1").await.unwrap();
    assert_eq!(
        serde_json::to_value(&rows[0]).unwrap()["params"],
        serde_json::json!(["Jonson", {"$sq": "bigint", "v": "9007199254740993"}])
    );
}

#[wasm_bindgen_test]
async fn secure_delete_vacuum_and_checkpoint_work_in_memory() {
    // The string-secrets upgrade calls all three (5d Decision 12a).
    let st = open(None).await;
    let mut tx = st.write().await.unwrap();
    tx.secure_delete(true).await.unwrap();
    projects::insert(&mut tx, &project("p1", "One"))
        .await
        .unwrap();
    tx.secure_delete(false).await.unwrap();
    tx.commit().await.unwrap();
    // VACUUM really rebuilds: the pages a delete freed are gone after it.
    app_state::set(&st, "big", Some(&"x".repeat(200_000)))
        .await
        .unwrap();
    let mut tx = st.write().await.unwrap();
    app_state::delete_in(&mut tx, "big").await.unwrap();
    tx.commit().await.unwrap();
    let free = |rows: Vec<Vec<String>>| rows[0][0].parse::<i64>().unwrap();
    assert!(free(st.debug_rows("PRAGMA freelist_count").await.unwrap()) > 0);
    st.vacuum().await.unwrap();
    assert_eq!(
        free(st.debug_rows("PRAGMA freelist_count").await.unwrap()),
        0
    );
    // No WAL in memory: the checkpoint has nothing to do and isn't busy.
    assert!(st.checkpoint().await.unwrap());
    assert_eq!(projects::get(&st, "p1").await.unwrap().unwrap().name, "One");
}

fn project(id: &str, name: &str) -> PersistedProject {
    PersistedProject {
        id: id.to_string(),
        name: name.to_string(),
        description: None,
        created_at: "2026-10-02T00:00:00.000Z".to_string(),
        updated_at: "2026-10-02T00:00:00.000Z".to_string(),
        custom_labels: Vec::new(),
        git_repo_path: None,
    }
}

#[wasm_bindgen_test]
async fn a_snapshot_reopens_with_nothing_pending() {
    let st = open(None).await;
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p-wasm", "Straße 東京"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    app_state::set(&st, "madeBy", Some("wasm")).await.unwrap();
    let image = st.snapshot().unwrap();

    let again = open(Some(image.clone())).await;
    assert_current(&again).await;
    // An up-to-date file runs no migration or data step: the open changes
    // nothing (its baseline transaction writes nothing).
    assert_eq!(again.snapshot().unwrap(), image);
    let p = projects::get(&again, "p-wasm").await.unwrap().unwrap();
    assert_eq!(p.name, "Straße 東京");
    assert_eq!(
        app_state::get(&again, "madeBy").await.unwrap().as_deref(),
        Some("wasm")
    );

    record_fixture(&image);
}

/// Writes `image` to `tests/fixtures/wasm-made/meta.db` when
/// `SEAQUEL_RECORD_WASM_FIXTURE=1` (Node's `fs`, through the global
/// `process`).
fn record_fixture(image: &[u8]) {
    use js_sys::{Function, Reflect, Uint8Array};
    use wasm_bindgen::{JsCast, JsValue};
    let global = js_sys::global();
    let Ok(process) = Reflect::get(&global, &JsValue::from_str("process")) else {
        return;
    };
    let env = Reflect::get(&process, &JsValue::from_str("env")).unwrap_or(JsValue::UNDEFINED);
    let flag = Reflect::get(&env, &JsValue::from_str("SEAQUEL_RECORD_WASM_FIXTURE"))
        .ok()
        .and_then(|v| v.as_string());
    if flag.as_deref() != Some("1") {
        return;
    }
    let get = Reflect::get(&process, &JsValue::from_str("getBuiltinModule"))
        .unwrap()
        .dyn_into::<Function>()
        .unwrap();
    let fs = get.call1(&process, &JsValue::from_str("fs")).unwrap();
    let write = Reflect::get(&fs, &JsValue::from_str("writeFileSync"))
        .unwrap()
        .dyn_into::<Function>()
        .unwrap();
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/wasm-made/meta.db"
    );
    write
        .call2(&fs, &JsValue::from_str(path), &Uint8Array::from(image))
        .unwrap();
}
