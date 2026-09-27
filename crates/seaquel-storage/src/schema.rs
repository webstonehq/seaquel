//! The baseline: the schema every released metadata file is brought up to
//! before the numbered migrations run.
//!
//! This is a port of `src/lib/storage/schema.ts` (`DDL_STATEMENTS` and
//! `upgradeSchema`) plus the column adds of `MigrationManager`'s v3 and v4
//! steps. It is frozen history: the next schema change is a numbered file in
//! `migrations/`, never another step here.
//!
//! One change from the TypeScript: tables that are missing are created
//! before any column is added. `upgradeSchema` added columns first, so a
//! `v2026.4.5-beta.1` file (which has no `ai_messages`) failed on
//! `ALTER TABLE ai_messages ...` on every launch from v2026.4.8 on. Creating
//! the missing tables first makes that file upgrade to the same structure as
//! one that went through v2026.4.5, and changes nothing for any other file.

use std::collections::HashSet;

use sqlx::{Row, SqliteConnection};

/// The storage version a file has once the baseline has run. `schema_version`
/// keeps one row per version a file reached; the highest is the current one.
pub const CURRENT_STORAGE_VERSION: i64 = 4;

/// One `CREATE` statement of the baseline. `table` is the table it creates,
/// or the table an index is on.
struct Ddl {
    table: &'static str,
    sql: &'static str,
}

