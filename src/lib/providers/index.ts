/**
 * Database provider factory.
 * Returns the appropriate provider based on the runtime environment.
 *
 * Three modes:
 *   - Tauri desktop          → CoreProvider   (Core over `core_call`/`core_stream`)
 *   - Web (hosted/self-host) → CoreProvider   (Core over `/api/rpc` and its WebSocket)
 *   - Demo (browser)         → DuckDBProvider (DuckDB-WASM)
 */

import type { DatabaseProvider } from "./types";
import { isTauri, isWeb } from "$lib/utils/environment";

export type { DatabaseProvider, ConnectRequest, ExecuteResult, ReadOnlyRows } from "./types";

let provider: DatabaseProvider | null = null;
let duckdbProvider: DatabaseProvider | null = null;

/**
 * Get the database provider for the current environment.
 */
export async function getProvider(): Promise<DatabaseProvider> {
  if (provider) return provider;

  if (isTauri() || isWeb()) {
    const { CoreProvider } = await import("./core-provider");
    provider = new CoreProvider();
  } else {
    const { DuckDBProvider } = await import("./duckdb-provider");
    provider = new DuckDBProvider();
  }

  return provider;
}

/**
 * Get the DuckDB provider for the current environment: Core on desktop, and
 * DuckDB-WASM in the browser, both in the demo and on web. The web server has no DuckDB engine (Decision 11b: it would read and
 * write the server's files), so web's in-browser DuckDB (the tutorial) runs
 * in the page.
 */
export async function getDuckDBProvider(): Promise<DatabaseProvider> {
  if (duckdbProvider) return duckdbProvider;

  if (isTauri()) {
    const { CoreProvider } = await import("./core-provider");
    duckdbProvider = new CoreProvider();
  } else {
    const { DuckDBProvider } = await import("./duckdb-provider");
    duckdbProvider = new DuckDBProvider();
  }

  return duckdbProvider;
}

/**
 * Reset the provider instance.
 * Mainly useful for testing.
 */
export function resetProvider(): void {
  provider = null;
  duckdbProvider = null;
}

export { ProviderRegistry } from "./provider-registry";
export { isDemo } from "$lib/utils/environment";
