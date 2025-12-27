export type QueryType = 'select' | 'insert' | 'update' | 'delete' | 'other';

/**
 * Detects the type of SQL query based on its first keyword.
 */
export function detectQueryType(query: string): QueryType {
	const trimmed = query.trim().toUpperCase();
	if (trimmed.startsWith('SELECT')) return 'select';
	if (trimmed.startsWith('INSERT')) return 'insert';
	if (trimmed.startsWith('UPDATE')) return 'update';
	if (trimmed.startsWith('DELETE')) return 'delete';
	return 'other';
}

/**
 * Returns true if the query is a write operation (INSERT, UPDATE, DELETE).
 */
export function isWriteQuery(query: string): boolean {
	const type = detectQueryType(query);
	return type === 'insert' || type === 'update' || type === 'delete';
}

/**
 * Extracts the table name from a simple SELECT query.
 * Returns null if the table cannot be determined.
 */
export function extractTableFromSelect(query: string): { schema?: string; table: string } | null {
	// Match: FROM [schema.]table
	// Handles: FROM table, FROM schema.table, FROM "table", FROM schema."table"
	const match = query.match(/\bFROM\s+(?:"?([a-z_][a-z0-9_]*)"?\.)?"?([a-z_][a-z0-9_]*)"?/i);
	if (!match) return null;
	return {
		schema: match[1] || undefined,
		table: match[2]
	};
}
