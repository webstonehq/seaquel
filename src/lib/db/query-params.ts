/**
 * Query parameter utilities.
 * Handles extraction and substitution of {{param_name}} placeholders.
 * @module db/query-params
 */

import type { QueryParameter, QueryParameterType, ParameterValue, DatabaseType } from '$lib/types';

/**
 * Regex to match {{param_name}} placeholders.
 * Captures the parameter name from within the double braces.
 */
const PARAM_REGEX = /\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}/g;

/**
 * Extract parameter names from a query string.
 * Returns unique parameter names in order of first appearance.
 */
export function extractParameters(query: string): string[] {
	const params: string[] = [];
	const seen = new Set<string>();

	let match;
	while ((match = PARAM_REGEX.exec(query)) !== null) {
		const name = match[1];
		if (!seen.has(name)) {
			seen.add(name);
			params.push(name);
		}
	}

	PARAM_REGEX.lastIndex = 0; // Reset regex state
	return params;
}

/**
 * Check if a query contains parameters.
 */
export function hasParameters(query: string): boolean {
	const result = PARAM_REGEX.test(query);
	PARAM_REGEX.lastIndex = 0;
	return result;
}

/**
 * Escape a value for inline substitution in SQL queries.
 * Handles strings, numbers, booleans, null, and dates.
 * @param value The value to escape
 * @param insideString If true, don't wrap strings in quotes (for parameters inside string literals)
 */
export function escapeValueForInline(value: unknown, insideString: boolean = false): string {
	if (value === null || value === undefined) {
		return insideString ? '' : 'NULL';
	}

	if (typeof value === 'number') {
		if (!Number.isFinite(value)) {
			return insideString ? '' : 'NULL';
		}
		return String(value);
	}

	if (typeof value === 'boolean') {
		return value ? '1' : '0';
	}

	if (value instanceof Date) {
		const isoStr = value.toISOString();
		return insideString ? isoStr.replace(/'/g, "''") : `'${isoStr}'`;
	}

	// String - escape single quotes by doubling them
	const str = String(value).replace(/'/g, "''");
	return insideString ? str : `'${str}'`;
}

/**
 * @deprecated Use escapeValueForInline instead
 */
export function escapeValueForMssql(value: unknown): string {
	return escapeValueForInline(value, false);
}

/**
 * Check if a position in the query is inside a string literal.
 * Handles single-quoted strings with escaped quotes ('').
 */
function isInsideStringLiteral(query: string, position: number): boolean {
	let insideString = false;
	let i = 0;

	while (i < position && i < query.length) {
		if (query[i] === "'") {
			// Check for escaped quote ('')
			if (i + 1 < query.length && query[i + 1] === "'") {
				i += 2; // Skip escaped quote
				continue;
			}
			insideString = !insideString;
		}
		i++;
	}

	return insideString;
}

/**
 * Substitute parameters in a query.
 * For PostgreSQL/SQLite: replaces {{name}} with $1, $2, etc. and returns bind values.
 * For MSSQL/DuckDB: replaces {{name}} with escaped inline values.
 *
 * Special handling for parameters inside string literals:
 * - For PostgreSQL/SQLite: breaks the string to concatenate with parameter: '%{{name}}%' -> '%' || $1 || '%'
 * - For MSSQL/DuckDB: substitutes the value directly without adding quotes
 */
export function substituteParameters(
	query: string,
	parameterValues: ParameterValue[],
	dbType: DatabaseType
): { sql: string; bindValues: unknown[] } {
	const paramMap = new Map(parameterValues.map((p) => [p.name, p.value]));
	const useInlineSubstitution = dbType === 'mssql' || dbType === 'duckdb';

	// For databases using inline substitution (MSSQL/DuckDB)
	if (useInlineSubstitution) {
		let result = '';
		let lastIndex = 0;
		let match;

		PARAM_REGEX.lastIndex = 0;
		while ((match = PARAM_REGEX.exec(query)) !== null) {
			const paramName = match[1];
			const value = paramMap.get(paramName);
			const insideString = isInsideStringLiteral(query, match.index);

			result += query.slice(lastIndex, match.index);
			result += escapeValueForInline(value, insideString);
			lastIndex = match.index + match[0].length;
		}
		result += query.slice(lastIndex);

		PARAM_REGEX.lastIndex = 0;
		return { sql: result, bindValues: [] };
	}

	// For databases using parameterized queries (PostgreSQL/SQLite)
	const bindValues: unknown[] = [];
	const usedParams = new Map<string, number>(); // name -> position (1-indexed)

	let result = '';
	let lastIndex = 0;
	let match;

	PARAM_REGEX.lastIndex = 0;
	while ((match = PARAM_REGEX.exec(query)) !== null) {
		const paramName = match[1];
		const insideString = isInsideStringLiteral(query, match.index);

		// Get or assign parameter position
		if (!usedParams.has(paramName)) {
			bindValues.push(paramMap.get(paramName) ?? null);
			usedParams.set(paramName, bindValues.length);
		}
		const paramPosition = usedParams.get(paramName)!;

		result += query.slice(lastIndex, match.index);

		if (insideString) {
			// Parameter is inside a string literal - need to break out and concatenate
			// Find the quote before and after to properly break the string
			// e.g., '%{{name}}%' becomes '%' || $1 || '%'
			result += `' || $${paramPosition} || '`;
		} else {
			result += `$${paramPosition}`;
		}

		lastIndex = match.index + match[0].length;
	}
	result += query.slice(lastIndex);

	PARAM_REGEX.lastIndex = 0;
	return { sql: result, bindValues };
}

/**
 * Create default parameter definitions from extracted parameter names.
 * All parameters default to 'text' type.
 */
export function createDefaultParameters(paramNames: string[]): QueryParameter[] {
	return paramNames.map((name) => ({
		name,
		type: 'text' as const,
		defaultValue: undefined,
		description: undefined
	}));
}

/**
 * Coerce a string value to the appropriate type based on parameter definition.
 */
export function coerceValue(value: string, type: QueryParameterType): unknown {
	if (value === '' || value === null || value === undefined) {
		return null;
	}

	switch (type) {
		case 'number': {
			const num = parseFloat(value);
			return isNaN(num) ? null : num;
		}
		case 'boolean':
			return value.toLowerCase() === 'true' || value === '1';
		case 'date':
		case 'datetime':
			// Keep as ISO string for database
			return value;
		case 'text':
		default:
			return value;
	}
}
