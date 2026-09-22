import type { DatabaseAdapter } from "./index";
import { validateIdentifier } from "./index";
import type { ExplainPlanNode, ExplainResult } from "$lib/types";
import { makeNodeIdFactory } from "./explain-helpers";
import { generateAlterTableSql, generateCreateTableDdl, generateAddColumnDdl } from "./alter-table";
import type { SqlWithBindings, CastLookup } from "./crud-helpers";
import {
  buildParamUpdate,
  buildParamSetDefault,
  buildParamInsert,
  buildParamDelete,
} from "./crud-helpers";
import type {
  SchemaTable,
  SchemaColumn,
  SchemaIndex,
  ForeignKeyRef,
  TableSizeInfo,
  IndexUsageInfo,
  DatabaseOverview,
  ColumnTypeInfo,
  CreateTableDefinition,
  CreateTableColumn,
} from "$lib/types";

interface PostgresSchemaRow {
  schema_name: string;
  table_name: string;
  table_type: string;
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
    return `SELECT table_schema AS schema_name, table_name, table_type
		FROM information_schema.tables
		WHERE table_type IN ('BASE TABLE', 'VIEW')
			AND table_schema NOT IN ('pg_catalog', 'information_schema')
		UNION ALL
		SELECT schemaname AS schema_name, matviewname AS table_name, 'MATERIALIZED VIEW' AS table_type
		FROM pg_matviews
		WHERE schemaname NOT IN ('pg_catalog', 'information_schema')
		ORDER BY schema_name, table_name`;
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

  parseExplainResult(rows: unknown[], analyze: boolean): ExplainResult {
    // PostgreSQL returns JSON in a column called "QUERY PLAN"
    const result = rows as { "QUERY PLAN"?: unknown; "query plan"?: unknown }[];
    const jsonPlan = result[0]?.["QUERY PLAN"] || result[0]?.["query plan"];
    const parsedPlan = typeof jsonPlan === "string" ? JSON.parse(jsonPlan) : jsonPlan;
    const top = (parsedPlan as Record<string, unknown>[])[0] ?? {};
    const rootNode = (top["Plan"] ?? {}) as Record<string, unknown>;

    const nextId = makeNodeIdFactory();
    const plan = this.convertPgPlanNode(rootNode, analyze, nextId);

    return {
      plan,
      planningTime: (top["Planning Time"] as number | undefined) ?? 0,
      executionTime: top["Execution Time"] as number | undefined,
      isAnalyze: analyze,
    };
  }

  private convertPgPlanNode(
    node: Record<string, unknown>,
    analyze: boolean,
    nextId: () => string,
  ): ExplainPlanNode {
    const plans = (node["Plans"] as Record<string, unknown>[] | undefined) ?? [];
    const children = plans.map((child) => this.convertPgPlanNode(child, analyze, nextId));

    const out: ExplainPlanNode = {
      id: nextId(),
      // oxlint-disable-next-line typescript/no-base-to-string
      nodeType: String(node["Node Type"] ?? "Unknown"),
      relationName: node["Relation Name"] as string | undefined,
      alias: node["Alias"] as string | undefined,
      startupCost: node["Startup Cost"] as number | undefined,
      totalCost: node["Total Cost"] as number | undefined,
      planRows: node["Plan Rows"] as number | undefined,
      planWidth: node["Plan Width"] as number | undefined,
      filter: node["Filter"] as string | undefined,
      indexName: node["Index Name"] as string | undefined,
      indexCond: node["Index Cond"] as string | undefined,
      joinType: node["Join Type"] as string | undefined,
      hashCond: node["Hash Cond"] as string | undefined,
      sortKey: node["Sort Key"] as string[] | undefined,
      children,
    };

    if (analyze) {
      out.actualStartupTime = node["Actual Startup Time"] as number | undefined;
      out.actualTotalTime = node["Actual Total Time"] as number | undefined;
      out.actualRows = node["Actual Rows"] as number | undefined;
      out.actualLoops = node["Actual Loops"] as number | undefined;
    }

    return out;
  }

