import { invoke } from "@tauri-apps/api/core";
import type { SqliteDatabase, SqliteProvider } from "./sqlite-types";
import { decodeRows, encodeParams } from "$lib/values";

interface DbQueryResult {
  columns: string[];
  rows: unknown[][];
}

interface DbExecuteResult {
  rows_affected: number;
  last_insert_id: number | null;
}

interface DbConnectResult {
  connection_id: string;
}

class TauriSqliteDatabase implements SqliteDatabase {
  // Serializes all write operations (execute + transaction) so they never
  // overlap. Reads (query) bypass the queue since SQLite WAL mode allows
  // concurrent readers.
  private writeQueue: Promise<void> = Promise.resolve();

  constructor(private connectionId: string) {}

  private rawExecute(sql: string, params?: unknown[]): Promise<DbExecuteResult> {
    return invoke<DbExecuteResult>("db_execute", {
      connectionId: this.connectionId,
      sql,
      values: encodeParams(params),
    });
  }

  private enqueueWrite<T>(fn: () => Promise<T>): Promise<T> {
    const prev = this.writeQueue;
    let resolve!: () => void;
    this.writeQueue = new Promise<void>((r) => {
      resolve = r;
    });
    const result = prev.then(fn);
    // Release the queue slot whether fn succeeds or fails
    result.then(resolve, resolve);
    return result;
  }

  async execute(sql: string, params?: unknown[]): Promise<number> {
    return this.enqueueWrite(async () => {
      const result = await this.rawExecute(sql, params);
      return result.rows_affected;
    });
  }

  async query<T = Record<string, unknown>>(sql: string, params?: unknown[]): Promise<T[]> {
    const result = await invoke<DbQueryResult>("db_query", {
      connectionId: this.connectionId,
      sql,
      values: encodeParams(params),
    });
    // App storage never produces tagged values today, but decode anyway so a
    // BIGINT or BLOB column can't leak a raw tag into the app.
    // Convert columnar → row objects for backward compatibility
    return decodeRows(result.rows).map((row) => {
      const obj: Record<string, unknown> = {};
      result.columns.forEach((col, i) => {
        obj[col] = row[i];
      });
      return obj as T;
    });
  }

  async transaction(statements: Array<{ sql: string; params?: unknown[] }>): Promise<void> {
    return this.enqueueWrite(async () => {
      await invoke("db_transaction", {
        connectionId: this.connectionId,
        statements: statements.map((s) => ({ sql: s.sql, params: encodeParams(s.params) })),
      });
    });
  }

  async close(): Promise<void> {
    await invoke("db_disconnect", { connectionId: this.connectionId });
  }
}

export class TauriSqliteProvider implements SqliteProvider {
  readonly id = "tauri-sqlite";

  isAvailable(): boolean {
    return typeof window !== "undefined" && "__TAURI__" in window;
  }

  async open(path: string): Promise<SqliteDatabase> {
    const result = await invoke<DbConnectResult>("db_connect", {
      config: {
        driver: "sqlite",
        connection_string: `sqlite:${path}`,
        create_if_missing: true,
      },
    });
    return new TauriSqliteDatabase(result.connection_id);
  }
}
