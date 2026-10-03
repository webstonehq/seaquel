/**
 * `CoreClient` on desktop:
 * - `call`: the `core_call` command, the request's JSON as raw bytes;
 * - `stream`: `core_stream({request, channel})` for `db.queryStream`,
 *   `db.run` and `db.page` (a run's events arrive as `type: "run"`). The
 *   request goes as a JSON
 *   string, since an invoke with a raw bytes body can't carry a channel.
 *   The command resolves when the stream ends, with the number of events it
 *   sent. Its reply can overtake them, so the stream ends only once that
 *   many have arrived (Tauri delivers a channel's messages in order); if
 *   none was a `done` or `error`, it was cancelled. Cancel is `db.cancel`;
 * - `events`: `core_events({channel})` (`connectionClosed` and
 *   `storageChanged`); `onResubscribed` runs when that registration
 *   succeeds (the page gets nothing from before it), and
 *   `onEventsUnavailable` when it fails. Registered once per page load (the
 *   desktop keeps one sink per webview, and a second registration from the
 *   same webview counts as a reload and cancels its streams).
 */

import { Channel, invoke } from "@tauri-apps/api/core";
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import type { CoreResponse } from "$lib/types/generated/CoreResponse";
import { encodeCoreRequest, tauriCoreTransport } from "$lib/storage/rust-client";
import { log } from "$lib/utils/logger";
import {
  callDb,
  cancelledEvent,
  errorEvent,
  StreamQueue,
  isWorkspaceEvent,
  isStreamFrame,
  type EventsUnavailableReason,
  type ResubscribedInfo,
  type WorkspaceEvent,
  type CoreClient,
  type CoreEvent,
  type EventOf,
  type StreamOptions,
  type StreamRequest,
  wellFormedRequest,
} from "./client";

export class TauriCoreClient implements CoreClient {
  private readonly handlers = new Set<(event: WorkspaceEvent) => void>();
  private readonly resubscribedHandlers = new Set<(info: ResubscribedInfo) => void>();
  private readonly unavailableHandlers = new Set<(reason: EventsUnavailableReason) => void>();
  /** `core_events` registrations that succeeded on this page. */
  private registrations = 0;
  /** Kept so the channel isn't collected while the page lives. */
  private eventsChannel: Channel<CoreEvent> | null = null;

  /**
   * @param safetyMs How long a stream whose `core_stream` resolved waits,
   *   with no message arriving, for events still missing before it ends as
   *   cancelled.
   */
  constructor(private readonly safetyMs = 30_000) {}

  async call(request: CoreRequest): Promise<CoreResponse> {
    return (await tauriCoreTransport(encodeCoreRequest(request))) as CoreResponse;
  }

  stream<R extends StreamRequest>(
    request: R,
    options: StreamOptions = {},
  ): AsyncIterable<EventOf<R>> {
    const { signal } = options;
    const streamId = request.params.params.streamId;
    const queue = new StreamQueue<EventOf<R>>(() => this.cancel(streamId));
    if (signal?.aborted) {
      // Nothing started, so nothing to cancel.
      queue.pushError(cancelledEvent());
      return queue;
    }

    // Channel messages that arrived (all of them: the channel is this
    // stream's alone), and how many `core_stream` says it sent (once it
    // resolved).
    let received = 0;
    let sent: number | null = null;
    let safety: ReturnType<typeof setTimeout> | null = null;
    const finishIfAllIn = () => {
      if (sent === null) return;
      if (safety !== null) clearTimeout(safety);
      safety = null;
      if (received >= sent) {
        queue.finish();
      } else {
        // Tauri delivers a channel's messages, so this shouldn't fire; if
        // some never come, don't hang the query forever. An idle wait: each
        // message that arrives starts it again.
        safety = setTimeout(() => {
          void log.warn(
            `core_stream ${streamId}: ${received} of ${sent} events arrived; ending it as cancelled`,
          );
          queue.finish();
        }, this.safetyMs);
      }
    };

    const channel = new Channel<CoreEvent>();
    channel.onmessage = (message) => {
      received += 1;
      // A query stream's events come as `stream`, a run's or page's as `run`,
      // a turn's as `ai`.
      if (isStreamFrame(message) && message.streamId === streamId) {
        queue.push(message.event as EventOf<R>);
      }
      finishIfAllIn();
    };

    const onAbort = () => {
      this.cancel(streamId);
      queue.pushError(cancelledEvent());
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    queue.onEnd(() => {
      signal?.removeEventListener("abort", onAbort);
      if (safety !== null) clearTimeout(safety);
      // Drop late events; the channel itself goes when the command returns.
      channel.onmessage = () => {};
    });

    invoke<number>("core_stream", {
      request: JSON.stringify(wellFormedRequest(request)),
      channel,
    }).then(
      (count) => {
        sent = typeof count === "number" ? count : 0;
        finishIfAllIn();
      },
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
    if (!this.eventsChannel) {
      const channel = new Channel<CoreEvent>();
      channel.onmessage = (event) => {
        if (!isWorkspaceEvent(event)) return;
        for (const h of this.handlers) h(event);
      };
      this.eventsChannel = channel;
      invoke<void>("core_events", { channel }).then(
        () => {
          const info = { initial: this.registrations === 0 };
          this.registrations += 1;
          for (const h of this.resubscribedHandlers) h(info);
        },
        (error: unknown) => {
          void log.error("Registering for Core events failed:", error);
          this.eventsChannel = null;
          for (const h of this.unavailableHandlers) h("EVENTS_UNAVAILABLE");
        },
      );
    }
    return () => {
      this.handlers.delete(handler);
    };
  }

  onResubscribed(handler: (info: ResubscribedInfo) => void): () => void {
    this.resubscribedHandlers.add(handler);
    return () => {
      this.resubscribedHandlers.delete(handler);
    };
  }

  onEventsUnavailable(handler: (reason: EventsUnavailableReason) => void): () => void {
    this.unavailableHandlers.add(handler);
    return () => {
      this.unavailableHandlers.delete(handler);
    };
  }
}
