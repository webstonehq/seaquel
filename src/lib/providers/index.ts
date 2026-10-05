/**
 * Database provider factory. Every build's connections go through Core
 * (`CoreProvider`): embedded on desktop, `seaquel-server` on web, and the
 * module in the demo's page (phase 8).
 *
 * The tutorial is the exception: on web and in the demo it runs on
 * DuckDB-WASM in the page (`getDuckDBProvider`), since the web server
 * has no DuckDB engine. On desktop it uses Core's SQLite.
 */

import type { DatabaseProvider } from "./types";
import type { TutorialProvider } from "./duckdb-provider";

export type { DatabaseProvider, ConnectRequest, ExecuteResult, ReadOnlyRows } from "./types";
export type { TutorialProvider } from "./duckdb-provider";

let provider: DatabaseProvider | null = null;
let duckdbProvider: TutorialProvider | null = null;

/** The page's provider: Core. */
export async function getProvider(): Promise<DatabaseProvider> {
  if (provider) return provider;
  const { CoreProvider } = await import("./core-provider");
  provider = new CoreProvider();
  return provider;
}

/**
 * Loads the tutorial's DuckDB-WASM provider. The branch is on the
 * build-time constant itself, so Rollup drops it (and DuckDB-WASM) from the
 * desktop build, whose tutorial runs on Core's SQLite.
 */
const loadDuckDBProvider =
  import.meta.env.VITE_BUILD_TARGET === "web" || import.meta.env.VITE_BUILD_TARGET === "demo"
    ? () => import("./duckdb-provider")
    : null;

/** The tutorial's DuckDB-WASM in the page (web and the demo builds only). */
export async function getDuckDBProvider(): Promise<TutorialProvider> {
  if (duckdbProvider) return duckdbProvider;
  if (!loadDuckDBProvider) {
    throw new Error("The tutorial's DuckDB-WASM is only in the web and demo builds");
  }
  const { DuckDBProvider } = await loadDuckDBProvider();
  duckdbProvider = new DuckDBProvider();
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
