/**
 * Shared wire-format types and helpers for providers that speak to a Rust
 * `ConnectionManager` — either over Tauri IPC (UnifiedTauriProvider) or HTTP +
 * WebSocket (HttpProvider). The wire shapes match the Rust `seaquel_db` crate
 * directly, so anything defined here is ground truth for both transports.
 */

import type { ConnectionConfig } from "./types";

// -------- Wire types (match Rust `seaquel_db` / `seaquel-server`) --------

export interface DbError {
  message: string;
  code: string;
}

export interface DbConnectResult {
  connection_id: string;
}

export interface DbQueryResult {
  columns: string[];
  rows: unknown[][];
}

export interface DbExecuteResult {
  rows_affected: number;
  last_insert_id: number | null;
}

/**
 * Internally-tagged stream event. Matches Rust's
 * `#[serde(tag = "type", rename_all = "camelCase")] enum StreamEvent`.
 *
 * For `batch`, the inner `StreamBatch` fields are flattened onto the event,
 * so the wire shape is `{type:"batch", columns, rows, is_final}` rather than
 * `{type:"batch", data:{columns,...}}`.
 */
export type DbStreamEvent =
  | { type: "batch"; columns: string[] | null; rows: unknown[][]; is_final: boolean }
  | { type: "done" }
  | { type: "error"; message: string; code: string };

// -------- Error handling --------

export function isDbError(error: unknown): error is DbError {
  return typeof error === "object" && error !== null && "message" in error && "code" in error;
}

/** Normalize any thrown value into an `Error` with a `"CODE: message"` shape. */
export function formatError(error: unknown): Error {
  if (isDbError(error)) return new Error(`${error.code}: ${error.message}`);
  if (error instanceof Error) return error;
  if (typeof error === "string") return new Error(error);
  return new Error("An unknown error occurred");
}

/** True for the error a SQLite connect/test returns when the database file doesn't exist. */
export function isFileNotFoundError(message: string | null): boolean {
  return message?.startsWith("FILE_NOT_FOUND:") ?? false;
}

// -------- Stream-frame helpers --------

/**
 * Format a `type: "error"` stream frame into the `"CODE: message"` shape the
 * UI expects. Both providers (Tauri Channel + HTTP WebSocket) deliver the
 * same wire shape, but missing/non-string fields would otherwise surface to
 * the user as `"undefined: undefined"` — guard them centrally instead of in
 * each call site.
 */
export function formatStreamErrorFrame(frame: { code?: unknown; message?: unknown }): string {
  const code = typeof frame.code === "string" ? frame.code : "ERROR";
  const message = typeof frame.message === "string" ? frame.message : "unknown stream error";
  return `${code}: ${message}`;
}

/**
 * Format an unexpected (`type` not in the known set) stream frame. Returned
 * as the `error` field of the terminal result so the stream caller fails
 * loudly rather than hanging — a silent ignore here was a bug we fixed once
 * and don't want to reintroduce by drifting the two providers' handlers.
 */
export function formatUnknownStreamFrame(frame: unknown): string {
  const type = (frame as { type?: unknown })?.type;
  return `unexpected stream event: ${typeof type === "string" ? type : "<missing>"}`;
}

// -------- ConnectionConfig → Rust ConnectConfig translation --------

/**
 * Translate the frontend's `ConnectionConfig` into the shape the Rust
 * `ConnectConfig` struct expects on the wire. Identical for Tauri IPC
 * (`invoke("db_connect", { config })`) and HTTP (`POST /api/db/connect`).
 */
export function toRustConfig(config: ConnectionConfig): Record<string, unknown> {
  if (config.type === "mssql") {
    return {
      driver: "mssql",
      host: config.host,
      port: config.port,
      database: config.databaseName,
      username: config.username,
      password: config.password,
      encrypt: config.sslMode !== "disable",
      trust_cert: config.sslMode !== "require",
    };
  }

  if (config.type === "duckdb") {
    const path = config.connectionString
      ? config.connectionString.replace(/^duckdb:\/\//, "").replace(/^duckdb:/, "") || ":memory:"
      : config.databaseName || ":memory:";
    return { driver: "duckdb", path };
  }

  if (config.type === "sqlite") {
    return {
      driver: "sqlite",
      connection_string: config.connectionString,
      create_if_missing: config.createIfMissing ?? false,
    };
  }

  // PostgreSQL, MySQL, MariaDB
  return {
    driver: config.type === "mariadb" ? "mysql" : config.type,
    connection_string: config.connectionString,
  };
}
