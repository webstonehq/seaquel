/**
 * Per-user `meta.db` connection pool for the web tenant container.
 *
 * Each authenticated user gets their own SQLite file at
 * `${DATA_DIR}/users/${user_id}/meta.db`. The file uses the **same schema**
 * as the desktop app (defined in `src/lib/storage/schema.ts`) — the
 * `SqliteDatabase` interface is the unification point and all 17 repos in
 * `src/lib/storage/repos/*` work unchanged on top of it.
 *
 * Pool semantics:
 * - Open lazily on first request from that user.
 * - Cache the `better-sqlite3` handle keyed by `user_id`.
 * - Evict oldest when the pool exceeds MAX_POOL_SIZE.
 *
 * The cap is set well above realistic concurrent-user counts for a single
 * tenant container: better-sqlite3 handles are cheap (a few KiB each + a
 * file descriptor), and an in-use connection being closed by LRU eviction
 * throws "statement still open". Sized so a tenant would have to exceed
 * thousands of distinct simultaneous users before the eviction path runs.
 * If a deployment ever approaches the cap, refcount + skip-busy-on-evict is
 * the next step — but for v1 this is the cheap fix.
 *
 * Tenant isolation is at the filesystem level — one user's repo code can
 * only ever touch their own file. Cross-user queries are structurally
 * impossible.
 */

import { createRequire } from "node:module";
import { mkdirSync } from "node:fs";
import type Database from "better-sqlite3";
import type { SqliteDatabase } from "$lib/storage/sqlite-types";
import { initializeSchema, CURRENT_STORAGE_VERSION } from "$lib/storage/schema";

const require = createRequire(import.meta.url);

interface PoolEntry {
  db: Database.Database;
  lastUsed: number;
}

const pool = new Map<string, PoolEntry>();
const MAX_POOL_SIZE = 4096;

function dataDir(): string {
  return process.env.DATA_DIR ?? process.cwd();
}

function userDbPath(userId: string): { dir: string; file: string } {
  // Keys come from Better Auth and are URL-safe by construction, but be
  // defensive — refuse anything that would let a caller escape their dir.
  if (!userId || userId.includes("/") || userId.includes("..") || userId.includes("\\")) {
    throw new Error(`refusing to use unsafe user id: ${JSON.stringify(userId)}`);
  }
  const dir = `${dataDir()}/users/${userId}`;
  return { dir, file: `${dir}/meta.db` };
}

function openRawSqlite(userId: string): Database.Database {
  const DatabaseCtor = require("better-sqlite3") as typeof Database;
  const { dir, file } = userDbPath(userId);
  mkdirSync(dir, { recursive: true });

  const db = new DatabaseCtor(file);
  db.pragma("journal_mode = WAL");
  db.pragma("busy_timeout = 5000");
  db.pragma("foreign_keys = ON");
  return db;
}

function evictIfNeeded(): void {
  if (pool.size < MAX_POOL_SIZE) return;
  let oldest: { key: string; lastUsed: number } | null = null;
  for (const [key, entry] of pool) {
    if (!oldest || entry.lastUsed < oldest.lastUsed) {
      oldest = { key, lastUsed: entry.lastUsed };
    }
  }
  if (oldest) {
    pool.get(oldest.key)?.db.close();
    pool.delete(oldest.key);
  }
}

/**
 * `SqliteDatabase` adapter over a synchronous `better-sqlite3` handle.
 *
 * The repos and `initializeSchema()` were written against the async
 * desktop/demo interface; this just wraps every call in `Promise.resolve`
 * so the same code paths run identically here.
 */
function adapter(db: Database.Database): SqliteDatabase {
  return {
    async execute(sql: string, params: unknown[] = []): Promise<number> {
      const info = db.prepare(sql).run(...(params as never[]));
      return info.changes;
    },

    async query<T = Record<string, unknown>>(sql: string, params: unknown[] = []): Promise<T[]> {
      const stmt = db.prepare(sql);
      // Use .raw(false) to get row objects keyed by column name, which is
      // the shape the desktop's TauriSqliteDatabase normalizes to.
      return stmt.all(...(params as never[])) as T[];
    },

    async transaction(statements: Array<{ sql: string; params?: unknown[] }>): Promise<void> {
      const tx = db.transaction((stmts: typeof statements) => {
        for (const { sql, params = [] } of stmts) {
          db.prepare(sql).run(...(params as never[]));
        }
      });
      tx(statements);
    },

    async close(): Promise<void> {
      // Pooled — never actually close from the adapter. The pool eviction
      // path or process shutdown closes the underlying handle.
    },
  };
}

/**
 * Open (or reuse) the user's meta.db, ensuring schema is initialized.
 */
export async function openUserDb(userId: string): Promise<Database.Database> {
  const cached = pool.get(userId);
  if (cached) {
    cached.lastUsed = Date.now();
    return cached.db;
  }

  const db = openRawSqlite(userId);

  // Schema bootstrap via the SqliteDatabase adapter — same code path the
  // desktop runs through `getDatabase()` in `$lib/storage/db.ts`.
  const a = adapter(db);
  const isFresh = await initializeSchema(a);
  if (isFresh) {
    db.prepare("INSERT INTO schema_version (version) VALUES (?)").run(CURRENT_STORAGE_VERSION);
  }

  evictIfNeeded();
  pool.set(userId, { db, lastUsed: Date.now() });
  return db;
}

/**
 * Get the SqliteDatabase-shaped adapter for a user, opening if necessary.
 * This is what the storage endpoints use.
 */
export async function userStorage(userId: string): Promise<SqliteDatabase> {
  const db = await openUserDb(userId);
  return adapter(db);
}

/** For testing / shutdown — closes every open handle. */
export function closeAllUserDbs(): void {
  for (const entry of pool.values()) {
    try {
      entry.db.close();
    } catch {
      // ignore — best-effort
    }
  }
  pool.clear();
}
