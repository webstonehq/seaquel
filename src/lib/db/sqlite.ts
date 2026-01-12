import type { DatabaseAdapter, ExplainNode } from "./index";
import { validateIdentifier } from "./index";
import type { SchemaTable, SchemaColumn, SchemaIndex, ForeignKeyRef, TableSizeInfo, IndexUsageInfo, DatabaseOverview } from "$lib/types";

interface SqliteSchemaRow {
	name: string;
	type: string;
}

interface SqliteColumnRow {
	cid: number;
	name: string;
	type: string;
	notnull: number;
	dflt_value: string | null;
	pk: number;
}

interface SqliteIndexRow {
	seq: number;
	name: string;
	unique: number;
	origin: string;
	partial: number;
}

interface SqliteForeignKeyRow {
	id: number;
	seq: number;
	table: string;
	from: string;
	to: string;
	on_update: string;
	on_delete: string;
	match: string;
}

export class SqliteAdapter implements DatabaseAdapter {
	getSchemaQuery(): string {
		return `SELECT name, type FROM sqlite_master
			WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
			ORDER BY name`;
	}

	getColumnsQuery(table: string, _schema: string): string {
		// SQLite uses PRAGMA for table info
		return `PRAGMA table_info('${validateIdentifier(table)}')`;
	}

	getIndexesQuery(table: string, _schema: string): string {
		return `PRAGMA index_list('${validateIdentifier(table)}')`;
	}

	getForeignKeysQuery(table: string, _schema: string): string {
		return `PRAGMA foreign_key_list('${validateIdentifier(table)}')`;
	}

	getExplainQuery(query: string, _analyze: boolean): string {
		// SQLite doesn't support ANALYZE in the same way as PostgreSQL
		// EXPLAIN QUERY PLAN shows the query plan without execution
		const baseQuery = query.replace(/;$/, "");
		return `EXPLAIN QUERY PLAN ${baseQuery}`;
	}

	parseExplainResult(rows: unknown[], _analyze: boolean): ExplainNode {
		// SQLite EXPLAIN QUERY PLAN returns: id, parent, notused, detail
		const result = rows as {
			id: number;
			parent: number;
			detail: string;
		}[];

		if (result.length === 0) {
			return { type: "Query Plan", label: "No plan available" };
		}

		// Build a map of nodes by id
		const nodeMap = new Map<number, ExplainNode & { parentId: number }>();
		for (const row of result) {
			const nodeType = this.extractSqliteNodeType(row.detail);
			nodeMap.set(row.id, {
				type: nodeType,
				label: row.detail,
				parentId: row.parent,
				children: [],
			});
		}

		// Build tree structure
		let root: ExplainNode | null = null;
		for (const [id, node] of nodeMap) {
			if (node.parentId === 0 || !nodeMap.has(node.parentId)) {
				// This is a root node
				if (!root) {
					root = node;
				}
			} else {
				const parent = nodeMap.get(node.parentId);
				if (parent) {
					if (!parent.children) parent.children = [];
					parent.children.push(node);
				}
			}
		}

		return root || { type: "Query Plan", label: "No plan available" };
	}

	private extractSqliteNodeType(detail: string): string {
		// Extract node type from SQLite EXPLAIN QUERY PLAN detail
		if (detail.includes("SCAN")) return "Scan";
		if (detail.includes("SEARCH")) return "Search";
		if (detail.includes("INDEX")) return "Index Scan";
		if (detail.includes("USING COVERING INDEX")) return "Covering Index Scan";
		if (detail.includes("SUBQUERY")) return "Subquery";
		if (detail.includes("COMPOUND")) return "Compound";
		if (detail.includes("UNION")) return "Union";
		if (detail.includes("EXCEPT")) return "Except";
		if (detail.includes("INTERSECT")) return "Intersect";
		return "Step";
	}

	parseSchemaResult(rows: unknown[]): SchemaTable[] {
		return (rows as SqliteSchemaRow[]).map((row) => ({
			name: row.name,
			schema: "main", // SQLite uses "main" as the default schema
			type: "table" as const,
			columns: [],
			indexes: [],
		}));
	}

