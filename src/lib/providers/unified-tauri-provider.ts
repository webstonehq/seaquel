/**
 * Unified Tauri database provider.
 * Single provider for all database types (PostgreSQL, MySQL, SQLite, MSSQL, DuckDB)
 * via the unified db_connect/db_query/db_execute/db_disconnect/db_test commands.
 */

import { Channel, invoke } from "@tauri-apps/api/core";
import type { DatabaseProvider, ConnectionConfig, ExecuteResult } from "./types";
import type { DbConnectResult, DbExecuteResult, DbQueryResult, DbStreamEvent } from "./wire";
import {
  formatError,
  formatStreamErrorFrame,
  formatUnknownStreamFrame,
  toRustConfig,
} from "./wire";
import { dedupeColumnNames } from "$lib/utils/row-access";
import { decodeRows, encodeParams } from "$lib/values";

export class UnifiedTauriProvider implements DatabaseProvider {
  readonly id = "unified-tauri";

  isAvailable(): boolean {
    return typeof window !== "undefined" && "__TAURI__" in window;
  }

  async connect(config: ConnectionConfig): Promise<string> {
    try {
      const result = await invoke<DbConnectResult>("db_connect", {
        config: toRustConfig(config),
      });
      return result.connection_id;
    } catch (error) {
      throw formatError(error);
    }
  }

  async disconnect(connectionId: string): Promise<void> {
    try {
      await invoke("db_disconnect", { connectionId });
    } catch (error) {
      throw formatError(error);
    }
  }

  async select<T = Record<string, unknown>>(
    connectionId: string,
    sql: string,
    params?: unknown[],
  ): Promise<T[]> {
    try {
      const result = await invoke<DbQueryResult>("db_query", {
        connectionId,
        sql,
        values: encodeParams(params),
      });
      // Convert columnar → row objects for frontend compatibility. Dedupe
      // column names first so `SELECT a.id, b.id FROM a JOIN b` preserves
      // both values (`{ id: ..., id_2: ... }`) instead of the second one
      // silently overwriting the first via `obj[col] = row[i]`.
      const columns = dedupeColumnNames(result.columns);
      return decodeRows(result.rows).map((row) => {
        const obj: Record<string, unknown> = {};
        for (let i = 0; i < columns.length; i++) {
          obj[columns[i]] = row[i];
        }
        return obj as T;
      });
    } catch (error) {
      throw formatError(error);
    }
  }

