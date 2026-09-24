# Generic Repository Factory Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate repeated row-mapping, upsert SQL generation, and CRUD boilerplate in `src/lib/storage/repository.ts` (1,374 lines → ~750 lines) by extracting a declarative `createRepo` factory.

**Architecture:** A `createRepo<T>()` factory takes a table name, ID column, and a column-mapping spec. It generates `loadAll`, `loadBy`, `save` (upsert), `saveAll`, `remove`, and `removeBy` methods, plus exposes `mapRow` and `toParams` for repos that need custom SQL. Repos that need extra methods (child entity handling, custom prune logic) spread the factory output and add/override.

**Tech Stack:** TypeScript, `SqliteDatabase` interface (query/execute)

---

### Task 1: Create the `createRepo` factory module

**Files:**
- Create: `src/lib/storage/create-repo.ts`

**Step 1: Write `create-repo.ts`**

```typescript
import type { SqliteDatabase } from "./sqlite-types";

// ── Shared utility ──────────────────────────────────────────────────────

export function safeJsonParse<T>(json: string | null | undefined, fallback: T): T {
  if (!json) return fallback;
  try {
    return JSON.parse(json);
  } catch {
    return fallback;
  }
}

// ── Column transform helpers ────────────────────────────────────────────

/** null in DB → undefined in model, undefined in model → null in DB */
export function nullable(dbColumn: string): ColumnDef {
  return { column: dbColumn, fromDb: (v) => v ?? undefined, toDb: (v) => v ?? null };
}

/** integer 0/1 in DB ↔ boolean in model */
export function bool(dbColumn: string): ColumnDef {
  return {
    column: dbColumn,
    fromDb: (v) => (v as number) === 1,
    toDb: (v) => ((v as boolean) ? 1 : 0),
  };
}

/** Optional boolean: null in DB → undefined, 0/1 → false/true */
export function optBool(dbColumn: string): ColumnDef {
  return {
    column: dbColumn,
    fromDb: (v) => (v === null || v === undefined ? undefined : Boolean(v)),
    toDb: (v) => (v === undefined ? null : (v as boolean) ? 1 : 0),
  };
}

/** JSON string in DB ↔ parsed value in model */
export function json<T>(dbColumn: string, fallback: T): ColumnDef {
  return {
    column: dbColumn,
    fromDb: (v) => safeJsonParse(v as string, fallback),
    toDb: (v) => (v != null ? JSON.stringify(v) : null),
  };
}

/** Direct column mapping — DB value passes through unchanged */
export function col(dbColumn: string): ColumnDef {
  return { column: dbColumn };
}

// ── Types ───────────────────────────────────────────────────────────────

export interface ColumnDef {
  column: string;
  fromDb?: (value: unknown) => unknown;
  toDb?: (value: unknown) => unknown;
}

export interface RepoSchema<T> {
  table: string;
  id: string; // DB column name used as primary key for upsert ON CONFLICT
  columns: { [K in keyof T]: ColumnDef };
}

export interface Repo<T> {
  loadAll(db: SqliteDatabase): Promise<T[]>;
  loadBy(db: SqliteDatabase, column: string, value: unknown): Promise<T[]>;
  loadOneBy(db: SqliteDatabase, column: string, value: unknown): Promise<T | null>;
  save(db: SqliteDatabase, model: T): Promise<void>;
  saveAll(db: SqliteDatabase, models: T[]): Promise<void>;
  remove(db: SqliteDatabase, id: unknown): Promise<void>;
  removeBy(db: SqliteDatabase, column: string, value: unknown): Promise<void>;
  mapRow(row: Record<string, unknown>): T;
  toParams(model: T): unknown[];
  upsertSql: string;
  insertSql: string;
}

// ── Factory ─────────────────────────────────────────────────────────────

export function createRepo<T>(schema: RepoSchema<T>): Repo<T> {
  const entries = Object.entries(schema.columns) as [string, ColumnDef][];
  const { table, id: idCol } = schema;

  // Pre-compute SQL fragments
  const dbCols = entries.map(([, def]) => def.column);
  const colList = dbCols.join(", ");
  const placeholders = dbCols.map(() => "?").join(", ");
  const updateSet = dbCols
    .filter((c) => c !== idCol)
    .map((c) => `${c} = excluded.${c}`)
    .join(", ");

  const upsertSql = `INSERT INTO ${table} (${colList}) VALUES (${placeholders}) ON CONFLICT(${idCol}) DO UPDATE SET ${updateSet}`;
  const insertSql = `INSERT INTO ${table} (${colList}) VALUES (${placeholders})`;

  function mapRow(row: Record<string, unknown>): T {
    const result: Record<string, unknown> = {};
    for (const [modelKey, def] of entries) {
      const raw = row[def.column];
      result[modelKey] = def.fromDb ? def.fromDb(raw) : raw;
    }
    return result as T;
  }

  function toParams(model: T): unknown[] {
    return entries.map(([modelKey, def]) => {
      const raw = (model as Record<string, unknown>)[modelKey];
      return def.toDb ? def.toDb(raw) : raw;
    });
  }

  return {
    mapRow,
    toParams,
    upsertSql,
    insertSql,

    async loadAll(db) {
      const rows = await db.query<Record<string, unknown>>(`SELECT * FROM ${table}`);
      return rows.map(mapRow);
    },

    async loadBy(db, column, value) {
      const rows = await db.query<Record<string, unknown>>(
        `SELECT * FROM ${table} WHERE ${column} = ?`,
        [value],
      );
      return rows.map(mapRow);
    },

    async loadOneBy(db, column, value) {
      const rows = await db.query<Record<string, unknown>>(
        `SELECT * FROM ${table} WHERE ${column} = ?`,
        [value],
      );
      return rows.length > 0 ? mapRow(rows[0]) : null;
    },

    async save(db, model) {
      await db.execute(upsertSql, toParams(model));
    },

    async saveAll(db, models) {
      for (const model of models) {
        await db.execute(upsertSql, toParams(model));
      }
    },

    async remove(db, id) {
      await db.execute(`DELETE FROM ${table} WHERE ${idCol} = ?`, [id]);
    },

    async removeBy(db, column, value) {
      await db.execute(`DELETE FROM ${table} WHERE ${column} = ?`, [value]);
    },
  };
}
```

