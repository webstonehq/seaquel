/**
 * Database provider factory.
 * Returns the appropriate provider based on the runtime environment.
 */

import type { DatabaseProvider } from './types';
import { isTauri } from '$lib/utils/environment';

export type { DatabaseProvider, ConnectionConfig, ExecuteResult } from './types';

let provider: DatabaseProvider | null = null;

/**
 * Get the database provider for the current environment.
 * Returns TauriDatabaseProvider in desktop app, DuckDBProvider in browser.
 */
export async function getProvider(): Promise<DatabaseProvider> {
	if (provider) return provider;

	if (isTauri()) {
		const { TauriDatabaseProvider } = await import('./tauri-provider');
		provider = new TauriDatabaseProvider();
	} else {
		const { DuckDBProvider } = await import('./duckdb-provider');
		provider = new DuckDBProvider();
	}

	return provider;
}

/**
 * Check if we're in demo mode (browser, not Tauri).
 */
export function isDemo(): boolean {
	return !isTauri();
}

/**
 * Reset the provider instance.
 * Mainly useful for testing.
 */
export function resetProvider(): void {
	provider = null;
}
