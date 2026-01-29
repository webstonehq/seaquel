import type { SchemaTable, SchemaTab } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { BaseTabManager, type TabStateAccessors } from './base-tab-manager.svelte.js';
import { getAdapter, type DatabaseAdapter } from '$lib/db';
import { mssqlQuery } from '$lib/services/mssql';
import type { ProviderRegistry } from '$lib/providers';
import { handleError, createError } from '$lib/errors';

/**
 * Manages schema tabs: add, remove, set active.
 * Handles table metadata loading.
 * Tabs are organized per-project.
 */
export class SchemaTabManager extends BaseTabManager<SchemaTab> {
	constructor(
		state: DatabaseState,
		tabOrdering: TabOrderingManager,
		schedulePersistence: (projectId: string | null) => void,
		private providers: ProviderRegistry
	) {
		super(state, tabOrdering, schedulePersistence);
	}

	protected get accessors(): TabStateAccessors<SchemaTab> {
		return {
			getTabs: () => this.state.schemaTabsByProject,
			setTabs: (r) => (this.state.schemaTabsByProject = r),
			getActiveId: () => this.state.activeSchemaTabIdByProject,
			setActiveId: (r) => (this.state.activeSchemaTabIdByProject = r)
		};
	}

	/**
	 * Add a schema tab for the specified table.
	 * Fetches table metadata (columns, indexes, foreign keys).
	 */
	async add(table: SchemaTable): Promise<string | null> {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return null;

		const projectId = this.state.activeProjectId;
		const connectionId = this.state.activeConnectionId;
		const tabs = this.getProjectTabs();
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
			const provider = await this.providers.getForType(this.state.activeConnection?.type ?? '');
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
			this.updateTab(existingTab.id, (t) => ({ ...t, table: updatedTable }));
			this.setActiveTabId(existingTab.id);
			this.schedulePersistence(projectId);
			return existingTab.id;
		}

		const newTab: SchemaTab = {
			id: `schema-tab-${Date.now()}`,
			table: updatedTable
		};

		return this.appendTab(newTab);
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
		const provider = providerConnectionId ? await this.providers.getForType(connectionType ?? '') : null;

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
				handleError(
					createError(
						'SCHEMA_LOAD_FAILED',
						error instanceof Error ? error.message : String(error),
						`Failed to load metadata for ${table.schema}.${table.name}`,
						{ table: table.name, schema: table.schema }
					),
					{ silent: true }
				);
			}
		});

		await Promise.allSettled(promises);
	}
}
