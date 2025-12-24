import type { DatabaseAdapter } from "./index";
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
