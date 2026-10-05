/**
 * The demo's fetch bridge: the page's
 * half of the module's model calls (`crates/seaquel-browser/src/fetch.rs`).
 * `openBrowserCore` passes it to the module's `open`, and `BrowserCore`
 * passes it again to every instance a trap restart opens.
 *
 * - `start(id, …)` sends the request Rust built (headers and body as given,
 *   the visitor's key among the headers) and resolves with the status once
 *   the head arrives. No cookie, no referrer, no cache, and no redirect is
 *   followed (a redirect fails the call).
 * - `read(id)` gives the next body chunk, or `null` at the end (and for an
 *   id that's finished, aborted or unknown).
 * - `abort(id)` stops the request: before the head (the fetch's signal) or
 *   after it (the body is cancelled, so the provider sees the client go and
 *   stops generating). Rust calls it when a turn is dropped or stopped.
 * - `abortAll()` stops every request still running. A trap restart calls
 *   it (`BrowserCore`'s `reinstantiate`): the dead instance can't drop its
 *   requests, so without it the provider would go on generating.
 *
 * Nothing here logs, and a failure rejects with fixed text: the browser's
 * message can name the URL. The key is never kept past the call: the
 * headers go to `fetch` and nowhere else.
 *
 * Demo only: imported by `$lib/core/browser`, which only the demo's start
 * loads (behind `import.meta.env.VITE_BUILD_TARGET === "demo"`).
 */
import type { FetchBridge } from "./transport";

interface Running {
  controller: AbortController;
  reader?: ReadableStreamDefaultReader<Uint8Array>;
}

export function makeFetchBridge(
  fetchFn: typeof fetch = (input, init) => globalThis.fetch(input, init),
): FetchBridge & { abortAll(): void } {
  const running = new Map<number, Running>();

  const stop = (id: number) => {
    const entry = running.get(id);
    if (!entry) return;
    running.delete(id);
    entry.controller.abort();
    entry.reader?.cancel().catch(() => {});
  };

  return {
    async start(id, method, url, headers, body) {
      stop(id);
      const entry: Running = { controller: new AbortController() };
      running.set(id, entry);
      const hasBody = method !== "GET" && method !== "HEAD" && body.byteLength > 0;
      let response: Response;
      try {
        response = await fetchFn(url, {
          method,
          headers,
          body: hasBody ? (body as Uint8Array<ArrayBuffer>) : undefined,
          signal: entry.controller.signal,
          credentials: "omit",
          referrerPolicy: "no-referrer",
          cache: "no-store",
          redirect: "error",
        });
      } catch {
        if (running.get(id) === entry) running.delete(id);
        throw new Error("The model request failed");
      }
      if (running.get(id) !== entry) {
        // Aborted while the head was on its way.
        response.body?.cancel().catch(() => {});
        throw new Error("The model request was aborted");
      }
      entry.reader = response.body?.getReader();
      return response.status;
    },

    async read(id) {
      const entry = running.get(id);
      if (!entry?.reader) {
        running.delete(id);
        return null;
      }
      try {
        const { done, value } = await entry.reader.read();
        if (done) {
          if (running.get(id) === entry) running.delete(id);
          return null;
        }
        return value;
      } catch {
        if (running.get(id) === entry) running.delete(id);
        throw new Error("The model's response failed");
      }
    },

    abort(id) {
      stop(id);
    },

    abortAll() {
      // Deleting the entry being visited is safe in a Map iteration.
      for (const id of running.keys()) stop(id);
    },
  };
}