/// `schema.ts`'s `DDL_STATEMENTS`, statement by statement and in the same
/// order. The text is kept exactly: SQLite stores it in `sqlite_master`, so a
/// fresh file's schema reads the same as one the TypeScript made.
const DDL_STATEMENTS: &[Ddl] = &[
    Ddl {
        table: "schema_version",
        sql: r#"CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL,
    migrated_at TEXT NOT NULL DEFAULT (datetime('now'))
  )"#,
    },
    Ddl {
        table: "projects",
        sql: r#"CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    git_repo_path TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
  )"#,
    },
    Ddl {
        table: "project_labels",
        sql: r#"CREATE TABLE IF NOT EXISTS project_labels (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    is_predefined INTEGER NOT NULL DEFAULT 0,
    color TEXT NOT NULL
  )"#,
    },
    Ddl {
        table: "app_state",
        sql: r#"CREATE TABLE IF NOT EXISTS app_state (
    key TEXT PRIMARY KEY,
    value TEXT
  )"#,
    },
    Ddl {
        table: "connections",
        sql: r#"CREATE TABLE IF NOT EXISTS connections (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    database_name TEXT NOT NULL,
    username TEXT NOT NULL,
    ssl_mode TEXT,
    connection_string TEXT,
    last_connected TEXT,
    ssh_tunnel TEXT,
    save_password INTEGER NOT NULL DEFAULT 0,
    save_ssh_password INTEGER NOT NULL DEFAULT 0,
    save_ssh_key_passphrase INTEGER NOT NULL DEFAULT 0,
    is_local_only INTEGER NOT NULL DEFAULT 0,
    shared_connection_id TEXT,
    ai_share_schema INTEGER,
    ai_share_data INTEGER,
    active_ai_provider_id TEXT,
    active_ai_model TEXT
  )"#,
    },
    Ddl {
        table: "connections",
        sql: r#"CREATE INDEX IF NOT EXISTS idx_connections_project ON connections(project_id)"#,
    },
    Ddl {
        table: "connection_labels",
        sql: r#"CREATE TABLE IF NOT EXISTS connection_labels (
    connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    label_id TEXT NOT NULL,
    PRIMARY KEY (connection_id, label_id)
  )"#,
    },
    Ddl {
        table: "project_state",
        sql: r#"CREATE TABLE IF NOT EXISTS project_state (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    active_view TEXT NOT NULL DEFAULT 'query',
    active_connection_id TEXT,
    active_query_tab_id TEXT,
    active_schema_tab_id TEXT,
    active_explain_tab_id TEXT,
    active_erd_tab_id TEXT,
    active_statistics_tab_id TEXT,
    active_workflow_tab_id TEXT,
    active_visualize_tab_id TEXT,
    active_starter_tab_id TEXT,
    active_dashboard_tab_id TEXT,
    active_create_table_tab_id TEXT,
    active_data_tab_id TEXT,
    tab_order TEXT NOT NULL DEFAULT '[]',
    connection_order TEXT NOT NULL DEFAULT '[]',
    starred_shared_query_ids TEXT NOT NULL DEFAULT '[]',
    starred_shared_dashboard_ids TEXT NOT NULL DEFAULT '[]',
    pane_layout TEXT
  )"#,
    },
    Ddl {
        table: "tabs",
        sql: r#"CREATE TABLE IF NOT EXISTS tabs (
    id TEXT NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    tab_type TEXT NOT NULL,
    name TEXT NOT NULL,
    query TEXT,
    saved_query_id TEXT,
    shared_query_id TEXT,
    table_name TEXT,
    schema_name TEXT,
    source_query TEXT,
    connection_id TEXT,
    starter_type TEXT,
    closable INTEGER,
    PRIMARY KEY (id, project_id)
  )"#,
    },
    Ddl {
        table: "tabs",
        sql: r#"CREATE INDEX IF NOT EXISTS idx_tabs_project ON tabs(project_id)"#,
    },
    Ddl {
        table: "saved_queries",
        sql: r#"CREATE TABLE IF NOT EXISTS saved_queries (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    query TEXT NOT NULL,
    parameters TEXT,
    starred INTEGER NOT NULL DEFAULT 0,
    shared INTEGER NOT NULL DEFAULT 0,
    description TEXT,
    database_type TEXT,
    tags TEXT,
    folder TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
  )"#,
    },
    Ddl {
        table: "saved_queries",
        sql: r#"CREATE INDEX IF NOT EXISTS idx_saved_queries_project ON saved_queries(project_id)"#,
    },
    Ddl {
        table: "query_versions",
        sql: r#"CREATE TABLE IF NOT EXISTS query_versions (
    id TEXT PRIMARY KEY,
    saved_query_id TEXT NOT NULL REFERENCES saved_queries(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    snapshot TEXT,
    diff TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(saved_query_id, version),
    CHECK ((snapshot IS NOT NULL AND diff IS NULL) OR (snapshot IS NULL AND diff IS NOT NULL))
  )"#,
    },
    Ddl {
        table: "query_versions",
        sql: r#"CREATE INDEX IF NOT EXISTS idx_query_versions_saved_query ON query_versions(saved_query_id, version DESC)"#,
    },
    Ddl {
        table: "query_history",
        sql: r#"CREATE TABLE IF NOT EXISTS query_history (
    id TEXT PRIMARY KEY,
    connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    query TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    execution_time REAL NOT NULL,
    row_count INTEGER NOT NULL,
    favorite INTEGER NOT NULL DEFAULT 0,
    connection_labels_snapshot TEXT,
    connection_name_snapshot TEXT NOT NULL DEFAULT ''
  )"#,
    },
    Ddl {
        table: "query_history",
        sql: r#"CREATE INDEX IF NOT EXISTS idx_history_conn_time ON query_history(connection_id, timestamp DESC)"#,
    },
    Ddl {
        table: "shared_repos",
        sql: r#"CREATE TABLE IF NOT EXISTS shared_repos (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL
  )"#,
    },
    Ddl {
        table: "saved_canvases",
        sql: r#"CREATE TABLE IF NOT EXISTS saved_canvases (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    data TEXT NOT NULL
  )"#,
    },
    Ddl {
        table: "theme_preferences",
        sql: r#"CREATE TABLE IF NOT EXISTS theme_preferences (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    light_theme_id TEXT NOT NULL DEFAULT 'default-light',
    dark_theme_id TEXT NOT NULL DEFAULT 'default-dark'
  )"#,
    },
    Ddl {
        table: "user_themes",
        sql: r#"CREATE TABLE IF NOT EXISTS user_themes (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL
  )"#,
    },
    Ddl {
        table: "license_state",
        sql: r#"CREATE TABLE IF NOT EXISTS license_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT NOT NULL DEFAULT '{}'
  )"#,
    },
    Ddl {
        table: "onboarding_state",
        sql: r#"CREATE TABLE IF NOT EXISTS onboarding_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT NOT NULL DEFAULT '{}'
  )"#,
    },
    Ddl {
        table: "tutorial_progress",
        sql: r#"CREATE TABLE IF NOT EXISTS tutorial_progress (
    lesson_id TEXT NOT NULL,
    challenge_id TEXT NOT NULL,
    state TEXT,
    PRIMARY KEY (lesson_id, challenge_id)
  )"#,
    },
    Ddl {
        table: "import_state",
        sql: r#"CREATE TABLE IF NOT EXISTS import_state (
    source TEXT PRIMARY KEY,
    has_offered_import INTEGER NOT NULL DEFAULT 0,
    last_check_timestamp TEXT
  )"#,
    },
    Ddl {
        table: "connection_overrides",
        sql: r#"CREATE TABLE IF NOT EXISTS connection_overrides (
    shared_connection_id TEXT PRIMARY KEY,
    username TEXT,
    host_override TEXT,
    port_override INTEGER,
    save_password INTEGER NOT NULL DEFAULT 0,
    save_ssh_password INTEGER NOT NULL DEFAULT 0,
    save_ssh_key_passphrase INTEGER NOT NULL DEFAULT 0
  )"#,
    },
    Ddl {
        table: "dashboards",
        sql: r#"CREATE TABLE IF NOT EXISTS dashboards (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    viewport TEXT NOT NULL DEFAULT '{"x":0,"y":0,"zoom":1}',
    widgets TEXT NOT NULL DEFAULT '[]',
    date_filter TEXT,
    starred INTEGER DEFAULT 0,
    shared INTEGER NOT NULL DEFAULT 0,
    description TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
  )"#,
    },
    Ddl {
        table: "dashboards",
        sql: r#"CREATE INDEX IF NOT EXISTS idx_dashboards_project ON dashboards(project_id)"#,
    },
    Ddl {
        table: "dashboard_versions",
        sql: r#"CREATE TABLE IF NOT EXISTS dashboard_versions (
    id TEXT PRIMARY KEY,
    dashboard_id TEXT NOT NULL REFERENCES dashboards(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    snapshot TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(dashboard_id, version)
  )"#,
    },
    Ddl {
        table: "dashboard_versions",
        sql: r#"CREATE INDEX IF NOT EXISTS idx_dashboard_versions_dashboard ON dashboard_versions(dashboard_id, version DESC)"#,
    },
    Ddl {
        table: "ai_chats",
        sql: r#"CREATE TABLE IF NOT EXISTS ai_chats (
    id TEXT PRIMARY KEY,
    connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
  )"#,
    },
    Ddl {
        table: "ai_chats",
        sql: r#"CREATE INDEX IF NOT EXISTS idx_ai_chats_connection ON ai_chats(connection_id)"#,
    },
    Ddl {
        table: "vault_state",
        sql: r#"CREATE TABLE IF NOT EXISTS vault_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    salt TEXT NOT NULL,
    kdf_params TEXT NOT NULL,
    verifier TEXT NOT NULL,
    verifier_nonce TEXT NOT NULL,
    created_at TEXT NOT NULL
  )"#,
    },
    Ddl {
        table: "user_credentials",
        sql: r#"CREATE TABLE IF NOT EXISTS user_credentials (
    scope TEXT NOT NULL,
    key TEXT NOT NULL,
    nonce TEXT NOT NULL,
    ciphertext TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (scope, key)
  )"#,
    },
    Ddl {
        table: "ai_messages",
        sql: r#"CREATE TABLE IF NOT EXISTS ai_messages (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL REFERENCES ai_chats(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    query TEXT,
    dashboard_id TEXT
  )"#,
    },
    Ddl {
        table: "ai_messages",
        sql: r#"CREATE INDEX IF NOT EXISTS idx_ai_messages_chat ON ai_messages(chat_id)"#,
    },
];

