/**
 * HTTP database provider.
 *
 * Talks to a `seaquel-server` instance over HTTP + WebSocket. Used in the web
 * build (BUILD_TARGET=web) — in hosted and self-hosted deployments alike, the
 * `seaquel-server` binary serves both the SvelteKit frontend and the API on
 * the same origin, so `baseUrl` defaults to `""` (relative URLs).
 *
 * Wire format and semantics mirror `UnifiedTauriProvider` exactly so the rest
 * of the frontend (query-execution hook, row-access helpers, AbortSignal
 * contract) is oblivious to the transport switch.
 */

import type { DatabaseProvider, ConnectionConfig, ExecuteResult } from "./types";
import type {
  DbConnectResult,
  DbExecuteResult,
  DbQueryResult,
  DbStreamEvent,
  StreamBatch,
  StreamOutcome,
} from "./wire";
import {
  collectReadOnly,
  formatError,
  formatStreamErrorFrame,
  formatUnknownStreamFrame,
  toRowObjects,
  toRustConfig,
} from "./wire";
import { decodeRows, encodeParams } from "$lib/values";

export interface HttpProviderOptions {
  /**
   * Base URL for the API (e.g. `https://acme.seaquel.app`). Defaults to
   * same-origin (`""`), which is the right choice for both hosted tenants
   * and self-hosted — the server binary serves the frontend and the API on
   * one port.
   *
   * Can also be overridden at build time via `VITE_SEAQUEL_API_URL`.
   */
  baseUrl?: string;
}

export class HttpProvider implements DatabaseProvider {
  readonly id = "http";

  private readonly baseUrl: string;

  constructor(options: HttpProviderOptions = {}) {
    const baseUrl = options.baseUrl ?? envApiBaseUrl() ?? "";
    // Empty string is allowed — same-origin mode. A non-empty value must be
    // http(s) so `wsUrl()` can derive the corresponding ws(s) scheme by
    // swapping the prefix. Anything else (e.g. `ftp://`, `localhost:8787`
    // missing the scheme) would silently produce an invalid WebSocket URL
    // at stream time, so fail loudly here instead.
    if (baseUrl && !/^https?:\/\//i.test(baseUrl)) {
      throw new Error(
        `HttpProvider: baseUrl must start with http:// or https:// (got ${JSON.stringify(baseUrl)})`,
      );
    }
    this.baseUrl = baseUrl;
  }

  isAvailable(): boolean {
    return typeof window !== "undefined" && !("__TAURI__" in window);
  }

  async connect(config: ConnectionConfig): Promise<string> {
    const result = await this.postJson<DbConnectResult>("/api/db/connect", toRustConfig(config));
    return result.connection_id;
  }

  async disconnect(connectionId: string): Promise<void> {
    await this.postJson<unknown>("/api/db/disconnect", { connection_id: connectionId });
  }

