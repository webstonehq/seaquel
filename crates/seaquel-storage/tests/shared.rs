//! Phase 5e storage: migration `0004_shared_links.sql` (where each shared
//! row's file is and the content it last synced, Decision 33 of the 5e
//! plan), the link reads and writes on saved queries, dashboards,
//! connections and projects, and the targeted `shared_repos` queries
//! (Decision 43).

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::path::{Path, PathBuf};

use common::*;
use seaquel_storage::{
    app_state, connections, dashboards, projects, saved_queries, shared_repos, RowLink, SharedLink,
    Storage, StorageError, StorageOptions, STORAGE_NEEDS_UPGRADE,
};
use seaquel_types::storage::{
    PersistedConnection, PersistedDashboard, PersistedProject, PersistedSavedQuery,
};
use serde_json::value::RawValue;

// ── Helpers ──

const RELEASES: &[&str] = &[
    "v2026.4.5-beta.1",
    "v2026.4.5",
    "v2026.4.8",
    "v2026.9.1",
    "v2026.9.2",
    "current",
];

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

async fn exec(st: &Storage, sql: &str) {
    sqlx::raw_sql(sql).execute(st.pool()).await.unwrap();
}

async fn columns_of(st: &Storage, table: &str) -> Vec<String> {
    sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .fetch_all(st.pool())
        .await
        .unwrap()
}

/// The new columns, by table (Decision 33).
const NEW_COLUMNS: &[(&str, &str)] = &[
    ("projects", "shared_dir"),
    ("saved_queries", "shared_path"),
    ("saved_queries", "shared_base"),
    ("saved_queries", "shared_file_id"),
    ("dashboards", "shared_path"),
    ("dashboards", "shared_base"),
    ("dashboards", "shared_file_id"),
    ("connections", "shared_base"),
    ("connections", "shared_file_id"),
    // Migration `0005` (Q31).
    ("connections", "shared_origin"),
];

fn assert_needs_upgrade(err: &StorageError, reason: &str) {
    assert_eq!(err.code(), STORAGE_NEEDS_UPGRADE, "{err}");
    assert!(
        err.to_string().contains(reason),
        "{err} should mention {reason}"
    );
}

/// The migrator 2026.9.x shipped: `0001` to `0003`, copied into `dir`.
async fn migrator_before_0004(dir: &Path) -> sqlx::migrate::Migrator {
    let before_0004 = dir.join("migrations");
    std::fs::create_dir_all(&before_0004).unwrap();
    for file in [
        "0001_name_keys.sql",
        "0002_window_state.sql",
        "0003_window_order_and_list_meta.sql",
    ] {
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("migrations")
                .join(file),
            before_0004.join(file),
        )
        .unwrap();
    }
    sqlx::migrate::Migrator::new(before_0004).await.unwrap()
}

// ── Migration 0004 ──

/// On every release's file (the beta-era one included, where
/// `saved_queries.project_id` and `dashboards.project_id` are nullable,
/// last and have no foreign key) the migration adds the nullable link
/// columns and their indexes, and leaves every row's links NULL; `0005`
/// adds `connections.shared_origin`, NULL too.
#[tokio::test]
async fn migration_0004_applies_on_every_release_schema() {
    for release in RELEASES {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        load_fixture(&path, &format!("schemas/{release}.sql")).await;
        // The file as 2026.9.x left it (baseline, `0001` to `0003`), with
        // rows that release wrote.
        Storage::open_with_migrator(
            &path,
            StorageOptions::default(),
            migrator_before_0004(dir.path()).await,
        )
        .await
        .unwrap_or_else(|e| panic!("{release}: {e}"))
        .close()
        .await;
        exec_file(
            &path,
            "INSERT INTO projects (id, name, created_at, updated_at, git_repo_path) \
               VALUES ('p', 'P', 'c', 'u', '/repo'); \
             INSERT INTO saved_queries (id, project_id, name, query, created_at, updated_at, shared) \
               VALUES ('q', 'p', 'Q', 'select 1', 'c', 'u', 1); \
             INSERT INTO dashboards (id, project_id, name, viewport, widgets, created_at, updated_at, shared) \
               VALUES ('d', 'p', 'D', '{}', '[]', 'c', 'u', 1); \
             INSERT INTO connections (id, project_id, name, type, host, port, database_name, username, \
               shared_connection_id) \
               VALUES ('c', 'p', 'C', 'postgres', 'h', 5432, 'app', 'me', 'repo-1:.seaquel/x.yaml');",
        )
        .await;

        let st = Storage::open(&path, StorageOptions::default())
            .await
            .unwrap_or_else(|e| panic!("{release}: {e}"));
        let applied: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(st.pool())
                .await
                .unwrap();
        assert_eq!(applied, [1, 2, 3, 4, 5, 6, 7], "{release}");

        for (table, column) in NEW_COLUMNS {
            assert!(
                columns_of(&st, table).await.iter().any(|c| c == column),
                "{release}: {table}.{column}"
            );
            let set: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM {table} WHERE {column} IS NOT NULL"
            ))
            .fetch_one(st.pool())
            .await
            .unwrap();
            assert_eq!(set, 0, "{release}: {table}.{column} starts NULL");
        }
        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'index' \
             AND name IN ('idx_saved_queries_shared_path', 'idx_dashboards_shared_path') \
             ORDER BY name",
        )
        .fetch_all(st.pool())
        .await
        .unwrap();
        assert_eq!(
            indexes,
            [
                "idx_dashboards_shared_path",
                "idx_saved_queries_shared_path"
            ],
            "{release}"
        );
        // The template's path stays where older releases keep it.
        let template: Option<String> =
            sqlx::query_scalar("SELECT shared_connection_id FROM connections WHERE id = 'c'")
                .fetch_one(st.pool())
                .await
                .unwrap();
        assert_eq!(template.as_deref(), Some("repo-1:.seaquel/x.yaml"));
        st.close().await;
    }
}