**Step 2: Verify it compiles**

Run: `npm run check`
Expected: No new errors

**Step 3: Commit**

```
feat: add createRepo factory for declarative repository definitions
```

---

### Task 2: Refactor `dashboardsRepo` (clean fit — 68 lines → ~20)

This is the cleanest candidate: standard loadByProject, save (upsert), remove, removeByProject.

**Files:**
- Modify: `src/lib/storage/repository.ts` — replace `dashboardsRepo` definition (lines 1028-1095)

**Step 1: Replace dashboardsRepo**

Replace the entire `dashboardsRepo` block (from `export const dashboardsRepo = {` through the closing `};`) with:

```typescript
import { createRepo, col, nullable, bool } from "./create-repo";

const dashboardRepo = createRepo<PersistedDashboard>({
  table: "dashboards",
  id: "id",
  columns: {
    id: col("id"),
    projectId: col("project_id"),
    name: col("name"),
    viewport: col("viewport"),
    widgets: col("widgets"),
    dateFilter: nullable("date_filter"),
    starred: {
      column: "starred",
      fromDb: (v) => ((v ?? 0) as number) === 1,
      toDb: (v) => ((v as boolean) ? 1 : 0),
    },
    shared: bool("shared"),
    description: nullable("description"),
    createdAt: col("created_at"),
    updatedAt: col("updated_at"),
  },
});

export const dashboardsRepo = {
  loadByProject: (db: SqliteDatabase, projectId: string) =>
    dashboardRepo.loadBy(db, "project_id", projectId),
  save: (db: SqliteDatabase, dashboard: PersistedDashboard) =>
    dashboardRepo.save(db, dashboard),
  remove: (db: SqliteDatabase, id: string) =>
    dashboardRepo.remove(db, id),
  removeByProject: (db: SqliteDatabase, projectId: string) =>
    dashboardRepo.removeBy(db, "project_id", projectId),
};
```

