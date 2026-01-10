/**
 * Web storage provider.
 * Uses localStorage for browser demo persistence.
 */

import type { StorageProvider, Store, StoreLoadOptions } from './types';

/**
 * Implements the Store interface using localStorage.
 * Each store gets a prefix to namespace its keys.
 */
class WebStore implements Store {
	private prefix: string;
	private cache: Map<string, unknown>;

	constructor(name: string, defaults?: Record<string, unknown>) {
		// Use the store name as a prefix, removing .json extension if present
		this.prefix = `seaquel_${name.replace('.json', '')}_`;
		this.cache = new Map();

		// Load existing data from localStorage into cache
		this.loadFromLocalStorage();

		// Apply defaults for missing keys
		if (defaults) {
			for (const [key, value] of Object.entries(defaults)) {
				if (!this.cache.has(key)) {
					this.cache.set(key, value);
				}
			}
		}
	}

	private loadFromLocalStorage(): void {
		try {
			for (let i = 0; i < localStorage.length; i++) {
				const fullKey = localStorage.key(i);
				if (fullKey && fullKey.startsWith(this.prefix)) {
					const key = fullKey.slice(this.prefix.length);
					const value = localStorage.getItem(fullKey);
					if (value !== null) {
						try {
							this.cache.set(key, JSON.parse(value));
						} catch {
							// If JSON parse fails, store as string
							this.cache.set(key, value);
						}
					}
				}
			}
		} catch (error) {
			console.error('Failed to load from localStorage:', error);
		}
	}

	async get<T>(key: string): Promise<T | null> {
		const value = this.cache.get(key);
		return value !== undefined ? (value as T) : null;
	}

	async set<T>(key: string, value: T): Promise<void> {
		this.cache.set(key, value);
		// Auto-save to localStorage
		try {
			localStorage.setItem(this.prefix + key, JSON.stringify(value));
		} catch (error) {
			console.error('Failed to save to localStorage:', error);
		}
	}

	async save(): Promise<void> {
		// No-op for web storage - we save on every set
		// But sync all cached values to localStorage just in case
		try {
			for (const [key, value] of this.cache.entries()) {
				localStorage.setItem(this.prefix + key, JSON.stringify(value));
			}
		} catch (error) {
			console.error('Failed to save to localStorage:', error);
		}
	}

	async clear(): Promise<void> {
		// Remove all keys with our prefix
		try {
			const keysToRemove: string[] = [];
			for (let i = 0; i < localStorage.length; i++) {
				const fullKey = localStorage.key(i);
				if (fullKey && fullKey.startsWith(this.prefix)) {
					keysToRemove.push(fullKey);
				}
			}
			for (const key of keysToRemove) {
				localStorage.removeItem(key);
			}
			this.cache.clear();
		} catch (error) {
			console.error('Failed to clear localStorage:', error);
		}
	}

	async delete(): Promise<void> {
		// For web storage, delete is the same as clear
		await this.clear();
	}
}

/**
 * Storage provider that uses localStorage.
 * Suitable for browser demo mode.
 */
export class WebStorageProvider implements StorageProvider {
	readonly id = 'web';

	isAvailable(): boolean {
		try {
			return typeof window !== 'undefined' && typeof localStorage !== 'undefined';
		} catch {
			return false;
		}
	}

	async load(name: string, options?: StoreLoadOptions): Promise<Store> {
		return new WebStore(name, options?.defaults);
	}
}
