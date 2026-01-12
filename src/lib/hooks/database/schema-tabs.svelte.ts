import type { SchemaTable, SchemaTab } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { getAdapter, type DatabaseAdapter } from '$lib/db';
import { mssqlQuery } from '$lib/services/mssql';
import { getProvider, getDuckDBProvider, type DatabaseProvider } from '$lib/providers';

/**
 * Manages schema tabs: add, remove, set active.
 * Handles table metadata loading.
 * Tabs are organized per-project.
 */
export class SchemaTabManager {
	constructor(
		private state: DatabaseState,
		private tabOrdering: TabOrderingManager,
		private schedulePersistence: (projectId: string | null) => void
	) {}

	/**
	 * Get the appropriate provider based on connection type.
	 */
	private async getProviderForConnection(dbType?: string): Promise<DatabaseProvider> {
		const connectionType = dbType ?? this.state.activeConnection?.type;
		if (connectionType === 'duckdb') {
			return getDuckDBProvider();
		}
		return getProvider();
	}

	/**
	 * Add a schema tab for the specified table.
	 * Fetches table metadata (columns, indexes, foreign keys).
	 */
	async add(table: SchemaTable): Promise<string | null> {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return null;

		const projectId = this.state.activeProjectId;
		const connectionId = this.state.activeConnectionId;
		const tabs = this.state.schemaTabsByProject[projectId] ?? [];
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
			const provider = await this.getProviderForConnection();
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
			this.state.schemaTabsByProject = {
				...this.state.schemaTabsByProject,
				[projectId]: updatedTabs
			};

			this.state.activeSchemaTabIdByProject = {
				...this.state.activeSchemaTabIdByProject,
				[projectId]: existingTab.id
			};
			this.schedulePersistence(projectId);
			return existingTab.id;
		}

		const newTab: SchemaTab = {
			id: `schema-tab-${Date.now()}`,
			table: updatedTable
		};

		// Update state using spread syntax
		this.state.schemaTabsByProject = {
			...this.state.schemaTabsByProject,
			[projectId]: [...tabs, newTab]
		};

		this.tabOrdering.add(newTab.id);

		this.state.activeSchemaTabIdByProject = {
			...this.state.activeSchemaTabIdByProject,
			[projectId]: newTab.id
		};

		this.schedulePersistence(projectId);
		return newTab.id;
	}

	/**
	 * Remove a schema tab by ID.
	 */
	remove(id: string): void {
		this.tabOrdering.removeTabGeneric(
			() => this.state.schemaTabsByProject,
			(r) => (this.state.schemaTabsByProject = r),
			() => this.state.activeSchemaTabIdByProject,
			(r) => (this.state.activeSchemaTabIdByProject = r),
			id
		);
		this.schedulePersistence(this.state.activeProjectId);
	}

	/**
	 * Set the active schema tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeProjectId) return;

		this.state.activeSchemaTabIdByProject = {
			...this.state.activeSchemaTabIdByProject,
			[this.state.activeProjectId]: id
		};
		this.schedulePersistence(this.state.activeProjectId);
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
		// Get provider once for all tables - look up connection type from state
		const connectionType = this.state.connections.find(c => c.id === connectionId)?.type;
		const provider = providerConnectionId ? await this.getProviderForConnection(connectionType) : null;

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