/// A column that `sql` (`ALTER TABLE ... ADD COLUMN`) adds when `table`
/// lacks it.
struct ColumnUpgrade {
    table: &'static str,
    column: &'static str,
    sql: &'static str,
}

/// `upgradeSchema`'s column adds, in its order. The order matters: it is the
/// column order an upgraded file ends up with.
///
/// `MigrationManager`'s v3 and v4 steps added `project_state`'s
/// `active_dashboard_tab_id` and `pane_layout`. Both are already in this
/// list, so they need no step of their own. (v3 also created a `dashboards`
/// table keyed by `connection_id`, but only for files below version 3, and
/// no release left a metadata file below 3.)
const COLUMN_UPGRADES: &[ColumnUpgrade] = &[
    ColumnUpgrade {
        table: "project_state",
        column: "active_dashboard_tab_id",
        sql: "ALTER TABLE project_state ADD COLUMN active_dashboard_tab_id TEXT",
    },
    ColumnUpgrade {
        table: "projects",
        column: "git_repo_path",
        sql: "ALTER TABLE projects ADD COLUMN git_repo_path TEXT",
    },
    ColumnUpgrade {
        table: "connections",
        column: "is_local_only",
        sql: "ALTER TABLE connections ADD COLUMN is_local_only INTEGER NOT NULL DEFAULT 0",
    },
    ColumnUpgrade {
        table: "connections",
        column: "shared_connection_id",
        sql: "ALTER TABLE connections ADD COLUMN shared_connection_id TEXT",
    },
    ColumnUpgrade {
        table: "saved_queries",
        column: "starred",
        sql: "ALTER TABLE saved_queries ADD COLUMN starred INTEGER NOT NULL DEFAULT 0",
    },
    ColumnUpgrade {
        table: "project_state",
        column: "starred_shared_query_ids",
        sql: "ALTER TABLE project_state ADD COLUMN starred_shared_query_ids TEXT NOT NULL DEFAULT '[]'",
    },
    ColumnUpgrade {
        table: "project_state",
        column: "starred_shared_dashboard_ids",
        sql: "ALTER TABLE project_state ADD COLUMN starred_shared_dashboard_ids TEXT NOT NULL DEFAULT '[]'",
    },
    ColumnUpgrade {
        table: "dashboards",
        column: "starred",
        sql: "ALTER TABLE dashboards ADD COLUMN starred INTEGER DEFAULT 0",
    },
    ColumnUpgrade {
        table: "connections",
        column: "ai_share_schema",
        sql: "ALTER TABLE connections ADD COLUMN ai_share_schema INTEGER",
    },
    ColumnUpgrade {
        table: "connections",
        column: "ai_share_data",
        sql: "ALTER TABLE connections ADD COLUMN ai_share_data INTEGER",
    },
    ColumnUpgrade {
        table: "connections",
        column: "active_ai_provider_id",
        sql: "ALTER TABLE connections ADD COLUMN active_ai_provider_id TEXT",
    },
    ColumnUpgrade {
        table: "connections",
        column: "active_ai_model",
        sql: "ALTER TABLE connections ADD COLUMN active_ai_model TEXT",
    },
    ColumnUpgrade {
        table: "project_state",
        column: "pane_layout",
        sql: "ALTER TABLE project_state ADD COLUMN pane_layout TEXT",
    },
    ColumnUpgrade {
        table: "dashboards",
        column: "shared",
        sql: "ALTER TABLE dashboards ADD COLUMN shared INTEGER NOT NULL DEFAULT 0",
    },
    ColumnUpgrade {
        table: "dashboards",
        column: "description",
        sql: "ALTER TABLE dashboards ADD COLUMN description TEXT",
    },
    ColumnUpgrade {
        table: "saved_queries",
        column: "shared",
        sql: "ALTER TABLE saved_queries ADD COLUMN shared INTEGER NOT NULL DEFAULT 0",
    },
    ColumnUpgrade {
        table: "saved_queries",
        column: "description",
        sql: "ALTER TABLE saved_queries ADD COLUMN description TEXT",
    },
    ColumnUpgrade {
        table: "saved_queries",
        column: "database_type",
        sql: "ALTER TABLE saved_queries ADD COLUMN database_type TEXT",
    },
    ColumnUpgrade {
        table: "saved_queries",
        column: "tags",
        sql: "ALTER TABLE saved_queries ADD COLUMN tags TEXT",
    },
    ColumnUpgrade {
        table: "saved_queries",
        column: "folder",
        sql: "ALTER TABLE saved_queries ADD COLUMN folder TEXT",
    },
    ColumnUpgrade {
        table: "project_state",
        column: "active_create_table_tab_id",
        sql: "ALTER TABLE project_state ADD COLUMN active_create_table_tab_id TEXT",
    },
    ColumnUpgrade {
        table: "project_state",
        column: "active_data_tab_id",
        sql: "ALTER TABLE project_state ADD COLUMN active_data_tab_id TEXT",
    },
    ColumnUpgrade {
        table: "ai_messages",
        column: "dashboard_id",
        sql: "ALTER TABLE ai_messages ADD COLUMN dashboard_id TEXT",
    },
    ColumnUpgrade {
        table: "project_state",
        column: "connection_order",
        sql: "ALTER TABLE project_state ADD COLUMN connection_order TEXT NOT NULL DEFAULT '[]'",
    },
];