**Step 2: Verify it compiles**

Run: `npm run check`
Expected: No new errors — all call sites (`dashboard-manager.svelte.ts`, `persistence-manager.svelte.ts`) use `.loadByProject()`, `.save()`, `.remove()`, `.removeByProject()`.

**Step 3: Commit**

```
refactor: convert dashboardsRepo to use createRepo factory
```

---

### Task 3: Refactor `connectionOverridesRepo` (clean fit — 81 lines → ~25)

**Files:**
- Modify: `src/lib/storage/repository.ts` — replace `connectionOverridesRepo` (lines 1186-1266)

**Step 1: Replace connectionOverridesRepo**

```typescript
const overrideRepo = createRepo<PersistedConnectionOverride>({
  table: "connection_overrides",
  id: "shared_connection_id",
  columns: {
    sharedConnectionId: col("shared_connection_id"),
    username: nullable("username"),
    hostOverride: nullable("host_override"),
    portOverride: nullable("port_override"),
    savePassword: bool("save_password"),
    saveSshPassword: bool("save_ssh_password"),
    saveSshKeyPassphrase: bool("save_ssh_key_passphrase"),
  },
});

export const connectionOverridesRepo = {
  load: (db: SqliteDatabase, sharedConnectionId: string) =>
    overrideRepo.loadOneBy(db, "shared_connection_id", sharedConnectionId),
  loadAll: (db: SqliteDatabase) => overrideRepo.loadAll(db),
  save: (db: SqliteDatabase, override: PersistedConnectionOverride) =>
    overrideRepo.save(db, override),
  remove: (db: SqliteDatabase, sharedConnectionId: string) =>
    overrideRepo.remove(db, sharedConnectionId),
};
```

**Step 2: Verify**

Run: `npm run check`

**Step 3: Commit**

```
refactor: convert connectionOverridesRepo to use createRepo factory
```

---

### Task 4: Refactor `aiChatsRepo` (two entities — 71 lines → ~35)

**Files:**
- Modify: `src/lib/storage/repository.ts` — replace `aiChatsRepo` (lines 1304-1374)

**Step 1: Replace aiChatsRepo**

```typescript
const chatRepo = createRepo<PersistedAIChat>({
  table: "ai_chats",
  id: "id",
  columns: {
    id: col("id"),
    connectionId: col("connection_id"),
    title: col("title"),
    createdAt: col("created_at"),
    updatedAt: col("updated_at"),
  },
});

const messageRepo = createRepo<PersistedAIMessage>({
  table: "ai_messages",
  id: "id",
  columns: {
    id: col("id"),
    chatId: col("chat_id"),
    role: col("role"),
    content: col("content"),
    timestamp: col("timestamp"),
    query: nullable("query"),
  },
});

export const aiChatsRepo = {
  loadByConnection: (db: SqliteDatabase, connectionId: string) =>
    chatRepo.loadBy(db, "connection_id", connectionId),
  saveChat: (db: SqliteDatabase, chat: PersistedAIChat) => chatRepo.save(db, chat),
  removeChat: (db: SqliteDatabase, chatId: string) => chatRepo.remove(db, chatId),
  removeByConnection: (db: SqliteDatabase, connectionId: string) =>
    chatRepo.removeBy(db, "connection_id", connectionId),

  async loadMessages(db: SqliteDatabase, chatId: string): Promise<PersistedAIMessage[]> {
    const rows = await db.query<Record<string, unknown>>(
      "SELECT * FROM ai_messages WHERE chat_id = ? ORDER BY timestamp ASC",
      [chatId],
    );
    return rows.map(messageRepo.mapRow);
  },

  async replaceAllMessages(
    db: SqliteDatabase,
    chatId: string,
    messages: PersistedAIMessage[],
  ): Promise<void> {
    await db.execute("DELETE FROM ai_messages WHERE chat_id = ?", [chatId]);
    for (const m of messages) {
      await db.execute(messageRepo.insertSql, messageRepo.toParams(m));
    }
  },
};
```

