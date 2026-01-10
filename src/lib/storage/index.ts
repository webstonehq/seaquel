/**
 * Storage provider factory.
 * Returns the appropriate storage provider based on the runtime environment.
 */

import type { StorageProvider, Store, StoreLoadOptions } from './types';
import { isTauri } from '$lib/utils/environment';

export type { StorageProvider, Store, StoreLoadOptions } from './types';

let provider: StorageProvider | null = null;

/**
 * Get the storage provider for the current environment.
 * Returns TauriStorageProvider in desktop app, WebStorageProvider in browser.
 */
export async function getStorageProvider(): Promise<StorageProvider> {
	if (provider) return provider;

	if (isTauri()) {
		const { TauriStorageProvider } = await import('./tauri-storage');
		provider = new TauriStorageProvider();
	} else {
		const { WebStorageProvider } = await import('./web-storage');
		provider = new WebStorageProvider();
	}

	return provider;
}

/**
 * Convenience function to load a store directly.
 * Uses the appropriate storage provider for the current environment.
 */
export async function loadStore(name: string, options?: StoreLoadOptions): Promise<Store> {
	const storageProvider = await getStorageProvider();
	return storageProvider.load(name, options);
}

/**
 * Reset the storage provider instance.
 * Mainly useful for testing.
 */
export function resetStorageProvider(): void {
	provider = null;
}