/// Bring the file behind `conn` up to the baseline schema, and record
/// [`CURRENT_STORAGE_VERSION`] in `schema_version` unless a row at least that
/// high is there.
///
/// Run it inside a transaction ([`crate::Storage::open`] does), so a failure
/// leaves the file as it was. It is idempotent: on a file that is already
/// up to date it changes nothing.
///
/// Steps, in order:
/// 1. create the tables that don't exist yet, with their indexes (on an empty
///    file this is every statement of `DDL_STATEMENTS`, in order);
/// 2. add the columns of `COLUMN_UPGRADES` that existing tables lack;
/// 3. move `saved_queries` and `dashboards` from `connection_id` to
///    `project_id`, rename `active_canvas_tab_id`, and rewrite the `canvas`
///    view to `workflow`;
/// 4. run every `DDL_STATEMENTS` entry again (`IF NOT EXISTS`), which adds
///    the indexes that depend on step 3;
/// 5. insert the version row.
pub async fn baseline(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    // 1. Missing tables first, so no column add targets a table that doesn't
    //    exist yet. Indexes on those tables come with them; indexes on
    //    existing tables wait for step 4, since they may name a column that
    //    step 3 adds.
    let existing = table_names(conn).await?;
    for ddl in DDL_STATEMENTS {
        if !existing.contains(ddl.table) {
            sqlx::query(ddl.sql).execute(&mut *conn).await?;
        }
    }

    // 2. Column adds. As in `upgradeSchema`, each table's columns are read
    //    once, before any of them runs.
    let mut read: Vec<(&str, HashSet<String>)> = Vec::new();
    for table in COLUMN_UPGRADES.iter().map(|u| u.table).chain([
        "saved_queries",
        "dashboards",
        "project_state",
    ]) {
        if !read.iter().any(|(t, _)| *t == table) {
            let cols = column_names(conn, table).await?;
            read.push((table, cols));
        }
    }
    let columns = |table: &str| -> &HashSet<String> {
        &read
            .iter()
            .find(|(t, _)| *t == table)
            .expect("every table named here was read above")
            .1
    };
    for upgrade in COLUMN_UPGRADES {
        if !columns(upgrade.table).contains(upgrade.column) {
            sqlx::query(upgrade.sql).execute(&mut *conn).await?;
        }
    }

    // 3a. saved_queries: connection_id -> project_id. Rows whose connection
    //     no longer exists are deleted.
    let sq = columns("saved_queries");
    if sq.contains("connection_id") && !sq.contains("project_id") {
        execute(conn, "ALTER TABLE saved_queries ADD COLUMN project_id TEXT").await?;
        execute(
            conn,
            "UPDATE saved_queries SET project_id = (
        SELECT project_id FROM connections WHERE connections.id = saved_queries.connection_id
      ) WHERE project_id IS NULL",
        )
        .await?;
        execute(conn, "DELETE FROM saved_queries WHERE project_id IS NULL").await?;
    }
    if sq.contains("connection_id") {
        execute(conn, "DROP INDEX IF EXISTS idx_saved_queries_connection").await?;
        execute(conn, "ALTER TABLE saved_queries DROP COLUMN connection_id").await?;
    }

    // 3b. dashboards: the same move.
    let dash = columns("dashboards");
    if dash.contains("connection_id") && !dash.contains("project_id") {
        execute(conn, "ALTER TABLE dashboards ADD COLUMN project_id TEXT").await?;
        execute(
            conn,
            "UPDATE dashboards SET project_id = (
        SELECT project_id FROM connections WHERE connections.id = dashboards.connection_id
      ) WHERE project_id IS NULL",
        )
        .await?;
        execute(conn, "DELETE FROM dashboards WHERE project_id IS NULL").await?;
    }
    if dash.contains("connection_id") {
        execute(conn, "DROP INDEX IF EXISTS idx_dashboards_connection").await?;
        execute(conn, "ALTER TABLE dashboards DROP COLUMN connection_id").await?;
    }

    // 3c. active_canvas_tab_id -> active_workflow_tab_id, and the view.
    let ps = columns("project_state");
    if ps.contains("active_canvas_tab_id") && !ps.contains("active_workflow_tab_id") {
        execute(
            conn,
            "ALTER TABLE project_state RENAME COLUMN active_canvas_tab_id TO active_workflow_tab_id",
        )
        .await?;
    }
    execute(
        conn,
        "UPDATE project_state SET active_view = 'workflow' WHERE active_view = 'canvas'",
    )
    .await?;

    // 4. Everything again; only what step 1 couldn't make yet is new.
    for ddl in DDL_STATEMENTS {
        execute(conn, ddl.sql).await?;
    }

    // 5. The version row. A fresh file gets its first row here; an older one
    //    (beta.1 wrote 1 and 3) gets 4 on top of what it has.
    let latest: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM schema_version")
        .fetch_one(&mut *conn)
        .await?;
    if latest.is_none_or(|v| v < CURRENT_STORAGE_VERSION) {
        sqlx::query("INSERT INTO schema_version (version) VALUES (?)")
            .bind(CURRENT_STORAGE_VERSION)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Whether [`baseline`] would change nothing on the file behind `conn`. It
/// only reads, so a read-only open can ask it.
///
/// It tests each step's own condition, not a full schema comparison: a
/// file the baseline leaves as it is counts as current even where it
/// differs from a fresh one (a `v2026.4.5-beta.1` file's nullable
/// `project_id`s), because the app couldn't change that either. Step by
/// step:
/// 1. every table of `DDL_STATEMENTS` exists;
/// 2. every column of `COLUMN_UPGRADES` exists;
/// 3. `saved_queries` and `dashboards` have no `connection_id`,
///    `project_state` doesn't have `active_canvas_tab_id` without
///    `active_workflow_tab_id` (the only case the rename runs), and no
///    row's `active_view` is `canvas`;
/// 4. every index of `DDL_STATEMENTS` exists (the tables already do);
/// 5. the highest `schema_version` is at least [`CURRENT_STORAGE_VERSION`].
pub async fn is_current(conn: &mut SqliteConnection) -> Result<bool, sqlx::Error> {
    // 1.
    let tables = table_names(conn).await?;
    if DDL_STATEMENTS.iter().any(|d| !tables.contains(d.table)) {
        return Ok(false);
    }
    // 2.
    let mut read: Vec<(&str, HashSet<String>)> = Vec::new();
    for table in COLUMN_UPGRADES.iter().map(|u| u.table).chain([
        "saved_queries",
        "dashboards",
        "project_state",
    ]) {
        if !read.iter().any(|(t, _)| *t == table) {
            let cols = column_names(conn, table).await?;
            read.push((table, cols));
        }
    }
    let has = |table: &str, column: &str| {
        read.iter()
            .any(|(t, cols)| *t == table && cols.contains(column))
    };
    if COLUMN_UPGRADES.iter().any(|u| !has(u.table, u.column)) {
        return Ok(false);
    }
    // 3.
    if has("saved_queries", "connection_id")
        || has("dashboards", "connection_id")
        || (has("project_state", "active_canvas_tab_id")
            && !has("project_state", "active_workflow_tab_id"))
    {
        return Ok(false);
    }
    let canvas: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM project_state WHERE active_view = 'canvas')",
    )
    .fetch_one(&mut *conn)
    .await?;
    if canvas {
        return Ok(false);
    }
    // 4.
    let indexes: HashSet<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'index'")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();
    if DDL_STATEMENTS
        .iter()
        .filter_map(|d| index_name(d.sql))
        .any(|name| !indexes.contains(name))
    {
        return Ok(false);
    }
    // 5.
    let latest: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM schema_version")
        .fetch_one(&mut *conn)
        .await?;
    Ok(latest.is_some_and(|v| v >= CURRENT_STORAGE_VERSION))
}