Note: `loadMessages` uses custom ORDER BY. `replaceAllMessages` uses `insertSql` (not upsert) since it deletes first.

**Step 2: Verify**

Run: `npm run check`

**Step 3: Commit**

```
refactor: convert aiChatsRepo to use createRepo factory
```

---

### Task 5: Refactor `connectionsRepo` (big upsert — 137 lines → ~50)

The main win here is the 21-column upsert. Child label handling stays custom.

**Files:**
- Modify: `src/lib/storage/repository.ts` — replace `connectionsRepo` (lines 127-263)

**Step 1: Replace connectionsRepo**

```typescript
const connectionRepo = createRepo<PersistedConnection>({
  table: "connections",
  id: "id",
  columns: {
    id: col("id"),
    projectId: col("project_id"),
    name: col("name"),
    type: col("type"),
    host: col("host"),
    port: col("port"),
    databaseName: col("database_name"),
    username: col("username"),
    sslMode: nullable("ssl_mode"),
    connectionString: nullable("connection_string"),
    lastConnected: {
      column: "last_connected",
      fromDb: (v) => (v ? new Date(v as string) : undefined),
      toDb: (v) =>
        v instanceof Date ? v.toISOString() : ((v as string | undefined) ?? null),
    },
    sshTunnel: {
      column: "ssh_tunnel",
      fromDb: (v) => safeJsonParse(v as string, undefined),
      toDb: (v) => (v ? JSON.stringify(v) : null),
    },
    savePassword: bool("save_password"),
    saveSshPassword: bool("save_ssh_password"),
    saveSshKeyPassphrase: bool("save_ssh_key_passphrase"),
    isLocalOnly: {
      column: "is_local_only",
      fromDb: (v) => ((v as number) === 1 ? true : undefined),
      toDb: (v) => ((v as boolean) ? 1 : 0),
    },
    sharedConnectionId: nullable("shared_connection_id"),
    aiShareSchema: optBool("ai_share_schema"),
    aiShareData: optBool("ai_share_data"),
    activeAIProviderId: nullable("active_ai_provider_id"),
    activeAIModel: nullable("active_ai_model"),
    // labelIds is not a DB column — handled separately
    labelIds: { column: "_label_ids", fromDb: () => [], toDb: () => null },
  },
});

export const connectionsRepo = {
  async loadAll(db: SqliteDatabase): Promise<PersistedConnection[]> {
    const rows = await db.query<Record<string, unknown>>("SELECT * FROM connections");
    const connections: PersistedConnection[] = [];
    for (const row of rows) {
      const conn = connectionRepo.mapRow(row);
      const labelRows = await db.query<{ label_id: string }>(
        "SELECT label_id FROM connection_labels WHERE connection_id = ?",
        [conn.id],
      );
      conn.labelIds = labelRows.map((l) => l.label_id);
      connections.push(conn);
    }
    return connections;
  },

  async save(db: SqliteDatabase, conn: PersistedConnection): Promise<void> {
    await connectionRepo.save(db, conn);
    // Replace labels
    await db.execute("DELETE FROM connection_labels WHERE connection_id = ?", [conn.id]);
    for (const labelId of conn.labelIds) {
      await db.execute(
        "INSERT INTO connection_labels (connection_id, label_id) VALUES (?, ?)",
        [conn.id, labelId],
      );
    }
  },

  async remove(db: SqliteDatabase, connectionId: string): Promise<void> {
    await db.execute("DELETE FROM connections WHERE id = ?", [connectionId]);
  },
};
```

**Important:** The `labelIds` field is a virtual column — it doesn't exist in the `connections` table. The `_label_ids` placeholder column in the schema is never selected (since `loadAll` builds its own query). The `toDb: () => null` means the upsert writes `null` to a non-existent column — this will cause an error.

**Alternative approach:** Exclude `labelIds` from the schema entirely. Instead, define the schema with only the real DB columns, and handle `labelIds` in `loadAll`/`save` manually. The `PersistedConnection` type includes `labelIds`, so we can't use `createRepo<PersistedConnection>` directly if the schema must match all keys.

