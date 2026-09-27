/**
 * `CoreClient` on desktop:
 * - `call`: the `core_call` command, the request's JSON as raw bytes;
 * - `stream`: `core_stream({request, channel})`. The request goes as a JSON
 *   string, since an invoke with a raw bytes body can't carry a channel.
 *   The command resolves when the stream ends, with the number of events it
 *   sent. Its reply can overtake them, so the stream ends only once that
 *   many have arrived (Tauri delivers a channel's messages in order); if
 *   none was a `done` or `error`, it was cancelled. Cancel is `db.cancel`;
 * - `events`: `core_events({channel})`, registered once per page load (the
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
  type ConnectionClosedEvent,
  type CoreClient,
  type CoreEvent,
  type QueryStreamRequest,
  type StreamEvent,
  type StreamOptions,
} from "./client";

export class TauriCoreClient implements CoreClient {
  private readonly handlers = new Set<(event: ConnectionClosedEvent) => void>();
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

  stream(request: QueryStreamRequest, options: StreamOptions = {}): AsyncIterable<StreamEvent> {
    const { signal } = options;
    const streamId = request.params.params.streamId;
    const queue = new StreamQueue(() => this.cancel(streamId));
    if (signal?.aborted) {
      // Nothing started, so nothing to cancel.
      queue.push(cancelledEvent());
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
      if (message.type === "stream" && message.streamId === streamId) queue.push(message.event);
      finishIfAllIn();
    };

    const onAbort = () => {
      this.cancel(streamId);
      queue.push(cancelledEvent());
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    queue.onEnd(() => {
      signal?.removeEventListener("abort", onAbort);
      if (safety !== null) clearTimeout(safety);
      // Drop late events; the channel itself goes when the command returns.
      channel.onmessage = () => {};
    });

    invoke<number>("core_stream", { request: JSON.stringify(request), channel }).then(
      (count) => {
        sent = typeof count === "number" ? count : 0;
        finishIfAllIn();
      },
      (error: unknown) => queue.push(errorEvent(error)),
    );
    return queue;
  }

  /** `db.cancel`, fire and forget: a stream that already ended ignores it. */
  private cancel(streamId: string): void {
    callDb(this, "cancel", { streamId }).catch((error: unknown) => {
      void log.warn("Cancelling a stream failed:", error);
    });
  }

  events(handler: (event: ConnectionClosedEvent) => void): () => void {
    this.handlers.add(handler);
    if (!this.eventsChannel) {
      const channel = new Channel<CoreEvent>();
      channel.onmessage = (event) => {
        if (event.type !== "connectionClosed") return;
        for (const h of this.handlers) h(event);
      };
      this.eventsChannel = channel;
      invoke<void>("core_events", { channel }).catch((error: unknown) => {
        void log.error("Registering for Core events failed:", error);
        this.eventsChannel = null;
      });
    }
    return () => {
      this.handlers.delete(handler);
    };
  }
}