/// A file the app opened before `0004_shared_links.sql` shipped (`0001`
/// to `0003` applied): the CLI refuses it until the app has run `0004`,
/// and leaves it as it was.
#[tokio::test]
async fn a_read_only_open_refuses_a_file_with_0004_pending() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let older = migrator_before_0004(dir.path()).await;
    Storage::open_with_migrator(&path, StorageOptions::default(), older)
        .await
        .unwrap()
        .close()
        .await;
    let before = snapshot(&path).await;

    let err = Storage::open(&path, read_only()).await.unwrap_err();
    assert_needs_upgrade(&err, "migration 4");
    assert_eq!(snapshot(&path).await, before);

    // The app's open applies it; then the CLI's works.
    Storage::open(&path, StorageOptions::default())
        .await
        .unwrap()
        .close()
        .await;
    Storage::open(&path, read_only())
        .await
        .unwrap()
        .close()
        .await;
}

/// A dev database from Tasks 3–6 (`0001` to `0004` applied, no `0005`):
/// the CLI refuses it until the app has opened it, and the app's open adds
/// `connections.shared_origin`, NULL on the links those builds stored
/// (exported ones included, which then count as imported until relinked).
#[tokio::test]
async fn a_dev_file_at_0004_is_refused_read_only_and_upgraded_by_a_writable_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let migrations = dir.path().join("migrations");
    std::fs::create_dir_all(&migrations).unwrap();
    for file in [
        "0001_name_keys.sql",
        "0002_window_state.sql",
        "0003_window_order_and_list_meta.sql",
        "0004_shared_links.sql",
    ] {
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("migrations")
                .join(file),
            migrations.join(file),
        )
        .unwrap();
    }
    let at_0004 = sqlx::migrate::Migrator::new(migrations).await.unwrap();
    Storage::open_with_migrator(&path, StorageOptions::default(), at_0004)
        .await
        .unwrap()
        .close()
        .await;
    exec_file(
        &path,
        "INSERT INTO projects (id, name, created_at, updated_at, git_repo_path) \
           VALUES ('p', 'P', 'c', 'u', '/repo'); \
         INSERT INTO connections (id, project_id, name, type, host, port, database_name, username, \
           shared_connection_id) \
           VALUES ('c', 'p', 'C', 'postgres', 'h', 5432, 'app', 'me', 'repo-1:.seaquel/x.yaml');",
    )
    .await;
    let before = snapshot(&path).await;

    let err = Storage::open(&path, read_only()).await.unwrap_err();
    assert_needs_upgrade(&err, "migration 5");
    assert_eq!(snapshot(&path).await, before);

    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    assert!(columns_of(&st, "connections")
        .await
        .contains(&"shared_origin".to_string()));
    assert_eq!(connections::origin(&st, "c").await.unwrap(), None);
    st.close().await;
    Storage::open(&path, read_only())
        .await
        .unwrap()
        .close()
        .await;
}