**Better approach — use Omit:**

Define the connection repo without `labelIds`:

```typescript
type ConnectionDbFields = Omit<PersistedConnection, "labelIds">;

const connectionRepo = createRepo<ConnectionDbFields>({
  table: "connections",
  id: "id",
  columns: {
    // ... all columns except labelIds
  },
});

export const connectionsRepo = {
  async loadAll(db: SqliteDatabase): Promise<PersistedConnection[]> {
    const rows = await db.query<Record<string, unknown>>("SELECT * FROM connections");
    const connections: PersistedConnection[] = [];
    for (const row of rows) {
      const base = connectionRepo.mapRow(row);
      const labelRows = await db.query<{ label_id: string }>(
        "SELECT label_id FROM connection_labels WHERE connection_id = ?",
        [base.id],
      );
      connections.push({ ...base, labelIds: labelRows.map((l) => l.label_id) });
    }
    return connections;
  },

  async save(db: SqliteDatabase, conn: PersistedConnection): Promise<void> {
    const { labelIds, ...dbFields } = conn;
    await connectionRepo.save(db, dbFields);
    await db.execute("DELETE FROM connection_labels WHERE connection_id = ?", [conn.id]);
    for (const labelId of labelIds) {
      await db.execute(
        "INSERT INTO connection_labels (connection_id, label_id) VALUES (?, ?)",
        [conn.id, labelId],
      );
    }
  },

  async remove(db: SqliteDatabase, connectionId: string): Promise<void> {
    await connectionRepo.remove(db, connectionId);
  },
};
```

**Step 2: Verify**

Run: `npm run check`

**Step 3: Commit**

```
refactor: convert connectionsRepo to use createRepo factory
```

---

### Task 6: Refactor `projectsRepo` (with child labels — 80 lines → ~35)

Same pattern as connections — base fields via factory, child labels handled manually.

**Files:**
- Modify: `src/lib/storage/repository.ts` — replace `projectsRepo` (lines 27-106)

**Step 1: Replace projectsRepo**

```typescript
type ProjectDbFields = Omit<PersistedProject, "customLabels">;

const projectRepo = createRepo<ProjectDbFields>({
  table: "projects",
  id: "id",
  columns: {
    id: col("id"),
    name: col("name"),
    description: nullable("description"),
    createdAt: col("created_at"),
    updatedAt: col("updated_at"),
    gitRepoPath: nullable("git_repo_path"),
  },
});

export const projectsRepo = {
  async loadAll(db: SqliteDatabase): Promise<PersistedProject[]> {
    const rows = await db.query<Record<string, unknown>>("SELECT * FROM projects");
    const projects: PersistedProject[] = [];
    for (const row of rows) {
      const base = projectRepo.mapRow(row);
      const labels = await db.query<{
        id: string;
        name: string;
        is_predefined: number;
        color: string;
      }>("SELECT id, name, is_predefined, color FROM project_labels WHERE project_id = ?", [
        base.id,
      ]);
      projects.push({
        ...base,
        customLabels: labels.map((l) => ({
          id: l.id,
          name: l.name,
          isPredefined: l.is_predefined === 1,
          color: l.color,
        })),
      });
    }
    return projects;
  },

  async save(db: SqliteDatabase, project: PersistedProject): Promise<void> {
    const { customLabels, ...dbFields } = project;
    await projectRepo.save(db, dbFields);
    await db.execute("DELETE FROM project_labels WHERE project_id = ?", [project.id]);
    for (const label of customLabels) {
      await db.execute(
        `INSERT INTO project_labels (id, project_id, name, is_predefined, color) VALUES (?, ?, ?, ?, ?)`,
        [label.id, project.id, label.name, label.isPredefined ? 1 : 0, label.color],
      );
    }
  },

  async saveAll(db: SqliteDatabase, projects: PersistedProject[]): Promise<void> {
    for (const project of projects) {
      await this.save(db, project);
    }
  },

  async remove(db: SqliteDatabase, projectId: string): Promise<void> {
    await projectRepo.remove(db, projectId);
  },
};
```