  async select<T = Record<string, unknown>>(
    connectionId: string,
    sql: string,
    params?: unknown[],
  ): Promise<T[]> {
    const result = await this.postJson<DbQueryResult>("/api/db/query", {
      connection_id: connectionId,
      sql,
      values: encodeParams(params),
    });
    // Columnar → row objects, column names deduped (`id`, `id_2`).
    return toRowObjects(result.columns, decodeRows(result.rows)) as T[];
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
   * The read-only stream (first frame with `"read_only": true`): Core's
   * token check, then the engine's read-only query, as one final batch.
   * Aborting `signal` closes the socket, which the server watches while the
   * query runs, and dropping the query cancels it.
   */
  selectReadOnly(
    connectionId: string,
    sql: string,
    signal?: AbortSignal,
  ): Promise<Record<string, unknown>[]> {
    return collectReadOnly(
      (onBatch) => this.stream(connectionId, sql, undefined, onBatch, signal, true),
      signal,
    );
  }

  /**
   * One query over the `/api/db/stream` WebSocket. `readOnly` is only ever
   * `true` from `selectReadOnly`; everything else streams read-write.
   */
  private async stream(
    connectionId: string,
    sql: string,
    params: unknown[] | undefined,
    onBatch: (batch: StreamBatch) => boolean | Promise<boolean>,
    signal: AbortSignal | undefined,
    readOnly: boolean,
  ): Promise<StreamOutcome> {
    if (signal?.aborted) return { aborted: true };

    const ws = new WebSocket(this.wsUrl("/api/db/stream"));

    let cancelled = false;
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
      // Drop the handlers so queued `onmessage`/`onclose` events after we
      // decided on a terminal state become no-ops — without this, a message
      // that arrives between `finish()` being called and `ws.close()` taking
      // effect would still run `handleBatchEvent` and push another batch at
      // the caller.
      ws.onmessage = null;
      ws.onerror = null;
      ws.onclose = null;
      // Close the socket if it's still open — stops the server's fetch loop.
      if (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING) {
        ws.close();
      }
    };

    const handleBatchEvent = async (event: {
      columns: string[] | null;
      rows: unknown[][];
      is_final: boolean;
    }) => {
      if (cancelled) return;

      // Rows are already columnar on the wire — forward them straight through.
      // Skipping the per-row `{ [col]: v }` object construction is a large
      // win at 1M+ row scale.
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

    ws.onopen = () => {
      // Send the query request as the first Text frame — the server's
      // `stream` handler expects exactly one JSON frame up front.
      ws.send(
        JSON.stringify({
          query_id: crypto.randomUUID(),
          connection_id: connectionId,
          sql,
          values: encodeParams(params),
          read_only: readOnly,
        }),
      );
    };

    ws.onmessage = (event) => {
      // Chain handlers so batches are processed in order. Errors propagate
      // to the terminal promise via `finish(...)`.
      processing = processing.then(async () => {
        if (terminal) return;
        try {
          const parsed = JSON.parse(event.data as string) as DbStreamEvent;
          if (parsed.type === "batch") {
            await handleBatchEvent(parsed);
          } else if (parsed.type === "done") {
            finish({ aborted: cancelled });
          } else if (parsed.type === "error") {
            finish({ aborted: false, error: formatStreamErrorFrame(parsed) });
          } else {
            // Protocol drift (unknown `type`). Fail explicitly — the old
            // silent-ignore branch left the terminal promise unresolved and
            // `selectStream` hung until the socket closed.
            finish({ aborted: false, error: formatUnknownStreamFrame(parsed) });
          }
        } catch (e) {
          finish({
            aborted: false,
            error: e instanceof Error ? e.message : String(e),
          });
        }
      });
    };

    ws.onerror = () => {
      // The `close` event follows and will finalize if we haven't already.
      // onerror itself carries no useful information in browsers, so we
      // just note it and let onclose resolve the terminal state.
    };

    ws.onclose = (ev) => {
      if (terminal) return;
      // If the socket closed before we saw `done` or `error`, treat it as
      // an error — unless the caller aborted, in which case it's an abort.
      if (cancelled || signal?.aborted) {
        finish({ aborted: true });
      } else {
        finish({
          aborted: false,
          error: ev.reason
            ? `WS_CLOSED: ${ev.reason}`
            : "WS_CLOSED: socket closed before stream completed",
        });
      }
    };

    const onAbort = () => {
      cancelled = true;
      // Closing the socket signals the server to stop. Its stream handler
      // watches the socket while it waits for the next event and drops the
      // query when the socket closes.
      finish({ aborted: true });
    };
    signal?.addEventListener("abort", onAbort);

    try {
      await terminalPromise;
      await processing;
    } finally {
      signal?.removeEventListener("abort", onAbort);
    }

    return terminal ?? { aborted: cancelled };
  }

  async execute(connectionId: string, sql: string, params?: unknown[]): Promise<ExecuteResult> {
    const result = await this.postJson<DbExecuteResult>("/api/db/execute", {
      connection_id: connectionId,
      sql,
      values: encodeParams(params),
    });
    return {
      rowsAffected: result.rows_affected,
      lastInsertId: result.last_insert_id ?? undefined,
    };
  }

  async test(config: ConnectionConfig): Promise<void> {
    await this.postJson<unknown>("/api/db/test", toRustConfig(config));
  }

  // -------- internals --------

  private postJson<T>(path: string, body: unknown): Promise<T> {
    return postJson<T>(this.baseUrl + path, body);
  }

  private wsUrl(path: string): string {
    if (this.baseUrl) {
      // Rewrite http(s) → ws(s). Any other scheme is a misconfiguration.
      return this.baseUrl.replace(/^http/, "ws") + path;
    }
    // Same-origin case: derive scheme from the page.
    if (typeof window !== "undefined") {
      const scheme = window.location.protocol === "https:" ? "wss" : "ws";
      return `${scheme}://${window.location.host}${path}`;
    }
    return `ws://localhost${path}`;
  }
}

/** `VITE_SEAQUEL_API_URL`, when the build sets one. */
export function envApiBaseUrl(): string | undefined {
  return typeof import.meta !== "undefined" && import.meta.env
    ? (import.meta.env.VITE_SEAQUEL_API_URL as string | undefined)
    : undefined;
}

/**
 * POST a JSON body and parse the JSON reply. A non-2xx reply with a `DbError`
 * body becomes an `Error` shaped `"CODE: message"` (via `formatError`); a
 * network failure becomes `NETWORK_ERROR`. Shared by `HttpProvider` and the
 * Rust `EngineClient`.
 */
export async function postJson<T>(url: string, body: unknown): Promise<T> {
  let response: Response;
  try {
    response = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch (e) {
    // Network-level failure (DNS, offline, CORS preflight). Surface as
    // a CONNECTION_ERROR so the UI shows the same error class as a
    // driver-level connect failure.
    throw formatError({
      code: "NETWORK_ERROR",
      message: e instanceof Error ? e.message : String(e),
    });
  }

  if (!response.ok) {
    // Server returns JSON `{message, code}` for DbError. On non-JSON
    // failures (unlikely but possible — 5xx from a proxy, etc.) fall
    // back to a generic HTTP error.
    let errorBody: unknown;
    try {
      errorBody = await response.json();
    } catch {
      throw new Error(`HTTP_${response.status}: ${response.statusText}`);
    }
    throw formatError(errorBody);
  }

  // 200 OK — some endpoints return empty body (disconnect, test,
  // transaction). Handle that without tripping JSON.parse.
  const text = await response.text();
  if (text.length === 0) return undefined as unknown as T;
  return JSON.parse(text) as T;
}
