/**
 * `CoreClient` in the demo (phase 8): Core runs in the page, so
 * every call goes to `BrowserCore` (`./transport.ts`):
 *
 * - `call`: the request's bytes to the module, the `CoreResponse` back; a
 *   refusal rejects with a `CoreCallError` as on desktop and web.
 * - `stream`: the module's events for that stream, ending with exactly one
 *   `done` or `error`. Aborting the signal, or leaving the loop early, sends
 *   `db.cancel` and ends it with `CANCELLED`; a stream that ends without a
 *   terminal event (Core cancelled it) ends the same way; a trap ends it
 *   with `CORE_RESTARTED`.
 * - `events`: `connectionClosed` and `storageChanged`. `onResubscribed`
 *   runs with `initial: true` once the first subscription is in, and with
 *   `initial: false` after each restart. A restart also closes every
 *   connection this page opened: each gets a `connectionClosed` with code
 *   `CORE_RESTARTED`, so the GUI marks it disconnected and reconnects it.
 * - `onEventsUnavailable` never fires: events can't stop arriving in the
 *   page without the module restarting.
 */
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import type { CoreResponse } from "$lib/types/generated/CoreResponse";
import { encodeCoreRequest } from "$lib/storage/rust-client";
import { log } from "$lib/utils/logger";
import {
  callDb,
  cancelledEvent,
  errorEvent,
  isWorkspaceEvent,
  isStreamFrame,
  StreamQueue,
  wellFormedRequest,
  type CoreClient,
  type CoreEvent,
  type EventOf,
  type EventsUnavailableReason,
  type ResubscribedInfo,
  type StreamOptions,
  type StreamRequest,
  type WorkspaceEvent,
} from "../client";
import { CORE_RESTARTED, type BrowserCore } from "./transport";

class BrowserCoreClient implements CoreClient {
  private readonly handlers = new Set<(event: WorkspaceEvent) => void>();
  private readonly resubscribedHandlers = new Set<(info: ResubscribedInfo) => void>();
  private subscribed = false;
  /** Core connection ids this page opened and hasn't closed. */
  private readonly connections = new Set<string>();

  constructor(private readonly core: BrowserCore) {
    core.onRestarted(() => this.restarted());
  }

  async call(request: CoreRequest): Promise<CoreResponse> {
    const response = JSON.parse(await this.core.call(encodeCoreRequest(request))) as CoreResponse;
    this.track(request, response);
    return response;
  }

  /** Keeps the ids `db.connect` gave out, so a restart can close them. */
  private track(request: CoreRequest, response: CoreResponse): void {
    if (request.method !== "db" || response.method !== "db") return;
    const inner = request.params;
    if (inner.method === "connect" && response.result.method === "connect") {
      this.connections.add(response.result.result.connectionId);
    } else if (inner.method === "disconnect") {
      this.connections.delete(inner.params.connectionId);
    }
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
    const onAbort = () => {
      this.cancel(streamId);
      queue.pushError(cancelledEvent());
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    queue.onEnd(() => signal?.removeEventListener("abort", onAbort));

    const body = encodeCoreRequest(wellFormedRequest(request) as unknown as CoreRequest);
    this.core
      .stream(body, (event: CoreEvent) => {
        if (isStreamFrame(event) && event.streamId === streamId) {
          queue.push(event.event as EventOf<R>);
        }
      })
      .then(
        () => queue.finish(),
        (error: unknown) => queue.pushError(errorEvent(error)),
      );
    return queue;
  }

  /** `db.cancel`, fire and forget: a stream that already ended ignores it. */
  private cancel(streamId: string): void {
    callDb(this, "cancel", { streamId }).catch((error: unknown) => {
      void log.warn("Cancelling a stream failed:", error);
    });
  }

  events(handler: (event: WorkspaceEvent) => void): () => void {
    this.handlers.add(handler);
    if (!this.subscribed) {
      this.subscribed = true;
      this.core.subscribe((event) => {
        if (!isWorkspaceEvent(event)) return;
        for (const h of this.handlers) h(event);
      });
      // As on desktop: the registration is in, so the page reloads what it
      // shows. Later, so a handler registered right after this one hears it.
      queueMicrotask(() => {
        for (const h of this.resubscribedHandlers) h({ initial: true });
      });
    }
    return () => {
      this.handlers.delete(handler);
    };
  }

  private restarted(): void {
    // The new instance has none of the old connections.
    const closed = [...this.connections];
    this.connections.clear();
    for (const connectionId of closed) {
      const event: WorkspaceEvent = {
        type: "connectionClosed",
        connectionId,
        code: CORE_RESTARTED,
        message: "Seaquel restarted after an internal error, which closed this connection.",
      };
      for (const h of this.handlers) h(event);
    }
    if (this.subscribed) {
      for (const h of this.resubscribedHandlers) h({ initial: false });
    }
  }

  onResubscribed(handler: (info: ResubscribedInfo) => void): () => void {
    this.resubscribedHandlers.add(handler);
    return () => {
      this.resubscribedHandlers.delete(handler);
    };
  }

  onEventsUnavailable(_handler: (reason: EventsUnavailableReason) => void): () => void {
    return () => {};
  }
}

/** The page's `CoreClient` over `core`. */
export function browserCoreClient(core: BrowserCore): CoreClient {
  return new BrowserCoreClient(core);
}
