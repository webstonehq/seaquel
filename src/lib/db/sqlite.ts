import type { DatabaseAdapter, ExplainNode } from "./index";
import type { SchemaTable, SchemaColumn, SchemaIndex } from "$lib/types";

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
		return `PRAGMA table_info('${table}')`;
	}

	getIndexesQuery(table: string, _schema: string): string {
		return `PRAGMA index_list('${table}')`;
	}

	getForeignKeysQuery(table: string, _schema: string): string {
		return `PRAGMA foreign_key_list('${table}')`;
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
		const fkColumns = new Set<string>();
		if (foreignKeys) {
			for (const fk of foreignKeys as SqliteForeignKeyRow[]) {
				fkColumns.add(fk.from);
			}
		}

		return (rows as SqliteColumnRow[]).map((col) => ({
			name: col.name,
			type: col.type || "BLOB", // SQLite allows empty type
			nullable: col.notnull === 0,
			defaultValue: col.dflt_value || undefined,
			isPrimaryKey: col.pk > 0,
			isForeignKey: fkColumns.has(col.name),
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
}
