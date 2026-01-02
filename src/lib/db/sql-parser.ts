import type { DatabaseType } from '$lib/types';

export interface ParsedStatement {
	sql: string;
	index: number;
	startOffset: number; // Character position where statement starts (in original input)
	endOffset: number; // Character position where statement ends (in original input)
}

type ParserState =
	| 'normal'
	| 'single_quote'
	| 'double_quote'
	| 'line_comment'
	| 'block_comment'
	| 'dollar_quote';

/**
 * Splits a SQL string containing multiple statements into individual statements.
 * Uses a state machine to properly handle semicolons inside:
 * - Single-quoted strings ('...')
 * - Double-quoted identifiers ("...")
 * - Line comments (-- ...)
 * - Block comments (/* ... *\/)
 * - Dollar-quoted strings ($$...$$ or $tag$...$tag$) for PostgreSQL
 */
export function splitSqlStatements(sql: string, _dbType: DatabaseType): ParsedStatement[] {
	const statements: ParsedStatement[] = [];
	let currentStatement = '';
	let statementStartOffset = 0; // Track where current statement started
	let state: ParserState = 'normal';
	let dollarTag = ''; // For tracking PostgreSQL dollar-quoted strings
	let i = 0;

	while (i < sql.length) {
		const char = sql[i];
		const nextChar = sql[i + 1] ?? '';

		switch (state) {
			case 'normal':
				if (char === "'" ) {
					state = 'single_quote';
					currentStatement += char;
				} else if (char === '"') {
					state = 'double_quote';
					currentStatement += char;
				} else if (char === '-' && nextChar === '-') {
					state = 'line_comment';
					currentStatement += char;
				} else if (char === '/' && nextChar === '*') {
					state = 'block_comment';
					currentStatement += char;
				} else if (char === '$') {
					// Check for PostgreSQL dollar-quoted string
					const dollarMatch = sql.slice(i).match(/^\$([a-zA-Z_][a-zA-Z0-9_]*)?\$/);
					if (dollarMatch) {
						dollarTag = dollarMatch[0];
						state = 'dollar_quote';
						currentStatement += dollarTag;
						i += dollarTag.length - 1; // -1 because loop will increment
					} else {
						currentStatement += char;
					}
				} else if (char === ';') {
					// End of statement
					const trimmed = currentStatement.trim();
					if (trimmed) {
						statements.push({
							sql: trimmed,
							index: statements.length,
							startOffset: statementStartOffset,
							endOffset: i // The semicolon position
						});
					}
					currentStatement = '';
					statementStartOffset = i + 1; // Next statement starts after the semicolon
				} else {
					currentStatement += char;
				}
				break;

			case 'single_quote':
				currentStatement += char;
				if (char === "'" && nextChar === "'") {
					// Escaped single quote
					currentStatement += nextChar;
					i++;
				} else if (char === "'") {
					state = 'normal';
				}
				break;

			case 'double_quote':
				currentStatement += char;
				if (char === '"' && nextChar === '"') {
					// Escaped double quote
					currentStatement += nextChar;
					i++;
				} else if (char === '"') {
					state = 'normal';
				}
				break;

			case 'line_comment':
				currentStatement += char;
				if (char === '\n') {
					state = 'normal';
				}
				break;

			case 'block_comment':
				currentStatement += char;
				if (char === '*' && nextChar === '/') {
					currentStatement += nextChar;
					i++;
					state = 'normal';
				}
				break;

			case 'dollar_quote':
				currentStatement += char;
				// Check if we've reached the closing dollar tag
				if (char === '$' && sql.slice(i, i + dollarTag.length) === dollarTag) {
					currentStatement += sql.slice(i + 1, i + dollarTag.length);
					i += dollarTag.length - 1;
					state = 'normal';
					dollarTag = '';
				}
				break;
		}

		i++;
	}

	// Add final statement if there's content
	const trimmed = currentStatement.trim();
	if (trimmed) {
		statements.push({
			sql: trimmed,
			index: statements.length,
			startOffset: statementStartOffset,
			endOffset: sql.length - 1 // End of input
		});
	}

	// Filter out statements that are only comments/whitespace
	const nonEmptyStatements = statements.filter((stmt) => !isOnlyComments(stmt.sql));

	// Re-index after filtering (preserve offsets)
	const reindexed = nonEmptyStatements.map((stmt, idx) => ({
		sql: stmt.sql,
		index: idx,
		startOffset: stmt.startOffset,
		endOffset: stmt.endOffset
	}));

	// If no statements found, return empty array
	if (reindexed.length === 0) {
		return [];
	}

	return reindexed;
}

/**
 * Finds the statement at a given cursor offset position.
 * Returns the first statement if cursor is between statements or at the start.
 */
export function getStatementAtOffset(
	sql: string,
	offset: number,
	dbType: DatabaseType
): ParsedStatement | null {
	const statements = splitSqlStatements(sql, dbType);
	if (statements.length === 0) return null;

	// Find statement containing the offset
	const found = statements.find(
		(s) => offset >= s.startOffset && offset <= s.endOffset
	);

	// If cursor is between statements or at the start, return first statement
	return found ?? statements[0];
}

/**
 * Checks if a SQL string contains only comments and whitespace (no actual SQL code).
 */
function isOnlyComments(sql: string): boolean {
	let i = 0;
	const len = sql.length;

	while (i < len) {
		const char = sql[i];
		const nextChar = sql[i + 1] ?? '';

		// Skip whitespace
		if (/\s/.test(char)) {
			i++;
			continue;
		}

		// Skip line comments
		if (char === '-' && nextChar === '-') {
			// Find end of line
			while (i < len && sql[i] !== '\n') {
				i++;
			}
			continue;
		}

		// Skip block comments
		if (char === '/' && nextChar === '*') {
			i += 2;
			while (i < len - 1) {
				if (sql[i] === '*' && sql[i + 1] === '/') {
					i += 2;
					break;
				}
				i++;
			}
			continue;
		}

		// Found a non-comment, non-whitespace character
		return false;
	}

	return true;
}
