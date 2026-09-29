/**
 * `CoreClient`: how the GUI talks to Seaquel Core, whatever the transport.
 *
 * - `call` sends one workspace request (`CoreRequest`) and resolves with its
 *   `CoreResponse`. Desktop: the `core_call` command; web: `POST /api/rpc`.
 *   A failure rejects with a `CoreCallError` (`"CODE: message"`).
 * - `stream` starts a stream call and yields its events, ending with exactly
 *   one `done` or `error`: a `db.queryStream` yields `StreamEvent`s, and the
 *   editor's `db.run` and `db.page` (phase 5b) and the data tab's
 *   `db.tablePage` (phase 5c) yield `RunEvent`s. A run's or
 *   page's text goes out well-formed (`wellFormedRequest`). Desktop: `core_stream` over a Tauri
 *   channel; web: the page's one `/api/rpc/stream` WebSocket. Aborting the
 *   signal, or leaving the `for await` early, cancels the stream; the
 *   iterator then ends with an `error` whose code is `CANCELLED`. A stream
 *   that ends with no `done` or `error` (Core was told to cancel it) ends the
 *   same way, so a consumer always sees one terminal event.
 * - `events` delivers `connectionClosed` events: a connection Core closed
 *   without the GUI asking (`WORKSPACE_EVICTED`, `CONNECTION_CLOSED`, …).
 *
 * `callDb` and the `Db*` types give each `db` method its params and result,
 * derived from the generated wire types the way `$lib/storage/rust-client`
 * types its storage calls.
 */

import type { CoreEvent } from "$lib/types/generated/CoreEvent";
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import type { CoreResponse } from "$lib/types/generated/CoreResponse";
import type { DbRequest } from "$lib/types/generated/DbRequest";
import type { DbResponse } from "$lib/types/generated/DbResponse";
import type { PageParams } from "$lib/types/generated/PageParams";
import type { QueryStreamParams } from "$lib/types/generated/QueryStreamParams";
import type { RpcError } from "$lib/types/generated/RpcError";
import type { RunEvent } from "$lib/types/generated/RunEvent";
import type { RunParams } from "$lib/types/generated/RunParams";
import type { StreamEvent } from "$lib/types/generated/StreamEvent";
import type { TablePageParams } from "$lib/types/generated/TablePageParams";
import { CoreCallError } from "$lib/storage/rust-client";

export type { CoreEvent, RunEvent, StreamEvent };

/** `db.queryStream`: yields `StreamEvent`s. */
export type QueryStreamRequest = {
  method: "db";
  params: { method: "queryStream"; params: QueryStreamParams };
};

/** `db.run`, the editor's run: yields `RunEvent`s. */
export type RunRequest = {
  method: "db";
  params: { method: "run"; params: RunParams };
};

/** `db.page`, one statement re-paged: yields `RunEvent`s. */
export type PageRequest = {
  method: "db";
  params: { method: "page"; params: PageParams };
};

/** `db.tablePage`, one page of a data tab (phase 5c): yields `RunEvent`s. */
export type TablePageRequest = {
  method: "db";
  params: { method: "tablePage"; params: TablePageParams };
};

/** Every request `stream` takes. */
export type StreamRequest = QueryStreamRequest | RunRequest | PageRequest | TablePageRequest;

/** Any stream's event. Both kinds end with one `done` or `error`. */
export type AnyStreamEvent = StreamEvent | RunEvent;

/** The events `request` yields. */
export type EventOf<R extends StreamRequest> = R extends QueryStreamRequest
  ? StreamEvent
  : RunEvent;

/** A connection Core closed without the GUI asking. */
export type ConnectionClosedEvent = Extract<CoreEvent, { type: "connectionClosed" }>;

export interface StreamOptions {
  /** Aborting it cancels the stream. */
  signal?: AbortSignal;
}

export interface CoreClient {
  call(request: CoreRequest): Promise<CoreResponse>;
  stream<R extends StreamRequest>(request: R, options?: StreamOptions): AsyncIterable<EventOf<R>>;
  /** Subscribe to `connectionClosed` events; returns the unsubscribe. */
  events(handler: (event: ConnectionClosedEvent) => void): () => void;
}

// -------- Codes --------

/** A stream that was cancelled (by this page, or by Core) ends with this code. */
export const CANCELLED = "CANCELLED";
/** Core needs the user to trust an SSH host key; the message holds its `SHA256:…`. */
export const UNKNOWN_HOST_KEY = "UNKNOWN_HOST_KEY";

// -------- Typed `db` calls --------

/** Every `db` method but the stream calls, which only `stream` serves. */
export type DbMethod = Exclude<DbRequest["method"], "queryStream" | "run" | "page" | "tablePage">;
export type DbParams<M extends DbMethod> = Extract<DbRequest, { method: M }>["params"];
export type DbResult<M extends DbMethod> = Extract<DbResponse, { method: M }>["result"];

/** One `db` call; rejects with a `CoreCallError`. */
export async function callDb<M extends DbMethod>(
  client: CoreClient,
  method: M,
  params: DbParams<M>,
): Promise<DbResult<M>> {
  const request = { method: "db", params: { method, params } } as CoreRequest;
  const response = await client.call(request);
  if (response?.method !== "db" || response.result?.method !== method) {
    // Never echo the request: connect and test carry secrets.
    throw new CoreCallError({
      code: "PROTOCOL_ERROR",
      message: `expected a db ${method} response`,
    });
  }
  return response.result.result as DbResult<M>;
}

// -------- Errors --------

export function isRpcError(value: unknown): value is RpcError {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as RpcError).code === "string" &&
    typeof (value as RpcError).message === "string"
  );
}

