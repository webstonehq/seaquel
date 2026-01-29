/**
 * Centralized provider lifecycle manager.
 * Replaces duplicated getOrCreate/getProviderFor patterns across managers.
 */

import type { DatabaseProvider } from './types';
import { getProvider, getDuckDBProvider } from './index';

export class ProviderRegistry {
  private provider: DatabaseProvider | null = null;
  private duckdbProvider: DatabaseProvider | null = null;

  /**
   * Get the appropriate provider for a given database type.
   * Lazily initializes and caches provider instances.
   */
  async getForType(dbType: string): Promise<DatabaseProvider> {
    if (dbType === 'duckdb') {
      return this.getOrCreateDuckDB();
    }
    return this.getOrCreateDefault();
  }

  /**
   * Get or create the default database provider (PostgreSQL/SQLite).
   */
  async getOrCreateDefault(): Promise<DatabaseProvider> {
    if (!this.provider) {
      this.provider = await getProvider();
    }
    return this.provider;
  }

  /**
   * Get or create the DuckDB provider.
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
