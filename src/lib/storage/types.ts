/**
 * Storage provider abstraction layer.
 * Enables the same persistence code to work with Tauri store (desktop) and localStorage (web).
 */

/**
 * Interface for a key-value store.
 * Provides a consistent API for both Tauri store and web localStorage.
 */
export interface Store {
	/**
	 * Get a value from the store.
	 * @param key The key to retrieve
	 * @returns The value, or null if not found
	 */
	get<T>(key: string): Promise<T | null>;

	/**
	 * Set a value in the store.
	 * @param key The key to set
	 * @param value The value to store
	 */
	set<T>(key: string, value: T): Promise<void>;

	/**
	 * Save the store to persistent storage.
	 * For web, this is a no-op since localStorage is synchronous.
	 */
	save(): Promise<void>;

	/**
	 * Clear all data in the store.
	 */
	clear(): Promise<void>;
}

/**
 * Options for loading a store.
 */
export interface StoreLoadOptions {
	/** Whether to auto-save after each set operation */
	autoSave?: boolean;
	/** Default values for the store */
	defaults?: Record<string, unknown>;
}

/**
 * Interface for the storage provider.
 * Provides a factory method to create stores.
 */
export interface StorageProvider {
	/**
	 * Provider identifier.
	 */
	readonly id: string;

	/**
	 * Check if this provider is available in the current environment.
	 */
	isAvailable(): boolean;

	/**
	 * Load or create a store with the given name.
	 * @param name The name of the store (used as filename for Tauri, key prefix for web)
	 * @param options Store options
	 * @returns A Store instance
	 */
	load(name: string, options?: StoreLoadOptions): Promise<Store>;
}
