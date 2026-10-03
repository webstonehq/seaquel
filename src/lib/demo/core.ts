/**
 * Core in the demo's page (phase 8, Decisions 2 and 19): the module opened
 * once, before the app renders (`src/routes/+layout.ts`), over the page's
 * DuckDB-WASM, and made the page's `CoreClient` and storage transport.
 *
 * Demo builds only: import it behind `import.meta.env.VITE_BUILD_TARGET ===
 * "demo"`, so desktop and web bundles never contain the module, the bridge
 * or the transport.
 *
 * DuckDB-WASM starts at the same time but isn't waited for: Core opens
 * without it (the metadata file is SQLite in the module), and the bridge
 * waits for DuckDB only on its first `connect`. A DuckDB that fails to start
 * fails that connect with the reason, and the next connect tries again.
 *
 * The assistant's model calls go through the page's `fetch`
 * (`makeFetchBridge`, which `openBrowserCore` passes to the module when
 * none is given, and again after each trap restart; phase 6 Task 8).
 */
import {
  makeDuckDbBridge,
  openBrowserCore,
  useBrowserCore,
  type OpenedBrowserCore,
} from "$lib/core/browser";
import type { BridgeDuckDb } from "$lib/core/browser/duckdb-bridge";
import { pageDuckDb } from "$lib/providers/duckdb-wasm";

let opening: Promise<OpenedBrowserCore> | null = null;
let opened: OpenedBrowserCore | null = null;

/** Opens the demo's Core once per page; a failed open is tried again on the next call. */
export function openDemoCore(): Promise<OpenedBrowserCore> {
  opening ??= open().catch((error: unknown) => {
    opening = null;
    throw error;
  });
  return opening;
}

/** The page's Core, once `openDemoCore` resolved. */
export function demoCore(): OpenedBrowserCore | null {
  return opened;
}

async function open(): Promise<OpenedBrowserCore> {
  // Started now, reported by the demo's connect if it fails.
  pageDuckDb().catch(() => {});
  const core = await openBrowserCore({ bridge: makeDuckDbBridge(lazyDuckDb(pageDuckDb)) });
  useBrowserCore(core);
  opened = core;
  return core;
}

/**
 * The page's DuckDB-WASM as the bridge uses it, started on the first
 * `connect`. Every other method names a connection, which only a `connect`
 * that resolved can have given, so they call the started instance directly:
 * they keep the bridge's contract (a request is posted before its promise
 * returns), which the driver's cancel and ROLLBACK on drop rely on.
 */
export function lazyDuckDb(start: () => Promise<BridgeDuckDb>): BridgeDuckDb {
  let db: BridgeDuckDb | null = null;
  const started = (): BridgeDuckDb => {
    if (!db) throw new Error("DuckDB hasn't started");
    return db;
  };
  return {
    connectInternal: () =>
      db
        ? db.connectInternal()
        : start().then((ready) => {
            db = ready;
            return ready.connectInternal();
          }),
    runQuery: (connection, sql) => started().runQuery(connection, sql),
    startPendingQuery: (connection, sql, allowStreamResult) =>
      started().startPendingQuery(connection, sql, allowStreamResult),
    pollPendingQuery: (connection) => started().pollPendingQuery(connection),
    fetchQueryResults: (connection) => started().fetchQueryResults(connection),
    cancelPendingQuery: (connection) => started().cancelPendingQuery(connection),
    disconnect: (connection) => started().disconnect(connection),
    // The bridge's liveness ping. While DuckDB is still starting, the start
    // has its own timeout (`startWithin`), so it counts as alive here.
    getVersion: () => (db ? (db.getVersion?.() ?? Promise.resolve("")) : Promise.resolve("")),
  };
}
