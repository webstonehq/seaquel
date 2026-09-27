/**
 * `DatabaseProvider` over Seaquel Core, on desktop and web: every call is a
 * `db` request through the page's `CoreClient`, and streams are
 * `db.queryStream`. The managers keep the provider interface; the transport
 * lives in `$lib/core`.
 *
 * Values follow the cell wire format (`$lib/values`): parameters go out
 * through `encodeParams`, rows come back through `decodeRows`. Errors reject
 * as `"CODE: message"` (`CoreCallError`).
 */

import {
  callDb,
  CANCELLED,
  getCoreClient,
  type CoreClient,
  type QueryStreamRequest,
} from "$lib/core";
import { decodeRows, encodeParams } from "$lib/values";
import type { ConnectRequest, DatabaseProvider, ExecuteResult, ReadOnlyRows } from "./types";
import {
  collectReadOnly,
  formatStreamErrorFrame,
  formatUnknownStreamFrame,
  toRowObjects,
  type StreamBatch,
  type StreamOutcome,
} from "./wire";

export class CoreProvider implements DatabaseProvider {
  readonly id = "core";

  /** `client` is read per call, so the page's client can be swapped (tests). */
  constructor(private readonly getClient: () => CoreClient = getCoreClient) {}

  isAvailable(): boolean {
    return true;
  }

  async connect(request: ConnectRequest): Promise<string> {
    const { connectionId } = await callDb(this.getClient(), "connect", request);
    return connectionId;
  }

  async test(request: ConnectRequest): Promise<void> {
    await callDb(this.getClient(), "test", request);
  }

  async disconnect(connectionId: string): Promise<void> {
    await callDb(this.getClient(), "disconnect", { connectionId });
  }

  async select<T = Record<string, unknown>>(
    connectionId: string,
    sql: string,
    params?: unknown[],
  ): Promise<T[]> {
    const result = await callDb(this.getClient(), "query", {
      connectionId,
      sql,
      params: encodeParams(params),
    });
    // Columnar → row objects, column names deduped (`id`, `id_2`).
    return toRowObjects(result.columns, decodeRows(result.rows)) as T[];
  }

  async execute(connectionId: string, sql: string, params?: unknown[]): Promise<ExecuteResult> {
    const result = await callDb(this.getClient(), "execute", {
      connectionId,
      sql,
      params: encodeParams(params),
    });
    return {
      rowsAffected: result.rows_affected,
      lastInsertId: result.last_insert_id ?? undefined,
    };
  }

  selectStream(
    connectionId: string,
    sql: string,
    params: unknown[] | undefined,
    onBatch: (batch: StreamBatch) => boolean | Promise<boolean>,
    signal?: AbortSignal,
  ): Promise<StreamOutcome> {
    return this.stream(connectionId, sql, params, onBatch, signal, false);
  }

  /**
   * The read-only stream (`readOnly: true`): Core's token check, then the
   * engine's read-only query. Aborting `signal` cancels it.
   */
  selectReadOnly(
    connectionId: string,
    sql: string,
    signal?: AbortSignal,
    maxRows?: number,
  ): Promise<ReadOnlyRows> {
    return collectReadOnly(
      (onBatch) => this.stream(connectionId, sql, undefined, onBatch, signal, true, maxRows),
      signal,
    );
  }

  /**
   * One `db.queryStream`. `onBatch` returning false, or `signal` aborting,
   * cancels it and resolves `{aborted: true}`. Batches are handed over one
   * at a time, each after the previous `onBatch` settled.
   */
  private async stream(
    connectionId: string,
    sql: string,
    params: unknown[] | undefined,
    onBatch: (batch: StreamBatch) => boolean | Promise<boolean>,
    signal: AbortSignal | undefined,
    readOnly: boolean,
    maxRows?: number,
  ): Promise<StreamOutcome> {
    if (signal?.aborted) return { aborted: true };

    const request: QueryStreamRequest = {
      method: "db",
      params: {
        method: "queryStream",
        params: {
          connectionId,
          streamId: crypto.randomUUID(),
          sql,
          params: encodeParams(params),
          ...(readOnly ? { readOnly: true } : {}),
          ...(maxRows !== undefined ? { maxRows } : {}),
        },
      },
    };

    // Our own abort, so a false from `onBatch` can cancel too.
    const controller = new AbortController();
    const forward = () => controller.abort();
    signal?.addEventListener("abort", forward, { once: true });
    try {
      for await (const event of this.getClient().stream(request, {
        signal: controller.signal,
      })) {
        if (event.type === "batch") {
          if (controller.signal.aborted) continue;
          const keepGoing = await onBatch({
            columns: event.columns,
            rows: decodeRows(event.rows),
            isFinal: event.is_final,
            truncated: event.truncated,
          });
          if (!keepGoing || signal?.aborted) {
            controller.abort();
            return { aborted: true };
          }
        } else if (event.type === "done") {
          return { aborted: false };
        } else if (event.type === "error") {
          if (event.code === CANCELLED && controller.signal.aborted) return { aborted: true };
          return { aborted: false, error: formatStreamErrorFrame(event) };
        } else {
          controller.abort();
          return { aborted: false, error: formatUnknownStreamFrame(event) };
        }
      }
      // The client always ends with a terminal event; this is belt and braces.
      return { aborted: true };
    } catch (error) {
      controller.abort();
      return { aborted: false, error: error instanceof Error ? error.message : String(error) };
    } finally {
      signal?.removeEventListener("abort", forward);
    }
  }
}
