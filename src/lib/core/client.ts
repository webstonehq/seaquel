/**
 * `CoreClient`: how the GUI talks to Seaquel Core, whatever the transport.
 *
 * - `call` sends one workspace request (`CoreRequest`) and resolves with its
 *   `CoreResponse`. Desktop: the `core_call` command; web: `POST /api/rpc`.
 *   A failure rejects with a `CoreCallError` (`"CODE: message"`).
 * - `stream` starts a stream call and yields its events, ending with exactly
 *   one `done` or `error`: a `db.queryStream` yields `StreamEvent`s, the
 *   editor's `db.run` and `db.page` (phase 5b) and the data tab's
 *   `db.tablePage` (phase 5c) yield `RunEvent`s, and an assistant turn
 *   (`ai.chat`, phase 6) yields `AiEvent`s. A run's or
 *   page's text goes out well-formed (`wellFormedRequest`). Desktop: `core_stream` over a Tauri
 *   channel; web: the page's one `/api/rpc/stream` WebSocket. Aborting the
 *   signal, or leaving the `for await` early, cancels the stream; the
 *   iterator then ends with an `error` whose code is `CANCELLED`. A stream
 *   that ends with no `done` or `error` (Core was told to cancel it) ends the
 *   same way, so a consumer always sees one terminal event.
 * - `events` delivers the workspace's events: `connectionClosed` (a
 *   connection Core closed without the GUI asking: `WORKSPACE_EVICTED`,
 *   `WINDOW_CLOSED` and `CONNECTION_REPLACED` (web, a closed tab's or a
 *   tab's replaced connection), `CONNECTION_CLOSED`, …) and `storageChanged` (a stored write committed,
 *   from any of the user's windows or tabs, this one's included: its
 *   `origin` is `pageOrigin()` then; phase 5d, Decisions 16–18). Events
 *   sent while the page wasn't subscribed are lost: `onResubscribed` says
 *   when to reload, and `onEventsUnavailable` when updates stopped.
 *
 * `callDb` and the `Db*` types give each `db` method its params and result,
 * derived from the generated wire types the way `$lib/storage/rust-client`
 * types its storage calls.
 */

import type { AiEvent } from "$lib/types/generated/AiEvent";
import type { ChatParams } from "$lib/types/generated/ChatParams";
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
import { wellFormed } from "./well-formed";

export { wellFormed, wellFormedJson } from "./well-formed";

export type { AiEvent, CoreEvent, RunEvent, StreamEvent };

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

/**
 * `ai.chat`, one assistant turn (phase 6): yields `AiEvent`s, as
 * `{type: "ai"}` frames. A turn Core cancelled ends with neither `done`
 * nor `error`, so it ends here as `CANCELLED`, as any stream does.
 */
export type AiChatRequest = {
  method: "ai";
  params: { method: "chat"; params: ChatParams };
};

/** Every request `stream` takes. */
export type StreamRequest =
  | QueryStreamRequest
  | RunRequest
  | PageRequest
  | TablePageRequest
  | AiChatRequest;

/** Any stream's event. Every kind ends with one `done` or `error`. */
export type AnyStreamEvent = StreamEvent | RunEvent | AiEvent;

/** The events `request` yields. */
export type EventOf<R extends StreamRequest> = R extends QueryStreamRequest
  ? StreamEvent
  : R extends AiChatRequest
    ? AiEvent
    : RunEvent;

/** Whether a stream transport's frame is a stream's own event (not a workspace event). */
export function isStreamFrame(
  event: CoreEvent,
): event is Extract<CoreEvent, { type: "stream" | "run" | "ai" }> {
  return event.type === "stream" || event.type === "run" || event.type === "ai";
}

/** A connection Core closed without the GUI asking. */
export type ConnectionClosedEvent = Extract<CoreEvent, { type: "connectionClosed" }>;

/**
 * A stored write committed (phase 5d): its kind, scope, ids (`null`: reload
 * the kind in the scope), the writer's `origin` and the change `seq`. Never
 * a value.
 */
export type StorageChangedEvent = Extract<CoreEvent, { type: "storageChanged" }>;

/** What `events` delivers. */
export type WorkspaceEvent = ConnectionClosedEvent | StorageChangedEvent;

/** `onResubscribed`'s argument: `initial` on the page's first subscription. */
export interface ResubscribedInfo {
  initial: boolean;
}

/**
 * Why `events` stopped delivering: `ACCESS_LOST` (web, 1008: signed out or
 * removed; nothing more until a new query reconnects), `TOO_MANY_TABS`
 * (web: the user has too many sockets open; retried with backoff) or
 * `EVENTS_UNAVAILABLE` (desktop: `core_events` couldn't be registered).
 */
export type EventsUnavailableReason = "ACCESS_LOST" | "TOO_MANY_TABS" | "EVENTS_UNAVAILABLE";

/** Whether `event` is one `events` delivers (not a stream's or a run's). */
export function isWorkspaceEvent(event: CoreEvent): event is WorkspaceEvent {
  return event.type === "connectionClosed" || event.type === "storageChanged";
}

export interface StreamOptions {
  /** Aborting it cancels the stream. */
  signal?: AbortSignal;
}

export interface CoreClient {
  call(request: CoreRequest): Promise<CoreResponse>;
  stream<R extends StreamRequest>(request: R, options?: StreamOptions): AsyncIterable<EventOf<R>>;
  /** Subscribe to `connectionClosed` and `storageChanged` events; returns the unsubscribe. */
  events(handler: (event: WorkspaceEvent) => void): () => void;
  /**
   * Runs each time the page's event channel starts: `initial` the first
   * time, then after every reconnect (web) or re-registration (desktop).
   * Events sent while the channel was down are lost, so a subscriber
   * reloads everything it shows. Register it before `events`, so the first
   * start isn't missed. Returns the unsubscribe.
   */
  onResubscribed(handler: (info: ResubscribedInfo) => void): () => void;
  /**
   * Runs when events stop arriving for a while (see
   * `EventsUnavailableReason`), so the page can say it isn't receiving
   * updates. The next `onResubscribed` means they're back. Returns the
   * unsubscribe.
   */
  onEventsUnavailable(handler: (reason: EventsUnavailableReason) => void): () => void;
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
 * `request` with a run's text, a page's SQL, a table page's filter values or
 * a turn's message made well-formed; anything else as it is.
 */
export function wellFormedRequest<R extends StreamRequest>(request: R): R {
  if (request.method === "ai") {
    const turn = request.params.params;
    const userMessage = { ...turn.userMessage, content: wellFormed(turn.userMessage.content) };
    return {
      ...request,
      params: { ...request.params, params: { ...turn, userMessage } },
    };
  }
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
