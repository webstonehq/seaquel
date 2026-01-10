import type { SchemaTable, SchemaTab } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { getAdapter, type DatabaseAdapter } from '$lib/db';
import { mssqlQuery } from '$lib/services/mssql';
import { getProvider } from '$lib/providers';

/**
 * Manages schema tabs: add, remove, set active.
 * Handles table metadata loading.
 */
export class SchemaTabManager {
	constructor(
		private state: DatabaseState,
		private tabOrdering: TabOrderingManager,
		private schedulePersistence: (connectionId: string | null) => void
	) {}

	/**
	 * Add a schema tab for the specified table.
	 * Fetches table metadata (columns, indexes, foreign keys).
	 */
	async add(table: SchemaTable): Promise<string | null> {
		if (!this.state.activeConnectionId || !this.state.activeConnection) return null;

		const connectionId = this.state.activeConnectionId;
		const tabs = this.state.schemaTabsByConnection[connectionId] ?? [];
		const adapter = getAdapter(this.state.activeConnection.type);
		const isMssql =
			this.state.activeConnection.type === 'mssql' &&
			this.state.activeConnection.mssqlConnectionId;
		const providerConnectionId = this.state.activeConnection.providerConnectionId;

		// Fetch table metadata - query columns and indexes
		let columnsResult: unknown[];
		let indexesResult: unknown[];
		let foreignKeysResult: unknown[] | undefined;

		if (isMssql) {
			const columnsQueryResult = await mssqlQuery(
				this.state.activeConnection.mssqlConnectionId!,
				adapter.getColumnsQuery(table.name, table.schema)
			);
			columnsResult = columnsQueryResult.rows;

			const indexesQueryResult = await mssqlQuery(
				this.state.activeConnection.mssqlConnectionId!,
				adapter.getIndexesQuery(table.name, table.schema)
			);
			indexesResult = indexesQueryResult.rows;

			if (adapter.getForeignKeysQuery) {
				const fkQueryResult = await mssqlQuery(
					this.state.activeConnection.mssqlConnectionId!,
					adapter.getForeignKeysQuery(table.name, table.schema)
				);
				foreignKeysResult = fkQueryResult.rows;
			}
		} else if (providerConnectionId) {
			const provider = await getProvider();
			columnsResult = await provider.select(
				providerConnectionId,
				adapter.getColumnsQuery(table.name, table.schema)
			);

			indexesResult = await provider.select(
				providerConnectionId,
				adapter.getIndexesQuery(table.name, table.schema)
			);

			// Fetch foreign keys if adapter supports it (needed for SQLite, DuckDB)
			if (adapter.getForeignKeysQuery) {
				foreignKeysResult = await provider.select(
					providerConnectionId,
					adapter.getForeignKeysQuery(table.name, table.schema)
				);
			}
		} else {
			return null;
		}

		// Update table with fetched columns and indexes
		const updatedTable: SchemaTable = {
			...table,
			columns: adapter.parseColumnsResult(columnsResult || [], foreignKeysResult),
			indexes: adapter.parseIndexesResult(indexesResult || [])
		};

		// Update this.state.schemas with the refreshed table metadata
		const connectionSchemas = [...(this.state.schemas[connectionId] ?? [])];
		const tableIndex = connectionSchemas.findIndex(
			(t) => t.name === table.name && t.schema === table.schema
		);
		if (tableIndex >= 0) {
			connectionSchemas[tableIndex] = updatedTable;
		}
		this.state.schemas = {
			...this.state.schemas,
			[connectionId]: connectionSchemas
		};

		// Check if table is already open
		const existingTab = tabs.find(
			(t) => t.table.name === table.name && t.table.schema === table.schema
		);
		if (existingTab) {
			// Update existing tab with new metadata
			const updatedTabs = tabs.map((t) =>
				t.id === existingTab.id ? { ...t, table: updatedTable } : t
			);
			this.state.schemaTabsByConnection = {
				...this.state.schemaTabsByConnection,
				[connectionId]: updatedTabs
			};

			this.state.activeSchemaTabIdByConnection = {
				...this.state.activeSchemaTabIdByConnection,
				[connectionId]: existingTab.id
			};
			this.schedulePersistence(connectionId);
			return existingTab.id;
		}

		const newTab: SchemaTab = {
			id: `schema-tab-${Date.now()}`,
			table: updatedTable
		};

		// Update state using spread syntax
		this.state.schemaTabsByConnection = {
			...this.state.schemaTabsByConnection,
			[connectionId]: [...tabs, newTab]
		};

		this.tabOrdering.add(newTab.id);

		this.state.activeSchemaTabIdByConnection = {
			...this.state.activeSchemaTabIdByConnection,
			[connectionId]: newTab.id
		};

		this.schedulePersistence(connectionId);
		return newTab.id;
	}

