//! Phase 5d-2 storage: migration `0002_window_state.sql`, the targeted
//! queries Core's `library`, `settings` and `ui` calls run inside one
//! write transaction (dashboards and their versions, saved workflows, AI
//! chats and messages, settings records, window view state), the legacy
//! mirror of `project_state` and `tabs`, and the
//! `backfill_dashboard_name_keys` data step.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use seaquel_storage::{
    ai_chats, app_state, connections, dashboard_versions, dashboards, import_state, onboarding,
    project_state, projects, saved_canvases, themes, tutorial, window_state, windows, IdName,
    Storage, StorageError, StorageOptions, DATA_STEPS_TABLE, STORAGE_NEEDS_UPGRADE,
};
use seaquel_types::names::name_key;
use seaquel_types::storage::{
    PersistedAIChat, PersistedAIMessage, PersistedConnection, PersistedCreateTableTab,
    PersistedDashboard, PersistedDashboardTab, PersistedDataTab, PersistedErdTab,
    PersistedExplainTab, PersistedExtensionsDuckdbTab, PersistedProject, PersistedProjectState,
    PersistedQueryTab, PersistedSchemaTab, PersistedStarterTab, PersistedStatisticsTab,
    PersistedWorkflowTab,
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

fn read_only() -> StorageOptions {
    StorageOptions {
        read_only: true,
        ..StorageOptions::default()
    }
}

fn raw(s: &str) -> Box<RawValue> {
    RawValue::from_string(s.into()).unwrap()
}

fn texts(v: &[Box<RawValue>]) -> Vec<&str> {
    v.iter().map(|r| r.get()).collect()
}

fn ids(v: Vec<IdName>) -> Vec<String> {
    v.into_iter().map(|n| n.id).collect()
}

async fn count(st: &Storage, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(st.pool()).await.unwrap()
}

async fn exec(st: &Storage, sql: &str) {
    sqlx::raw_sql(sql).execute(st.pool()).await.unwrap();
}

fn project(id: &str) -> PersistedProject {
    PersistedProject {
        id: id.into(),
        name: id.into(),
        description: None,
        created_at: "c".into(),
        updated_at: "u".into(),
        custom_labels: vec![],
        git_repo_path: None,
    }
}

fn connection(id: &str, project_id: &str) -> PersistedConnection {
    PersistedConnection {
        id: id.into(),
        project_id: project_id.into(),
        name: id.into(),
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

/// Projects `p` and `q`, and a connection `c` in `p`.
async fn seeded(st: &Storage) {
    let mut tx = st.write().await.unwrap();
    projects::insert(&mut tx, &project("p")).await.unwrap();
    projects::insert(&mut tx, &project("q")).await.unwrap();
    connections::insert(&mut tx, &connection("c", "p"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

fn dashboard(id: &str, project_id: &str, name: &str) -> PersistedDashboard {
    PersistedDashboard {
        id: id.into(),
        project_id: project_id.into(),
        name: name.into(),
        viewport: r#"{"x":0,"y":0,"zoom":1}"#.into(),
        widgets: "[]".into(),
        date_filter: None,
        starred: false,
        shared: false,
        description: None,
        created_at: "2026-10-01T00:00:00.000Z".into(),
        updated_at: "2026-10-01T00:00:00.000Z".into(),
        shared_path: None,
    }
}

fn chat(id: &str, connection_id: &str, updated_at: &str) -> PersistedAIChat {
    PersistedAIChat {
        id: id.into(),
        connection_id: connection_id.into(),
        title: "T".into(),
        created_at: "2026-10-01T00:00:00.000Z".into(),
        updated_at: updated_at.into(),
    }
}

fn message(id: &str, chat_id: &str, role: &str, content: &str, ts: &str) -> PersistedAIMessage {
    PersistedAIMessage {
        id: id.into(),
        chat_id: chat_id.into(),
        role: role.into(),
        content: content.into(),
        timestamp: ts.into(),
        query: None,
        dashboard_id: None,
    }
}

/// The lines of `EXPLAIN QUERY PLAN sql`, joined.
async fn plan(st: &Storage, sql: &str) -> String {
    let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(&format!("EXPLAIN QUERY PLAN {sql}"))
        .fetch_all(st.pool())
        .await
        .unwrap();
    rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n")
}

/// No full scan of a table and no sort: every `SCAN` walks an index (or
/// `json_each`, the parameter list), and no temp B-tree orders the rows.
fn assert_indexed(sql: &str, plan: &str, index: &str) {
    for line in plan.lines() {
        if line.contains("SCAN") && !line.contains("json_each") {
            assert!(line.contains("INDEX"), "a full scan in\n{sql}:\n{plan}");
        }
        assert!(!line.contains("TEMP B-TREE"), "a sort in\n{sql}:\n{plan}");
    }
    assert!(plan.contains(index), "{index} unused in\n{sql}:\n{plan}");
}

// ── Migration 0002 ──

/// On every release's file (the beta-era one included, where
/// `dashboards.project_id` is nullable, last and has no foreign key) the
/// migration adds the window tables and the dashboards' key, and they work.
#[tokio::test]
async fn migration_0002_applies_on_every_release_schema() {
    for release in [
        "v2026.4.5-beta.1",
        "v2026.4.5",
        "v2026.4.8",
        "v2026.9.1",
        "v2026.9.2",
        "current",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        load_fixture(&path, &format!("schemas/{release}.sql")).await;
        let st = Storage::open(&path, StorageOptions::default())
            .await
            .unwrap_or_else(|e| panic!("{release}: {e}"));
        let applied: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(st.pool())
                .await
                .unwrap();
        // Every later migration runs too (`0004`, phase 5e).
        assert_eq!(applied, [1, 2, 3, 4, 5], "{release}");

        let mut tx = st.write().await.unwrap();
        projects::insert(&mut tx, &project("p")).await.unwrap();
        dashboards::insert(&mut tx, &dashboard("d", "p", "Sales"))
            .await
            .unwrap();
        windows::touch(&mut tx, "main", "2026-10-01T00:00:00.000Z")
            .await
            .unwrap();
        assert!(
            window_state::put_if_newer(&mut tx, "main", "p", 1, "{}", "2026-10-01T00:00:00.000Z")
                .await
                .unwrap()
                .written
        );
        tx.commit().await.unwrap();
        assert_eq!(
            ids(dashboards::with_name_key(&st, "p", &name_key("SALES"))
                .await
                .unwrap()),
            ["d"],
            "{release}"
        );
        // A removed project takes its window states with it.
        let mut tx = st.write().await.unwrap();
        projects::delete_with_orphans(&mut tx, "p").await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            count(&st, "SELECT COUNT(*) FROM window_state").await,
            0,
            "{release}"
        );
        st.close().await;
    }
}

/// Migration `0003` on a file that already has `0002`'s tables and rows:
/// the window rows are numbered in the order the old queries read them
/// (so the most recent window and view state don't change), and the
/// workflows' `meta` and the versions' `widget_count` are filled from
/// their bodies, a row that isn't JSON marked (`'null'`, `-1`).
#[tokio::test]
async fn migration_0003_numbers_the_windows_and_fills_the_list_meta() {
    let dir = tempfile::tempdir().unwrap();
    let before_0003 = dir.path().join("migrations");
    std::fs::create_dir(&before_0003).unwrap();
    for file in ["0001_name_keys.sql", "0002_window_state.sql"] {
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("migrations")
                .join(file),
            before_0003.join(file),
        )
        .unwrap();
    }
    let path = dir.path().join("seaquel.db");
    let older = sqlx::migrate::Migrator::new(before_0003).await.unwrap();
    let st = Storage::open_with_migrator(&path, StorageOptions::default(), older)
        .await
        .unwrap();
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    dashboards::insert(&mut tx, &dashboard("d", "p", "D"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    // Before 0003: `windows` read the later row first on equal times,
    // `window_state` the earlier one.
    exec(
        &st,
        r#"INSERT INTO windows (window_id, active_project_id, updated_at) VALUES
             ('a', 'p', '2026-10-02'), ('b', 'p', '2026-10-02'), ('c', 'p', '2026-10-01');
           INSERT INTO window_state (window_id, project_id, state, rev, updated_at) VALUES
             ('a', 'p', '{"w":"a"}', 1, '2026-10-02'), ('b', 'p', '{"w":"b"}', 1, '2026-10-02'),
             ('c', 'p', '{"w":"c"}', 1, '2026-10-01');
           INSERT INTO saved_canvases (id, project_id, data) VALUES
             ('w1', 'p', '{"id":"w1","name":"One","createdAt":"c","updatedAt":"u","nodes":[]}'),
             ('w2', 'p', 'not json');
           INSERT INTO dashboard_versions (id, dashboard_id, version, snapshot, created_at) VALUES
             ('v1', 'd', 1, '{"widgets":[1,2]}', 't'), ('v2', 'd', 2, 'nope', 't');"#,
    )
    .await;
    st.close().await;

    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(applied, [1, 2, 3, 4, 5]);
    let seqs: Vec<(String, i64)> =
        sqlx::query_as("SELECT window_id, write_seq FROM windows ORDER BY window_id")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(
        seqs,
        [("a".into(), 2), ("b".into(), 3), ("c".into(), 1)],
        "the old order: time, then the later row"
    );
    let seqs: Vec<(String, i64)> =
        sqlx::query_as("SELECT window_id, write_seq FROM window_state ORDER BY window_id")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(
        seqs,
        [("a".into(), 3), ("b".into(), 2), ("c".into(), 1)],
        "the old order: time, then the earlier row"
    );
    assert_eq!(
        windows::most_recent_active(&st)
            .await
            .unwrap()
            .unwrap()
            .window_id,
        "b"
    );
    assert_eq!(
        window_state::most_recent(&st, "p")
            .await
            .unwrap()
            .unwrap()
            .window_id,
        "a"
    );
    let metas: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id, meta FROM saved_canvases ORDER BY id")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(
        metas,
        [
            (
                "w1".into(),
                Some(r#"{"name":"One","createdAt":"c","updatedAt":"u"}"#.into())
            ),
            ("w2".into(), Some("null".into()))
        ]
    );
    let counts: Vec<(String, Option<i64>)> =
        sqlx::query_as("SELECT id, widget_count FROM dashboard_versions ORDER BY id")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(counts, [("v1".into(), Some(2)), ("v2".into(), Some(-1))]);
    // New writes number on from there.
    let mut tx = st.write().await.unwrap();
    windows::touch(&mut tx, "c", "2026-09-01").await.unwrap();
    tx.commit().await.unwrap();
    let c: i64 = sqlx::query_scalar("SELECT write_seq FROM windows WHERE window_id = 'c'")
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(c, 4, "the last write, whatever its time");
    st.close().await;
}

// ── The dashboards' name key ──

const DASHBOARD_STEP: &str = "backfill_dashboard_name_keys";

async fn dashboard_keys(st: &Storage) -> Vec<(String, Option<String>)> {
    sqlx::query_as("SELECT id, name_key FROM dashboards ORDER BY rowid")
        .fetch_all(st.pool())
        .await
        .unwrap()
}

/// A file from before `0002`: the migration adds the column, and the step
/// fills every dashboard's key (a name that isn't UTF-8 stays NULL and
/// matches nothing), once.
#[tokio::test]
async fn the_dashboard_name_key_step_fills_old_rows() {
    for fixture in ["schemas/current.sql", "schemas/v2026.4.5-beta.1.sql"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        load_fixture(&path, fixture).await;
        let seed = if fixture.contains("beta") {
            // beta.1 has no project_id on dashboards; the baseline adds it
            // (nullable), so the seed goes in after a first open.
            let st = Storage::open(&path, StorageOptions::default())
                .await
                .unwrap();
            exec(
                &st,
                &format!(
                    "DELETE FROM {DATA_STEPS_TABLE} WHERE name = '{DASHBOARD_STEP}'; \
                     UPDATE dashboards SET name_key = NULL;"
                ),
            )
            .await;
            st.close().await;
            true
        } else {
            false
        };
        // Only a beta-era file can hold a dashboard with no project.
        let b_project = if seed { "NULL" } else { "'p'" };
        let mut conn = raw_connect(&path).await;
        for sql in [
            "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p', 'P', 'c', 'u')"
                .to_string(),
            "INSERT INTO dashboards (id, project_id, name, created_at, updated_at) \
             VALUES ('a', 'p', ' Straße ', 'c', 'u')"
                .to_string(),
            "INSERT INTO dashboards (id, project_id, name, created_at, updated_at) \
             VALUES ('bad', 'p', CAST(X'FF' AS TEXT), 'c', 'u')"
                .to_string(),
            format!(
                "INSERT INTO dashboards (id, project_id, name, created_at, updated_at) \
                 VALUES ('b', {b_project}, 'ÄRGER', 'c', 'u')"
            ),
        ] {
            sqlx::query(&sql).execute(&mut conn).await.unwrap();
        }
        sqlx::Connection::close(conn).await.unwrap();

        let refused = Storage::open(&path, read_only()).await.unwrap_err();
        assert_eq!(refused.code(), STORAGE_NEEDS_UPGRADE, "{fixture}");

        let st = Storage::open(&path, StorageOptions::default())
            .await
            .unwrap();
        assert_eq!(
            dashboard_keys(&st).await,
            [
                ("a".to_string(), Some(name_key("strasse"))),
                ("bad".to_string(), None),
                ("b".to_string(), Some(name_key("ärger"))),
            ],
            "{fixture} (seeded after a first open: {seed})"
        );
        assert_eq!(
            ids(dashboards::with_name_key(&st, "p", &name_key("STRASSE"))
                .await
                .unwrap()),
            ["a"]
        );
        assert!(dashboards::with_name_key(&st, "p", &name_key("\u{fffd}"))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            count(
                &st,
                &format!("SELECT COUNT(*) FROM {DATA_STEPS_TABLE} WHERE name = '{DASHBOARD_STEP}'")
            )
            .await,
            1
        );
        st.close().await;
        Storage::open(&path, read_only())
            .await
            .unwrap()
            .close()
            .await;
    }
}

/// The CLI refuses a file whose dashboards step hasn't run, naming it.
#[tokio::test]
async fn a_read_only_open_refuses_a_file_with_the_dashboard_step_pending() {
    let dir = tempfile::tempdir().unwrap();
    let (st, path) = fresh(dir.path()).await;
    exec(
        &st,
        &format!("DELETE FROM {DATA_STEPS_TABLE} WHERE name = '{DASHBOARD_STEP}'"),
    )
    .await;
    st.close().await;
    let before = snapshot(&path).await;
    let err = Storage::open(&path, read_only()).await.unwrap_err();
    assert_eq!(err.code(), STORAGE_NEEDS_UPGRADE, "{err}");
    assert!(
        matches!(&err, StorageError::DataStepPending { step, .. } if step == DASHBOARD_STEP),
        "{err:?}"
    );
    assert_eq!(snapshot(&path).await, before);
}

/// Linear in rows and name length: 20,000 dashboards and one 200 KB name.
// A native test measuring its own wall time; the wasm32 rule behind
// `disallowed_types` doesn't apply.
#[allow(clippy::disallowed_types)]
#[tokio::test]
async fn the_dashboard_name_key_step_is_linear() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    load_fixture(&path, "schemas/current.sql").await;
    let mut conn = raw_connect(&path).await;
    sqlx::raw_sql(
        "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p', 'P', 'c', 'u'); \
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000) \
         INSERT INTO dashboards (id, project_id, name, created_at, updated_at) \
         SELECT 'd' || i, 'p', 'Dashboard ' || i, 'c', 'u' FROM n; \
         INSERT INTO dashboards (id, project_id, name, created_at, updated_at) \
         VALUES ('long', 'p', hex(zeroblob(100000)), 'c', 'u');",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::Connection::close(conn).await.unwrap();

    let started = std::time::Instant::now();
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(
        count(
            &st,
            "SELECT COUNT(*) FROM dashboards WHERE name_key IS NULL"
        )
        .await,
        0
    );
    assert!(elapsed.as_secs() < 10, "open took {elapsed:?}");
    st.close().await;
}

/// Storage writes the key with every name, a case-only rename included;
/// an older release's rename NULLs it (the trigger), and the lookup still
/// finds that row by its name.
#[tokio::test]
async fn an_older_release_renaming_a_dashboard_nulls_its_key() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    dashboards::insert(&mut tx, &dashboard("d", "p", "Sales"))
        .await
        .unwrap();
    dashboards::insert(&mut tx, &dashboard("e", "p", "Other"))
        .await
        .unwrap();
    assert!(dashboards::update(&mut tx, &dashboard("d", "p", "SALES"))
        .await
        .unwrap());
    tx.commit().await.unwrap();
    assert_eq!(
        dashboard_keys(&st).await,
        [
            ("d".to_string(), Some(name_key("sales"))),
            ("e".to_string(), Some(name_key("other")))
        ],
        "a case-only rename keeps its key"
    );

    exec(&st, "UPDATE dashboards SET name = 'sales' WHERE id = 'e'").await;
    assert_eq!(dashboard_keys(&st).await[1], ("e".to_string(), None));
    assert_eq!(
        ids(dashboards::with_name_key(&st, "p", &name_key("Sales"))
            .await
            .unwrap()),
        ["d", "e"]
    );
    assert!(dashboards::with_name_key(&st, "q", &name_key("Sales"))
        .await
        .unwrap()
        .is_empty());
    st.close().await;
}

/// `refill_name_keys` (every writable open, through Core) also fills
/// dashboards an older release wrote or renamed after the step ran.
#[tokio::test]
async fn refill_covers_dashboards() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    assert!(!seaquel_storage::refill_name_keys(&st).await.unwrap());
    exec(
        &st,
        "INSERT INTO dashboards (id, project_id, name, created_at, updated_at) \
         VALUES ('old', 'p', 'Older', 'c', 'u')",
    )
    .await;
    assert!(seaquel_storage::refill_name_keys(&st).await.unwrap());
    assert_eq!(
        dashboard_keys(&st).await,
        [("old".to_string(), Some(name_key("older")))]
    );
    assert!(!seaquel_storage::refill_name_keys(&st).await.unwrap());
    st.close().await;
}

/// 5d-2 Task 7 review: an older release's replace-all save writes
/// `saved_canvases` rows (and versions) with no `meta` (`widget_count`);
/// every writable open fills them again, as `refill_name_keys` does. Only
/// an indexed `EXISTS` read when there's nothing to fill: a row that can't
/// be read or counted is marked (`'null'`, `-1`) instead of staying NULL,
/// so it isn't found again.
#[tokio::test]
async fn list_meta_is_refilled_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let (st, path) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    dashboards::insert(&mut tx, &dashboard("d", "p", "D"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(!seaquel_storage::refill_list_meta(&st).await.unwrap());
    // What an older release writes: whole rows, no meta, no count.
    exec(
        &st,
        r#"INSERT INTO saved_canvases (id, project_id, data) VALUES
             ('w1', 'p', '{"name":"Old","updatedAt":"u"}'),
             ('bad', 'p', 'not json');
           INSERT INTO dashboard_versions (id, dashboard_id, version, snapshot, created_at) VALUES
             ('v1', 'd', 1, '{"widgets":[1,2]}', 't'), ('v2', 'd', 2, 'nope', 't');"#,
    )
    .await;
    assert!(seaquel_storage::refill_list_meta(&st).await.unwrap());
    let metas: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id, meta FROM saved_canvases ORDER BY id")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(
        metas,
        [
            ("bad".into(), Some("null".into())),
            (
                "w1".into(),
                Some(r#"{"name":"Old","createdAt":null,"updatedAt":"u"}"#.into())
            ),
        ]
    );
    let counts: Vec<(String, Option<i64>)> =
        sqlx::query_as("SELECT id, widget_count FROM dashboard_versions ORDER BY id")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(counts, [("v1".into(), Some(2)), ("v2".into(), Some(-1))]);
    // Marked rows aren't found again; lists read them as before.
    assert!(!seaquel_storage::refill_list_meta(&st).await.unwrap());
    let listed = saved_canvases::list_meta(&st, "p").await.unwrap();
    assert_eq!(
        listed.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        ["w1"]
    );
    let versions = dashboard_versions::list_meta_by_project(&st, "p")
        .await
        .unwrap();
    assert_eq!(
        versions.iter().map(|v| v.widget_count).collect::<Vec<_>>(),
        [Some(2), None]
    );
    // The pending check reads a partial index, never the bodies.
    let plan_text = plan(&st, seaquel_storage::LIST_META_PENDING).await;
    for index in [
        "idx_saved_canvases_meta_pending",
        "idx_dashboard_versions_count_pending",
    ] {
        assert!(plan_text.contains(index), "{index}: {plan_text}");
    }
    for line in plan_text.lines().filter(|l| l.contains("SCAN")) {
        assert!(
            line.contains("INDEX") || line.contains("CONSTANT ROW"),
            "{plan_text}"
        );
    }
    st.close().await;
    // A web user's (capped) open fills them too.
    exec_file(&path, "UPDATE saved_canvases SET meta = NULL").await;
    let st = Storage::open(&path, capped(64 * 1024 * 1024, 2))
        .await
        .unwrap();
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM saved_canvases WHERE meta IS NULL")
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(pending, 0);
    st.close().await;
}

// ── Dashboards ──

#[tokio::test]
async fn dashboards_insert_get_list_update_delete_and_count() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut d = dashboard("d", "p", "Sales");
    d.date_filter = Some(r#"{"range":"7d"}"#.into());
    d.description = Some("desc".into());
    d.starred = true;
    let mut tx = st.write().await.unwrap();
    dashboards::insert(&mut tx, &d).await.unwrap();
    dashboards::insert(&mut tx, &dashboard("e", "p", "E"))
        .await
        .unwrap();
    dashboards::insert(&mut tx, &dashboard("f", "q", "F"))
        .await
        .unwrap();
    // Read inside the transaction: it sees its own writes.
    assert_eq!(
        dashboards::get(&mut tx, "d").await.unwrap(),
        Some(d.clone())
    );
    assert!(dashboards::insert(&mut tx, &dashboard("d", "p", "Again"))
        .await
        .is_err());
    tx.commit().await.unwrap();

    assert_eq!(
        dashboards::list(&st, "p")
            .await
            .unwrap()
            .iter()
            .map(|d| d.id.as_str())
            .collect::<Vec<_>>(),
        ["d", "e"]
    );
    assert_eq!(dashboards::count(&st).await.unwrap(), 3);

    // Every field but project_id and created_at changes.
    let mut changed = d.clone();
    changed.project_id = "q".into();
    changed.created_at = "never".into();
    changed.name = "Renamed".into();
    changed.widgets = r#"[{"id":"w"}]"#.into();
    changed.date_filter = None;
    changed.description = None;
    changed.starred = false;
    changed.shared = true;
    changed.updated_at = "2026-10-02T00:00:00.000Z".into();
    let mut tx = st.write().await.unwrap();
    assert!(dashboards::update(&mut tx, &changed).await.unwrap());
    assert!(
        !dashboards::update(&mut tx, &dashboard("missing", "p", "M"))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let stored = dashboards::get(&st, "d").await.unwrap().unwrap();
    assert_eq!(stored.project_id, "p");
    assert_eq!(stored.created_at, d.created_at);
    assert_eq!(stored.name, "Renamed");
    assert_eq!(stored.widgets, changed.widgets);
    assert_eq!(stored.date_filter, None);
    assert!(!stored.starred && stored.shared);

    // Every file can hold a NULL `starred`; it reads as false.
    exec(&st, "UPDATE dashboards SET starred = NULL WHERE id = 'e'").await;
    assert!(!dashboards::get(&st, "e").await.unwrap().unwrap().starred);

    let mut tx = st.write().await.unwrap();
    assert!(dashboards::delete(&mut tx, "d").await.unwrap());
    assert!(!dashboards::delete(&mut tx, "d").await.unwrap());
    tx.commit().await.unwrap();
    assert_eq!(dashboards::get(&st, "d").await.unwrap(), None);
    assert_eq!(dashboards::count(&st).await.unwrap(), 2);
    st.close().await;
}

#[tokio::test]
async fn the_dashboard_lookup_uses_its_index() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let plan = plan(&st, dashboards::NAME_KEY_LOOKUP).await;
    assert!(!plan.contains("SCAN"), "{plan}");
    assert_eq!(plan.matches("idx_dashboards_name_key").count(), 2, "{plan}");
    st.close().await;
}

#[tokio::test]
async fn dashboard_versions_number_after_the_highest_inside_the_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    dashboards::insert(&mut tx, &dashboard("d", "p", "D"))
        .await
        .unwrap();
    dashboards::insert(&mut tx, &dashboard("e", "q", "E"))
        .await
        .unwrap();
    let first = dashboard_versions::append(&mut tx, "v1", "d", "{\"a\":1}", "t1")
        .await
        .unwrap();
    assert_eq!(first.version, 1.0);
    assert_eq!((first.bytes, first.widget_count), (7, None));
    tx.commit().await.unwrap();
    // A gap, as a prune or an older release leaves: numbering goes on from
    // the highest.
    exec(
        &st,
        "INSERT INTO dashboard_versions (id, dashboard_id, version, snapshot, created_at) \
         VALUES ('v7', 'd', 7, '{}', 't7')",
    )
    .await;
    let mut tx = st.write().await.unwrap();
    let next = dashboard_versions::append(&mut tx, "v8", "d", "{}", "t8")
        .await
        .unwrap();
    // It sees the version it just wrote.
    let after = dashboard_versions::append(&mut tx, "v9", "d", "{}", "t9")
        .await
        .unwrap();
    let other = dashboard_versions::append(&mut tx, "w1", "e", "{}", "t")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        (next.version, after.version, other.version),
        (8.0, 9.0, 1.0)
    );

    let meta = dashboard_versions::list_meta(&st, "d").await.unwrap();
    assert_eq!(
        meta.iter()
            .map(|m| (m.id.as_str(), m.version, m.bytes, m.keyframe))
            .collect::<Vec<_>>(),
        [
            ("v1", 1.0, 7, true),
            ("v7", 7.0, 2, true),
            ("v8", 8.0, 2, true),
            ("v9", 9.0, 2, true)
        ]
    );
    assert_eq!(
        dashboard_versions::list_by_project(&st, "p")
            .await
            .unwrap()
            .iter()
            .map(|v| v.id.as_str())
            .collect::<Vec<_>>(),
        ["v1", "v7", "v8", "v9"]
    );

    let mut tx = st.write().await.unwrap();
    let deleted = dashboard_versions::delete_ids(
        &mut tx,
        "d",
        &["v1".into(), "v7".into(), "w1".into(), "none".into()],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(deleted, 2, "another dashboard's version is left alone");
    assert_eq!(
        count(
            &st,
            "SELECT COUNT(*) FROM dashboard_versions WHERE id = 'w1'"
        )
        .await,
        1
    );
    st.close().await;
}

/// 5d-2 Task 7 probe fix: `dashboardVersionsList` answers the versions
/// without their snapshots (the probe got 294 MiB for one project), with
/// what the history shows: number, time, widget count and size. The count
/// is written with the version; a row an older release wrote (no count)
/// is counted when listed. `get` answers one version whole, only under its
/// own dashboard.
#[tokio::test]
async fn dashboard_version_lists_carry_no_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let big = format!(
        r#"{{"name":"D","widgets":[{{"id":"a","sql":"{}"}},{{"id":"b"}},{{"id":"c"}}],"viewport":{{}}}}"#,
        "s".repeat(200_000)
    );
    let mut tx = st.write().await.unwrap();
    dashboards::insert(&mut tx, &dashboard("d", "p", "D"))
        .await
        .unwrap();
    dashboards::insert(&mut tx, &dashboard("e", "q", "E"))
        .await
        .unwrap();
    let v1 = dashboard_versions::append(&mut tx, "v1", "d", &big, "t1")
        .await
        .unwrap();
    dashboard_versions::append(&mut tx, "w1", "e", r#"{"widgets":[]}"#, "t")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        (v1.version, v1.widget_count, v1.bytes),
        (1.0, Some(3), big.len() as u64)
    );
    // An older release's rows: no count. One of them isn't JSON, one has
    // no widgets list.
    exec(
        &st,
        r#"INSERT INTO dashboard_versions (id, dashboard_id, version, snapshot, created_at)
           VALUES ('v2', 'd', 2, '{"widgets":[{"id":"x"}]}', 't2'),
                  ('v3', 'd', 3, 'not json', 't3'),
                  ('v4', 'd', 4, '{"widgets":{}}', 't4'),
                  ('v5', 'd', 5, '{"widgets":[]}', CAST(X'FF' AS TEXT))"#,
    )
    .await;
    let list = dashboard_versions::list_meta_by_project(&st, "p")
        .await
        .unwrap();
    assert_eq!(
        list.iter()
            .map(|v| (
                v.id.as_str(),
                v.version,
                v.widget_count,
                v.created_at.as_str()
            ))
            .collect::<Vec<_>>(),
        [
            ("v1", 1.0, Some(3), "t1"),
            ("v2", 2.0, Some(1), "t2"),
            ("v3", 3.0, None, "t3"),
            ("v4", 4.0, None, "t4"),
            // Review fix: text that isn't UTF-8 is read lossily, not a
            // failed list.
            ("v5", 5.0, Some(0), "\u{FFFD}"),
        ]
    );
    assert_eq!(list[0].bytes, big.len() as u64);
    let json = serde_json::to_string(&list).unwrap();
    assert!(
        json.len() < 1_000 && !json.contains("snapshot") && !json.contains("sss"),
        "{json}"
    );
    assert_eq!(
        serde_json::to_value(&list[0]).unwrap(),
        serde_json::json!({"id": "v1", "dashboardId": "d", "version": 1, "createdAt": "t1",
            "widgetCount": 3, "bytes": big.len()})
    );

    let whole = dashboard_versions::get(&st, "d", "v1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (whole.version, whole.snapshot.as_str()),
        (1.0, big.as_str())
    );
    // Another dashboard's version, named under this one, isn't there.
    assert!(dashboard_versions::get(&st, "d", "w1")
        .await
        .unwrap()
        .is_none());
    assert!(dashboard_versions::get(&st, "e", "v1")
        .await
        .unwrap()
        .is_none());
    assert!(dashboard_versions::get(&st, "d", "none")
        .await
        .unwrap()
        .is_none());
    st.close().await;
}

/// On a beta-era file `dashboards.project_id` has no foreign key and is
/// last; a dashboard's delete still takes its versions.
#[tokio::test]
async fn dashboard_delete_takes_its_versions_on_a_beta_file() {
    for fixture in ["schemas/v2026.4.5-beta.1.sql", "schemas/current.sql"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        load_fixture(&path, fixture).await;
        let st = Storage::open(&path, StorageOptions::default())
            .await
            .unwrap();
        seeded(&st).await;
        let mut tx = st.write().await.unwrap();
        dashboards::insert(&mut tx, &dashboard("d", "p", "D"))
            .await
            .unwrap();
        dashboards::insert(&mut tx, &dashboard("e", "p", "E"))
            .await
            .unwrap();
        for (id, d) in [("v1", "d"), ("v2", "d"), ("w1", "e")] {
            dashboard_versions::append(&mut tx, id, d, "{}", "t")
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
        assert_eq!(
            dashboards::list(&st, "p").await.unwrap().len(),
            2,
            "{fixture}"
        );

        let mut tx = st.write().await.unwrap();
        assert!(dashboards::delete(&mut tx, "d").await.unwrap());
        tx.commit().await.unwrap();
        let left: Vec<String> = sqlx::query_scalar("SELECT id FROM dashboard_versions ORDER BY id")
            .fetch_all(st.pool())
            .await
            .unwrap();
        assert_eq!(left, ["w1"], "{fixture}");
        st.close().await;
    }
}

// ── Saved workflows ──

#[tokio::test]
async fn saved_workflows_insert_get_list_update_delete_and_count() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    saved_canvases::insert(&mut tx, "w1", "p", r#"{"id":"w1","name":"One"}"#)
        .await
        .unwrap();
    saved_canvases::insert(&mut tx, "w2", "p", r#"{"id":"w2","name":"Two"}"#)
        .await
        .unwrap();
    saved_canvases::insert(&mut tx, "w3", "q", r#"{"id":"w3"}"#)
        .await
        .unwrap();
    let row = saved_canvases::get(&mut tx, "w1").await.unwrap().unwrap();
    assert_eq!(
        (row.id.as_str(), row.project_id.as_str()),
        ("w1", "p"),
        "read in the transaction"
    );
    tx.commit().await.unwrap();
    // Rows today's load skips: not JSON, `null`, not UTF-8.
    exec(
        &st,
        "INSERT INTO saved_canvases (id, project_id, data) VALUES ('bad', 'p', 'not json'); \
         INSERT INTO saved_canvases (id, project_id, data) VALUES ('nul', 'p', 'null'); \
         INSERT INTO saved_canvases (id, project_id, data) VALUES ('bin', 'p', CAST(X'FF' AS TEXT));",
    )
    .await;
    assert_eq!(
        texts(&saved_canvases::list(&st, "p").await.unwrap()),
        [r#"{"id":"w1","name":"One"}"#, r#"{"id":"w2","name":"Two"}"#]
    );
    // `get` finds a row whose data doesn't read, so an update can replace
    // it; its data is `None`.
    for id in ["bad", "bin"] {
        let row = saved_canvases::get(&st, id).await.unwrap().unwrap();
        assert!(row.data.is_none(), "{id}");
    }
    assert_eq!(
        saved_canvases::get(&st, "w1")
            .await
            .unwrap()
            .unwrap()
            .data
            .unwrap()
            .get(),
        r#"{"id":"w1","name":"One"}"#
    );
    assert_eq!(saved_canvases::count(&st).await.unwrap(), 6);

    let mut tx = st.write().await.unwrap();
    assert!(saved_canvases::update(&mut tx, "bad", r#"{"id":"bad"}"#)
        .await
        .unwrap());
    assert!(!saved_canvases::update(&mut tx, "none", "{}").await.unwrap());
    assert!(saved_canvases::delete(&mut tx, "w2").await.unwrap());
    assert!(!saved_canvases::delete(&mut tx, "w2").await.unwrap());
    tx.commit().await.unwrap();
    assert_eq!(
        texts(&saved_canvases::list(&st, "p").await.unwrap()),
        [r#"{"id":"w1","name":"One"}"#, r#"{"id":"bad"}"#]
    );
    // Byte for byte.
    let mut tx = st.write().await.unwrap();
    saved_canvases::update(&mut tx, "w1", "{ \"id\" : \"w1\" }")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        saved_canvases::get(&st, "w1")
            .await
            .unwrap()
            .unwrap()
            .data
            .unwrap()
            .get(),
        "{ \"id\" : \"w1\" }"
    );
    st.close().await;
}

/// 5d-2 Task 7 probe fix: `workflowsList` answers the workflows without
/// their bodies (the probe got 480 MiB for one project): id, project,
/// name, times and size, from `meta`, which every write stores with the
/// data. A row an older release wrote (no `meta`) is read from its body;
/// rows that don't read stay out, as they did from the whole list.
#[tokio::test]
async fn saved_workflow_lists_carry_no_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let rows = "r".repeat(300_000);
    let one = format!(
        r#"{{"id":"w1","name":"One","nodes":[{{"rows":"{rows}"}}],"projectId":"p","createdAt":"c1","updatedAt":"u1"}}"#
    );
    let mut tx = st.write().await.unwrap();
    saved_canvases::insert(&mut tx, "w1", "p", &one)
        .await
        .unwrap();
    saved_canvases::insert(&mut tx, "w3", "q", r#"{"id":"w3","name":"Other"}"#)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    exec(
        &st,
        r#"INSERT INTO saved_canvases (id, project_id, data) VALUES
             ('old', 'p', '{"id":"old","name":"Old","createdAt":"c0"}'),
             ('num', 'p', '{"id":"num","name":5}'),
             ('arr', 'p', '[1]'),
             ('bad', 'p', 'not json'),
             ('nul', 'p', 'null'),
             ('bin', 'p', CAST(X'FF' AS TEXT)),
             ('sur', 'p', '{"name":"\ud800x"}'),
             ('ff', 'p', CAST(X'7B226E616D65223A22FF227D' AS TEXT))"#,
    )
    .await;
    // Review fix: a name SQLite hands back as bytes that aren't UTF-8 (a
    // lone surrogate's `->>` is CESU-8, `EDA080`; a stored `FF` is copied
    // through by `json_object`) is read lossily; it never fails the list.
    let list = saved_canvases::list_meta(&st, "p").await.unwrap();
    assert_eq!(
        list.iter()
            .map(|w| (
                w.id.as_str(),
                w.project_id.as_str(),
                w.name.as_str(),
                w.created_at.as_deref(),
                w.updated_at.as_deref()
            ))
            .collect::<Vec<_>>(),
        [
            ("w1", "p", "One", Some("c1"), Some("u1")),
            ("old", "p", "Old", Some("c0"), None),
            ("num", "p", "", None, None),
            ("arr", "p", "", None, None),
            ("sur", "p", "\u{FFFD}\u{FFFD}\u{FFFD}x", None, None),
            ("ff", "p", "\u{FFFD}", None, None),
        ]
    );
    assert_eq!(list[0].bytes, one.len() as u64);
    let json = serde_json::to_string(&list).unwrap();
    assert!(json.len() < 1_000 && !json.contains("rrr"), "{json}");
    assert_eq!(
        serde_json::to_value(&list[0]).unwrap(),
        serde_json::json!({"id": "w1", "projectId": "p", "name": "One", "createdAt": "c1",
            "updatedAt": "u1", "bytes": one.len()})
    );
    // The same ids as the whole list, but `ff`, whose body isn't UTF-8
    // (opening it says it can't be read).
    assert_eq!(saved_canvases::list(&st, "p").await.unwrap().len(), 5);

    // An update stores the new name with the data.
    let mut tx = st.write().await.unwrap();
    saved_canvases::update(
        &mut tx,
        "w1",
        r#"{"id":"w1","name":"Renamed","updatedAt":"u2"}"#,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let meta = |st: Storage| async move {
        sqlx::query_scalar::<_, Option<String>>("SELECT meta FROM saved_canvases WHERE id = 'w1'")
            .fetch_one(st.pool())
            .await
            .unwrap()
    };
    assert_eq!(
        meta(st.clone()).await.as_deref(),
        Some(r#"{"name":"Renamed","createdAt":null,"updatedAt":"u2"}"#)
    );
    // Data changed without its meta (anything but storage's own writes)
    // clears it, so the list reads the body instead of a stale name.
    exec(
        &st,
        r#"UPDATE saved_canvases SET data = '{"name":"Elsewhere"}' WHERE id = 'w1'"#,
    )
    .await;
    assert_eq!(meta(st.clone()).await, None);
    assert_eq!(
        saved_canvases::list_meta(&st, "p").await.unwrap()[0].name,
        "Elsewhere"
    );
    st.close().await;
}

#[tokio::test]
async fn saved_workflow_meta_is_listed_through_its_index() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let sql = saved_canvases::list_meta_sql();
    assert_indexed(&sql, &plan(&st, &sql).await, "idx_saved_canvases_project");
    st.close().await;
}

#[tokio::test]
async fn saved_workflows_are_listed_through_their_index() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let sql = saved_canvases::LIST;
    assert_indexed(sql, &plan(&st, sql).await, "idx_saved_canvases_project");
    st.close().await;
}

// ── AI chats and messages ──

#[tokio::test]
async fn chats_insert_get_list_update_delete_and_count() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    ai_chats::insert(&mut tx, &chat("a", "c", "2026-10-01T00:00:00.000Z"))
        .await
        .unwrap();
    ai_chats::insert(&mut tx, &chat("b", "c", "2026-10-03T00:00:00.000Z"))
        .await
        .unwrap();
    assert!(ai_chats::get(&mut tx, "a").await.unwrap().is_some());
    ai_chats::put_messages(
        &mut tx,
        "a",
        &[message("m1", "a", "user", "hi", "2026-10-01T00:00:00.000Z")],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        ai_chats::list(&st, "c")
            .await
            .unwrap()
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "a"],
        "most recently updated first"
    );
    assert_eq!(ai_chats::count(&st).await.unwrap(), 2);

    let mut renamed = chat("a", "other", "2026-10-04T00:00:00.000Z");
    renamed.title = "New".into();
    renamed.created_at = "never".into();
    let mut tx = st.write().await.unwrap();
    assert!(ai_chats::update(&mut tx, &renamed).await.unwrap());
    assert!(!ai_chats::update(&mut tx, &chat("none", "c", "t"))
        .await
        .unwrap());
    tx.commit().await.unwrap();
    let stored = ai_chats::get(&st, "a").await.unwrap().unwrap();
    assert_eq!(
        (
            stored.connection_id.as_str(),
            stored.title.as_str(),
            stored.created_at.as_str(),
            stored.updated_at.as_str()
        ),
        (
            "c",
            "New",
            "2026-10-01T00:00:00.000Z",
            "2026-10-04T00:00:00.000Z"
        )
    );

    let mut tx = st.write().await.unwrap();
    assert!(ai_chats::delete(&mut tx, "a").await.unwrap());
    assert!(!ai_chats::delete(&mut tx, "a").await.unwrap());
    tx.commit().await.unwrap();
    assert_eq!(count(&st, "SELECT COUNT(*) FROM ai_messages").await, 0);
    assert_eq!(ai_chats::count(&st).await.unwrap(), 1);
    st.close().await;
}

/// Messages are upserted by id: a listed one that exists keeps its place
/// (its rowid), new ones are added in list order, and ones not listed
/// stay. Equal timestamps read in insertion order.
#[tokio::test]
async fn put_messages_upserts_and_keeps_order_on_equal_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let t = "2026-10-01T00:00:00.000Z";
    let later = "2026-10-01T00:00:01.000Z";
    let mut tx = st.write().await.unwrap();
    ai_chats::insert(&mut tx, &chat("a", "c", t)).await.unwrap();
    // The user's message and the assistant's placeholder, one millisecond.
    // `z` sorts after `a` by id: only insertion order keeps them right.
    ai_chats::put_messages(
        &mut tx,
        "a",
        &[
            message("z-user", "a", "user", "question", t),
            message("a-assistant", "a", "assistant", "", t),
        ],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = st.write().await.unwrap();
    let mut answered = message("a-assistant", "a", "assistant", "the answer", t);
    answered.query = Some("SELECT 1".into());
    answered.dashboard_id = Some("d".into());
    ai_chats::put_messages(
        &mut tx,
        "a",
        &[
            answered.clone(),
            message("n1", "a", "user", "next", later),
            message("n0", "a", "assistant", "", later),
        ],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let got = ai_chats::load_messages(&st, "a").await.unwrap();
    assert_eq!(
        got.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["z-user", "a-assistant", "n1", "n0"]
    );
    assert_eq!(got[1], answered);
    assert_eq!(ai_chats::message_count(&st, "a").await.unwrap(), 4);

    let mut tx = st.write().await.unwrap();
    assert_eq!(
        ai_chats::delete_messages(&mut tx, "a", &["n1".into(), "n0".into(), "none".into()])
            .await
            .unwrap(),
        2
    );
    tx.commit().await.unwrap();
    assert_eq!(ai_chats::message_count(&st, "a").await.unwrap(), 2);
    st.close().await;
}

/// Message ids are global (`ai_messages.id` is the primary key), and the
/// GUI makes them: an id that belongs to another chat is reported, and the
/// put writes nothing.
#[tokio::test]
async fn a_message_id_of_another_chat_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let t = "2026-10-01T00:00:00.000Z";
    let mut tx = st.write().await.unwrap();
    ai_chats::insert(&mut tx, &chat("a", "c", t)).await.unwrap();
    ai_chats::insert(&mut tx, &chat("b", "c", t)).await.unwrap();
    ai_chats::put_messages(&mut tx, "a", &[message("m", "a", "user", "mine", t)])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = st.write().await.unwrap();
    assert_eq!(
        ai_chats::message_chat_ids(&mut tx, &["m".into(), "new".into()])
            .await
            .unwrap(),
        [("m".to_string(), "a".to_string())]
    );
    let outcome = ai_chats::put_messages(
        &mut tx,
        "b",
        &[
            message("new", "b", "user", "fine", t),
            message("m", "b", "user", "theirs", t),
        ],
    )
    .await
    .unwrap();
    assert_eq!(outcome, ai_chats::PutMessages::OtherChat { id: "m".into() });
    tx.commit().await.unwrap();
    assert_eq!(
        ai_chats::load_messages(&st, "a").await.unwrap()[0].content,
        "mine"
    );
    assert!(ai_chats::load_messages(&st, "b").await.unwrap().is_empty());
    st.close().await;
}

/// The chat's stored bytes (the web budget): UTF-8 bytes of `content`,
/// summed through `idx_ai_messages_chat` without reading the text, and
/// for a set of ids (the messages a put replaces).
#[allow(clippy::disallowed_types)]
#[tokio::test]
async fn content_bytes_uses_the_chat_index() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    for (sql, index) in [
        (ai_chats::CONTENT_BYTES, "idx_ai_messages_chat"),
        (ai_chats::MESSAGE_COUNT, "idx_ai_messages_chat"),
        // In order straight from the index: no sort of whole rows.
        (ai_chats::LOAD_MESSAGES, "idx_ai_messages_chat_time"),
    ] {
        assert_indexed(sql, &plan(&st, sql).await, index);
    }

    let t = "2026-10-01T00:00:00.000Z";
    let mut tx = st.write().await.unwrap();
    ai_chats::insert(&mut tx, &chat("a", "c", t)).await.unwrap();
    ai_chats::insert(&mut tx, &chat("small", "c", t))
        .await
        .unwrap();
    ai_chats::put_messages(
        &mut tx,
        "small",
        &[
            message("s1", "small", "user", "héllo", t),
            message("s2", "small", "assistant", "", t),
        ],
    )
    .await
    .unwrap();
    // 5,000 messages, 64 MiB of content: a full web chat.
    let body = "x".repeat(64 * 1024 * 1024 / 5_000);
    let batch: Vec<PersistedAIMessage> = (0..5_000)
        .map(|i| message(&format!("m{i}"), "a", "user", &body, t))
        .collect();
    ai_chats::put_messages(&mut tx, "a", &batch).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(ai_chats::content_bytes(&st, "small").await.unwrap(), 6);
    assert_eq!(
        ai_chats::content_bytes_of(&st, "small", &["s1".into(), "m1".into(), "none".into()])
            .await
            .unwrap(),
        6,
        "only that chat's listed messages"
    );
    assert_eq!(ai_chats::content_bytes(&st, "none").await.unwrap(), 0);

    let started = std::time::Instant::now();
    let bytes = ai_chats::content_bytes(&st, "a").await.unwrap();
    let elapsed = started.elapsed();
    assert_eq!(bytes, 5_000 * body.len() as u64);
    assert!(
        elapsed < Duration::from_millis(500),
        "summing a full chat took {elapsed:?}"
    );
    st.close().await;
}

// ── Settings records ──

#[tokio::test]
async fn setting_delete_removes_the_row() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let mut tx = st.write().await.unwrap();
    app_state::set_in(&mut tx, "editorKeybindingMode", Some("vim"))
        .await
        .unwrap();
    app_state::set_in(&mut tx, "kept", Some("v")).await.unwrap();
    assert_eq!(
        app_state::get(&mut tx, "editorKeybindingMode")
            .await
            .unwrap()
            .as_deref(),
        Some("vim")
    );
    assert!(app_state::delete_in(&mut tx, "editorKeybindingMode")
        .await
        .unwrap());
    assert!(!app_state::delete_in(&mut tx, "editorKeybindingMode")
        .await
        .unwrap());
    tx.commit().await.unwrap();
    assert_eq!(
        count(
            &st,
            "SELECT COUNT(*) FROM app_state WHERE key = 'editorKeybindingMode'"
        )
        .await,
        0,
        "no row, not a row holding NULL"
    );
    assert_eq!(
        app_state::get(&st, "kept").await.unwrap().as_deref(),
        Some("v")
    );
    st.close().await;
}

#[tokio::test]
async fn themes_preferences_and_user_themes() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    assert_eq!(themes::preferences(&st).await.unwrap(), None);
    let mut tx = st.write().await.unwrap();
    themes::set_preferences(&mut tx, "theme-a", "default-dark")
        .await
        .unwrap();
    themes::insert(&mut tx, "theme-a", r#"{"id":"theme-a","name":"A"}"#)
        .await
        .unwrap();
    themes::insert(&mut tx, "theme-b", r#"{"id":"theme-b"}"#)
        .await
        .unwrap();
    assert!(themes::insert(&mut tx, "theme-a", "{}").await.is_err());
    tx.commit().await.unwrap();
    let prefs = themes::preferences(&st).await.unwrap().unwrap();
    assert_eq!(
        (prefs.light_theme_id.as_str(), prefs.dark_theme_id.as_str()),
        ("theme-a", "default-dark")
    );
    // Rows today's load skips.
    exec(
        &st,
        "INSERT INTO user_themes (id, data) VALUES ('bad', '{'); \
         INSERT INTO user_themes (id, data) VALUES ('nul', 'null'); \
         INSERT INTO user_themes (id, data) VALUES ('bin', CAST(X'FF' AS TEXT));",
    )
    .await;
    assert_eq!(
        texts(&themes::list(&st).await.unwrap()),
        [r#"{"id":"theme-a","name":"A"}"#, r#"{"id":"theme-b"}"#]
    );
    assert!(themes::get(&st, "bad")
        .await
        .unwrap()
        .unwrap()
        .data
        .is_none());
    assert!(themes::get(&st, "none").await.unwrap().is_none());
    assert_eq!(themes::count(&st).await.unwrap(), 5);

    let mut tx = st.write().await.unwrap();
    assert!(
        themes::update(&mut tx, "theme-b", r#"{"id":"theme-b","x":1}"#)
            .await
            .unwrap()
    );
    assert!(!themes::update(&mut tx, "none", "{}").await.unwrap());
    assert!(themes::delete(&mut tx, "theme-a").await.unwrap());
    assert!(!themes::delete(&mut tx, "theme-a").await.unwrap());
    themes::set_preferences(&mut tx, "default-light", "default-dark")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        themes::get(&st, "theme-b")
            .await
            .unwrap()
            .unwrap()
            .data
            .unwrap()
            .get(),
        r#"{"id":"theme-b","x":1}"#
    );
    assert_eq!(
        count(&st, "SELECT COUNT(*) FROM theme_preferences").await,
        1
    );
    st.close().await;
}

#[tokio::test]
async fn onboarding_tutorial_and_import_state_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    assert!(onboarding::get(&st).await.unwrap().is_none());
    assert!(tutorial::list(&st).await.unwrap().is_empty());
    assert_eq!(import_state::get(&st, "tableplus").await.unwrap(), None);

    let mut tx = st.write().await.unwrap();
    onboarding::set(&mut tx, &raw(r#"{"learnEnabled":true}"#))
        .await
        .unwrap();
    tutorial::save_in(&mut tx, "l1", "c1", Some("{\"done\":true}"))
        .await
        .unwrap();
    tutorial::save_in(&mut tx, "l1", "c2", None).await.unwrap();
    tutorial::save_in(&mut tx, "l2", "c1", Some("x"))
        .await
        .unwrap();
    import_state::save_in(&mut tx, "tableplus", true, Some("t"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        onboarding::get(&st).await.unwrap().unwrap().get(),
        r#"{"learnEnabled":true}"#
    );
    assert_eq!(tutorial::list(&st).await.unwrap().len(), 3);
    let imported = import_state::get(&st, "tableplus").await.unwrap().unwrap();
    assert!(imported.has_offered_import);
    assert_eq!(imported.last_check_timestamp.as_deref(), Some("t"));

    // A row that isn't UTF-8 is skipped, not a failed list.
    exec(
        &st,
        "INSERT INTO tutorial_progress (lesson_id, challenge_id, state) \
         VALUES ('l3', 'c1', CAST(X'FF' AS TEXT));",
    )
    .await;
    assert_eq!(tutorial::list(&st).await.unwrap().len(), 3);
    // Stored onboarding that isn't JSON reads as nothing.
    exec(&st, "UPDATE onboarding_state SET data = 'nope'").await;
    assert!(onboarding::get(&st).await.unwrap().is_none());

    let mut tx = st.write().await.unwrap();
    assert_eq!(tutorial::remove_lesson_in(&mut tx, "l1").await.unwrap(), 2);
    tx.commit().await.unwrap();
    assert_eq!(tutorial::list(&st).await.unwrap().len(), 1);
    let mut tx = st.write().await.unwrap();
    assert_eq!(tutorial::remove_all_in(&mut tx).await.unwrap(), 2);
    tx.commit().await.unwrap();
    assert_eq!(
        count(&st, "SELECT COUNT(*) FROM tutorial_progress").await,
        0
    );
    st.close().await;
}

// ── Windows and view state ──

#[tokio::test]
async fn windows_touch_and_set_active_project() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    windows::touch(&mut tx, "main", "t1").await.unwrap();
    let w = windows::get(&mut tx, "main").await.unwrap().unwrap();
    assert_eq!((w.active_project_id, w.updated_at.as_str()), (None, "t1"));
    windows::set_active_project(&mut tx, "main", "p", "t2")
        .await
        .unwrap();
    // A touch keeps the active project.
    windows::touch(&mut tx, "main", "t3").await.unwrap();
    windows::set_active_project(&mut tx, "win-new", "q", "t4")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let w = windows::get(&st, "main").await.unwrap().unwrap();
    assert_eq!(
        (w.active_project_id.as_deref(), w.updated_at.as_str()),
        (Some("p"), "t3")
    );
    assert_eq!(
        windows::get(&st, "win-new")
            .await
            .unwrap()
            .unwrap()
            .active_project_id
            .as_deref(),
        Some("q")
    );
    assert_eq!(windows::get(&st, "none").await.unwrap(), None);
    st.close().await;
}

/// `rev` orders saves: only a higher one replaces the stored state; the
/// first save of a window and project always lands. A refused (stale)
/// save reports the stored `rev`, so the page can move past it.
#[tokio::test]
async fn put_if_newer_ignores_an_older_rev() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    windows::touch(&mut tx, "w", "t").await.unwrap();
    let put = |written, rev| window_state::Put { written, rev };
    assert_eq!(
        window_state::put_if_newer(&mut tx, "w", "p", 0, r#"{"v":0}"#, "t0")
            .await
            .unwrap(),
        put(true, 0)
    );
    assert_eq!(
        window_state::put_if_newer(&mut tx, "w", "p", 5, r#"{"v":5}"#, "t5")
            .await
            .unwrap(),
        put(true, 5)
    );
    for rev in [5, 4, 0] {
        assert_eq!(
            window_state::put_if_newer(&mut tx, "w", "p", rev, r#"{"v":"old"}"#, "tx")
                .await
                .unwrap(),
            put(false, 5),
            "{rev}"
        );
    }
    tx.commit().await.unwrap();
    let row = window_state::get(&st, "w", "p").await.unwrap().unwrap();
    assert_eq!(
        (row.state.unwrap().get(), row.rev, row.updated_at.as_str()),
        (r#"{"v":5}"#, 5, "t5")
    );
    assert!(window_state::get(&st, "w", "q").await.unwrap().is_none());

    // A state that doesn't read loads as `None`, keeping its rev.
    exec(&st, "UPDATE window_state SET state = 'nope'").await;
    let row = window_state::get(&st, "w", "p").await.unwrap().unwrap();
    assert!(row.state.is_none());
    assert_eq!(row.rev, 5);
    st.close().await;
}

#[tokio::test]
async fn most_recent_is_the_projects_last_saved_window() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    assert!(window_state::most_recent(&st, "p").await.unwrap().is_none());
    let mut tx = st.write().await.unwrap();
    for (w, p, at) in [
        ("a", "p", "2026-10-01T00:00:00.000Z"),
        ("b", "p", "2026-10-03T00:00:00.000Z"),
        ("c", "p", "2026-10-02T00:00:00.000Z"),
        ("d", "q", "2026-10-09T00:00:00.000Z"),
    ] {
        windows::touch(&mut tx, w, at).await.unwrap();
        window_state::put_if_newer(&mut tx, w, p, 1, &format!("{{\"w\":\"{w}\"}}"), at)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    // The last write committed, whatever the times say (5d-2 Task 7): the
    // legacy mirror, written by every save, shows `c`'s too.
    let recent = window_state::most_recent(&st, "p").await.unwrap().unwrap();
    assert_eq!(
        (recent.window_id.as_str(), recent.state.unwrap().get()),
        ("c", r#"{"w":"c"}"#)
    );
    st.close().await;
}

#[tokio::test]
async fn most_recent_uses_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let sql = window_state::MOST_RECENT;
    let plan = plan(&st, sql).await;
    assert_indexed(sql, &plan, "idx_window_state_project_seq");
    st.close().await;
}

/// Each prune is one indexed `DELETE` bounded by `PRUNE_BATCH`, and
/// never deletes a spared window (the saving one, and `main` on desktop).
#[tokio::test]
async fn each_prune_is_one_bounded_delete_and_spares_the_given_window() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    for (sql, index) in [
        (windows::PRUNE_UNUSED, "idx_windows_updated"),
        (windows::PRUNE_OVER_COUNT, "idx_windows_write_seq"),
        (
            window_state::PRUNE_FOR_PROJECT,
            "idx_window_state_project_seq",
        ),
    ] {
        let plan = plan(&st, sql).await;
        assert_indexed(sql, &plan, index);
        assert!(sql.contains("LIMIT"), "{sql}");
        assert_eq!(sql.matches("DELETE").count(), 1, "{sql}");
    }

    // 30 windows, `w00` the oldest, each with a state in `p`, and `main`
    // used before all of them.
    let at = |i: usize| format!("2026-09-{:02}T00:00:00.000Z", i + 1);
    let mut tx = st.write().await.unwrap();
    windows::touch(&mut tx, "main", &at(0)).await.unwrap();
    for i in 0..30 {
        let w = format!("w{i:02}");
        windows::touch(&mut tx, &w, &at(i)).await.unwrap();
        window_state::put_if_newer(&mut tx, &w, "p", 1, "{}", &at(i))
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let window_ids = |st: &Storage| {
        let st = st.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT window_id FROM windows ORDER BY window_id")
                .fetch_all(st.pool())
                .await
                .unwrap()
        }
    };

    // Unused before the 10th: w00–w08 go, except the spared w03 and main.
    let mut tx = st.write().await.unwrap();
    let gone = windows::prune(&mut tx, &at(9), None, &["w03", "main"])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(gone, 8);
    let left = window_ids(&st).await;
    assert!(left.contains(&"w03".to_string()) && left.contains(&"main".to_string()));
    assert!(!left.contains(&"w00".to_string()) && left.contains(&"w09".to_string()));
    // Their states went with them.
    assert_eq!(count(&st, "SELECT COUNT(*) FROM window_state").await, 22);

    // At most 5 windows: the 5 most recent stay, plus the spared ones.
    let mut tx = st.write().await.unwrap();
    windows::prune(&mut tx, "0000", Some(5), &["w03"])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        window_ids(&st).await,
        ["w03", "w25", "w26", "w27", "w28", "w29"]
    );

    // At most 2 states in the project: the newest two, and the spared one.
    let mut tx = st.write().await.unwrap();
    let gone = window_state::prune_for_project(&mut tx, "p", 2, &["w03"])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(gone, 3);
    let states: Vec<String> =
        sqlx::query_scalar("SELECT window_id FROM window_state ORDER BY window_id")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(states, ["w03", "w28", "w29"]);

    // One call deletes at most `PRUNE_BATCH` rows.
    let batch = windows::PRUNE_BATCH as usize;
    let mut tx = st.write().await.unwrap();
    for i in 0..batch + 10 {
        windows::touch(&mut tx, &format!("x{i:04}"), "2000")
            .await
            .unwrap();
    }
    assert_eq!(
        windows::prune(&mut tx, "2001", None, &[]).await.unwrap(),
        batch as u64
    );
    assert_eq!(
        windows::prune(&mut tx, "2001", None, &[]).await.unwrap(),
        10
    );
    tx.commit().await.unwrap();
    st.close().await;
}

// ── The legacy mirror ──

fn full_state(project_id: &str) -> PersistedProjectState {
    PersistedProjectState {
        project_id: project_id.into(),
        query_tabs: vec![
            PersistedQueryTab {
                id: "q1".into(),
                name: "Query 1".into(),
                query: "SELECT 1".into(),
                query_id: Some("saved-1".into()),
            },
            PersistedQueryTab {
                id: "q2".into(),
                name: "Query 2".into(),
                query: "".into(),
                query_id: None,
            },
        ],
        schema_tabs: vec![PersistedSchemaTab {
            id: "s1".into(),
            table_name: "users".into(),
            schema_name: "public".into(),
            connection_id: Some("c".into()),
        }],
        explain_tabs: vec![PersistedExplainTab {
            id: "e1".into(),
            name: "Explain".into(),
            source_query: "SELECT 2".into(),
        }],
        erd_tabs: vec![PersistedErdTab {
            id: "r1".into(),
            name: "ERD".into(),
            connection_id: None,
        }],
        statistics_tabs: vec![PersistedStatisticsTab {
            id: "st1".into(),
            name: "Stats".into(),
            connection_id: "c".into(),
        }],
        workflow_tabs: vec![PersistedWorkflowTab {
            id: "wf1".into(),
            name: "Flow".into(),
            connection_id: "c".into(),
        }],
        tab_order: raw(r#"["q1","q2","s1"]"#),
        connection_order: Some(raw(r#"["c"]"#)),
        active_query_tab_id: Some("q1".into()),
        active_schema_tab_id: Some("s1".into()),
        active_explain_tab_id: Some("e1".into()),
        active_erd_tab_id: Some("r1".into()),
        active_statistics_tab_id: Some("st1".into()),
        active_workflow_tab_id: Some("wf1".into()),
        active_view: "query".into(),
        active_connection_id: Some("c".into()),
        starter_tabs: vec![PersistedStarterTab {
            id: "start".into(),
            ty: "getting-started".into(),
            name: "Start".into(),
            closable: true,
        }],
        active_starter_tab_id: Some("start".into()),
        saved_workflows: vec![raw(r#"{"id":"workflow-1","name":"W"}"#)],
        connection_tabs: None,
        active_connection_tab_id: None,
        dashboard_tabs: vec![PersistedDashboardTab {
            id: "d1".into(),
            name: "Dash".into(),
            dashboard_id: "dashboard-1".into(),
        }],
        active_dashboard_tab_id: Some("d1".into()),
        starred_shared_query_ids: Some(raw(r#"["shared-q"]"#)),
        starred_shared_dashboard_ids: Some(raw(r#"["shared-d"]"#)),
        create_table_tabs: vec![PersistedCreateTableTab {
            id: "ct1".into(),
            connection_id: "c".into(),
            name: "New table".into(),
            table_definition: r#"{"name":"t"}"#.into(),
        }],
        active_create_table_tab_id: Some("ct1".into()),
        data_tabs: vec![PersistedDataTab {
            id: "dt1".into(),
            connection_id: "c".into(),
            table_name: "orders".into(),
            schema_name: "public".into(),
        }],
        active_data_tab_id: Some("dt1".into()),
        extensions_duckdb_tabs: None,
        active_extensions_duckdb_tab_id: None,
        pane_layout: Some(raw(
            r#"{"panes":[{"id":"pane-1","tabIds":["q1"],"activeTabId":"q1"}],"activePaneId":"pane-1"}"#,
        )),
    }
}

/// What an older window had before this save: other tabs and ids, the
/// same order and starred lists (which the window state doesn't carry).
fn older_state(project_id: &str) -> PersistedProjectState {
    PersistedProjectState {
        query_tabs: vec![PersistedQueryTab {
            id: "old".into(),
            name: "Old".into(),
            query: "SELECT 0".into(),
            query_id: None,
        }],
        schema_tabs: vec![],
        explain_tabs: vec![],
        erd_tabs: vec![],
        statistics_tabs: vec![],
        workflow_tabs: vec![],
        starter_tabs: vec![],
        dashboard_tabs: vec![],
        create_table_tabs: vec![],
        data_tabs: vec![],
        tab_order: raw(r#"["old"]"#),
        active_query_tab_id: Some("old".into()),
        active_view: "schema".into(),
        active_connection_id: None,
        pane_layout: None,
        saved_workflows: vec![raw(r#"{"id":"workflow-old"}"#)],
        ..full_state(project_id)
    }
}

async fn table_rows(st: &Storage, table: &str) -> Vec<(serde_json::Value, serde_json::Value)> {
    let mut conn = st.pool().acquire().await.unwrap();
    let columns: Vec<String> =
        sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    rows(&mut conn, table, &columns).await
}

async fn with_project(dir: &Path, name: &str) -> Storage {
    let st = Storage::open(&dir.join(name), StorageOptions::default())
        .await
        .unwrap();
    seeded(&st).await;
    st
}

/// The mirror writes exactly the `project_state` and `tabs` rows today's
/// save writes for the same state, and leaves `saved_canvases` alone.
#[tokio::test]
async fn the_legacy_mirror_equals_todays_save_without_canvases() {
    let dir = tempfile::tempdir().unwrap();
    let today = with_project(dir.path(), "today.db").await;
    project_state::save(&today, &full_state("p")).await.unwrap();

    let mirrored = with_project(dir.path(), "mirror.db").await;
    project_state::save(&mirrored, &older_state("p"))
        .await
        .unwrap();
    let mut tx = mirrored.write().await.unwrap();
    let skipped = project_state::write_legacy_mirror(&mut tx, &full_state("p"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(skipped, 0);

    for table in ["project_state", "tabs"] {
        assert_eq!(
            table_rows(&mirrored, table).await,
            table_rows(&today, table).await,
            "{table}"
        );
    }
    let canvases: Vec<String> = sqlx::query_scalar("SELECT id FROM saved_canvases")
        .fetch_all(mirrored.pool())
        .await
        .unwrap();
    assert_eq!(canvases, ["workflow-old"], "the mirror never touches them");

    // And the load an older release does reads the same state back.
    let a = project_state::load(&today, "p").await.unwrap().unwrap();
    let b = project_state::load(&mirrored, "p").await.unwrap().unwrap();
    let strip = |mut s: PersistedProjectState| {
        s.saved_workflows.clear();
        serde_json::to_value(&s).unwrap()
    };
    assert_eq!(strip(a), strip(b));
    today.close().await;
    mirrored.close().await;
}

/// The window state has no connection order or starred lists: the mirror
/// keeps the stored ones (and a project with no row gets the defaults),
/// and writes the saving window's active connection (Q14).
#[tokio::test]
async fn the_mirror_keeps_the_stored_connection_order() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    assert!(project_state::sidebar(&st, "p").await.unwrap().is_none());
    let mut tx = st.write().await.unwrap();
    project_state::set_connection_order(&mut tx, "p", &["c".into(), "d".into()])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        project_state::sidebar(&st, "p")
            .await
            .unwrap()
            .unwrap()
            .get(),
        r#"["c","d"]"#
    );
    exec(
        &st,
        "UPDATE project_state SET starred_shared_query_ids = '[\"keep\"]' WHERE project_id = 'p'",
    )
    .await;

    let mut state = full_state("p");
    state.connection_order = Some(raw(r#"["ignored"]"#));
    state.starred_shared_query_ids = Some(raw(r#"["ignored"]"#));
    state.active_connection_id = Some("c".into());
    let mut tx = st.write().await.unwrap();
    project_state::write_legacy_mirror(&mut tx, &state)
        .await
        .unwrap();
    project_state::write_legacy_mirror(&mut tx, &full_state("q"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let p = project_state::load(&st, "p").await.unwrap().unwrap();
    assert_eq!(p.connection_order.unwrap().get(), r#"["c","d"]"#);
    assert_eq!(p.starred_shared_query_ids.unwrap().get(), r#"["keep"]"#);
    assert_eq!(p.active_connection_id.as_deref(), Some("c"));
    assert_eq!(p.query_tabs.len(), 2);
    let q = project_state::load(&st, "q").await.unwrap().unwrap();
    assert_eq!(q.connection_order.unwrap().get(), "[]");
    assert_eq!(q.starred_shared_dashboard_ids.unwrap().get(), "[]");

    // The order call leaves the view alone.
    let mut tx = st.write().await.unwrap();
    project_state::set_connection_order(&mut tx, "p", &[])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let p = project_state::load(&st, "p").await.unwrap().unwrap();
    assert_eq!(p.connection_order.unwrap().get(), "[]");
    assert_eq!(p.query_tabs.len(), 2);
    assert_eq!(p.active_query_tab_id.as_deref(), Some("q1"));
    st.close().await;
}

/// `tabs`' key is `(id, project_id)`: one repeated id failed today's whole
/// save. The mirror keeps the first tab with an id and skips the rest.
/// DuckDB extensions tabs have no column there and aren't mirrored.
#[tokio::test]
async fn the_mirror_skips_a_repeated_tab_id() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut state = full_state("p");
    state.query_tabs[1].id = "q1".into();
    state.data_tabs[0].id = "s1".into();
    state.extensions_duckdb_tabs = Some(vec![PersistedExtensionsDuckdbTab {
        id: "x1".into(),
        name: "Extensions".into(),
        connection_id: "c".into(),
    }]);
    assert!(
        project_state::save(&st, &state).await.is_err(),
        "today's save fails on it"
    );

    let mut tx = st.write().await.unwrap();
    let skipped = project_state::write_legacy_mirror(&mut tx, &state)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(skipped, 2);
    let tabs: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, tab_type, name FROM tabs WHERE project_id = 'p' ORDER BY rowid")
            .fetch_all(st.pool())
            .await
            .unwrap();
    let got: Vec<(&str, &str)> = tabs
        .iter()
        .map(|(id, ty, _)| (id.as_str(), ty.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("q1", "query"),
            ("s1", "schema"),
            ("e1", "explain"),
            ("r1", "erd"),
            ("st1", "statistics"),
            ("wf1", "canvas"),
            ("start", "starter"),
            ("d1", "dashboard"),
            ("ct1", "create_table"),
        ]
    );
    assert_eq!(tabs[0].2, "Query 1", "the first tab with the id wins");
    st.close().await;
}

// ── Writes take the write transaction ──

/// Every 5d-2 write takes `&mut WriteTx`, so it runs under the write
/// mutex: while another writer holds its transaction, none of them can
/// start, and once it commits they all land.
// A native test that needs a second task; the wasm32 rule behind
// `disallowed_methods` doesn't apply.
#[allow(clippy::disallowed_methods)]
#[tokio::test]
async fn every_5d2_write_takes_a_write_tx() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let holder = st.write().await.unwrap();

    let writer = st.clone();
    let task = tokio::spawn(async move {
        let t = "2026-10-01T00:00:00.000Z";
        let mut tx = writer.write().await.unwrap();
        dashboards::insert(&mut tx, &dashboard("d", "p", "D"))
            .await
            .unwrap();
        dashboards::update(&mut tx, &dashboard("d", "p", "D2"))
            .await
            .unwrap();
        dashboard_versions::append(&mut tx, "v", "d", "{}", t)
            .await
            .unwrap();
        dashboard_versions::delete_ids(&mut tx, "d", &["none".into()])
            .await
            .unwrap();
        saved_canvases::insert(&mut tx, "w", "p", "{}")
            .await
            .unwrap();
        saved_canvases::update(&mut tx, "w", "{\"a\":1}")
            .await
            .unwrap();
        ai_chats::insert(&mut tx, &chat("a", "c", t)).await.unwrap();
        ai_chats::update(&mut tx, &chat("a", "c", t)).await.unwrap();
        ai_chats::put_messages(&mut tx, "a", &[message("m", "a", "user", "x", t)])
            .await
            .unwrap();
        ai_chats::delete_messages(&mut tx, "a", &["none".into()])
            .await
            .unwrap();
        app_state::set_in(&mut tx, "k", Some("v")).await.unwrap();
        app_state::delete_in(&mut tx, "gone").await.unwrap();
        themes::set_preferences(&mut tx, "a", "b").await.unwrap();
        themes::insert(&mut tx, "theme-x", "{}").await.unwrap();
        themes::update(&mut tx, "theme-x", "{\"a\":1}")
            .await
            .unwrap();
        onboarding::set(&mut tx, &raw("{}")).await.unwrap();
        tutorial::save_in(&mut tx, "l", "c", None).await.unwrap();
        tutorial::remove_lesson_in(&mut tx, "none").await.unwrap();
        import_state::save_in(&mut tx, "dbeaver", false, None)
            .await
            .unwrap();
        windows::touch(&mut tx, "main", t).await.unwrap();
        windows::set_active_project(&mut tx, "main", "p", t)
            .await
            .unwrap();
        window_state::put_if_newer(&mut tx, "main", "p", 1, "{}", t)
            .await
            .unwrap();
        windows::prune(&mut tx, "2000", Some(20), &["main"])
            .await
            .unwrap();
        window_state::prune_for_project(&mut tx, "p", 20, &["main"])
            .await
            .unwrap();
        project_state::set_connection_order(&mut tx, "p", &["c".into()])
            .await
            .unwrap();
        project_state::write_legacy_mirror(&mut tx, &full_state("p"))
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!task.is_finished(), "a write ran beside a held transaction");
    assert_eq!(dashboards::count(&st).await.unwrap(), 0);
    holder.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("the writes didn't finish once the lock was free")
        .unwrap();
    assert_eq!(dashboards::count(&st).await.unwrap(), 1);
    assert_eq!(count(&st, "SELECT COUNT(*) FROM window_state").await, 1);
    st.close().await;
}

// ── Rows a user can't fix ──

/// A dashboard, chat or message with a value that isn't UTF-8 (a
/// hand-edited file) is skipped by the reads, not a failed list.
#[tokio::test]
async fn dashboard_and_chat_reads_skip_rows_that_dont_decode() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let t = "2026-10-01T00:00:00.000Z";
    let mut tx = st.write().await.unwrap();
    dashboards::insert(&mut tx, &dashboard("good", "p", "Good"))
        .await
        .unwrap();
    dashboards::insert(&mut tx, &dashboard("bad", "p", "Bad"))
        .await
        .unwrap();
    ai_chats::insert(&mut tx, &chat("a", "c", t)).await.unwrap();
    ai_chats::insert(&mut tx, &chat("b", "c", t)).await.unwrap();
    ai_chats::put_messages(
        &mut tx,
        "a",
        &[
            message("m1", "a", "user", "fine", t),
            message("m2", "a", "assistant", "x", t),
        ],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    exec(
        &st,
        "UPDATE dashboards SET widgets = CAST(X'FF' AS TEXT) WHERE id = 'bad'; \
         UPDATE ai_chats SET title = CAST(X'FF' AS TEXT) WHERE id = 'b'; \
         UPDATE ai_messages SET content = CAST(X'FE' AS TEXT) WHERE id = 'm2';",
    )
    .await;

    let listed: Vec<String> = dashboards::list(&st, "p")
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(listed, ["good"]);
    assert!(dashboards::get(&st, "bad").await.unwrap().is_none());
    let chats: Vec<String> = ai_chats::list(&st, "c")
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(chats, ["a"]);
    assert!(ai_chats::get(&st, "b").await.unwrap().is_none());
    let messages: Vec<String> = ai_chats::load_messages(&st, "a")
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(messages, ["m1"]);
    // Still counted and deletable.
    assert_eq!(ai_chats::message_count(&st, "a").await.unwrap(), 2);
    let mut tx = st.write().await.unwrap();
    assert!(dashboards::delete(&mut tx, "bad").await.unwrap());
    assert!(ai_chats::delete(&mut tx, "b").await.unwrap());
    tx.commit().await.unwrap();
    st.close().await;
}

/// A name that isn't UTF-8 can never get a key, so it doesn't make
/// `refill_name_keys` report (and redo) work on every open, in any of the
/// four tables.
#[tokio::test]
async fn refill_skips_names_that_arent_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    exec(
        &st,
        "INSERT INTO dashboards (id, project_id, name, created_at, updated_at) \
         VALUES ('bad', 'p', CAST(X'FF' AS TEXT), 'c', 'u'); \
         INSERT INTO connections (id, project_id, name, type, host, port, database_name, \
           username) VALUES ('badc', 'p', CAST(X'FE' AS TEXT), 'postgres', 'h', 1, 'd', 'u'); \
         INSERT INTO projects (id, name, created_at, updated_at) \
           VALUES ('badp', CAST(X'FD' AS TEXT), 'c', 'u'); \
         INSERT INTO saved_queries (id, project_id, name, query, created_at, updated_at) \
           VALUES ('bads', 'p', CAST(X'FC' AS TEXT), 'q', 'c', 'u');",
    )
    .await;
    assert!(!seaquel_storage::refill_name_keys(&st).await.unwrap());
    // A fillable row next to them is still found, and filled.
    exec(
        &st,
        "INSERT INTO dashboards (id, project_id, name, created_at, updated_at) \
         VALUES ('ok', 'p', 'Fine', 'c', 'u')",
    )
    .await;
    assert!(seaquel_storage::refill_name_keys(&st).await.unwrap());
    assert!(!seaquel_storage::refill_name_keys(&st).await.unwrap());
    assert_eq!(
        dashboard_keys(&st).await,
        [
            ("bad".to_string(), None),
            ("ok".to_string(), Some(name_key("fine")))
        ]
    );
    st.close().await;
}

// ── Task 4 additions: `windowGet`'s read and a provider's vault rows ──

/// `windowGet`'s fallback (Decision 22): the most recently used window
/// that has an active project, whichever it is; windows without one are
/// passed over.
#[tokio::test]
async fn most_recent_active_is_the_latest_window_with_a_project() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    assert!(windows::most_recent_active(&st).await.unwrap().is_none());
    let mut tx = st.write().await.unwrap();
    windows::set_active_project(&mut tx, "a", "p1", "2026-10-01T00:00:00.000Z")
        .await
        .unwrap();
    windows::set_active_project(&mut tx, "b", "p2", "2026-10-03T00:00:00.000Z")
        .await
        .unwrap();
    // Newer, but with no active project.
    windows::touch(&mut tx, "c", "2026-10-05T00:00:00.000Z")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let w = windows::most_recent_active(&st).await.unwrap().unwrap();
    assert_eq!(
        (w.window_id.as_str(), w.active_project_id.as_deref()),
        ("b", Some("p2"))
    );
    st.close().await;
}

#[tokio::test]
async fn most_recent_active_uses_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let sql = windows::MOST_RECENT_ACTIVE;
    let plan = plan(&st, sql).await;
    assert_indexed(sql, &plan, "idx_windows_write_seq");
    st.close().await;
}

/// A removed AI provider's vault row goes inside the removal's write
/// (Decision 20); rows of other scopes or keys stay.
#[tokio::test]
async fn a_credential_is_removed_by_scope_and_key_inside_a_write() {
    use seaquel_storage::user_credentials;
    use seaquel_types::storage::PersistedCredential;
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    for (scope, key) in [
        ("ai-api-key-provider", "prov-1"),
        ("ai-api-key-provider", "prov-2"),
        ("db", "prov-1"),
    ] {
        user_credentials::save(
            &st,
            &PersistedCredential {
                scope: scope.into(),
                key: key.into(),
                nonce: "n".into(),
                ciphertext: "c".into(),
                updated_at: "t".into(),
            },
        )
        .await
        .unwrap();
    }
    let mut tx = st.write().await.unwrap();
    assert_eq!(
        user_credentials::remove_in(&mut tx, "ai-api-key-provider", "prov-1")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        user_credentials::remove_in(&mut tx, "ai-api-key-provider", "none")
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
    let left: Vec<(String, String)> =
        sqlx::query_as("SELECT scope, key FROM user_credentials ORDER BY scope, key")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(
        left,
        vec![
            ("ai-api-key-provider".to_string(), "prov-2".to_string()),
            ("db".to_string(), "prov-1".to_string()),
        ]
    );
    st.close().await;
}

/// Windows used in the same millisecond (5d-2 Task 7 probe fix): "most
/// recent" is the last write committed, in both tables, whatever the rows'
/// times and insertion order, so `windowGet`, a new window's copy and the
/// legacy mirror (written by every save) agree. A row updated in place
/// keeps its rowid, so rowid alone can't say which write came last; each
/// write records the file's next `write_seq`. The prunes delete by the
/// same order they keep by.
#[tokio::test]
async fn equal_times_order_by_the_last_write_committed() {
    const AT: &str = "2026-10-04T00:00:00.000Z";
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    // `a` saves first, then `b`, then `c`, each in its own write; then `a`
    // again, updating its rows in place (same rowids), all at one time.
    for (w, n) in [("a", 1), ("b", 1), ("c", 1), ("a", 2)] {
        let mut tx = st.write().await.unwrap();
        windows::set_active_project(&mut tx, w, "p", AT)
            .await
            .unwrap();
        window_state::put_if_newer(&mut tx, w, "p", n, &format!("{{\"w\":\"{w}{n}\"}}"), AT)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    for _ in 0..3 {
        let w = windows::most_recent_active(&st).await.unwrap().unwrap();
        assert_eq!(w.window_id, "a", "the window used last");
        let s = window_state::most_recent(&st, "p").await.unwrap().unwrap();
        assert_eq!(
            (s.window_id.as_str(), s.state.unwrap().get()),
            ("a", r#"{"w":"a2"}"#),
            "the view state saved last"
        );
    }
    // `c` saving after `a` makes it the most recent.
    let mut tx = st.write().await.unwrap();
    windows::touch(&mut tx, "c", AT).await.unwrap();
    window_state::put_if_newer(&mut tx, "c", "p", 2, r#"{"w":"c2"}"#, AT)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let s = window_state::most_recent(&st, "p").await.unwrap().unwrap();
    assert_eq!(s.window_id, "c");
    // A stale save writes nothing and so doesn't count as the latest.
    let mut tx = st.write().await.unwrap();
    assert!(
        !window_state::put_if_newer(&mut tx, "b", "p", 1, r#"{"w":"b-old"}"#, AT)
            .await
            .unwrap()
            .written
    );
    tx.commit().await.unwrap();
    assert_eq!(
        window_state::most_recent(&st, "p")
            .await
            .unwrap()
            .unwrap()
            .window_id,
        "c"
    );
    // Keeping two view states drops the oldest write: "b".
    let mut tx = st.write().await.unwrap();
    assert_eq!(
        window_state::prune_for_project(&mut tx, "p", 2, &[])
            .await
            .unwrap(),
        1
    );
    // Keeping one window keeps "c", touched last: "a" and "b" go.
    assert_eq!(
        windows::prune(&mut tx, "2000-01-01", Some(1), &[])
            .await
            .unwrap(),
        2
    );
    tx.commit().await.unwrap();
    let left: Vec<String> = sqlx::query_scalar("SELECT window_id FROM windows")
        .fetch_all(st.pool())
        .await
        .unwrap();
    assert_eq!(left, vec!["c".to_string()]);
    let states: Vec<String> = sqlx::query_scalar("SELECT window_id FROM window_state")
        .fetch_all(st.pool())
        .await
        .unwrap();
    assert_eq!(states, vec!["c".to_string()]);
    st.close().await;
}

#[tokio::test]
async fn the_prunes_use_their_indexes_without_a_sort() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    for (sql, index) in [
        (windows::PRUNE_UNUSED, "idx_windows_updated"),
        (windows::PRUNE_OVER_COUNT, "idx_windows_write_seq"),
        (
            window_state::PRUNE_FOR_PROJECT,
            "idx_window_state_project_seq",
        ),
    ] {
        let plan = plan(&st, sql).await;
        assert_indexed(sql, &plan, index);
    }
    st.close().await;
}

/// Phase 5d-2 review: the web's per-user backstop. With `max_bytes` the
/// file can't grow past its cap: a write past it fails with `STORAGE_FULL`
/// and leaves nothing, and smaller writes still go through. Without it
/// (the desktop) the same write succeeds.
#[tokio::test]
async fn a_write_past_the_size_cap_is_storage_full() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let cap = 2 * 1024 * 1024;
    let st = Storage::open(
        &path,
        StorageOptions {
            max_bytes: Some(cap),
            // One connection: a failed write must leave it usable.
            max_connections: 1,
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap();
    let pages: i64 = sqlx::query_scalar("PRAGMA max_page_count")
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(pages as u64, cap / seaquel_storage::CAP_PAGE_SIZE);
    let big = "x".repeat(3 * 1024 * 1024);
    for _ in 0..3 {
        let mut tx = st.write().await.unwrap();
        app_state::set_in(&mut tx, "before", Some("kept?"))
            .await
            .unwrap();
        let e = app_state::set_in(&mut tx, "big", Some(&big))
            .await
            .unwrap_err();
        assert_eq!(e.code(), seaquel_storage::STORAGE_FULL, "{e}");
        drop(tx);
        // SQLite rolled the whole transaction back; the connection is fine.
        assert_eq!(app_state::get(&st, "big").await.unwrap(), None);
        assert_eq!(app_state::get(&st, "before").await.unwrap(), None);
    }
    // A commit that fails the same way is rolled back too.
    let mut tx = st.write().await.unwrap();
    let _ = app_state::set_in(&mut tx, "big", Some(&big)).await;
    let _ = tx.commit().await;
    let mut tx = st.write().await.unwrap();
    app_state::set_in(&mut tx, "small", Some("x"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    st.close().await;

    let other = tempfile::tempdir().unwrap();
    let (st, _) = fresh(other.path()).await;
    let mut tx = st.write().await.unwrap();
    app_state::set_in(&mut tx, "big", Some(&big)).await.unwrap();
    tx.commit().await.unwrap();
    st.close().await;
}

fn capped(max_bytes: u64, max_connections: u32) -> StorageOptions {
    StorageOptions {
        max_bytes: Some(max_bytes),
        max_connections,
        ..StorageOptions::default()
    }
}

/// A plain pool on the same file: another process's writer.
async fn outsider(path: &Path) -> sqlx::SqlitePool {
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .busy_timeout(Duration::from_secs(10));
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap()
}

/// Phase 5d-2 re-review (critical): a `write()` cancelled while its
/// `BEGIN IMMEDIATE` waits for another writer leaves no transaction open on
/// the pooled connection. sqlx still runs that `BEGIN` once the lock frees;
/// the dropped guard's `ROLLBACK` then ends it, so the next read and write
/// on the one connection work and are seen by others.
#[tokio::test]
async fn a_cancelled_write_leaves_no_transaction_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let st = Storage::open(
        &path,
        StorageOptions {
            max_connections: 1,
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap();
    let other = outsider(&path).await;
    let mut held = other.acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *held)
        .await
        .unwrap();
    // Our `BEGIN IMMEDIATE` is sent, then waits on the other writer's
    // lock; the caller gives up and drops the future.
    assert!(tokio::time::timeout(Duration::from_millis(200), st.write())
        .await
        .is_err());
    sqlx::query("COMMIT").execute(&mut *held).await.unwrap();
    drop(held);
    let wait = Duration::from_secs(20);
    tokio::time::timeout(wait, app_state::get(&st, "k"))
        .await
        .expect("the pool's connection comes back")
        .unwrap();
    let mut tx = tokio::time::timeout(wait, st.write())
        .await
        .expect("a new write begins")
        .unwrap();
    app_state::set_in(&mut tx, "k", Some("v")).await.unwrap();
    tx.commit().await.unwrap();
    // Committed for real: another connection sees it, and can write.
    let seen: Option<String> = sqlx::query_scalar("SELECT value FROM app_state WHERE key = 'k'")
        .fetch_optional(&other)
        .await
        .unwrap();
    assert_eq!(seen.as_deref(), Some("v"));
    sqlx::query("INSERT INTO app_state (key, value) VALUES ('other', '1')")
        .execute(&other)
        .await
        .unwrap();
    other.close().await;
    st.close().await;
}

/// Phase 5d-2 re-review: a file at or over the cap still opens (the schema
/// work, here a pending migration that adds pages, runs uncapped); reads
/// and deletes work, and only a write that grows it is refused.
#[tokio::test]
async fn a_file_over_the_cap_opens_reads_and_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let (st, path) = fresh(dir.path()).await;
    let mut tx = st.write().await.unwrap();
    app_state::set_in(&mut tx, "big", Some(&"x".repeat(3 * 1024 * 1024)))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    st.close().await;
    let migrator = sqlx::migrate::Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/test_migrations"),
    )
    .await
    .unwrap();
    let st = Storage::open_with_migrator(&path, capped(2 * 1024 * 1024, 2), migrator)
        .await
        .expect("a file over the cap opens");
    assert_eq!(count(&st, "SELECT COUNT(*) FROM race_marker").await, 1);
    assert_eq!(
        app_state::get(&st, "big").await.unwrap().map(|v| v.len()),
        Some(3 * 1024 * 1024)
    );
    let mut tx = st.write().await.unwrap();
    let e = app_state::set_in(&mut tx, "more", Some(&"y".repeat(512 * 1024)))
        .await
        .unwrap_err();
    assert_eq!(e.code(), seaquel_storage::STORAGE_FULL);
    drop(tx);
    let mut tx = st.write().await.unwrap();
    assert!(app_state::delete_in(&mut tx, "big").await.unwrap());
    tx.commit().await.unwrap();
    assert_eq!(app_state::get(&st, "big").await.unwrap(), None);
    st.close().await;
}

/// A user at the cap can delete to make room: filled until a write is
/// refused, a delete succeeds and then small writes go through again.
#[tokio::test]
async fn a_delete_succeeds_at_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let st = Storage::open(&path, capped(2 * 1024 * 1024, 1))
        .await
        .unwrap();
    let chunk = "z".repeat(100 * 1024);
    let mut stored = 0;
    loop {
        let mut tx = st.write().await.unwrap();
        match app_state::set_in(&mut tx, &format!("k{stored}"), Some(&chunk)).await {
            Ok(()) => match tx.commit().await {
                Ok(()) => stored += 1,
                Err(e) => {
                    assert_eq!(e.code(), seaquel_storage::STORAGE_FULL);
                    break;
                }
            },
            Err(e) => {
                assert_eq!(e.code(), seaquel_storage::STORAGE_FULL);
                break;
            }
        }
        assert!(stored < 100, "the cap never hit");
    }
    assert!(stored > 0);
    let mut tx = st.write().await.unwrap();
    assert!(app_state::delete_in(&mut tx, "k0").await.unwrap());
    tx.commit().await.unwrap();
    let mut tx = st.write().await.unwrap();
    app_state::set_in(&mut tx, "small", Some("fits"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        app_state::get(&st, "small").await.unwrap().as_deref(),
        Some("fits")
    );
    st.close().await;
}
