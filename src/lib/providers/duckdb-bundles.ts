/**
 * Where DuckDB-WASM comes from, and starting it without hanging.
 */
import type { DuckDBBundles } from "@duckdb/duckdb-wasm";

/** How long the worker gets to load and instantiate before we give up. */
export const DUCKDB_START_TIMEOUT_MS = 30_000;

/**
 * Loads the web build's own copy. The condition is on the build-time
 * constant itself, so Rollup folds it and drops the import (and the ~73 MB
 * of WASM it pulls in) from the desktop and demo builds.
 */
const loadLocalBundles =
  import.meta.env.VITE_BUILD_TARGET === "web"
    ? async () => (await import("./duckdb-local-bundles")).LOCAL_BUNDLES
    : null;

/**
 * The bundles for this build. Web serves its own copy (an air-gapped
 * self-hosted install can't reach a CDN). The demo keeps jsDelivr: it's a
 * static site on seaquel.app whose copy lives in the website repo, and
 * adding ~73 MB of WASM there is a separate decision.
 */
export async function duckdbBundles(
  jsDelivr: () => DuckDBBundles,
  loadLocal: (() => Promise<DuckDBBundles>) | null = loadLocalBundles,
): Promise<DuckDBBundles> {
  return loadLocal ? loadLocal() : jsDelivr();
}

/**
 * The blob worker's script: DuckDB-WASM's worker loaded by `importScripts`,
 * after the worker's console is silenced. DuckDB-WASM 1.32's worker logs
 * every request that fails with `console.log`, a statement's SQL and values
 * included, whatever logger the page passes (`VoidLogger` only quiets the
 * page side), and it has no setting for that (Task 7 probe, item 8). Its
 * errors still reach the page as rejected requests; only the worker's own
 * console output goes.
 *
 * `warn` and `error` are silenced too, on purpose: in the 1.32 worker they
 * print what a query named (`"FAIL WITH: …"` and file errors with the file
 * name; "HEAD request … failed", "fall back to full HTTP read for: <url>"
 * and "Buffering missing file: <name>" with the URL or path a query passed
 * to `read_csv` and friends, a token in its query string included), and its
 * emscripten log hook sends DuckDB's own C++ messages there.
 */
export function duckdbWorkerScript(mainWorkerUrl: string): string {
  return [
    `for (const level of ["log", "info", "debug", "trace", "warn", "error"]) self.console[level] = () => {};`,
    `importScripts(${JSON.stringify(mainWorkerUrl)});`,
  ].join("\n");
}

/** An absolute URL, since `importScripts` in a blob worker has no base. */
export function absoluteUrl(url: string, base: string = globalThis.location?.href ?? ""): string {
  return new URL(url, base || undefined).href;
}

/** The worker events `startWithin` listens for. */
export interface WorkerEvents {
  addEventListener(type: "error", listener: (event: Event) => void): void;
  removeEventListener(type: "error", listener: (event: Event) => void): void;
}

/**
 * `start`, or a rejection if the worker reports an error first (its script
 * failed to load, e.g. the CDN is unreachable) or `timeoutMs` passes.
 * duckdb-wasm doesn't reject pending requests when its worker fails, so
 * without this `instantiate()` would never settle.
 */
export function startWithin<T>(
  start: Promise<T>,
  worker: WorkerEvents,
  timeoutMs: number = DUCKDB_START_TIMEOUT_MS,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const onError = (event: Event) => {
      const message = (event as ErrorEvent).message;
      finish(() =>
        reject(
          new Error(
            `DuckDB failed to start${message ? `: ${message}` : " (its worker couldn't load)"}`,
          ),
        ),
      );
    };
    const finish = (settle: () => void) => {
      if (timer !== undefined) clearTimeout(timer);
      timer = undefined;
      worker.removeEventListener("error", onError);
      settle();
    };
    worker.addEventListener("error", onError);
    timer = setTimeout(
      () =>
        finish(() =>
          reject(new Error(`DuckDB didn't start within ${Math.round(timeoutMs / 1000)} s`)),
        ),
      timeoutMs,
    );
    start.then(
      (value) => finish(() => resolve(value)),
      (error: unknown) => finish(() => reject(error)),
    );
  });
}
