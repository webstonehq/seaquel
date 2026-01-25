/**
 * Tauri storage provider.
 * Wraps @tauri-apps/plugin-store for desktop app persistence.
 */

import { load as tauriLoad } from '@tauri-apps/plugin-store';
import { remove } from '@tauri-apps/plugin-fs';
import { invoke } from '@tauri-apps/api/core';
import type { StorageProvider, Store, StoreLoadOptions } from './types';

/**
 * Wraps a Tauri store to implement the Store interface.
 */
class TauriStore implements Store {
	constructor(
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		private store: Awaited<ReturnType<typeof tauriLoad>>,
		private filename: string,
		private dataDir: string
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

	async delete(): Promise<void> {
		await this.store.clear();
		await this.store.save();
		try {
			await remove(`${this.dataDir}/${this.filename}`);
		} catch {
			// File might not exist, ignore errors
		}
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
		const dataDir = await invoke<string>('get_data_dir');
		const fullPath = `${dataDir}/${name}`;
		const store = await tauriLoad(fullPath, {
			autoSave: options?.autoSave ?? true,
			defaults: options?.defaults ?? {}
		});
		return new TauriStore(store, name, dataDir);
	}
}
