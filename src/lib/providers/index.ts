/**
 * Database provider factory.
 * Returns the appropriate provider based on the runtime environment.
 *
 * Three modes:
 *   - Tauri desktop         → UnifiedTauriProvider (IPC to embedded Rust)
 *   - Web (hosted/self-host) → HttpProvider        (HTTP + WS to seaquel-server)
 *   - Demo (browser)         → DuckDBProvider / WebSqliteDatabaseProvider (WASM)
 */

import type { DatabaseProvider } from "./types";
import { isTauri, isWeb } from "$lib/utils/environment";

export type { DatabaseProvider, ConnectionConfig, ExecuteResult } from "./types";

let provider: DatabaseProvider | null = null;
let duckdbProvider: DatabaseProvider | null = null;

/**
 * Get the database provider for the current environment.
 */
export async function getProvider(): Promise<DatabaseProvider> {
  if (provider) return provider;

  if (isTauri()) {
    const { UnifiedTauriProvider } = await import("./unified-tauri-provider");
    provider = new UnifiedTauriProvider();
  } else if (isWeb()) {
    const { HttpProvider } = await import("./http-provider");
    provider = new HttpProvider();
  } else {
    const { DuckDBProvider } = await import("./duckdb-provider");
    provider = new DuckDBProvider();
  }

  return provider;
}

/**
 * Get the DuckDB provider for the current environment.
 *
 * In web mode, all database types (including DuckDB) route through the same
 * HttpProvider — the server handles driver dispatch.
 */
export async function getDuckDBProvider(): Promise<DatabaseProvider> {
  if (duckdbProvider) return duckdbProvider;

  if (isTauri()) {
    const { UnifiedTauriProvider } = await import("./unified-tauri-provider");
    duckdbProvider = new UnifiedTauriProvider();
  } else if (isWeb()) {
    const { HttpProvider } = await import("./http-provider");
    duckdbProvider = new HttpProvider();
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
