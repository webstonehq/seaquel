/**
 * `CoreClient` on web:
 * - `call`: `POST /api/rpc`;
 * - `stream` and `events`: one WebSocket per page at `/api/rpc/stream`,
 *   shared by every stream (`db.queryStream`, and the editor's `db.run` and
 *   `db.page`, whose events come as `type: "run"`). Client frames are
 *   `{"op":"start","streamId","request"}` (the `CoreRequest` inline) and
 *   `{"op":"cancel","streamId"}`; server frames are `CoreEvent` JSON.
 *
 * - **Multiplexing.** The server runs at most 16 streams per socket
 *   (`TOO_MANY_STREAMS`), so at most `maxStreams` are started at once and the
 *   rest wait their turn. A start the server still refuses (a cancelled
 *   stream it hasn't wound down yet) is retried with a growing delay, up to
 *   `maxStreamRetries` times, then fails with `TOO_MANY_STREAMS`.
 * - **Disconnects.** When the socket closes, every started stream ends with
 *   `WS_CLOSED`: the server cancels a socket's streams when it closes, so
 *   they can't carry on over the next socket. That includes the proxy's
 *   lifetime cap (a 1000 close after 12 h), which a query rarely spans.
 *   Streams not started yet wait for the next socket. The socket reopens
 *   with backoff (doubling up to `maxDelayMs`) while anything needs it (a
 *   waiting stream, or an `events` subscriber). The backoff only resets once
 *   a socket has proven itself: its first message, or `stableMs` open.
 *   Events sent while no socket was open are lost.
 * - **Too many tabs.** The server allows a few sockets per user and closes
 *   the next with 1013 `TOO_MANY_SOCKETS`. Every stream then ends with
 *   `TOO_MANY_TABS`, and the socket is retried with the growing backoff.
 * - **Access lost.** The server closes with 1008 (policy) when the session
 *   lost access (signed out, removed from the team). Every stream then ends
 *   with `ACCESS_LOST`, `onAccessLost` runs once, and the socket stays
 *   closed: only a new query tries again.
 */

import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import type { CoreResponse } from "$lib/types/generated/CoreResponse";
import { encodeCoreRequest, httpCoreTransport } from "$lib/storage/rust-client";
import { log } from "$lib/utils/logger";
import { errorToast } from "$lib/utils/toast";
import { m } from "$lib/paraglide/messages.js";
import {
  cancelledEvent,
  streamError,
  StreamQueue,
  type ConnectionClosedEvent,
  type CoreClient,
  type AnyStreamEvent,
  type CoreEvent,
  type EventOf,
  type StreamOptions,
  type StreamRequest,
  wellFormedRequest,
} from "./client";

/** The server's cap on streams per socket. */
export const MAX_STREAMS = 16;
export const TOO_MANY_STREAMS = "TOO_MANY_STREAMS";
/** A started stream whose socket closed ends with this code. */
export const WS_CLOSED = "WS_CLOSED";
/** Streams end with this code when the server closed the socket for lost access. */
export const ACCESS_LOST = "ACCESS_LOST";
/** Streams end with this code when the server refused the socket: too many open tabs. */
export const TOO_MANY_TABS = "TOO_MANY_TABS";
/** The close code the server uses for lost access. */
export const POLICY_VIOLATION = 1008;
/** The close code for "try again later"; with a `TOO_MANY_SOCKETS` reason, too many tabs. */
export const TRY_AGAIN_LATER = 1013;
const TOO_MANY_SOCKETS = "TOO_MANY_SOCKETS";

type WebSocketLike = Pick<WebSocket, "readyState" | "send" | "close"> & {
  onopen: ((event: Event) => void) | null;
  onmessage: ((event: MessageEvent) => void) | null;
  onerror: ((event: Event) => void) | null;
  onclose: ((event: CloseEvent) => void) | null;
};

export interface HttpCoreClientOptions {
  /** The socket's URL; defaults to `/api/rpc/stream` on the page's origin. */
  url?: string;
  /** Defaults to the global `WebSocket`. */
  createSocket?: (url: string) => WebSocketLike;
  maxStreams?: number;
  /** Reconnect backoff: first delay, doubled per failure up to `maxDelayMs`. */
  initialDelayMs?: number;
  maxDelayMs?: number;
  /** First delay before retrying a start the server refused with `TOO_MANY_STREAMS`. */
  retryDelayMs?: number;
  /** How many times one stream's refused start is retried. */
  maxStreamRetries?: number;
  /** How long a socket must stay open (without a message) before the backoff resets. */
  stableMs?: number;
  /** Runs when the server closes the socket for lost access. Defaults to an error toast. */
  onAccessLost?: (message: string) => void;
}