**Step 2: Verify**

Run: `npm run check`

**Step 3: Commit**

```
refactor: convert projectsRepo to use createRepo factory
```

---

### Task 7: Refactor `savedQueriesRepo` (custom saveAll — 90 lines → ~40)

**Files:**
- Modify: `src/lib/storage/repository.ts` — replace `savedQueriesRepo` (lines 614-703)

**Step 1: Replace savedQueriesRepo**

```typescript
const savedQueryRepo = createRepo<PersistedSavedQuery>({
  table: "saved_queries",
  id: "id",
  columns: {
    id: col("id"),
    projectId: col("project_id"),
    name: col("name"),
    query: col("query"),
    parameters: {
      column: "parameters",
      fromDb: (v) => safeJsonParse(v as string, undefined),
      toDb: (v) => (v ? JSON.stringify(v) : null),
    },
    starred: bool("starred"),
    shared: bool("shared"),
    description: nullable("description"),
    databaseType: nullable("database_type"),
    tags: {
      column: "tags",
      fromDb: (v) => safeJsonParse(v as string, undefined),
      toDb: (v) => (v ? JSON.stringify(v) : null),
    },
    folder: nullable("folder"),
    createdAt: col("created_at"),
    updatedAt: col("updated_at"),
  },
});

export const savedQueriesRepo = {
  loadByProject: (db: SqliteDatabase, projectId: string) =>
    savedQueryRepo.loadBy(db, "project_id", projectId),

  async saveAll(
    db: SqliteDatabase,
    projectId: string,
    queries: PersistedSavedQuery[],
  ): Promise<void> {
    // Delete removed queries (but preserve CASCADE children like query_versions)
    const currentIds = queries.map((q) => q.id);
    if (currentIds.length > 0) {
      const placeholders = currentIds.map(() => "?").join(",");
      await db.execute(
        `DELETE FROM saved_queries WHERE project_id = ? AND id NOT IN (${placeholders})`,
        [projectId, ...currentIds],
      );
    } else {
      await db.execute("DELETE FROM saved_queries WHERE project_id = ?", [projectId]);
    }
    for (const q of queries) {
      await savedQueryRepo.save(db, q);
    }
  },

  removeByProject: (db: SqliteDatabase, projectId: string) =>
    savedQueryRepo.removeBy(db, "project_id", projectId),
};
```

**Step 2: Verify**

Run: `npm run check`

**Step 3: Commit**

```
refactor: convert savedQueriesRepo to use createRepo factory
```

---

### Task 8: Refactor `queryHistoryRepo` (replaceAll pattern — 63 lines → ~25)

**Files:**
- Modify: `src/lib/storage/repository.ts` — replace `queryHistoryRepo` (lines 808-870)

**Step 1: Replace queryHistoryRepo**

```typescript
const historyRepo = createRepo<PersistedQueryHistoryItem>({
  table: "query_history",
  id: "id",
  columns: {
    id: col("id"),
    connectionId: col("connection_id"),
    query: col("query"),
    timestamp: col("timestamp"),
    executionTime: col("execution_time"),
    rowCount: col("row_count"),
    favorite: bool("favorite"),
    connectionLabelsSnapshot: json("connection_labels_snapshot", []),
    connectionNameSnapshot: col("connection_name_snapshot"),
  },
});

export const queryHistoryRepo = {
  async loadByConnection(
    db: SqliteDatabase,
    connectionId: string,
  ): Promise<PersistedQueryHistoryItem[]> {
    const rows = await db.query<Record<string, unknown>>(
      "SELECT * FROM query_history WHERE connection_id = ? ORDER BY timestamp DESC",
      [connectionId],
    );
    return rows.map(historyRepo.mapRow);
  },

  async replaceAll(
    db: SqliteDatabase,
    connectionId: string,
    items: PersistedQueryHistoryItem[],
  ): Promise<void> {
    await db.execute("DELETE FROM query_history WHERE connection_id = ?", [connectionId]);
    for (const h of items) {
      await db.execute(historyRepo.insertSql, historyRepo.toParams(h));
    }
  },

  removeByConnection: (db: SqliteDatabase, connectionId: string) =>
    historyRepo.removeBy(db, "connection_id", connectionId),
};
```

