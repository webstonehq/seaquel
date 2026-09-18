import type { SqliteDatabase, SqliteProvider } from "./sqlite-types";

/**
 * Browser-side `SqliteDatabase` implementation for the web tenant build.
 *
 * Sends each `execute` / `query` / `transaction` call as a `POST` to the
 * SvelteKit server, which dispatches to the authenticated user's per-user
 * `meta.db` (see `src/lib/server/storage.ts`). The wire shapes match
 * `TauriSqliteDatabase`'s exactly so all 17 repos in `src/lib/storage/repos/*`
 * work unchanged on top of this provider.
 *
 * No bundled persistence — server owns the file. Calling `close()` is a
 * no-op because the server pools handles across requests.
 */

class HttpSqliteDatabase implements SqliteDatabase {
  // Serialize writes so transactions can't overlap with executes from the
  // same tab — same discipline as TauriSqliteDatabase. SQLite WAL on the
  // server allows concurrent reads, so query() bypasses the queue.
  private writeQueue: Promise<void> = Promise.resolve();

  constructor(private readonly baseUrl: string) {}

  private enqueueWrite<T>(fn: () => Promise<T>): Promise<T> {
    const prev = this.writeQueue;
    let release!: () => void;
    this.writeQueue = new Promise<void>((r) => {
      release = r;
    });
    const result = prev.then(fn);
    result.then(release, release);
    return result;
  }

  private async post<T>(path: string, body: unknown): Promise<T> {
    const res = await fetch(this.baseUrl + path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
      credentials: "same-origin",
    });
    if (!res.ok) {
      const text = await res.text();
      let message = text;
      try {
        message = (JSON.parse(text) as { message?: string }).message ?? text;
      } catch {
        // not JSON — keep raw text
      }
      throw new Error(`${path} failed (${res.status}): ${message}`);
    }
    // Success body is expected to be JSON. If the server returns malformed
    // JSON we previously surfaced only the parser's terse `Unexpected token`
    // — useless for debugging protocol drift. Include the status and a
    // truncated snippet of the raw body so the failure points at the wire.
    const text = await res.text();
    try {
      return JSON.parse(text) as T;
    } catch (e) {
      const snippet = text.length > 200 ? `${text.slice(0, 200)}…` : text;
      const cause = e instanceof Error ? e.message : String(e);
      throw new Error(
        `${path} returned non-JSON body (${res.status}): ${cause}; body: ${JSON.stringify(snippet)}`,
      );
    }
  }

  async execute(sql: string, params?: unknown[]): Promise<number> {
    return this.enqueueWrite(() =>
      this.post<number>("/api/storage/exec", { sql, params: params ?? [] }),
    );
  }

  async query<T = Record<string, unknown>>(sql: string, params?: unknown[]): Promise<T[]> {
    return this.post<T[]>("/api/storage/query", { sql, params: params ?? [] });
  }

  async transaction(statements: Array<{ sql: string; params?: unknown[] }>): Promise<void> {
    await this.enqueueWrite(() =>
      this.post<null>("/api/storage/transaction", {
        statements: statements.map((s) => ({ sql: s.sql, params: s.params ?? [] })),
      }),
    );
  }

  async close(): Promise<void> {
    // No-op — the server's `userStorage()` pool owns lifecycle. Closing the
    // server-side handle on every browser navigation would be wasteful.
  }
}

export class HttpSqliteProvider implements SqliteProvider {
  readonly id = "http-sqlite";

  isAvailable(): boolean {
    return typeof window !== "undefined" && !("__TAURI__" in window);
  }

  async open(_path: string): Promise<SqliteDatabase> {
    // The path is ignored — the server already knows which user is calling
    // (from the session cookie) and routes to that user's meta.db. Same
    // signature as the desktop/demo providers so `getDatabase()` in
    // `db.ts` stays uniform.
    return new HttpSqliteDatabase("");
  }
}