interface Entry {
  queue: StreamQueue<AnyStreamEvent>;
  frame: string;
  /** Its start frame went out on the current socket. */
  started: boolean;
  /** It has had any event: a refused start can only come first. */
  heard: boolean;
  /** Starts the server refused with `TOO_MANY_STREAMS` so far. */
  retries: number;
}

const OPEN = 1;

function defaultUrl(): string {
  if (typeof window === "undefined") return "ws://localhost/api/rpc/stream";
  const scheme = window.location.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${window.location.host}/api/rpc/stream`;
}

export class HttpCoreClient implements CoreClient {
  private readonly url: string;
  private readonly createSocket: (url: string) => WebSocketLike;
  private readonly maxStreams: number;
  private readonly initialDelayMs: number;
  private readonly maxDelayMs: number;
  private readonly retryDelayMs: number;
  private readonly maxStreamRetries: number;
  private readonly stableMs: number;
  private readonly onAccessLost: (message: string) => void;
  /** Set by a 1008 close: no reconnect until a new query asks for one. */
  private accessLost = false;

  private socket: WebSocketLike | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private failures = 0;
  /** Every stream not ended yet, in start order. */
  private readonly streams = new Map<string, Entry>();
  private readonly handlers = new Set<(event: ConnectionClosedEvent) => void>();

  constructor(options: HttpCoreClientOptions = {}) {
    this.url = options.url ?? defaultUrl();
    this.createSocket =
      options.createSocket ?? ((url) => new WebSocket(url) as unknown as WebSocketLike);
    this.maxStreams = options.maxStreams ?? MAX_STREAMS;
    this.initialDelayMs = options.initialDelayMs ?? 500;
    this.maxDelayMs = options.maxDelayMs ?? 30_000;
    this.retryDelayMs = options.retryDelayMs ?? 100;
    this.maxStreamRetries = options.maxStreamRetries ?? 8;
    this.stableMs = options.stableMs ?? 5_000;
    this.onAccessLost = options.onAccessLost ?? ((message) => errorToast(message));
  }

  async call(request: CoreRequest): Promise<CoreResponse> {
    return (await httpCoreTransport(encodeCoreRequest(request))) as CoreResponse;
  }

  stream<R extends StreamRequest>(
    request: R,
    options: StreamOptions = {},
  ): AsyncIterable<EventOf<R>> {
    const { signal } = options;
    const streamId = request.params.params.streamId;
    const queue = new StreamQueue<EventOf<R>>(() => this.cancel(streamId));
    if (signal?.aborted) {
      queue.pushError(cancelledEvent());
      return queue;
    }

    const entry: Entry = {
      queue: queue as StreamQueue<AnyStreamEvent>,
      frame: JSON.stringify({ op: "start", streamId, request: wellFormedRequest(request) }),
      started: false,
      heard: false,
      retries: 0,
    };
    this.streams.set(streamId, entry);

    const onAbort = () => {
      this.cancel(streamId);
      queue.pushError(cancelledEvent());
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    queue.onEnd(() => {
      signal?.removeEventListener("abort", onAbort);
      if (this.streams.get(streamId) === entry) this.streams.delete(streamId);
      // A slot may have freed up.
      this.pump();
    });

    // A new query shouldn't sit out a reconnect backoff, and it's the one
    // thing that tries again after access was lost.
    this.accessLost = false;
    if (!this.socket) this.ensureSocket(true);
    this.pump();
    return queue;
  }

  events(handler: (event: ConnectionClosedEvent) => void): () => void {
    this.handlers.add(handler);
    this.ensureSocket();
    return () => {
      this.handlers.delete(handler);
    };
  }

  /** Tell the server to stop a started stream. One not started just never starts. */
  private cancel(streamId: string): void {
    const entry = this.streams.get(streamId);
    if (entry?.started && this.socket?.readyState === OPEN) {
      this.socket.send(JSON.stringify({ op: "cancel", streamId }));
    }
  }

  /** Start waiting streams while there are free slots; open the socket if needed. */
  private pump(): void {
    const waiting = [...this.streams.values()].filter((e) => !e.started && !e.queue.ended);
    if (waiting.length === 0) return;
    if (this.socket?.readyState !== OPEN) {
      this.ensureSocket();
      return;
    }
    let running = [...this.streams.values()].filter((e) => e.started && !e.queue.ended).length;
    for (const entry of waiting) {
      if (running >= this.maxStreams) break;
      entry.started = true;
      running += 1;
      this.socket.send(entry.frame);
    }
  }

  /** Open the socket unless one is open or opening. `now` skips a pending backoff. */
  private ensureSocket(now = false): void {
    if (this.socket || this.accessLost) return;
    if (this.reconnectTimer !== null) {
      if (!now) return;
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    let socket: WebSocketLike;
    try {
      socket = this.createSocket(this.url);
    } catch (error) {
      void log.error("Opening the Core stream socket failed:", error);
      this.scheduleReconnect();
      return;
    }
    this.socket = socket;
    let opened = false;
    let stableTimer: ReturnType<typeof setTimeout> | null = null;
    // A socket that closes right after the upgrade (the server refusing it)
    // mustn't reset the backoff, or it would reconnect every half second.
    const proven = () => {
      if (stableTimer !== null) clearTimeout(stableTimer);
      stableTimer = null;
      this.failures = 0;
    };
    socket.onopen = () => {
      opened = true;
      stableTimer = setTimeout(proven, this.stableMs);
      this.pump();
    };
    socket.onmessage = (message) => {
      if (stableTimer !== null) proven();
      this.onMessage(message.data);
    };
    socket.onerror = () => {
      // `close` follows with whatever there is to know.
    };
    socket.onclose = (event) => {
      if (stableTimer !== null) clearTimeout(stableTimer);
      if (this.socket !== socket) return;
      this.socket = null;
      if (event?.code === POLICY_VIOLATION) {
        this.loseAccess();
        return;
      }
      this.failures += 1;
      // Back off first, so the streams ending below don't reopen at once.
      this.scheduleReconnect();
      if (
        event?.code === TRY_AGAIN_LATER &&
        String(event.reason ?? "").includes(TOO_MANY_SOCKETS)
      ) {
        const message = m.core_stream_too_many_tabs();
        for (const entry of this.streams.values()) {
          entry.queue.pushError(streamError(TOO_MANY_TABS, message));
        }
        return;
      }
      for (const entry of this.streams.values()) {
        if (entry.started) {
          entry.queue.pushError(
            streamError(WS_CLOSED, "The connection to the server closed before the query ended"),
          );
        } else if (!opened) {
          // The server couldn't be reached: don't leave a query waiting.
          entry.queue.pushError(
            streamError(WS_CLOSED, "Couldn't reach the server to run the query"),
          );
        }
      }
    };
  }

  /** A 1008 close: end every stream, say so once, and stay closed. */
  private loseAccess(): void {
    this.accessLost = true;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    const message = m.core_stream_access_lost();
    for (const entry of this.streams.values()) {
      entry.queue.pushError(streamError(ACCESS_LOST, message));
    }
    this.onAccessLost(message);
  }

  private scheduleReconnect(): void {
    if (this.accessLost) return;
    const needed = this.handlers.size > 0 || this.streams.size > 0;
    if (!needed || this.reconnectTimer !== null) return;
    const delay = Math.min(
      this.initialDelayMs * 2 ** Math.max(0, this.failures - 1),
      this.maxDelayMs,
    );
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.ensureSocket();
    }, delay);
  }

  private onMessage(data: unknown): void {
    let event: CoreEvent;
    try {
      event = JSON.parse(String(data)) as CoreEvent;
    } catch {
      void log.warn("Ignoring a Core stream frame that isn't JSON");
      return;
    }
    if (event.type === "connectionClosed") {
      for (const handler of this.handlers) handler(event);
      return;
    }
    // A query stream's events come as `stream`, a run's or page's as `run`.
    if (event.type !== "stream" && event.type !== "run") {
      void log.warn(
        `Ignoring a Core stream frame of type ${String((event as { type?: unknown }).type)}`,
      );
      return;
    }
    const entry = this.streams.get(event.streamId);
    if (!entry) {
      if (event.event.type === "error") {
        void log.warn(`Core stream error for no running stream: ${event.event.code}`);
      }
      return;
    }
    if (
      event.event.type === "error" &&
      event.event.code === TOO_MANY_STREAMS &&
      !entry.heard &&
      !entry.queue.ended &&
      entry.retries < this.maxStreamRetries
    ) {
      // A cancelled stream the server hasn't wound down yet still holds its
      // slot. Try this one again, a little later each time.
      entry.started = false;
      const delay = Math.min(this.retryDelayMs * 2 ** entry.retries, this.maxDelayMs);
      entry.retries += 1;
      setTimeout(() => this.pump(), delay);
      return;
    }
    entry.heard = true;
    entry.queue.push(event.event);
  }
}
