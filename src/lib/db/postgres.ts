import type { DatabaseAdapter, ExplainNode } from "./index";
import { validateIdentifier } from "./index";
import type { SchemaTable, SchemaColumn, SchemaIndex, ForeignKeyRef, TableSizeInfo, IndexUsageInfo, DatabaseOverview } from "$lib/types";

interface PostgresSchemaRow {
	schema_name: string;
	table_name: string;
}

interface PostgresColumnRow {
	column_name: string;
	data_type: string;
	is_nullable: string;
	column_default: string | null;
	is_primary_key: boolean;
	is_foreign_key: boolean;
	foreign_key_ref: string | null;
}

interface PostgresIndexRow {
	indexname: string;
	indexdef: string;
	schemaname: string;
	tablename: string;
}

interface PostgresTableSizeRow {
	schema_name: string;
	table_name: string;
	row_count: number;
	total_size: string;
	total_size_bytes: number;
	data_size: string;
	index_size: string;
}

interface PostgresIndexUsageRow {
	schema_name: string;
	table_name: string;
	index_name: string;
	size: string;
	scans: number;
	rows_read: number;
	unused: boolean;
}

interface PostgresDatabaseOverviewRow {
	database_name: string;
	total_size: string;
	total_size_bytes: number;
	table_count: number;
	index_count: number;
	connection_count: number;
}

export class PostgresAdapter implements DatabaseAdapter {
	getSchemaQuery(): string {
		return `SELECT
			table_schema AS schema_name,
			table_name
		FROM
			information_schema.tables
		WHERE
			table_type = 'BASE TABLE'
			AND table_schema NOT IN ('pg_catalog', 'information_schema')
		ORDER BY
			table_schema, table_name`;
	}

	getColumnsQuery(table: string, schema: string): string {
		return `SELECT
			column_name,
			data_type,
			is_nullable,
			column_default,
			(SELECT EXISTS (
				SELECT 1 FROM information_schema.key_column_usage kcu
				JOIN information_schema.table_constraints tc ON kcu.constraint_name = tc.constraint_name
					AND kcu.table_schema = tc.table_schema
					AND kcu.table_name = tc.table_name
				WHERE kcu.column_name = c.column_name
					AND kcu.table_schema = c.table_schema
					AND kcu.table_name = c.table_name
					AND tc.constraint_type = 'PRIMARY KEY'
			)) as is_primary_key,
			(SELECT EXISTS (
				SELECT 1 FROM information_schema.key_column_usage kcu
				JOIN information_schema.table_constraints tc ON kcu.constraint_name = tc.constraint_name
					AND kcu.table_schema = tc.table_schema
					AND kcu.table_name = tc.table_name
				WHERE kcu.column_name = c.column_name
					AND kcu.table_schema = c.table_schema
					AND kcu.table_name = c.table_name
					AND tc.constraint_type = 'FOREIGN KEY'
			)) as is_foreign_key,
			(SELECT ccu.table_schema || '.' || ccu.table_name || '.' || ccu.column_name
				FROM information_schema.key_column_usage kcu
				JOIN information_schema.table_constraints tc ON kcu.constraint_name = tc.constraint_name
					AND kcu.table_schema = tc.table_schema
					AND kcu.table_name = tc.table_name
				JOIN information_schema.referential_constraints rc ON tc.constraint_name = rc.constraint_name
					AND tc.table_schema = rc.constraint_schema
				JOIN information_schema.constraint_column_usage ccu ON rc.unique_constraint_name = ccu.constraint_name
					AND rc.unique_constraint_schema = ccu.constraint_schema
				WHERE kcu.column_name = c.column_name
					AND kcu.table_schema = c.table_schema
					AND kcu.table_name = c.table_name
					AND tc.constraint_type = 'FOREIGN KEY'
				LIMIT 1
			) as foreign_key_ref
		FROM information_schema.columns c
		WHERE table_name = '${validateIdentifier(table)}' AND table_schema = '${validateIdentifier(schema)}'
		ORDER BY ordinal_position`;
	}

	getIndexesQuery(table: string, schema: string): string {
		return `SELECT
			indexname,
			indexdef,
			schemaname,
			tablename
		FROM pg_indexes
		WHERE tablename = '${validateIdentifier(table)}' AND schemaname = '${validateIdentifier(schema)}'`;
	}

	getExplainQuery(query: string, analyze: boolean): string {
		const baseQuery = query.replace(/;$/, "");
		return analyze
			? `EXPLAIN (ANALYZE, FORMAT JSON) ${baseQuery}`
			: `EXPLAIN (FORMAT JSON) ${baseQuery}`;
	}

	parseExplainResult(rows: unknown[], analyze: boolean): ExplainNode {
		// PostgreSQL returns JSON in a column called "QUERY PLAN"
		const result = rows as { "QUERY PLAN"?: unknown; "query plan"?: unknown }[];
		const jsonPlan = result[0]?.["QUERY PLAN"] || result[0]?.["query plan"];
		const parsedPlan =
			typeof jsonPlan === "string" ? JSON.parse(jsonPlan) : jsonPlan;
		const plan = (parsedPlan as { Plan: Record<string, unknown> }[])[0]?.Plan;

		return this.convertPgPlanNode(plan, analyze);
	}

	private convertPgPlanNode(
		node: Record<string, unknown>,
		analyze: boolean
	): ExplainNode {
		const result: ExplainNode = {
			type: String(node["Node Type"] || "Unknown"),
			label: this.buildPgNodeLabel(node),
			cost: node["Total Cost"] as number | undefined,
			rows: node["Plan Rows"] as number | undefined,
		};

		if (analyze) {
			result.actualTime = node["Actual Total Time"] as number | undefined;
			result.actualRows = node["Actual Rows"] as number | undefined;
		}

		const plans = node["Plans"] as Record<string, unknown>[] | undefined;
		if (plans && plans.length > 0) {
			result.children = plans.map((child) =>
				this.convertPgPlanNode(child, analyze)
			);
		}

		return result;
	}