/** The Core error code `error` carries, if any. */
export function errorCode(error: unknown): string | null {
  if (typeof error === "object" && error !== null && "code" in error) {
    const { code } = error as { code?: unknown };
    if (typeof code === "string") return code;
  }
  return null;
}

/** A stream's terminal error: the same shape in a `StreamEvent` and a `RunEvent`. */
export type StreamErrorEvent = Extract<StreamEvent, { type: "error" }>;

/** A stream's terminal `error` event. */
export function streamError(code: string, message: string): StreamErrorEvent {
  return { type: "error", code, message };
}

/** The terminal event of a stream that was cancelled. */
export function cancelledEvent(message = "The query was cancelled"): StreamErrorEvent {
  return streamError(CANCELLED, message);
}

/** A rejected transport call as a terminal stream event. */
export function errorEvent(error: unknown): StreamErrorEvent {
  if (isRpcError(error)) return streamError(error.code, error.message);
  if (error instanceof CoreCallError) {
    return streamError(error.code, error.message.replace(`${error.code}: `, ""));
  }
  return streamError("UNKNOWN", error instanceof Error ? error.message : String(error));
}

function isTerminal(event: AnyStreamEvent): boolean {
  return event.type === "done" || event.type === "error";
}

// -------- Well-formed text --------

/**
 * `text` with every lone surrogate replaced by U+FFFD. Core's JSON reader
 * refuses a lone surrogate (which `JSON.stringify` writes as `\udXXX`), and
 * U+FFFD is one UTF-16 unit as the surrogate was, so a cursor offset into
 * the text still points at the same place. `String.prototype.toWellFormed`
 * where the WebView has it.
 */
export function wellFormed(text: string): string {
  const native = (text as { toWellFormed?: () => string }).toWellFormed;
  if (typeof native === "function") return native.call(text);
  let out: string[] | undefined;
  let from = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c < 0xd800 || c > 0xdfff) continue;
    if (c <= 0xdbff) {
      const next = text.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        i++;
        continue;
      }
    }
    (out ??= []).push(text.slice(from, i), "\uFFFD");
    from = i + 1;
  }
  return out ? out.join("") + text.slice(from) : text;
}

/**
 * `request` with a run's text, a page's SQL or a table page's filter values
 * made well-formed; anything else as it is.
 */
export function wellFormedRequest<R extends StreamRequest>(request: R): R {
  const inner = request.params;
  if (inner.method === "run") {
    const params = { ...inner.params, text: wellFormed(inner.params.text) };
    return { ...request, params: { ...inner, params } };
  }
  if (inner.method === "page") {
    const source = { ...inner.params.source, sql: wellFormed(inner.params.source.sql) };
    return { ...request, params: { ...inner, params: { ...inner.params, source } } };
  }
  if (inner.method === "tablePage") {
    const query = inner.params.query;
    const filters = query.filters.map((f) => ({ ...f, value: wellFormed(f.value) }));
    const params = { ...inner.params, query: { ...query, filters } };
    return { ...request, params: { ...inner, params } };
  }
  return request;
}

// -------- The iterator both transports hand out --------

/**
 * One stream's events for one consumer. The transport `push`es events as
 * they arrive; the first `done` or `error` ends it, and later events are
 * dropped. `finish` ends a stream that got no terminal event as cancelled.
 * A consumer that stops early (`break`, `return`) runs `onStop`, which
 * cancels the stream in Core. `T` is the stream's event type: a query
 * stream's `StreamEvent` or a run's `RunEvent`, whose terminal `error`
 * shapes agree, so the transports' own endings (`cancelledEvent`,
 * `errorEvent`) are events of either.
 */
export class StreamQueue<
  T extends AnyStreamEvent = StreamEvent,
> implements AsyncIterableIterator<T> {
  private readonly buffer: T[] = [];
  private waiter: ((result: IteratorResult<T>) => void) | null = null;
  private terminated = false;
  private readonly endHandlers: Array<() => void> = [];

  constructor(private readonly onStop: () => void = () => {}) {}

  /** True once the terminal event is in (delivered or not). */
  get ended(): boolean {
    return this.terminated;
  }

  /** Runs once, when the stream ends for any reason. */
  onEnd(handler: () => void): void {
    if (this.terminated) handler();
    else this.endHandlers.push(handler);
  }

  /** A transport's own error ending (cancelled, closed, refused): valid for either stream kind. */
  pushError(event: StreamErrorEvent): void {
    this.push(event as T);
  }

  push(event: T): void {
    if (this.terminated) return;
    if (isTerminal(event)) this.end();
    if (this.waiter) {
      const resolve = this.waiter;
      this.waiter = null;
      resolve({ value: event, done: false });
    } else {
      this.buffer.push(event);
    }
  }

  /** The transport has nothing more: no terminal event means cancelled. */
  finish(): void {
    if (!this.terminated) this.pushError(cancelledEvent());
  }

  private end(): void {
    this.terminated = true;
    for (const handler of this.endHandlers.splice(0)) handler();
  }

  next(): Promise<IteratorResult<T>> {
    const value = this.buffer.shift();
    if (value) return Promise.resolve({ value, done: false });
    if (this.terminated) return Promise.resolve({ value: undefined, done: true });
    return new Promise((resolve) => {
      this.waiter = resolve;
    });
  }

  return(): Promise<IteratorResult<T>> {
    this.buffer.length = 0;
    if (!this.terminated) {
      this.onStop();
      this.end();
    }
    return Promise.resolve({ value: undefined, done: true });
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<T> {
    return this;
  }
}
