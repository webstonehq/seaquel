/**
 * Schema adapter functions to convert between TutorialTable/SchemaTable and QueryBuilderTable.
 * These functions enable the visual query builder to work with both tutorial schemas
 * and real database schemas from the Manage section.
 * @module utils/schema-adapter
 */

import type {
	TutorialTable,
	SchemaTable,
	QueryBuilderTable,
	QueryBuilderColumn
} from '$lib/types';

/**
 * Convert a TutorialTable array to QueryBuilderTable array.
 * TutorialTable uses `primaryKey?: boolean` and `foreignKey?: { table, column }`.
 * @param schema - Tutorial schema tables
 * @returns Unified QueryBuilderTable array
 */
export function tutorialToQueryBuilder(schema: TutorialTable[]): QueryBuilderTable[] {
	return schema.map((table) => ({
		name: table.name,
		columns: table.columns.map((col) => ({
			name: col.name,
			type: col.type,
			primaryKey: col.primaryKey ?? false,
			foreignKey: col.foreignKey
				? { table: col.foreignKey.table, column: col.foreignKey.column }
				: undefined
		}))
	}));
}

/**
 * Convert a SchemaTable array to QueryBuilderTable array.
 * SchemaTable uses `isPrimaryKey: boolean` and `foreignKeyRef?: { referencedSchema, referencedTable, referencedColumn }`.
 * @param schema - Database schema tables
 * @returns Unified QueryBuilderTable array
 */
export function schemaToQueryBuilder(schema: SchemaTable[]): QueryBuilderTable[] {
	return schema.map((table) => ({
		name: table.name,
		columns: table.columns.map((col): QueryBuilderColumn => ({
			name: col.name,
			type: col.type,
			primaryKey: col.isPrimaryKey,
			foreignKey: col.foreignKeyRef
				? { table: col.foreignKeyRef.referencedTable, column: col.foreignKeyRef.referencedColumn }
				: undefined
		}))
	}));
}

/**
 * Get a table by name from a QueryBuilderTable array.
 * @param schema - QueryBuilderTable array to search
 * @param name - Table name to find
 * @returns The table or undefined if not found
 */
export function getQueryBuilderTable(
	schema: QueryBuilderTable[],
	name: string
): QueryBuilderTable | undefined {
	return schema.find((t) => t.name === name);
}

/**
 * Get all table names from a QueryBuilderTable array.
 * @param schema - QueryBuilderTable array
 * @returns Array of table names
 */
export function getQueryBuilderTableNames(schema: QueryBuilderTable[]): string[] {
	return schema.map((t) => t.name);
}
