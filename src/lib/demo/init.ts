/**
 * Demo initialization logic.
 * Sets up DuckDB with sample data when running in browser mode.
 */

import { getDuckDBProvider, isDemo } from '$lib/providers';
import type { DuckDBProvider } from '$lib/providers/duckdb-provider';
import { SAMPLE_SCHEMA, SAMPLE_DATA } from './sample-data';

/**
 * Initialize the demo environment.
 * Creates a DuckDB connection and loads sample data.
 *
 * @returns Connection ID if in demo mode, null otherwise
 */
export async function initializeDemo(): Promise<string | null> {
	if (!isDemo()) {
		return null;
	}

	const provider = (await getDuckDBProvider()) as DuckDBProvider;

	// Connect to in-memory DuckDB
	const connectionId = await provider.connect({
		type: 'duckdb',
		databaseName: ':memory:'
	});

	// Load schema
	const schemaStatements = SAMPLE_SCHEMA.split(';')
		.map((s) => s.trim())
		.filter((s) => s.length > 0);

	for (const statement of schemaStatements) {
		try {
			await provider.execute(connectionId, statement);
		} catch (error) {
			console.warn('[Demo] Schema statement failed:', statement, error);
		}
	}

	// Load data
	const dataStatements = SAMPLE_DATA.split(';')
		.map((s) => s.trim())
		.filter((s) => s.length > 0);

	for (const statement of dataStatements) {
		try {
			await provider.execute(connectionId, statement);
		} catch (error) {
			console.warn('[Demo] Data statement failed:', statement, error);
		}
	}

	return connectionId;
}

/**
 * Get a demo connection configuration for the UI.
 */
export function getDemoConnectionConfig() {
	return {
		id: 'demo-connection',
		name: 'Demo Database',
		type: 'duckdb' as const,
		host: 'browser',
		port: 0,
		databaseName: 'demo',
		username: '',
		password: ''
	};
}
