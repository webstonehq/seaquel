import type { DatabaseAdapter, ExplainNode } from "./index";
import { validateIdentifier } from "./index";
import type { SchemaTable, SchemaColumn, SchemaIndex, ForeignKeyRef } from "$lib/types";

interface MssqlSchemaRow {
	schema_name: string;
	table_name: string;
}

interface MssqlColumnRow {
	column_name: string;
	data_type: string;
	is_nullable: string;
	column_default: string | null;
	is_primary_key: number;
	is_foreign_key: number;
	referenced_schema: string | null;
	referenced_table: string | null;
	referenced_column: string | null;
}

interface MssqlIndexRow {
	index_name: string;
	column_name: string;
	is_unique: number;
	index_type: string;
}

export class MssqlAdapter implements DatabaseAdapter {
	getSchemaQuery(): string {
		return `SELECT
			s.name AS schema_name,
			t.name AS table_name
		FROM sys.tables t
		INNER JOIN sys.schemas s ON t.schema_id = s.schema_id
		WHERE t.is_ms_shipped = 0
		ORDER BY s.name, t.name`;
	}

	getColumnsQuery(table: string, schema: string): string {
		const safeTable = validateIdentifier(table);
		const safeSchema = validateIdentifier(schema);

		return `SELECT
			c.name AS column_name,
			TYPE_NAME(c.user_type_id) AS data_type,
			CASE WHEN c.is_nullable = 1 THEN 'YES' ELSE 'NO' END AS is_nullable,
			dc.definition AS column_default,
			CASE WHEN pk.column_id IS NOT NULL THEN 1 ELSE 0 END AS is_primary_key,
			CASE WHEN fk.parent_column_id IS NOT NULL THEN 1 ELSE 0 END AS is_foreign_key,
			rs.name AS referenced_schema,
			rt.name AS referenced_table,
			rc.name AS referenced_column
		FROM sys.columns c
		INNER JOIN sys.tables t ON c.object_id = t.object_id
		INNER JOIN sys.schemas s ON t.schema_id = s.schema_id
		LEFT JOIN sys.default_constraints dc ON c.default_object_id = dc.object_id
		LEFT JOIN (
			SELECT ic.object_id, ic.column_id
			FROM sys.index_columns ic
			INNER JOIN sys.indexes i ON ic.object_id = i.object_id AND ic.index_id = i.index_id
			WHERE i.is_primary_key = 1
		) pk ON c.object_id = pk.object_id AND c.column_id = pk.column_id
		LEFT JOIN sys.foreign_key_columns fk ON c.object_id = fk.parent_object_id AND c.column_id = fk.parent_column_id
		LEFT JOIN sys.tables rt ON fk.referenced_object_id = rt.object_id
		LEFT JOIN sys.schemas rs ON rt.schema_id = rs.schema_id
		LEFT JOIN sys.columns rc ON fk.referenced_object_id = rc.object_id AND fk.referenced_column_id = rc.column_id
		WHERE t.name = '${safeTable}' AND s.name = '${safeSchema}'
		ORDER BY c.column_id`;
	}

	getIndexesQuery(table: string, schema: string): string {
		const safeTable = validateIdentifier(table);
		const safeSchema = validateIdentifier(schema);

		return `SELECT
			i.name AS index_name,
			c.name AS column_name,
			i.is_unique,
			i.type_desc AS index_type
		FROM sys.indexes i
		INNER JOIN sys.index_columns ic ON i.object_id = ic.object_id AND i.index_id = ic.index_id
		INNER JOIN sys.columns c ON ic.object_id = c.object_id AND ic.column_id = c.column_id
		INNER JOIN sys.tables t ON i.object_id = t.object_id
		INNER JOIN sys.schemas s ON t.schema_id = s.schema_id
		WHERE t.name = '${safeTable}'
			AND s.name = '${safeSchema}'
			AND i.name IS NOT NULL
		ORDER BY i.name, ic.key_ordinal`;
	}

	getExplainQuery(query: string, analyze: boolean): string {
		const baseQuery = query.replace(/;$/, "");
		// SQL Server uses SET SHOWPLAN_XML for execution plans
		// For actual execution statistics, use SET STATISTICS PROFILE
		if (analyze) {
			// Return the query as-is for now - actual execution plans in SQL Server
			// require special handling with SET statements
			return baseQuery;
		}
		return baseQuery;
	}

	parseExplainResult(_rows: unknown[], _analyze: boolean): ExplainNode {
		// SQL Server execution plans are complex XML documents
		// For now, return a placeholder - full implementation would parse XML
		return {
			type: "Query Plan",
			label: "SQL Server execution plans require special handling",
			children: [],
		};
	}

	parseSchemaResult(rows: unknown[]): SchemaTable[] {
		return (rows as MssqlSchemaRow[]).map((row) => ({
			name: row.table_name,
			schema: row.schema_name,
			type: "table" as const,
			columns: [],
			indexes: [],
		}));
	}

	parseColumnsResult(rows: unknown[]): SchemaColumn[] {
		return (rows as MssqlColumnRow[]).map((col) => {
			let foreignKeyRef: ForeignKeyRef | undefined;
			if (col.is_foreign_key && col.referenced_table) {
				foreignKeyRef = {
					referencedSchema: col.referenced_schema || "dbo",
					referencedTable: col.referenced_table,
					referencedColumn: col.referenced_column || "",
				};
			}
			return {
				name: col.column_name,
				type: col.data_type,
				nullable: col.is_nullable === "YES",
				defaultValue: col.column_default || undefined,
				isPrimaryKey: col.is_primary_key === 1,
				isForeignKey: col.is_foreign_key === 1,
				foreignKeyRef,
			};
		});
	}

	parseIndexesResult(rows: unknown[]): SchemaIndex[] {
		// Group by index name since each row is a column in the index
		const indexMap = new Map<string, MssqlIndexRow[]>();
		for (const row of rows as MssqlIndexRow[]) {
			const existing = indexMap.get(row.index_name) || [];
			existing.push(row);
			indexMap.set(row.index_name, existing);
		}

		return Array.from(indexMap.entries()).map(([name, cols]) => ({
			name,
			columns: cols.map((c) => c.column_name),
			unique: cols[0].is_unique === 1,
			type: cols[0].index_type.toLowerCase(),
		}));
	}
}
