import type { DatabaseAdapter, ExplainNode } from "./index";
import type { SchemaTable, SchemaColumn, SchemaIndex } from "$lib/types";

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
}

interface PostgresIndexRow {
	indexname: string;
	indexdef: string;
	schemaname: string;
	tablename: string;
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
				SELECT 1 FROM information_schema.constraint_column_usage ccu
				JOIN information_schema.table_constraints tc ON ccu.constraint_name = tc.constraint_name
				WHERE ccu.column_name = c.column_name
					AND ccu.table_schema = c.table_schema
					AND ccu.table_name = c.table_name
					AND tc.constraint_type = 'PRIMARY KEY'
			)) as is_primary_key,
			(SELECT EXISTS (
				SELECT 1 FROM information_schema.constraint_column_usage ccu
				JOIN information_schema.table_constraints tc ON ccu.constraint_name = tc.constraint_name
				WHERE ccu.column_name = c.column_name
					AND ccu.table_schema = c.table_schema
					AND ccu.table_name = c.table_name
					AND tc.constraint_type = 'FOREIGN KEY'
			)) as is_foreign_key
		FROM information_schema.columns c
		WHERE table_name = '${table}' AND table_schema = '${schema}'
		ORDER BY ordinal_position`;
	}

	getIndexesQuery(table: string, schema: string): string {
		return `SELECT
			indexname,
			indexdef,
			schemaname,
			tablename
		FROM pg_indexes
		WHERE tablename = '${table}' AND schemaname = '${schema}'`;
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
		return (rows as PostgresColumnRow[]).map((col) => ({
			name: col.column_name,
			type: col.data_type,
			nullable: col.is_nullable === "YES",
			defaultValue: col.column_default || undefined,
			isPrimaryKey: col.is_primary_key,
			isForeignKey: col.is_foreign_key,
		}));
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
}