**Step 2: Verify**

Run: `npm run check`

**Step 3: Commit**

```
refactor: convert queryHistoryRepo to use createRepo factory
```

---

### Task 9: Refactor `queryVersionsRepo` and `dashboardVersionsRepo` (insert + custom prune — 148 lines → ~70)

**Files:**
- Modify: `src/lib/storage/repository.ts` — replace both version repos (lines 707-804 and 1099-1172)

**Step 1: Replace queryVersionsRepo**

```typescript
const queryVersionRepo = createRepo<PersistedQueryVersion>({
  table: "query_versions",
  id: "id",
  columns: {
    id: col("id"),
    queryId: col("saved_query_id"),
    version: col("version"),
    snapshot: col("snapshot"),
    diff: col("diff"),
    createdAt: col("created_at"),
  },
});

export const queryVersionsRepo = {
  async loadByQuery(db: SqliteDatabase, queryId: string): Promise<PersistedQueryVersion[]> {
    const rows = await db.query<Record<string, unknown>>(
      "SELECT * FROM query_versions WHERE saved_query_id = ? ORDER BY version ASC",
      [queryId],
    );
    return rows.map(queryVersionRepo.mapRow);
  },

  async loadByProject(db: SqliteDatabase, projectId: string): Promise<PersistedQueryVersion[]> {
    const rows = await db.query<Record<string, unknown>>(
      `SELECT qv.* FROM query_versions qv
       JOIN saved_queries sq ON sq.id = qv.saved_query_id
       WHERE sq.project_id = ?
       ORDER BY qv.saved_query_id, qv.version ASC`,
      [projectId],
    );
    return rows.map(queryVersionRepo.mapRow);
  },

  async insert(db: SqliteDatabase, version: PersistedQueryVersion): Promise<void> {
    await db.execute(queryVersionRepo.insertSql, queryVersionRepo.toParams(version));
  },

  // pruneOldVersions stays as-is — it has complex logic with resolveVersions
  async pruneOldVersions(db: SqliteDatabase, queryId: string, keepCount: number): Promise<void> {
    const allVersions = await this.loadByQuery(db, queryId);
    if (allVersions.length <= keepCount) return;

    const sorted = [...allVersions].sort((a, b) => b.version - a.version);
    const cutoffVersion = sorted[keepCount - 1]?.version;
    if (cutoffVersion === undefined) return;

    const { resolveVersions } = await import("$lib/utils/query-versions");
    const resolved = resolveVersions(
      allVersions.map((v) => ({ ...v, createdAt: new Date(v.createdAt) })),
    );

    const oldestSurvivor = sorted[keepCount - 1];
    const resolvedSurvivor = resolved.find((r) => r.id === oldestSurvivor.id);

    await db.execute(
      `DELETE FROM query_versions WHERE saved_query_id = ? AND version < ?`,
      [queryId, cutoffVersion],
    );

    if (oldestSurvivor.snapshot === null && resolvedSurvivor) {
      await db.execute(`UPDATE query_versions SET snapshot = ?, diff = NULL WHERE id = ?`, [
        resolvedSurvivor.query,
        oldestSurvivor.id,
      ]);
    }
  },
};
```

**Step 2: Replace dashboardVersionsRepo**