  parseSchemaResult(rows: unknown[]): SchemaTable[] {
    return (rows as PostgresSchemaRow[]).map((row) => ({
      name: row.table_name,
      schema: row.schema_name,
      type:
        row.table_type === "VIEW"
          ? "view"
          : row.table_type === "MATERIALIZED VIEW"
            ? "materialized-view"
            : "table",
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
      databaseName: row?.database_name ?? "Unknown",
      totalSize: row?.total_size ?? "0 bytes",
      totalSizeBytes: Number(row?.total_size_bytes) || 0,
      tableCount: Number(row?.table_count) || 0,
      indexCount: Number(row?.index_count) || 0,
      connectionCount: Number(row?.connection_count) || 0,
    };
  }

  // === CREATE TABLE METHODS ===

  getColumnTypes(): ColumnTypeInfo[] {
    return [
      // String
      { name: "text", category: "String" },
      { name: "varchar", category: "String", hasLength: true },
      { name: "char", category: "String", hasLength: true },
      // Numeric
      { name: "integer", category: "Numeric" },
      { name: "bigint", category: "Numeric" },
      { name: "smallint", category: "Numeric" },
      { name: "serial", category: "Numeric" },
      { name: "bigserial", category: "Numeric" },
      { name: "numeric", category: "Numeric", hasPrecision: true },
      { name: "real", category: "Numeric" },
      { name: "double precision", category: "Numeric" },
      { name: "money", category: "Numeric" },
      // Date/Time
      { name: "date", category: "Date/Time" },
      { name: "time", category: "Date/Time" },
      { name: "timestamp", category: "Date/Time" },
      { name: "timestamptz", category: "Date/Time" },
      { name: "interval", category: "Date/Time" },
      // Boolean
      { name: "boolean", category: "Boolean" },
      // JSON
      { name: "json", category: "JSON" },
      { name: "jsonb", category: "JSON" },
      // Binary
      { name: "bytea", category: "Binary" },
      // UUID
      { name: "uuid", category: "UUID" },
      // Network
      { name: "inet", category: "Network" },
      { name: "cidr", category: "Network" },
      { name: "macaddr", category: "Network" },
      // Other
      { name: "point", category: "Other" },
      { name: "line", category: "Other" },
      { name: "polygon", category: "Other" },
      { name: "xml", category: "Other" },
      { name: "tsvector", category: "Other" },
      { name: "tsquery", category: "Other" },
    ];
  }

  private static readonly quote = (n: string) => `"${n}"`;

  generateCreateTableSql(definition: CreateTableDefinition): string {
    return generateCreateTableDdl(definition, PostgresAdapter.quote);
  }

  generateAddColumnSql(schema: string, table: string, column: CreateTableColumn): string {
    return generateAddColumnDdl(schema, table, column, PostgresAdapter.quote);
  }

  generateAlterTableSql(originalDef: CreateTableDefinition, newDef: CreateTableDefinition): string {
    return generateAlterTableSql(originalDef, newDef, {
      quote: PostgresAdapter.quote,
      supportsDropColumn: true,
      supportsAlterColumn: true,
      useModifyColumn: false,
    });
  }

  getSchemasQuery(): string {
    return `SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast') ORDER BY schema_name;`;
  }

  // === CRUD SQL GENERATION ===

  private qi(id: string): string {
    return `"${id.replace(/"/g, '""')}"`;
  }

  quoteIdentifier(id: string): string {
    return this.qi(id);
  }

  paginateQuery(baseQuery: string, limit: number, offset: number): string {
    return `${baseQuery} LIMIT ${limit} OFFSET ${offset}`;
  }

  buildUpdateSql(
    schema: string,
    table: string,
    column: string,
    newValue: unknown,
    primaryKeys: string[],
    row: Record<string, unknown>,
    castLookup?: CastLookup,
  ): SqlWithBindings {
    return buildParamUpdate(
      schema,
      table,
      column,
      newValue,
      primaryKeys,
      row,
      (id) => this.qi(id),
      castLookup,
    );
  }

  buildSetDefaultSql(
    schema: string,
    table: string,
    column: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings {
    return buildParamSetDefault(schema, table, column, primaryKeys, row, (id) => this.qi(id));
  }

  buildInsertSql(
    schema: string,
    table: string,
    values: Record<string, unknown>,
    castLookup?: CastLookup,
  ): SqlWithBindings {
    return buildParamInsert(schema, table, values, (id) => this.qi(id), castLookup);
  }

  buildDeleteSql(
    schema: string,
    table: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings {
    return buildParamDelete(schema, table, primaryKeys, row, (id) => this.qi(id));
  }
}