	private buildPgNodeLabel(node: Record<string, unknown>): string {
		const parts: string[] = [String(node["Node Type"] || "")];
		if (node["Relation Name"]) parts.push(`on ${node["Relation Name"]}`);
		if (node["Index Name"]) parts.push(`using ${node["Index Name"]}`);
		if (node["Join Type"]) parts.push(`(${node["Join Type"]})`);
		return parts.join(" ");
	}

	parseSchemaResult(rows: unknown[]): SchemaTable[] {
		return (rows as PostgresSchemaRow[]).map((row) => ({
			name: row.table_name,
			schema: row.schema_name,
			type: "table" as const,
			columns: [],
			indexes: [],
		}));
	}

	parseColumnsResult(rows: unknown[]): SchemaColumn[] {
		return (rows as PostgresColumnRow[]).map((col) => {
			let foreignKeyRef: ForeignKeyRef | undefined;
			if (col.foreign_key_ref) {
				const parts = col.foreign_key_ref.split(".");
				if (parts.length === 3) {
					foreignKeyRef = {
						referencedSchema: parts[0],
						referencedTable: parts[1],
						referencedColumn: parts[2],
					};
				}
			}
			return {
				name: col.column_name,
				type: col.data_type,
				nullable: col.is_nullable === "YES",
				defaultValue: col.column_default || undefined,
				isPrimaryKey: col.is_primary_key,
				isForeignKey: col.is_foreign_key,
				foreignKeyRef,
			};
		});
	}

	parseIndexesResult(rows: unknown[]): SchemaIndex[] {
		return (rows as PostgresIndexRow[]).map((idx) => ({
			name: idx.indexname,
			columns: this.parseIndexColumns(idx.indexdef),
			unique: idx.indexdef.includes("UNIQUE"),
			type: "btree",
		}));
	}

	private parseIndexColumns(indexdef: string): string[] {
		const match = indexdef.match(/\((.*?)\)/);
		if (!match) return [];
		return match[1].split(",").map((col) => col.trim());
	}

	// === STATISTICS METHODS ===

	getTableSizesQuery(): string {
		// Use pg_stat_user_tables.n_live_tup which is more reliable than reltuples
		// (reltuples returns -1 when table has never been analyzed)
		return `SELECT
			schemaname AS schema_name,
			relname AS table_name,
			COALESCE(n_live_tup, 0) AS row_count,
			pg_size_pretty(pg_total_relation_size(schemaname || '.' || relname)) AS total_size,
			pg_total_relation_size(schemaname || '.' || relname) AS total_size_bytes,
			pg_size_pretty(pg_relation_size(schemaname || '.' || relname)) AS data_size,
			pg_size_pretty(pg_indexes_size(schemaname || '.' || relname)) AS index_size
		FROM pg_stat_user_tables
		ORDER BY pg_total_relation_size(schemaname || '.' || relname) DESC`;
	}

	getIndexUsageQuery(): string {
		return `SELECT
			schemaname AS schema_name,
			relname AS table_name,
			indexrelname AS index_name,
			pg_size_pretty(pg_relation_size(indexrelid)) AS size,
			idx_scan AS scans,
			idx_tup_read AS rows_read,
			idx_scan = 0 AS unused
		FROM pg_stat_user_indexes
		ORDER BY idx_scan ASC, pg_relation_size(indexrelid) DESC`;
	}

	getDatabaseOverviewQuery(): string {
		return `SELECT
			current_database() AS database_name,
			pg_size_pretty(pg_database_size(current_database())) AS total_size,
			pg_database_size(current_database()) AS total_size_bytes,
			(SELECT count(*) FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog', 'information_schema')) AS table_count,
			(SELECT count(*) FROM pg_indexes WHERE schemaname NOT IN ('pg_catalog', 'information_schema')) AS index_count,
			(SELECT count(*) FROM pg_stat_activity WHERE datname = current_database()) AS connection_count`;
	}

	parseTableSizesResult(rows: unknown[]): TableSizeInfo[] {
		return (rows as PostgresTableSizeRow[]).map((row) => ({
			schema: row.schema_name,
			name: row.table_name,
			rowCount: Number(row.row_count) || 0,
			totalSize: row.total_size,
			totalSizeBytes: Number(row.total_size_bytes) || 0,
			dataSize: row.data_size,
			indexSize: row.index_size,
		}));
	}

	parseIndexUsageResult(rows: unknown[]): IndexUsageInfo[] {
		return (rows as PostgresIndexUsageRow[]).map((row) => ({
			schema: row.schema_name,
			table: row.table_name,
			indexName: row.index_name,
			size: row.size,
			scans: Number(row.scans) || 0,
			rowsRead: Number(row.rows_read) || 0,
			unused: row.unused,
		}));
	}

	parseDatabaseOverviewResult(rows: unknown[]): DatabaseOverview {
		const row = (rows as PostgresDatabaseOverviewRow[])[0];
		return {
			databaseName: row?.database_name ?? 'Unknown',
			totalSize: row?.total_size ?? '0 bytes',
			totalSizeBytes: Number(row?.total_size_bytes) || 0,
			tableCount: Number(row?.table_count) || 0,
			indexCount: Number(row?.index_count) || 0,
			connectionCount: Number(row?.connection_count) || 0,
		};
	}
}