/// The migration is expand-only: an older release's saves (the frozen
/// replace-all and upsert writers, which name only the columns it knows)
/// still work on a migrated file and leave every link as it was, so the
/// next sync sees their change as a row change against an unchanged base.
/// A connection's link path is `shared_connection_id`, a column older
/// releases write: it is whatever their copy held.
#[tokio::test]
async fn an_older_releases_writes_still_work_after_0004() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    saved_queries::set_link(&mut tx, "sq", &link(QUERY_PATH, "h1", "f1"))
        .await
        .unwrap();
    dashboards::set_link(&mut tx, "d", &link(BOARD_PATH, "h2", "f2"))
        .await
        .unwrap();
    connections::set_link(&mut tx, "c", &link(TEMPLATE_ID, "h3", "f3"))
        .await
        .unwrap();
    projects::set_shared_dir(&mut tx, "p", Some("main"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // The older release's copies: edited, and knowing nothing of links.
    let mut q = saved_queries::get(&st, "sq").await.unwrap().unwrap();
    q.query = "select 2".into();
    q.shared_path = None;
    saved_queries::save_all(&st, "p", &[q]).await.unwrap();
    let mut d = dashboards::get(&st, "d").await.unwrap().unwrap();
    d.widgets = "[1]".into();
    d.shared_path = None;
    dashboards::save(&st, &d).await.unwrap();
    let mut c = connections::get(&st, "c").await.unwrap().unwrap();
    c.host = "db.example".into();
    c.shared_connection_id = Some("repo-1:.seaquel/projects/p/connections/old.yaml".into());
    connections::save(&st, &c).await.unwrap();
    let mut p = projects::get(&st, "p").await.unwrap().unwrap();
    p.name = "Renamed".into();
    projects::save_all(&st, &[p]).await.unwrap();

    // The edits landed.
    let q = saved_queries::get(&st, "sq").await.unwrap().unwrap();
    assert_eq!(q.query, "select 2");
    assert_eq!(q.shared_path.as_deref(), Some(QUERY_PATH));
    let d = dashboards::get(&st, "d").await.unwrap().unwrap();
    assert_eq!(d.widgets, "[1]");
    assert_eq!(d.shared_path.as_deref(), Some(BOARD_PATH));
    assert_eq!(
        connections::get(&st, "c").await.unwrap().unwrap().host,
        "db.example"
    );
    assert_eq!(
        projects::get(&st, "p").await.unwrap().unwrap().name,
        "Renamed"
    );
    // All nine columns survive.
    assert_eq!(
        saved_queries::links(&st, "p").await.unwrap()[0].link,
        link(QUERY_PATH, "h1", "f1")
    );
    assert_eq!(
        dashboards::links(&st, "p").await.unwrap()[0].link,
        link(BOARD_PATH, "h2", "f2")
    );
    assert_eq!(
        connections::links(&st, "p").await.unwrap()[0].link,
        link(
            "repo-1:.seaquel/projects/p/connections/old.yaml",
            "h3",
            "f3"
        )
    );
    assert_eq!(
        projects::shared_dir(&st, "p").await.unwrap().as_deref(),
        Some("main")
    );
    st.close().await;
}

// ── Links on rows ──

/// Projects `p` (linked to `/repo`) and `q`, a saved query `sq` and a
/// dashboard `d` in `p`, and a connection `c` in `p`, written as Core
/// writes them.
async fn seeded(st: &Storage) {
    let mut tx = st.write().await.unwrap();
    let mut p = project("p");
    p.git_repo_path = Some("/repo".into());
    projects::insert(&mut tx, &p).await.unwrap();
    projects::insert(&mut tx, &project("q")).await.unwrap();
    saved_queries::insert(&mut tx, &saved_query("sq", "p", "Sales"))
        .await
        .unwrap();
    dashboards::insert(&mut tx, &dashboard("d", "p", "Board"))
        .await
        .unwrap();
    connections::insert(&mut tx, &connection("c", "p"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
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

fn saved_query(id: &str, project_id: &str, name: &str) -> PersistedSavedQuery {
    PersistedSavedQuery {
        id: id.into(),
        name: name.into(),
        query: "select 1".into(),
        project_id: project_id.into(),
        created_at: "c".into(),
        updated_at: "u".into(),
        parameters: None,
        starred: false,
        shared: true,
        description: None,
        database_type: None,
        tags: None,
        folder: None,
        shared_path: None,
    }
}

fn dashboard(id: &str, project_id: &str, name: &str) -> PersistedDashboard {
    PersistedDashboard {
        id: id.into(),
        project_id: project_id.into(),
        name: name.into(),
        viewport: "{}".into(),
        widgets: "[]".into(),
        date_filter: None,
        starred: false,
        shared: true,
        description: None,
        created_at: "c".into(),
        updated_at: "u".into(),
        shared_path: None,
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

fn link(path: &str, base: &str, file_id: &str) -> SharedLink {
    SharedLink {
        path: Some(path.into()),
        base: Some(base.into()),
        file_id: Some(file_id.into()),
    }
}

const QUERY_PATH: &str = ".seaquel/projects/p/queries/sales.sql";
const BOARD_PATH: &str = ".seaquel/projects/p/dashboards/board.json";
const TEMPLATE_ID: &str = "repo-1:.seaquel/projects/p/connections/c.yaml";

/// `set_link` writes the three link columns of one row and nothing else;
/// `links` reads them for every row of the project; the reads that give
/// the GUI its rows carry `sharedPath`, and only when it's set.
#[tokio::test]
async fn set_link_writes_the_link_and_the_reads_carry_shared_path() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;

    let mut tx = st.write().await.unwrap();
    assert!(
        saved_queries::set_link(&mut tx, "sq", &link(QUERY_PATH, "h1", "f1"))
            .await
            .unwrap()
    );
    assert!(
        dashboards::set_link(&mut tx, "d", &link(BOARD_PATH, "h2", "f2"))
            .await
            .unwrap()
    );
    assert!(
        connections::set_link(&mut tx, "c", &link(TEMPLATE_ID, "h3", "f3"))
            .await
            .unwrap()
    );
    // Missing rows: nothing written.
    assert!(
        !saved_queries::set_link(&mut tx, "nope", &SharedLink::default())
            .await
            .unwrap()
    );
    assert!(
        !dashboards::set_link(&mut tx, "nope", &SharedLink::default())
            .await
            .unwrap()
    );
    assert!(
        !connections::set_link(&mut tx, "nope", &SharedLink::default())
            .await
            .unwrap()
    );
    // Reads inside the write see it.
    assert_eq!(
        saved_queries::links(&mut tx, "p").await.unwrap(),
        [RowLink {
            id: "sq".into(),
            link: link(QUERY_PATH, "h1", "f1")
        }]
    );
    tx.commit().await.unwrap();

    assert_eq!(
        dashboards::links(&st, "p").await.unwrap(),
        [RowLink {
            id: "d".into(),
            link: link(BOARD_PATH, "h2", "f2")
        }]
    );
    // A connection's path is its `shared_connection_id`, as older releases
    // store it.
    assert_eq!(
        connections::links(&st, "p").await.unwrap(),
        [RowLink {
            id: "c".into(),
            link: link(TEMPLATE_ID, "h3", "f3")
        }]
    );
    assert_eq!(
        connections::get(&st, "c")
            .await
            .unwrap()
            .unwrap()
            .shared_connection_id
            .as_deref(),
        Some(TEMPLATE_ID)
    );
    assert!(saved_queries::links(&st, "q").await.unwrap().is_empty());

    let q = saved_queries::get(&st, "sq").await.unwrap().unwrap();
    assert_eq!(q.shared_path.as_deref(), Some(QUERY_PATH));
    assert_eq!(q.name, "Sales");
    assert_eq!(
        saved_queries::load_by_project(&st, "p").await.unwrap()[0]
            .shared_path
            .as_deref(),
        Some(QUERY_PATH)
    );
    let d = dashboards::get(&st, "d").await.unwrap().unwrap();
    assert_eq!(d.shared_path.as_deref(), Some(BOARD_PATH));
    assert_eq!(
        dashboards::list(&st, "p").await.unwrap()[0]
            .shared_path
            .as_deref(),
        Some(BOARD_PATH)
    );
    let json = serde_json::to_value(&q).unwrap();
    assert_eq!(json["sharedPath"], QUERY_PATH);

    // `update` (a library patch) leaves the link alone; clearing it is
    // `set_link` with nothing.
    let mut tx = st.write().await.unwrap();
    let mut renamed = q.clone();
    renamed.name = "Revenue".into();
    renamed.shared_path = None;
    assert!(saved_queries::update(&mut tx, &renamed).await.unwrap());
    let mut d2 = d.clone();
    d2.shared_path = None;
    assert!(dashboards::update(&mut tx, &d2).await.unwrap());
    assert_eq!(
        saved_queries::links(&mut tx, "p").await.unwrap()[0].link,
        link(QUERY_PATH, "h1", "f1")
    );
    assert_eq!(
        dashboards::links(&mut tx, "p").await.unwrap()[0].link,
        link(BOARD_PATH, "h2", "f2")
    );
    assert!(
        saved_queries::set_link(&mut tx, "sq", &SharedLink::default())
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let q = saved_queries::get(&st, "sq").await.unwrap().unwrap();
    assert_eq!(q.shared_path, None);
    assert!(serde_json::to_value(&q)
        .unwrap()
        .get("sharedPath")
        .is_none());
    st.close().await;
}

/// `by_shared_path` finds a project's rows by their file's path, exactly,
/// in rowid order, and only in that project.
#[tokio::test]
async fn by_shared_path_finds_rows_by_their_file() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    saved_queries::insert(&mut tx, &saved_query("other", "q", "Sales"))
        .await
        .unwrap();
    dashboards::insert(&mut tx, &dashboard("other", "q", "Board"))
        .await
        .unwrap();
    for (id, path) in [("sq", QUERY_PATH), ("other", QUERY_PATH)] {
        saved_queries::set_link(&mut tx, id, &link(path, "h", "f"))
            .await
            .unwrap();
    }
    for (id, path) in [("d", BOARD_PATH), ("other", BOARD_PATH)] {
        dashboards::set_link(&mut tx, id, &link(path, "h", "f"))
            .await
            .unwrap();
    }
    assert_eq!(
        saved_queries::by_shared_path(&mut tx, "p", QUERY_PATH)
            .await
            .unwrap(),
        ["sq"]
    );
    tx.commit().await.unwrap();
    assert_eq!(
        saved_queries::by_shared_path(&st, "q", QUERY_PATH)
            .await
            .unwrap(),
        ["other"]
    );
    assert!(
        saved_queries::by_shared_path(&st, "p", &QUERY_PATH.to_uppercase())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        dashboards::by_shared_path(&st, "p", BOARD_PATH)
            .await
            .unwrap(),
        ["d"]
    );
    assert!(dashboards::by_shared_path(&st, "p", QUERY_PATH)
        .await
        .unwrap()
        .is_empty());
    st.close().await;
}

/// The lines of `EXPLAIN QUERY PLAN sql`, joined.
async fn plan(st: &Storage, sql: &str) -> String {
    let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(&format!("EXPLAIN QUERY PLAN {sql}"))
        .fetch_all(st.pool())
        .await
        .unwrap();
    rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n")
}

fn assert_no_scan(sql: &str, plan: &str) {
    for line in plan.lines() {
        if line.contains("SCAN") {
            assert!(line.contains("INDEX"), "a full scan in\n{sql}:\n{plan}");
        }
        assert!(!line.contains("TEMP B-TREE"), "a sort in\n{sql}:\n{plan}");
    }
}

/// The path lookups search `(project_id, shared_path)`; the link writes and
/// the per-project link reads use an index too, so neither walks the table.
#[tokio::test]
async fn set_link_and_by_shared_path_use_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    for (sql, index) in [
        (
            saved_queries::BY_SHARED_PATH,
            "idx_saved_queries_shared_path",
        ),
        (dashboards::BY_SHARED_PATH, "idx_dashboards_shared_path"),
    ] {
        let p = plan(&st, sql).await;
        assert_no_scan(sql, &p);
        assert!(p.contains(index), "{index} unused in\n{sql}:\n{p}");
    }
    for sql in [
        saved_queries::SET_LINK,
        dashboards::SET_LINK,
        connections::SET_LINK,
        saved_queries::LINKS,
        dashboards::LINKS,
        connections::LINKS,
        projects::SET_SHARED_DIR,
    ] {
        let p = plan(&st, sql).await;
        assert_no_scan(sql, &p);
        assert!(p.contains("INDEX"), "no index in\n{sql}:\n{p}");
    }
    st.close().await;
}

/// A project's directory in its repo: set, read and cleared; and the
/// projects linked to one repo path.
#[tokio::test]
async fn a_projects_shared_dir_and_the_projects_of_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    assert!(projects::set_shared_dir(&mut tx, "p", Some("main"))
        .await
        .unwrap());
    assert!(!projects::set_shared_dir(&mut tx, "nope", Some("main"))
        .await
        .unwrap());
    assert_eq!(
        projects::shared_dir(&mut tx, "p").await.unwrap().as_deref(),
        Some("main")
    );
    tx.commit().await.unwrap();
    assert_eq!(projects::shared_dir(&st, "q").await.unwrap(), None);
    assert_eq!(projects::shared_dir(&st, "nope").await.unwrap(), None);
    assert_eq!(
        projects::ids_with_repo_path(&st, "/repo").await.unwrap(),
        ["p"]
    );
    assert!(projects::ids_with_repo_path(&st, "/other")
        .await
        .unwrap()
        .is_empty());
    // A rename (`update`) keeps the directory.
    let mut tx = st.write().await.unwrap();
    let mut p = projects::get(&mut tx, "p").await.unwrap().unwrap();
    p.name = "Renamed".into();
    assert!(projects::update(&mut tx, &p).await.unwrap());
    assert!(projects::set_shared_dir(&mut tx, "q", Some("q-dir"))
        .await
        .unwrap());
    assert!(projects::set_shared_dir(&mut tx, "q", None).await.unwrap());
    tx.commit().await.unwrap();
    assert_eq!(
        projects::shared_dir(&st, "p").await.unwrap().as_deref(),
        Some("main")
    );
    assert_eq!(projects::shared_dir(&st, "q").await.unwrap(), None);
    st.close().await;
}

/// No path, hash or id in a link's `Debug`: paths hold project and query
/// names.
#[test]
fn a_links_debug_shows_no_path() {
    let l = link(
        "/Users/someone/acme/.seaquel/x.sql",
        "deadbeef",
        "file-id-1",
    );
    let row = RowLink {
        id: "sq".into(),
        link: l.clone(),
    };
    for text in [format!("{l:?}"), format!("{row:?}")] {
        for canary in ["someone", "acme", "deadbeef", "file-id-1"] {
            assert!(!text.contains(canary), "{text}");
        }
    }
    assert!(format!("{row:?}").contains("sq"));
}

// ── The repo list ──

fn raw(s: &str) -> Box<RawValue> {
    RawValue::from_string(s.into()).unwrap()
}

fn texts(v: &[Box<RawValue>]) -> Vec<&str> {
    v.iter().map(|r| r.get()).collect()
}

const REPO_A: &str = r#"{"id":"repo-a","name":"Team","path":"/work/team","remoteUrl":"git@x:t.git","branch":"main","lastSyncAt":null,"syncStatus":"synced","extra":{"kept":[1,2.50,"é"]}}"#;
const REPO_B: &str = r#"{"id":"repo-b","name":"Other","path":"/work/other","remoteUrl":"","branch":"main","lastSyncAt":null,"syncStatus":"uninitialized"}"#;

/// `insert`, `list`, `get`, `get_by_path` and `delete` on `&mut WriteTx`,
/// and an older release's `load_all` reads what they wrote (and the other
/// way round).
#[tokio::test]
async fn the_repo_list_reads_and_writes_one_repo_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    // A row an older release saved, and one that doesn't parse.
    shared_repos::save_all(&st, &[raw(REPO_B)], Some("repo-b"))
        .await
        .unwrap();
    exec(
        &st,
        "INSERT INTO shared_repos (id, data) VALUES ('bad', 'nope')",
    )
    .await;

    let mut tx = st.write().await.unwrap();
    shared_repos::insert(&mut tx, &raw(REPO_A)).await.unwrap();
    assert_eq!(
        texts(&shared_repos::list(&mut tx).await.unwrap()),
        [REPO_B, REPO_A]
    );
    tx.commit().await.unwrap();

    assert_eq!(
        shared_repos::get(&st, "repo-a")
            .await
            .unwrap()
            .unwrap()
            .get(),
        REPO_A
    );
    assert!(shared_repos::get(&st, "bad").await.unwrap().is_none());
    assert!(shared_repos::get(&st, "nope").await.unwrap().is_none());
    assert_eq!(
        shared_repos::get_by_path(&st, "/work/team")
            .await
            .unwrap()
            .unwrap()
            .get(),
        REPO_A
    );
    assert!(shared_repos::get_by_path(&st, "/work")
        .await
        .unwrap()
        .is_none());

    // The older release's load sees both, and keeps its active repo.
    let state = shared_repos::load_all(&st).await.unwrap();
    assert_eq!(texts(&state.repos), [REPO_B, REPO_A]);
    assert_eq!(state.active_repo_id.as_deref(), Some("repo-b"));

    // An id that exists fails rather than overwriting; a value that isn't
    // an object with a string id is refused.
    let mut tx = st.write().await.unwrap();
    assert!(shared_repos::insert(&mut tx, &raw(REPO_A)).await.is_err());
    drop(tx);
    let mut tx = st.write().await.unwrap();
    for bad in [r#"[1]"#, r#"{"path":"/x"}"#, r#"{"id":7}"#, r#"{"id":""}"#] {
        assert!(
            shared_repos::insert(&mut tx, &raw(bad)).await.is_err(),
            "{bad}"
        );
    }
    assert!(shared_repos::delete(&mut tx, "repo-b").await.unwrap());
    assert!(!shared_repos::delete(&mut tx, "repo-b").await.unwrap());
    tx.commit().await.unwrap();
    assert_eq!(texts(&shared_repos::list(&st).await.unwrap()), [REPO_A]);
    // `activeRepoId` is never touched by the targeted calls.
    assert_eq!(
        app_state::get(&st, "activeRepoId")
            .await
            .unwrap()
            .as_deref(),
        Some("repo-b")
    );
    st.close().await;
}

/// `register` is Core's: inside one write, `get_by_path` then `insert`.
/// Since writers queue, a second registration of the same path finds the
/// first one's row, so one path has one repo.
#[tokio::test]
async fn register_is_idempotent_by_path() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;

    async fn register(st: &Storage, id: &str, path: &str) -> String {
        let mut tx = st.write().await.unwrap();
        if let Some(found) = shared_repos::get_by_path(&mut tx, path).await.unwrap() {
            let v: serde_json::Value = serde_json::from_str(found.get()).unwrap();
            return v["id"].as_str().unwrap().to_string();
        }
        let json = serde_json::json!({"id": id, "name": "n", "path": path, "remoteUrl": "",
            "branch": "main", "lastSyncAt": null, "syncStatus": "uninitialized"});
        shared_repos::insert(&mut tx, &raw(&json.to_string()))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id.to_string()
    }

    let (a, b) = tokio::join!(
        register(&st, "repo-1", "/work/team"),
        register(&st, "repo-2", "/work/team")
    );
    assert_eq!(a, b);
    assert_eq!(shared_repos::list(&st).await.unwrap().len(), 1);
    assert_eq!(register(&st, "repo-3", "/work/other").await, "repo-3");
    assert_eq!(shared_repos::list(&st).await.unwrap().len(), 2);
    st.close().await;
}

/// `update_json` sets the named fields (in place, or added at the end) and
/// keeps every other byte of the stored JSON: unknown fields, their order,
/// number spellings and escapes, so older releases read the row as they
/// wrote it.
#[tokio::test]
async fn update_json_keeps_unknown_fields_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    // Spaces and a repeated key, as a hand edit could leave: the last
    // `name` is the one JSON.parse reads, so that's the one replaced.
    let stored = r#"{ "id": "repo-a", "name": "Old", "path":"/work/team", "name" : "Team", "branch":"main", "extra": {"kept":[1,2.50,"é"]}, "syncStatus":"behind" }"#;
    exec(
        &st,
        &format!("INSERT INTO shared_repos (id, data) VALUES ('repo-a', '{stored}')"),
    )
    .await;

    let mut tx = st.write().await.unwrap();
    let name = raw(r#""Renamed \"x\"""#);
    let at = raw(r#""2026-10-01T00:00:00.000Z""#);
    let url = raw(r#""git@host:t.git""#);
    assert!(shared_repos::update_json(
        &mut tx,
        "repo-a",
        &[("name", &*name), ("lastSyncAt", &*at), ("remoteUrl", &*url)],
    )
    .await
    .unwrap());
    assert!(
        !shared_repos::update_json(&mut tx, "nope", &[("name", &*name)])
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    let got = shared_repos::get(&st, "repo-a").await.unwrap().unwrap();
    assert_eq!(
        got.get(),
        r#"{ "id": "repo-a", "name": "Old", "path":"/work/team", "name" : "Renamed \"x\"", "branch":"main", "extra": {"kept":[1,2.50,"é"]}, "syncStatus":"behind" ,"lastSyncAt":"2026-10-01T00:00:00.000Z","remoteUrl":"git@host:t.git"}"#
    );

    // Nothing named: nothing changes. `null` is a value like any other.
    let mut tx = st.write().await.unwrap();
    assert!(shared_repos::update_json(&mut tx, "repo-a", &[])
        .await
        .unwrap());
    let null = raw("null");
    assert!(
        shared_repos::update_json(&mut tx, "repo-a", &[("lastSyncAt", &*null)])
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let after = shared_repos::get(&st, "repo-a").await.unwrap().unwrap();
    assert_eq!(
        after.get(),
        got.get().replace(
            r#""lastSyncAt":"2026-10-01T00:00:00.000Z""#,
            r#""lastSyncAt":null"#
        )
    );

    // An empty object gains the field without a leading comma; a row that
    // isn't an object is an error, not a rewrite.
    exec(
        &st,
        "INSERT INTO shared_repos (id, data) VALUES ('empty', '{ }'), ('arr', '[1]')",
    )
    .await;
    let mut tx = st.write().await.unwrap();
    assert!(
        shared_repos::update_json(&mut tx, "empty", &[("name", &*name)])
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let mut tx = st.write().await.unwrap();
    assert!(
        shared_repos::update_json(&mut tx, "arr", &[("name", &*name)])
            .await
            .is_err()
    );
    drop(tx);
    let arr: String = sqlx::query_scalar("SELECT data FROM shared_repos WHERE id = 'arr'")
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(arr, "[1]");
    let empty: String = sqlx::query_scalar("SELECT data FROM shared_repos WHERE id = 'empty'")
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(empty, r#"{ "name":"Renamed \"x\""}"#);
    st.close().await;
}

/// A read-only storage (the CLI) reads the repo list and the links, and
/// refuses every write before touching the file.
#[tokio::test]
async fn a_read_only_storage_reads_links_and_refuses_link_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (st, path) = fresh(dir.path()).await;
    seeded(&st).await;
    let mut tx = st.write().await.unwrap();
    saved_queries::set_link(&mut tx, "sq", &link(QUERY_PATH, "h", "f"))
        .await
        .unwrap();
    shared_repos::insert(&mut tx, &raw(REPO_A)).await.unwrap();
    tx.commit().await.unwrap();
    st.close().await;

    let ro = Storage::open(&path, read_only()).await.unwrap();
    assert_eq!(saved_queries::links(&ro, "p").await.unwrap().len(), 1);
    assert_eq!(shared_repos::list(&ro).await.unwrap().len(), 1);
    assert_eq!(
        ro.write().await.unwrap_err().code(),
        seaquel_storage::STORAGE_READ_ONLY
    );
    ro.close().await;
}

/// `get_by_path` reads a repeated `"path"` key as `JSON.parse` does: the
/// last one wins.
#[tokio::test]
async fn get_by_path_reads_the_last_of_a_repeated_path() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    exec(
        &st,
        r#"INSERT INTO shared_repos (id, data) VALUES
           ('r', '{"id":"r","path":"/first","name":"n","path":"/last"}')"#,
    )
    .await;
    assert!(shared_repos::get_by_path(&st, "/first")
        .await
        .unwrap()
        .is_none());
    assert!(shared_repos::get_by_path(&st, "/last")
        .await
        .unwrap()
        .is_some());
    st.close().await;
}

/// `update_json` refuses to set `id`, so the JSON's id can't drift from
/// the row's; nothing is written.
#[tokio::test]
async fn update_json_refuses_the_id() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    let mut tx = st.write().await.unwrap();
    shared_repos::insert(&mut tx, &raw(REPO_B)).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = st.write().await.unwrap();
    let other = raw(r#""repo-x""#);
    let name = raw(r#""N""#);
    assert!(
        shared_repos::update_json(&mut tx, "repo-b", &[("name", &*name), ("id", &*other)])
            .await
            .is_err()
    );
    drop(tx);
    assert_eq!(
        shared_repos::get(&st, "repo-b")
            .await
            .unwrap()
            .unwrap()
            .get(),
        REPO_B
    );
    st.close().await;
}

// ── Migration 0005: where a linked connection came from (Q31) ──

/// `set_origin` records whether a linked connection was exported from
/// this project or imported from the repo; the GUI's reads carry it as
/// `sharedOrigin`; a link cleared by `set_link` clears it too, and a link
/// stored with a path keeps it. The column is only Core's: `update` (and an
/// older release's upsert) leaves it alone.
#[tokio::test]
async fn a_connections_origin_follows_its_link() {
    let dir = tempfile::tempdir().unwrap();
    let (st, _) = fresh(dir.path()).await;
    seeded(&st).await;
    assert!(columns_of(&st, "connections")
        .await
        .contains(&"shared_origin".to_string()));

    let mut tx = st.write().await.unwrap();
    assert!(
        connections::set_link(&mut tx, "c", &link(TEMPLATE_ID, "b", "f"))
            .await
            .unwrap()
    );
    assert!(
        connections::set_origin(&mut tx, "c", Some(connections::ORIGIN_EXPORTED))
            .await
            .unwrap()
    );
    assert!(
        !connections::set_origin(&mut tx, "nope", Some(connections::ORIGIN_IMPORTED))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let row = connections::list_in_project(&st, "p")
        .await
        .unwrap()
        .remove(0);
    assert_eq!(row.shared_origin.as_deref(), Some("exported"));

    // A new base for the same link keeps the origin; `update` leaves it.
    let mut tx = st.write().await.unwrap();
    connections::set_link(&mut tx, "c", &link(TEMPLATE_ID, "b2", "f"))
        .await
        .unwrap();
    let mut c = connection("c", "p");
    c.shared_connection_id = Some(TEMPLATE_ID.into());
    c.host = "other".into();
    connections::update(&mut tx, &c).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        connections::origin(&st, "c").await.unwrap().as_deref(),
        Some("exported")
    );

    // Clearing the link clears the origin.
    let mut tx = st.write().await.unwrap();
    connections::set_link(&mut tx, "c", &SharedLink::default())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(connections::origin(&st, "c").await.unwrap(), None);
    let row = connections::list_in_project(&st, "p")
        .await
        .unwrap()
        .remove(0);
    assert_eq!(row.shared_origin, None);
    st.close().await;
}