  async selectStream(
    connectionId: string,
    sql: string,
    params: unknown[] | undefined,
    onBatch: (batch: {
      columns: string[] | null;
      rows: unknown[][];
      isFinal: boolean;
    }) => boolean | Promise<boolean>,
    signal?: AbortSignal,
  ): Promise<{ aborted: boolean; error?: string }> {
    // If the caller hands us an already-aborted signal, short-circuit
    // entirely. The previous behavior was to still fire the invoke and
    // let it run to completion in the background, which wastes a full
    // query's worth of CPU and holds the sqlx connection for no reason.
    if (signal?.aborted) {
      return { aborted: true };
    }

    const channel = new Channel<DbStreamEvent>();
    // Per-call query ID so `db_cancel_stream` can target *this* stream
    // even if several are in flight concurrently across tabs.
    const queryId = crypto.randomUUID();

    let cancelled = false;

    // Serialize onBatch invocations so we don't interleave async callbacks when
    // batches arrive faster than the caller can process them.
    let processing: Promise<void> = Promise.resolve();
    let terminal: { aborted: boolean; error?: string } | null = null;
    let resolveTerminal: (() => void) | null = null;
    const terminalPromise = new Promise<void>((resolve) => {
      resolveTerminal = resolve;
    });

    const finish = (value: { aborted: boolean; error?: string }) => {
      if (terminal) return;
      terminal = value;
      resolveTerminal?.();
    };

    const handleBatchEvent = async (event: {
      columns: string[] | null;
      rows: unknown[][];
      is_final: boolean;
    }) => {
      if (cancelled) return;

      // Rows are already columnar on the wire — forward them straight through.
      // Skipping the per-row `{ [col]: v }` object construction is a large
      // win at 1M+ row scale (previously ~1M object allocations interleaved
      // with IPC for a select-all-rows query).
      const keepGoing = await onBatch({
        columns: event.columns,
        rows: decodeRows(event.rows),
        isFinal: event.is_final,
      });

      if (!keepGoing || signal?.aborted) {
        cancelled = true;
      }

      if (event.is_final) {
        finish({ aborted: cancelled });
      }
    };

    channel.onmessage = (event) => {
      // Chain handlers so batches are processed in order. Errors in a handler
      // propagate to the terminal promise below via `finish(...)`.
      processing = processing.then(async () => {
        if (terminal) return;
        try {
          if (event.type === "batch") {
            await handleBatchEvent(event);
          } else if (event.type === "done") {
            finish({ aborted: cancelled });
          } else if (event.type === "error") {
            finish({ aborted: false, error: formatStreamErrorFrame(event) });
          } else {
            // Protocol drift (unknown `type`). Fail explicitly — the old
            // silent-ignore branch left `selectStream` hanging on the
            // terminal promise until the channel was torn down.
            finish({ aborted: false, error: formatUnknownStreamFrame(event) });
          }
        } catch (e) {
          finish({
            aborted: false,
            error: e instanceof Error ? e.message : String(e),
          });
        }
      });
    };

    const onAbort = () => {
      cancelled = true;
      // Tell Rust to stop fetching rows. Fire-and-forget — if it fails
      // (e.g. the stream already finished naturally), we don't care.
      void invoke("db_cancel_stream", { queryId }).catch(() => {});
      // Resolve the terminal promise immediately so selectStream returns
      // without waiting for the invoke to wind down. Rust will catch up
      // on its next loop iteration and the still-pending invoke will
      // eventually resolve in the background.
      finish({ aborted: true });
    };
    // The already-aborted case is handled by the early return at the top
    // of this method; here we only need to listen for a mid-stream abort.
    signal?.addEventListener("abort", onAbort);

    // Kick off the invoke. We do NOT include it in the await below — if
    // the user cancels, we return as soon as `terminal` is set and let
    // the invoke resolve in the background. The `.catch` prevents an
    // unhandled rejection if Rust errors out.
    const invokePromise = invoke<void>("db_query_stream", {
      queryId,
      connectionId,
      sql,
      values: encodeParams(params),
      onEvent: channel,
    }).catch((error) => {
      finish({
        aborted: false,
        error: formatError(error).message,
      });
    });

    try {
      await terminalPromise;
      // Drain any pending batch callbacks so the caller observes a
      // consistent final state before we resolve.
      await processing;
    } finally {
      if (signal) signal.removeEventListener("abort", onAbort);
    }

    // Keep a reference to the invoke promise alive so the Channel isn't
    // GC'd mid-flight if Rust is still wrapping up. The await below
    // resolves almost immediately in the normal case (invoke returns
    // right after Done) and within a loop iteration in the cancel case
    // (Rust cancels the stream and returns).
    void invokePromise;

    return terminal ?? { aborted: cancelled };
  }

  async execute(connectionId: string, sql: string, params?: unknown[]): Promise<ExecuteResult> {
    try {
      const result = await invoke<DbExecuteResult>("db_execute", {
        connectionId,
        sql,
        values: encodeParams(params),
      });
      return {
        rowsAffected: result.rows_affected,
        lastInsertId: result.last_insert_id ?? undefined,
      };
    } catch (error) {
      throw formatError(error);
    }
  }

  async test(config: ConnectionConfig): Promise<void> {
    try {
      await invoke("db_test", { config: toRustConfig(config) });
    } catch (error) {
      throw formatError(error);
    }
  }
}