```typescript
const dashboardVersionRepo = createRepo<PersistedDashboardVersion>({
  table: "dashboard_versions",
  id: "id",
  columns: {
    id: col("id"),
    dashboardId: col("dashboard_id"),
    version: col("version"),
    snapshot: col("snapshot"),
    createdAt: col("created_at"),
  },
});

export const dashboardVersionsRepo = {
  async loadByDashboard(
    db: SqliteDatabase,
    dashboardId: string,
  ): Promise<PersistedDashboardVersion[]> {
    const rows = await db.query<Record<string, unknown>>(
      "SELECT * FROM dashboard_versions WHERE dashboard_id = ? ORDER BY version ASC",
      [dashboardId],
    );
    return rows.map(dashboardVersionRepo.mapRow);
  },

  async loadByProject(
    db: SqliteDatabase,
    projectId: string,
  ): Promise<PersistedDashboardVersion[]> {
    const rows = await db.query<Record<string, unknown>>(
      `SELECT dv.* FROM dashboard_versions dv
       JOIN dashboards d ON d.id = dv.dashboard_id
       WHERE d.project_id = ?
       ORDER BY dv.dashboard_id, dv.version ASC`,
      [projectId],
    );
    return rows.map(dashboardVersionRepo.mapRow);
  },

  async insert(db: SqliteDatabase, version: PersistedDashboardVersion): Promise<void> {
    await db.execute(dashboardVersionRepo.insertSql, dashboardVersionRepo.toParams(version));
  },

  async pruneOldVersions(
    db: SqliteDatabase,
    dashboardId: string,
    keepCount: number,
  ): Promise<void> {
    await db.execute(
      `DELETE FROM dashboard_versions
       WHERE dashboard_id = ?
         AND version <= (
           SELECT version FROM dashboard_versions
           WHERE dashboard_id = ?
           ORDER BY version DESC
           LIMIT 1 OFFSET ?
         )`,
      [dashboardId, dashboardId, keepCount],
    );
  },
};
```

**Step 3: Verify**

Run: `npm run check`

**Step 4: Commit**

```
refactor: convert version repos to use createRepo factory
```

---

### Task 10: Move `safeJsonParse` to `create-repo.ts` and update imports

Currently `safeJsonParse` is defined locally in `repository.ts`. After refactoring, both `repository.ts` and `create-repo.ts` need it. Move it to `create-repo.ts` and import from there.

**Files:**
- Modify: `src/lib/storage/repository.ts` — remove local `safeJsonParse`, import from `create-repo`
- Modify: `src/lib/storage/create-repo.ts` — already has it exported

**Step 1:** Remove `safeJsonParse` function definition from top of `repository.ts` (lines 16-23). Add import:

```typescript
import { createRepo, col, nullable, bool, optBool, json, safeJsonParse } from "./create-repo";
```

**Step 2: Verify**

Run: `npm run check`

**Step 3: Commit**

```
refactor: centralize safeJsonParse in create-repo module
```

---

### Task 11: Update `src/lib/storage/index.ts` exports

**Files:**
- Modify: `src/lib/storage/index.ts`

**Step 1:** Add re-exports for the factory (for potential use by other modules):

```typescript
export { createRepo, col, nullable, bool, optBool, json, safeJsonParse } from "./create-repo";
export type { ColumnDef, RepoSchema, Repo } from "./create-repo";
```

**Step 2: Verify**

Run: `npm run check`

**Step 3: Commit**

```
refactor: export createRepo utilities from storage barrel
```

---

### Repos left untouched (and why)

| Repo | Reason |
|------|--------|
| `appStateRepo` | Key-value pattern, 14 lines — no benefit |
| `projectStateRepo` | 344 lines of complex multi-table logic with optional columns, try-catch for migrations — fundamentally different pattern |
| `sharedReposRepo` | JSON blob storage, 28 lines — no benefit |
| `themeRepo` | Multiple sub-operations across tables, 41 lines |
| `licenseRepo` | Single-row JSON blob, 13 lines |
| `onboardingRepo` | Single-row JSON blob, 13 lines |
| `tutorialRepo` | Composite primary key, 30 lines |
| `importStateRepo` | Custom key (`source`), 31 lines |

---

### Summary

| Metric | Before | After |
|--------|--------|-------|
| `repository.ts` lines | 1,374 | ~750 |
| `create-repo.ts` lines | 0 | ~110 |
| **Net reduction** | — | **~500 lines** |
| Repos using factory | 0 | 9 |
| Column mappings defined once | 0 | 9 schemas |
| Upsert SQL hand-written | 9 | 0 |
