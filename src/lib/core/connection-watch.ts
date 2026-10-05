/**
 * One place that hears about connections Core no longer holds. Every `CoreClient` the page gets from
 * `getCoreClient()` goes through `watchClient`: a `db` call that rejects
 * with `CONNECTION_NOT_FOUND`, or a stream (a query stream, a run, a page,
 * a table page, a turn) that ends with it, names the Core connection id
 * the request carried to every `onConnectionNotFound` handler.
 * `ConnectionManager` listens, marks that connection disconnected and
 * reconnects it once, quietly.
 *
 * The failed call is never retried here: a write must not run twice, so
 * the user runs it again. `db.disconnect` isn't reported (the page asked
 * for it to go), nor is `db.alive`, which asks exactly this.
 */
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import { errorCode, type CoreClient, type StreamRequest } from "./client";

export const CONNECTION_NOT_FOUND = "CONNECTION_NOT_FOUND";

/** Methods whose `CONNECTION_NOT_FOUND` says nothing new. */
const NOT_REPORTED = new Set(["disconnect", "alive"]);

const handlers = new Set<(connectionId: string) => void>();

/** Hear about each Core connection id a call found gone; returns the unsubscribe. */
export function onConnectionNotFound(handler: (connectionId: string) => void): () => void {
  handlers.add(handler);
  return () => handlers.delete(handler);
}

/** Tell every handler that Core no longer holds `connectionId`. */
export function reportConnectionNotFound(connectionId: string): void {
  // A handler may unsubscribe while this runs; a Set's iteration allows it.
  for (const handler of handlers) {
    try {
      handler(connectionId);
    } catch {
      // A handler's failure is its own; the call's error stands.
    }
  }
}

/** The Core connection id a `db` or `ai` request names, if any. */
function connectionIdOf(request: CoreRequest | StreamRequest): string | null {
  const outer = request as {
    method?: unknown;
    params?: { method?: unknown; params?: unknown };
  };
  if (outer.method !== "db" && outer.method !== "ai") return null;
  const method = outer.params?.method;
  if (typeof method !== "string" || NOT_REPORTED.has(method)) return null;
  const params = outer.params?.params as { connectionId?: unknown } | undefined;
  return typeof params?.connectionId === "string" ? params.connectionId : null;
}

const wrapped = new WeakMap<CoreClient, CoreClient>();

/** `client`, reporting `CONNECTION_NOT_FOUND`s (one wrapper per client). */
export function watchClient(client: CoreClient): CoreClient {
  const known = wrapped.get(client);
  if (known) return known;
  const watched: CoreClient = {
    async call(request) {
      try {
        return await client.call(request);
      } catch (error) {
        if (errorCode(error) === CONNECTION_NOT_FOUND) {
          const id = connectionIdOf(request);
          if (id) reportConnectionNotFound(id);
        }
        throw error;
      }
    },
    stream(request, options) {
      const inner = client.stream(request, options);
      return {
        async *[Symbol.asyncIterator]() {
          for await (const event of inner) {
            if (
              event.type === "error" &&
              (event as { code?: unknown }).code === CONNECTION_NOT_FOUND
            ) {
              const id = connectionIdOf(request);
              if (id) reportConnectionNotFound(id);
            }
            yield event;
          }
        },
      };
    },
    events: (handler) => client.events(handler),
    onResubscribed: (handler) => client.onResubscribed(handler),
    onEventsUnavailable: (handler) => client.onEventsUnavailable(handler),
  };
  wrapped.set(client, watched);
  return watched;
}
