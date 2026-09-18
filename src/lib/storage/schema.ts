import type { SqliteDatabase } from "./sqlite-types";

const SCHEMA_VERSION = 1;

/**
 * Current storage/data-migration version.
 * Increment when adding new data migrations in MigrationManager.
 * On a fresh database the DDL is already up-to-date, so this version
 * is recorded directly to skip all data migrations.
 */
export const CURRENT_STORAGE_VERSION = 4;

const DDL_STATEMENTS = [
  // Version tracking
  `CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL,
    migrated_at TEXT NOT NULL DEFAULT (datetime('now'))
  )`,

  // Projects
  `CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    git_repo_path TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
  )`,

  `CREATE TABLE IF NOT EXISTS project_labels (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    is_predefined INTEGER NOT NULL DEFAULT 0,
    color TEXT NOT NULL
  )`,

  // App state (key-value for simple settings)
  `CREATE TABLE IF NOT EXISTS app_state (
    key TEXT PRIMARY KEY,
    value TEXT
  )`,

  // Connections
  `CREATE TABLE IF NOT EXISTS connections (
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
  )`,
  `CREATE INDEX IF NOT EXISTS idx_connections_project ON connections(project_id)`,

  `CREATE TABLE IF NOT EXISTS connection_labels (
    connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    label_id TEXT NOT NULL,
    PRIMARY KEY (connection_id, label_id)
  )`,

  // Project UI state
  `CREATE TABLE IF NOT EXISTS project_state (
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
  )`,

  // All tab types (discriminated by tab_type)
  `CREATE TABLE IF NOT EXISTS tabs (
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
  )`,
  `CREATE INDEX IF NOT EXISTS idx_tabs_project ON tabs(project_id)`,

  // Saved queries (unified: local + shared)
  `CREATE TABLE IF NOT EXISTS saved_queries (
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
  )`,
  `CREATE INDEX IF NOT EXISTS idx_saved_queries_project ON saved_queries(project_id)`,

  // Query versions (version history for saved queries)
  `CREATE TABLE IF NOT EXISTS query_versions (
    id TEXT PRIMARY KEY,
    saved_query_id TEXT NOT NULL REFERENCES saved_queries(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    snapshot TEXT,
    diff TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(saved_query_id, version),
    CHECK ((snapshot IS NOT NULL AND diff IS NULL) OR (snapshot IS NULL AND diff IS NOT NULL))
  )`,
  `CREATE INDEX IF NOT EXISTS idx_query_versions_saved_query ON query_versions(saved_query_id, version DESC)`,

  // Query history
  `CREATE TABLE IF NOT EXISTS query_history (
    id TEXT PRIMARY KEY,
    connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    query TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    execution_time REAL NOT NULL,
    row_count INTEGER NOT NULL,
    favorite INTEGER NOT NULL DEFAULT 0,
    connection_labels_snapshot TEXT,
    connection_name_snapshot TEXT NOT NULL DEFAULT ''
  )`,
  `CREATE INDEX IF NOT EXISTS idx_history_conn_time ON query_history(connection_id, timestamp DESC)`,

  // Shared repos
  `CREATE TABLE IF NOT EXISTS shared_repos (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL
  )`,

  // Saved canvases
  `CREATE TABLE IF NOT EXISTS saved_canvases (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    data TEXT NOT NULL
  )`,

  // Theme preferences (singleton)
  `CREATE TABLE IF NOT EXISTS theme_preferences (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    light_theme_id TEXT NOT NULL DEFAULT 'default-light',
    dark_theme_id TEXT NOT NULL DEFAULT 'default-dark'
  )`,

  // User themes
  `CREATE TABLE IF NOT EXISTS user_themes (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL
  )`,

  // License state (singleton)
  `CREATE TABLE IF NOT EXISTS license_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT NOT NULL DEFAULT '{}'
  )`,

  // Onboarding state (singleton)
  `CREATE TABLE IF NOT EXISTS onboarding_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT NOT NULL DEFAULT '{}'
  )`,

  // Tutorial progress
  `CREATE TABLE IF NOT EXISTS tutorial_progress (
    lesson_id TEXT NOT NULL,
    challenge_id TEXT NOT NULL,
    state TEXT,
    PRIMARY KEY (lesson_id, challenge_id)
  )`,

  // Import state
  `CREATE TABLE IF NOT EXISTS import_state (
    source TEXT PRIMARY KEY,
    has_offered_import INTEGER NOT NULL DEFAULT 0,
    last_check_timestamp TEXT
  )`,

  // Connection overrides for shared connection templates
  `CREATE TABLE IF NOT EXISTS connection_overrides (
    shared_connection_id TEXT PRIMARY KEY,
    username TEXT,
    host_override TEXT,
    port_override INTEGER,
    save_password INTEGER NOT NULL DEFAULT 0,
    save_ssh_password INTEGER NOT NULL DEFAULT 0,
    save_ssh_key_passphrase INTEGER NOT NULL DEFAULT 0
  )`,

  // Dashboards (unified: local + shared)
  `CREATE TABLE IF NOT EXISTS dashboards (
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
  )`,
  `CREATE INDEX IF NOT EXISTS idx_dashboards_project ON dashboards(project_id)`,

  // Dashboard versions (version history for dashboards)
  `CREATE TABLE IF NOT EXISTS dashboard_versions (
    id TEXT PRIMARY KEY,
    dashboard_id TEXT NOT NULL REFERENCES dashboards(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    snapshot TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(dashboard_id, version)
  )`,
  `CREATE INDEX IF NOT EXISTS idx_dashboard_versions_dashboard ON dashboard_versions(dashboard_id, version DESC)`,

  // AI chats
  `CREATE TABLE IF NOT EXISTS ai_chats (
    id TEXT PRIMARY KEY,
    connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
  )`,
  `CREATE INDEX IF NOT EXISTS idx_ai_chats_connection ON ai_chats(connection_id)`,

  // Vault state (singleton per meta.db — used only on web builds).
  // `verifier` is a known-plaintext blob encrypted with VK; unlocking
  // decrypts it to confirm the passphrase before trusting any other
  // decryption. Binary columns are base64-encoded TEXT so they travel
  // unchanged through the JSON /api/storage/* pipe.
  `CREATE TABLE IF NOT EXISTS vault_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    salt TEXT NOT NULL,
    kdf_params TEXT NOT NULL,
    verifier TEXT NOT NULL,
    verifier_nonce TEXT NOT NULL,
    created_at TEXT NOT NULL
  )`,

  // Encrypted credentials (web-only — desktop keeps using the OS keychain
  // via TauriKeyringService). One row per (scope, key). `scope` mirrors
  // KeyringService categories; `key` is the connectionId / providerId, or
  // empty string for singletons (license key, primary AI API key).
  `CREATE TABLE IF NOT EXISTS user_credentials (
    scope TEXT NOT NULL,
    key TEXT NOT NULL,
    nonce TEXT NOT NULL,
    ciphertext TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (scope, key)
  )`,

  // AI messages
  `CREATE TABLE IF NOT EXISTS ai_messages (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL REFERENCES ai_chats(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    query TEXT,
    dashboard_id TEXT
  )`,
  `CREATE INDEX IF NOT EXISTS idx_ai_messages_chat ON ai_messages(chat_id)`,
];

