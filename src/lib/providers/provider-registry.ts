/**
 * Centralized provider lifecycle manager: one provider for every driver,
 * Core (embedded on desktop, `seaquel-server` on web, the in-page module in
 * the demo). The web server has no SQLite or DuckDB, and `ConnectionManager`
 * refuses those types on web before reaching here.
 */

import type { DatabaseProvider } from "./types";
import { getProvider } from "./index";

export class ProviderRegistry {
  private provider: DatabaseProvider | null = null;

  /** The provider for a database type (Core for every type). */
  async getForType(_dbType: string): Promise<DatabaseProvider> {
    return this.getOrCreateDefault();
  }

  /** Get or create the default database provider. */
  async getOrCreateDefault(): Promise<DatabaseProvider> {
    if (!this.provider) {
      this.provider = await getProvider();
    }
    return this.provider;
  }

  /**
   * Reset cached provider instances.
   * Call on disconnect or cleanup.
   */
  reset(): void {
    this.provider = null;
  }
}
