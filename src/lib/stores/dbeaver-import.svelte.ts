import { load } from "@tauri-apps/plugin-store";
import type { ImportableConnection } from "$lib/types/dbeaver";
import { discoverDbeaverConnections } from "$lib/services/dbeaver-import";

interface PersistedDbeaverImportState {
	hasOfferedImport: boolean;
	lastCheckTimestamp?: string;
}

const STORE_FILE = "dbeaver_import_state.json";

class DbeaverImportStore {
	// Dialog state
	isOpen = $state(false);
	isLoading = $state(false);

	// Discovered connections
	connections = $state<ImportableConnection[]>([]);

	// Persistence tracking
	hasOfferedImport = $state(false);
	private initialized = false;

	/**
	 * Initialize the store - loads persisted state
	 */
	async initialize(): Promise<void> {
		if (this.initialized) return;
		this.initialized = true;

		// Load persisted state
		try {
			const store = await load(STORE_FILE, {
				autoSave: false,
				defaults: { state: null },
			});
			const persisted = (await store.get("state")) as PersistedDbeaverImportState | null;

			if (persisted) {
				this.hasOfferedImport = persisted.hasOfferedImport;
			}
		} catch (error) {
			console.error("Failed to load DBeaver import state:", error);
		}
	}

	/**
	 * Check for DBeaver connections and open dialog if found
	 * Called when user clicks on DBeaver card
	 */
	async checkAndShowDialog(existingConnectionIds: string[]): Promise<void> {
		this.isLoading = true;

		try {
			const importable = await discoverDbeaverConnections(existingConnectionIds);

			if (importable.length > 0) {
				this.connections = importable;
				this.isOpen = true;
			}
		} catch (error) {
			console.error("Failed to check DBeaver connections:", error);
		} finally {
			this.isLoading = false;
		}
	}

	/**
	 * Toggle selection state for a connection
	 */
	toggleConnection(index: number): void {
		if (this.connections[index] && !this.connections[index].isDuplicate) {
			this.connections[index].selected = !this.connections[index].selected;
			// Trigger reactivity
			this.connections = [...this.connections];
		}
	}

	/**
	 * Select all non-duplicate connections
	 */
	selectAll(): void {
		this.connections = this.connections.map((c) => ({
			...c,
			selected: !c.isDuplicate,
		}));
	}

	/**
	 * Deselect all connections
	 */
	deselectAll(): void {
		this.connections = this.connections.map((c) => ({
			...c,
			selected: false,
		}));
	}

	/**
	 * Get all selected connections
	 */
	getSelectedConnections(): ImportableConnection[] {
		return this.connections.filter((c) => c.selected);
	}

	/**
	 * Dismiss the dialog without importing
	 */
	async dismiss(): Promise<void> {
		this.isOpen = false;
		this.hasOfferedImport = true;
		await this.persist();
	}

	/**
	 * Complete the import and close dialog
	 */
	async completeImport(): Promise<void> {
		this.isOpen = false;
		this.hasOfferedImport = true;
		await this.persist();
	}

	/**
	 * Persist state to store
	 */
	private async persist(): Promise<void> {
		try {
			const store = await load(STORE_FILE, {
				autoSave: true,
				defaults: { state: null },
			});

			const state: PersistedDbeaverImportState = {
				hasOfferedImport: this.hasOfferedImport,
				lastCheckTimestamp: new Date().toISOString(),
			};

			await store.set("state", state);
			await store.save();
		} catch (error) {
			console.error("Failed to persist DBeaver import state:", error);
		}
	}
}

export const dbeaverImportStore = new DbeaverImportStore();