/**
 * Initialize the database schema. Returns true if this was a fresh database
 * (tables were just created), false if the schema already existed.
 */
export async function initializeSchema(db: SqliteDatabase): Promise<boolean> {
  // Check if schema already exists
  const existing = await db.query<{ name: string }>(
    "SELECT name FROM sqlite_master WHERE type='table' AND name='schema_version'",
  );

  if (existing.length > 0) {
    // Schema already initialized — run incremental DDL for any new tables/indexes
    await upgradeSchema(db);
    return false;
  }

  // Create all tables (but don't insert schema_version yet —
  // the caller will do that after checking for JSON migration)
  const statements = DDL_STATEMENTS.map((sql) => ({ sql }));
  await db.transaction(statements);

  return true;
}

/**
 * Run all DDL statements on an existing database.
 * Every statement uses IF NOT EXISTS, so this is safe to run repeatedly.
 * This ensures new tables/indexes added to DDL_STATEMENTS are created
 * on existing databases without requiring a data migration.
 */
async function upgradeSchema(db: SqliteDatabase): Promise<void> {
  // === Column migrations (must run BEFORE DDL so new indexes can reference new columns) ===

  // Column additions (ALTER TABLE doesn't support IF NOT EXISTS)
  const columnUpgrades = [
    {
      table: "project_state",
      column: "active_dashboard_tab_id",
      sql: "ALTER TABLE project_state ADD COLUMN active_dashboard_tab_id TEXT",
    },
    {
      table: "projects",
      column: "git_repo_path",
      sql: "ALTER TABLE projects ADD COLUMN git_repo_path TEXT",
    },
    {
      table: "connections",
      column: "is_local_only",
      sql: "ALTER TABLE connections ADD COLUMN is_local_only INTEGER NOT NULL DEFAULT 0",
    },
    {
      table: "connections",
      column: "shared_connection_id",
      sql: "ALTER TABLE connections ADD COLUMN shared_connection_id TEXT",
    },
    {
      table: "saved_queries",
      column: "starred",
      sql: "ALTER TABLE saved_queries ADD COLUMN starred INTEGER NOT NULL DEFAULT 0",
    },
    {
      table: "project_state",
      column: "starred_shared_query_ids",
      sql: "ALTER TABLE project_state ADD COLUMN starred_shared_query_ids TEXT NOT NULL DEFAULT '[]'",
    },
    {
      table: "project_state",
      column: "starred_shared_dashboard_ids",
      sql: "ALTER TABLE project_state ADD COLUMN starred_shared_dashboard_ids TEXT NOT NULL DEFAULT '[]'",
    },
    {
      table: "dashboards",
      column: "starred",
      sql: "ALTER TABLE dashboards ADD COLUMN starred INTEGER DEFAULT 0",
    },
    {
      table: "connections",
      column: "ai_share_schema",
      sql: "ALTER TABLE connections ADD COLUMN ai_share_schema INTEGER",
    },
    {
      table: "connections",
      column: "ai_share_data",
      sql: "ALTER TABLE connections ADD COLUMN ai_share_data INTEGER",
    },
    {
      table: "connections",
      column: "active_ai_provider_id",
      sql: "ALTER TABLE connections ADD COLUMN active_ai_provider_id TEXT",
    },
    {
      table: "connections",
      column: "active_ai_model",
      sql: "ALTER TABLE connections ADD COLUMN active_ai_model TEXT",
    },
    {
      table: "project_state",
      column: "pane_layout",
      sql: "ALTER TABLE project_state ADD COLUMN pane_layout TEXT",
    },
    {
      table: "dashboards",
      column: "shared",
      sql: "ALTER TABLE dashboards ADD COLUMN shared INTEGER NOT NULL DEFAULT 0",
    },
    {
      table: "dashboards",
      column: "description",
      sql: "ALTER TABLE dashboards ADD COLUMN description TEXT",
    },
    {
      table: "saved_queries",
      column: "shared",
      sql: "ALTER TABLE saved_queries ADD COLUMN shared INTEGER NOT NULL DEFAULT 0",
    },
    {
      table: "saved_queries",
      column: "description",
      sql: "ALTER TABLE saved_queries ADD COLUMN description TEXT",
    },
    {
      table: "saved_queries",
      column: "database_type",
      sql: "ALTER TABLE saved_queries ADD COLUMN database_type TEXT",
    },
    {
      table: "saved_queries",
      column: "tags",
      sql: "ALTER TABLE saved_queries ADD COLUMN tags TEXT",
    },
    {
      table: "saved_queries",
      column: "folder",
      sql: "ALTER TABLE saved_queries ADD COLUMN folder TEXT",
    },
    {
      table: "project_state",
      column: "active_create_table_tab_id",
      sql: "ALTER TABLE project_state ADD COLUMN active_create_table_tab_id TEXT",
    },
    {
      table: "project_state",
      column: "active_data_tab_id",
      sql: "ALTER TABLE project_state ADD COLUMN active_data_tab_id TEXT",
    },
    {
      table: "ai_messages",
      column: "dashboard_id",
      sql: "ALTER TABLE ai_messages ADD COLUMN dashboard_id TEXT",
    },
    {
      table: "project_state",
      column: "connection_order",
      sql: "ALTER TABLE project_state ADD COLUMN connection_order TEXT NOT NULL DEFAULT '[]'",
    },
  ];

  // Fetch column info once per table for all migrations below
  const tableNames = new Set(columnUpgrades.map((u) => u.table));
  tableNames.add("saved_queries");
  tableNames.add("dashboards");
  tableNames.add("project_state");
  const columnsByTable = new Map<string, Set<string>>();
  for (const table of tableNames) {
    const cols = await db.query<{ name: string }>(`PRAGMA table_info(${table})`);
    columnsByTable.set(table, new Set(cols.map((c) => c.name)));
  }

  // Column additions (ALTER TABLE doesn't support IF NOT EXISTS)
  for (const upgrade of columnUpgrades) {
    if (!columnsByTable.get(upgrade.table)!.has(upgrade.column)) {
      await db.execute(upgrade.sql);
    }
  }

  // Migrate saved_queries from connection_id to project_id
  const sqCols = columnsByTable.get("saved_queries")!;
  if (sqCols.has("connection_id") && !sqCols.has("project_id")) {
    await db.execute("ALTER TABLE saved_queries ADD COLUMN project_id TEXT");
    await db.execute(
      `UPDATE saved_queries SET project_id = (
        SELECT project_id FROM connections WHERE connections.id = saved_queries.connection_id
      ) WHERE project_id IS NULL`,
    );
    // Delete orphans where connection no longer exists
    await db.execute("DELETE FROM saved_queries WHERE project_id IS NULL");
  }
  // Drop legacy connection_id column and its index (now project-level)
  if (sqCols.has("connection_id")) {
    await db.execute("DROP INDEX IF EXISTS idx_saved_queries_connection");
    await db.execute("ALTER TABLE saved_queries DROP COLUMN connection_id");
  }

  // Migrate dashboards from connection_id to project_id
  const dbCols = columnsByTable.get("dashboards")!;
  if (dbCols.has("connection_id") && !dbCols.has("project_id")) {
    await db.execute("ALTER TABLE dashboards ADD COLUMN project_id TEXT");
    await db.execute(
      `UPDATE dashboards SET project_id = (
        SELECT project_id FROM connections WHERE connections.id = dashboards.connection_id
      ) WHERE project_id IS NULL`,
    );
    // Delete orphans where connection no longer exists
    await db.execute("DELETE FROM dashboards WHERE project_id IS NULL");
  }
  // Drop legacy connection_id column and its index (now project-level)
  if (dbCols.has("connection_id")) {
    await db.execute("DROP INDEX IF EXISTS idx_dashboards_connection");
    await db.execute("ALTER TABLE dashboards DROP COLUMN connection_id");
  }

  // Rename active_canvas_tab_id → active_workflow_tab_id
  const psCols = columnsByTable.get("project_state")!;
  if (psCols.has("active_canvas_tab_id") && !psCols.has("active_workflow_tab_id")) {
    await db.execute(
      "ALTER TABLE project_state RENAME COLUMN active_canvas_tab_id TO active_workflow_tab_id",
    );
  }

  // Migrate active_view value "canvas" → "workflow"
  await db.execute(
    "UPDATE project_state SET active_view = 'workflow' WHERE active_view = 'canvas'",
  );

  // === DDL statements (creates new tables/indexes, safe to run repeatedly) ===
  await db.transaction(DDL_STATEMENTS.map((sql) => ({ sql })));
}

export { SCHEMA_VERSION };