	parseColumnsResult(rows: unknown[], foreignKeys?: unknown[]): SchemaColumn[] {
		const fkMap = new Map<string, ForeignKeyRef>();
		if (foreignKeys) {
			for (const fk of foreignKeys as SqliteForeignKeyRow[]) {
				fkMap.set(fk.from, {
					referencedSchema: "main", // SQLite uses "main" as default schema
					referencedTable: fk.table,
					referencedColumn: fk.to,
				});
			}
		}

		return (rows as SqliteColumnRow[]).map((col) => ({
			name: col.name,
			type: col.type || "BLOB", // SQLite allows empty type
			nullable: col.notnull === 0,
			defaultValue: col.dflt_value || undefined,
			isPrimaryKey: col.pk > 0,
			isForeignKey: fkMap.has(col.name),
			foreignKeyRef: fkMap.get(col.name),
		}));
	}

	parseIndexesResult(rows: unknown[]): SchemaIndex[] {
		return (rows as SqliteIndexRow[])
			.filter((idx) => idx.name && !idx.name.startsWith("sqlite_"))
			.map((idx) => ({
				name: idx.name,
				columns: [], // Would need separate PRAGMA index_info call for columns
				unique: idx.unique === 1,
				type: "btree",
			}));
	}

	// === STATISTICS METHODS ===
	// Note: SQLite has limited statistics compared to PostgreSQL

	getTableSizesQuery(): string {
		// SQLite doesn't track individual table sizes easily
		// We get table names first, then row counts are fetched separately
		return `SELECT
			name AS table_name,
			'main' AS schema_name
		FROM sqlite_master
		WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
		ORDER BY name`;
	}

	getTableRowCountQuery(table: string, _schema: string): string {
		return `SELECT COUNT(*) AS row_count FROM "${validateIdentifier(table)}"`;
	}

	getIndexUsageQuery(): string {
		// SQLite doesn't track index usage statistics
		// Return indexes from sqlite_master
		return `SELECT
			m.name AS index_name,
			m.tbl_name AS table_name,
			'main' AS schema_name
		FROM sqlite_master m
		WHERE m.type = 'index' AND m.name NOT LIKE 'sqlite_%'
		ORDER BY m.tbl_name, m.name`;
	}

	getDatabaseOverviewQuery(): string {
		// Get basic database info using PRAGMA
		return `SELECT
			(SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%') AS table_count,
			(SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name NOT LIKE 'sqlite_%') AS index_count,
			(SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()) AS total_size_bytes`;
	}

	parseTableSizesResult(rows: unknown[]): TableSizeInfo[] {
		// SQLite doesn't provide detailed size info per table
		return (rows as { table_name: string; schema_name: string }[]).map((row) => ({
			schema: row.schema_name || 'main',
			name: row.table_name,
			rowCount: 0, // Would need separate COUNT(*) queries
			totalSize: 'N/A',
			totalSizeBytes: 0,
		}));
	}

	parseIndexUsageResult(rows: unknown[]): IndexUsageInfo[] {
		// SQLite doesn't track index usage
		return (rows as { index_name: string; table_name: string; schema_name: string }[]).map((row) => ({
			schema: row.schema_name || 'main',
			table: row.table_name,
			indexName: row.index_name,
			size: 'N/A',
			scans: 0,
			unused: false, // Unknown
		}));
	}

	parseDatabaseOverviewResult(rows: unknown[]): DatabaseOverview {
		const row = (rows as { table_count: number; index_count: number; total_size_bytes: number }[])[0];
		const sizeBytes = Number(row?.total_size_bytes) || 0;
		return {
			databaseName: 'SQLite Database',
			totalSize: this.formatBytes(sizeBytes),
			totalSizeBytes: sizeBytes,
			tableCount: Number(row?.table_count) || 0,
			indexCount: Number(row?.index_count) || 0,
		};
	}

	private formatBytes(bytes: number): string {
		if (bytes === 0) return '0 bytes';
		const k = 1024;
		const sizes = ['bytes', 'KB', 'MB', 'GB', 'TB'];
		const i = Math.floor(Math.log(bytes) / Math.log(k));
		return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
	}
}