/// The index a `CREATE INDEX IF NOT EXISTS <name> ON …` statement makes, or
/// `None` for a table.
fn index_name(sql: &str) -> Option<&str> {
    sql.strip_prefix("CREATE INDEX IF NOT EXISTS ")?
        .split_whitespace()
        .next()
}

async fn execute(conn: &mut SqliteConnection, sql: &'static str) -> Result<(), sqlx::Error> {
    sqlx::query(sql).execute(conn).await.map(drop)
}

async fn table_names(conn: &mut SqliteConnection) -> Result<HashSet<String>, sqlx::Error> {
    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table'")
        .fetch_all(&mut *conn)
        .await?;
    rows.iter().map(|r| r.try_get::<String, _>(0)).collect()
}

/// The columns of `table`, or none when it doesn't exist. `table` is always
/// one of this module's constants, never input.
async fn column_names(
    conn: &mut SqliteConnection,
    table: &str,
) -> Result<HashSet<String>, sqlx::Error> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(&mut *conn)
        .await?;
    rows.iter()
        .map(|r| r.try_get::<String, _>("name"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_statement_names_its_table() {
        assert_eq!(DDL_STATEMENTS.len(), 35);
        for ddl in DDL_STATEMENTS {
            let (kind, rest) = ddl
                .sql
                .split_once(" IF NOT EXISTS ")
                .expect("every statement uses IF NOT EXISTS");
            match kind {
                "CREATE TABLE" => assert!(rest.starts_with(&format!("{} (", ddl.table))),
                "CREATE INDEX" => assert!(rest.contains(&format!(" ON {}(", ddl.table))),
                other => panic!("unexpected statement kind {other}"),
            }
        }
    }

    #[test]
    fn every_index_statement_has_a_name() {
        let names: Vec<&str> = DDL_STATEMENTS
            .iter()
            .filter_map(|d| index_name(d.sql))
            .collect();
        assert_eq!(names.len(), 9);
        assert!(names.contains(&"idx_ai_messages_chat"));
        assert!(names.iter().all(|n| n.starts_with("idx_")));
    }

    #[test]
    fn column_upgrades_match_their_sql() {
        assert_eq!(COLUMN_UPGRADES.len(), 24);
        for u in COLUMN_UPGRADES {
            let prefix = format!("ALTER TABLE {} ADD COLUMN {} ", u.table, u.column);
            assert!(u.sql.starts_with(&prefix), "{}", u.sql);
        }
    }

    #[test]
    fn v3_and_v4_column_adds_are_covered() {
        for column in ["active_dashboard_tab_id", "pane_layout"] {
            assert!(COLUMN_UPGRADES
                .iter()
                .any(|u| u.table == "project_state" && u.column == column));
        }
    }
}