	/**
	 * Remove a schema tab by ID.
	 */
	remove(id: string): void {
		this.tabOrdering.removeTabGeneric(
			() => this.state.schemaTabsByConnection,
			(r) => (this.state.schemaTabsByConnection = r),
			() => this.state.activeSchemaTabIdByConnection,
			(r) => (this.state.activeSchemaTabIdByConnection = r),
			id
		);
		this.schedulePersistence(this.state.activeConnectionId);
	}

	/**
	 * Set the active schema tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeConnectionId) return;

		this.state.activeSchemaTabIdByConnection = {
			...this.state.activeSchemaTabIdByConnection,
			[this.state.activeConnectionId]: id
		};
		this.schedulePersistence(this.state.activeConnectionId);
	}

	/**
	 * Load column and index metadata for all tables in the background.
	 * Updates the schema state progressively as each table's metadata is loaded.
	 */
	async loadTableMetadataInBackground(
		connectionId: string,
		tables: SchemaTable[],
		adapter: DatabaseAdapter,
		providerConnectionId?: string,
		mssqlConnectionId?: string
	): Promise<void> {
		// Get provider once for all tables
		const provider = providerConnectionId ? await getProvider() : null;

		// Process tables in parallel but update state as each completes
		const promises = tables.map(async (table, index) => {
			try {
				let columnsResult: unknown[];
				let indexesResult: unknown[];
				let foreignKeysResult: unknown[] | undefined;

				if (mssqlConnectionId) {
					const columnsQueryResult = await mssqlQuery(
						mssqlConnectionId,
						adapter.getColumnsQuery(table.name, table.schema)
					);
					columnsResult = columnsQueryResult.rows;

					const indexesQueryResult = await mssqlQuery(
						mssqlConnectionId,
						adapter.getIndexesQuery(table.name, table.schema)
					);
					indexesResult = indexesQueryResult.rows;

					if (adapter.getForeignKeysQuery) {
						const fkQueryResult = await mssqlQuery(
							mssqlConnectionId,
							adapter.getForeignKeysQuery(table.name, table.schema)
						);
						foreignKeysResult = fkQueryResult.rows;
					}
				} else if (provider && providerConnectionId) {
					columnsResult = await provider.select(
						providerConnectionId,
						adapter.getColumnsQuery(table.name, table.schema)
					);

					indexesResult = await provider.select(
						providerConnectionId,
						adapter.getIndexesQuery(table.name, table.schema)
					);

					// Fetch foreign keys if adapter supports it (needed for SQLite, DuckDB)
					if (adapter.getForeignKeysQuery) {
						foreignKeysResult = await provider.select(
							providerConnectionId,
							adapter.getForeignKeysQuery(table.name, table.schema)
						);
					}
				} else {
					// No valid connection, skip
					return;
				}

				const updatedTable: SchemaTable = {
					...table,
					columns: adapter.parseColumnsResult(columnsResult || [], foreignKeysResult),
					indexes: adapter.parseIndexesResult(indexesResult || [])
				};

				// Update the schema state with the new table metadata
				const currentSchemas = this.state.schemas[connectionId];
				if (currentSchemas) {
					const updatedSchemas = [...currentSchemas];
					updatedSchemas[index] = updatedTable;
					this.state.schemas = {
						...this.state.schemas,
						[connectionId]: updatedSchemas
					};
				}
			} catch (error) {
				console.error(`Failed to load metadata for table ${table.schema}.${table.name}:`, error);
			}
		});

		await Promise.allSettled(promises);
	}
}
