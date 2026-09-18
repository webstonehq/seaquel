/**
 * Centralized provider lifecycle manager.
 * Replaces duplicated getOrCreate/getProviderFor patterns across managers.
 *
 * Three modes:
 *   - Tauri desktop          → UnifiedTauriProvider  (IPC)
 *   - Web (hosted/self-host) → HttpProvider          (HTTP + WS to seaquel-server)
 *   - Demo (browser)         → DuckDBProvider        (DuckDB-WASM only — the demo
 *                                                     UI disables newConnections,
 *                                                     so no other driver is ever
 *                                                     selectable)
 */

import type { DatabaseProvider } from "./types";
import { getProvider, getDuckDBProvider } from "./index";
import { isTauri, isWeb } from "$lib/utils/environment";

export class ProviderRegistry {
  private provider: DatabaseProvider | null = null;
  private duckdbProvider: DatabaseProvider | null = null;

  /**
   * Get the appropriate provider for a given database type.
   * Lazily initializes and caches provider instances.
   */
  async getForType(_dbType: string): Promise<DatabaseProvider> {
    // Tauri and Web both have a single unified provider that handles every
    // driver (the server or Rust sidecar dispatches internally).
    if (isTauri() || isWeb()) {
      return this.getOrCreateDefault();
    }
    // Demo mode: DuckDB-WASM is the only in-browser engine. Non-duckdb types
    // would fail at connect, but the demo UI prevents that from being reached.
    return this.getOrCreateDuckDB();
  }

  /**
   * Get or create the default database provider.
   */
  async getOrCreateDefault(): Promise<DatabaseProvider> {
    if (!this.provider) {
      this.provider = await getProvider();
    }
    return this.provider;
  }

  /**
   * Get or create the DuckDB provider (WASM in demo, HTTP in web, Tauri on desktop).
   */
  async getOrCreateDuckDB(): Promise<DatabaseProvider> {
    if (!this.duckdbProvider) {
      this.duckdbProvider = await getDuckDBProvider();
    }
    return this.duckdbProvider;
  }

  /**
   * Reset cached provider instances.
   * Call on disconnect or cleanup.
   */
  reset(): void {
    this.provider = null;
    this.duckdbProvider = null;
  }
}
