/**
 * Tauri storage provider.
 * Wraps @tauri-apps/plugin-store for desktop app persistence.
 */

import { load as tauriLoad } from '@tauri-apps/plugin-store';
import type { StorageProvider, Store, StoreLoadOptions } from './types';

/**
 * Wraps a Tauri store to implement the Store interface.
 */
class TauriStore implements Store {
	constructor(
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		private store: Awaited<ReturnType<typeof tauriLoad>>
	) {}

	async get<T>(key: string): Promise<T | null> {
		return (await this.store.get(key)) as T | null;
	}

	async set<T>(key: string, value: T): Promise<void> {
		await this.store.set(key, value);
	}

	async save(): Promise<void> {
		await this.store.save();
	}

	async clear(): Promise<void> {
		await this.store.clear();
	}
}

/**
 * Storage provider that uses Tauri's store plugin.
 * Stores data in JSON files in the app data directory.
 */
export class TauriStorageProvider implements StorageProvider {
	readonly id = 'tauri';

	isAvailable(): boolean {
		return typeof window !== 'undefined' && '__TAURI__' in window;
	}

	async load(name: string, options?: StoreLoadOptions): Promise<Store> {
		const store = await tauriLoad(name, {
			autoSave: options?.autoSave ?? true,
			defaults: options?.defaults ?? {}
		});
		return new TauriStore(store);
	}
}
