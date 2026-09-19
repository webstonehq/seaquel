import type { DatabaseAdapter } from "./index";
import { validateIdentifier } from "./index";
import type { ExplainPlanNode, ExplainResult } from "$lib/types";
import { makeNodeIdFactory } from "./explain-helpers";
import { generateAlterTableSql, generateCreateTableDdl, generateAddColumnDdl } from "./alter-table";
import type { SqlWithBindings } from "./crud-helpers";
import {
  buildInlineUpdate,
  buildInlineSetDefault,
  buildInlineInsert,
  buildInlineDelete,
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

interface DuckDBSchemaRow {
  schema_name: string;
  table_name: string;
  table_type: string;
}

interface DuckDBColumnRow {
  column_name: string;
  data_type: string;
  is_nullable: string;
  column_default: string | null;
  is_primary_key: boolean;
}

/**
 * DuckDB adapter for SQL generation and result parsing.
 * DuckDB uses PostgreSQL-compatible SQL syntax.
 */
export class DuckDBAdapter implements DatabaseAdapter {
  getSchemaQuery(): string {
    return `SELECT
			table_schema AS schema_name,
			table_name,
			table_type
		FROM
			information_schema.tables
		WHERE
			table_type IN ('BASE TABLE', 'VIEW')
			AND table_schema NOT IN ('pg_catalog', 'information_schema')
		ORDER BY
			table_schema, table_name`;
  }

  getColumnsQuery(table: string, schema: string): string {
    return `SELECT
			c.column_name,
			c.data_type,
			c.is_nullable,
			c.column_default,
			COALESCE((
				SELECT true
				FROM duckdb_constraints() dc
				WHERE dc.constraint_type = 'PRIMARY KEY'
					AND dc.table_name = c.table_name
					AND dc.schema_name = c.table_schema
					AND list_contains(dc.constraint_column_names, c.column_name)
			), false) AS is_primary_key
		FROM information_schema.columns c
		WHERE c.table_name = '${validateIdentifier(table)}' AND c.table_schema = '${validateIdentifier(schema)}'
		ORDER BY c.ordinal_position`;
  }

  getIndexesQuery(_table: string, _schema: string): string {
    // DuckDB doesn't have traditional indexes exposed via information_schema
    // Return empty result
    return `SELECT NULL AS index_name WHERE 1=0`;
  }

  getForeignKeysQuery(table: string, schema: string): string {
    // DuckDB disallows cross-schema foreign keys, so the referenced table
    // always lives in the source table's schema. Parallel unnest zips the
    // source and referenced column arrays position-wise.
    return `SELECT
			schema_name,
			referenced_table,
			unnest(constraint_column_names) AS source_column,
			unnest(referenced_column_names) AS referenced_column
		FROM duckdb_constraints()
		WHERE constraint_type = 'FOREIGN KEY'
			AND table_name = '${validateIdentifier(table)}'
			AND schema_name = '${validateIdentifier(schema)}'`;
  }

  getExplainQuery(query: string, analyze: boolean): string {
    const baseQuery = query.replace(/;$/, "");
    // DuckDB supports a structured JSON format that preserves the plan tree
    // (including multi-child joins). Ask for it in both cases so we can parse
    // the result the same way regardless of whether ANALYZE is requested.
    return analyze
      ? `EXPLAIN (ANALYZE, FORMAT JSON) ${baseQuery}`
      : `EXPLAIN (FORMAT JSON) ${baseQuery}`;
  }

  parseExplainResult(rows: unknown[], analyze: boolean): ExplainResult {
    // DuckDB's EXPLAIN returns a single row with two columns:
    //   { explain_key: "physical_plan" | "analyzed_plan", explain_value: <json text> }
    const rec = (rows as Record<string, unknown>[])[0] ?? {};
    const rawValue =
      (typeof rec.explain_value === "string" && rec.explain_value) ||
      // Fallback to whichever column carries the JSON body for future-proofing.
      (Object.values(rec).find(
        (v) => typeof v === "string" && v.trim().startsWith(analyze ? "{" : "["),
      ) as string | undefined) ||
      "";

    const nextId = makeNodeIdFactory();
    const unknownPlan = (): ExplainPlanNode => ({
      id: nextId(),
      nodeType: "Query Plan",
      filter: rawValue || "No plan available",
      children: [],
    });

    let parsed: unknown;
    try {
      parsed = rawValue ? JSON.parse(rawValue) : null;
    } catch {
      return {
        plan: unknownPlan(),
        planningTime: 0,
        executionTime: undefined,
        isAnalyze: analyze,
      };
    }

    if (analyze) {
      // Analyzed plan: { latency, children: [ EXPLAIN_ANALYZE wrapper whose child is the real root ] }
      const root = parsed as Record<string, unknown> | null;
      const firstChild = (root?.children as Record<string, unknown>[] | undefined)?.[0];
      // Skip the EXPLAIN_ANALYZE wrapper — it contributes no useful info.
      const realRoot =
        (firstChild?.operator_name as string | undefined)?.trim() === "EXPLAIN_ANALYZE"
          ? (firstChild?.children as Record<string, unknown>[] | undefined)?.[0]
          : firstChild;
      if (!realRoot) {
        return {
          plan: unknownPlan(),
          planningTime: 0,
          executionTime: undefined,
          isAnalyze: true,
        };
      }
      const plan = this.convertDuckDbAnalyzeNode(realRoot, nextId);
      const latencySeconds = typeof root?.latency === "number" ? root.latency : undefined;
      return {
        plan,
        planningTime: 0,
        executionTime: latencySeconds !== undefined ? latencySeconds * 1000 : undefined,
        isAnalyze: true,
      };
    }

    // Plain EXPLAIN: an array with one root node of shape { name, children, extra_info }.
    const arr = Array.isArray(parsed) ? parsed : [];
    const rootNode = (arr[0] ?? null) as Record<string, unknown> | null;
    if (!rootNode) {
      return {
        plan: unknownPlan(),
        planningTime: 0,
        executionTime: undefined,
        isAnalyze: false,
      };
    }
    return {
      plan: this.convertDuckDbPlanNode(rootNode, nextId),
      planningTime: 0,
      executionTime: undefined,
      isAnalyze: false,
    };
  }

  /**
   * Convert a DuckDB EXPLAIN (FORMAT JSON) node into an ExplainPlanNode.
   * Node shape: { name: string, children: Node[], extra_info: object }
   */
  private convertDuckDbPlanNode(
    node: Record<string, unknown>,
    nextId: () => string,
  ): ExplainPlanNode {
    const extra = (node.extra_info as Record<string, unknown> | undefined) ?? {};
    const out: ExplainPlanNode = {
      id: nextId(),
      nodeType: ((node.name as string | undefined) ?? "Operator").trim(),
      children: ((node.children as Record<string, unknown>[] | undefined) ?? []).map((c) =>
        this.convertDuckDbPlanNode(c, nextId),
      ),
    };
    this.applyDuckDbExtraInfo(out, extra);
    return out;
  }

  /**
   * Convert a DuckDB EXPLAIN ANALYZE (FORMAT JSON) node into an ExplainPlanNode.
   * Node shape: { operator_name, operator_type, operator_timing (seconds),
   *               operator_cardinality, extra_info, children }
   *
   * DuckDB reports `operator_timing` as the *exclusive* time spent in that
   * operator (not including children). Downstream consumers (see
   * `analyzeExplainPlan`) follow PostgreSQL's convention where
   * `actualTotalTime` is *cumulative* — the root covers the whole query.
   * We roll the exclusive values up here so percentages/tiers behave
   * correctly (otherwise descendants commonly exceed 100% of the root).
   */
  private convertDuckDbAnalyzeNode(
    node: Record<string, unknown>,
    nextId: () => string,
  ): ExplainPlanNode {
    const extra = (node.extra_info as Record<string, unknown> | undefined) ?? {};
    const exclusiveMs =
      typeof node.operator_timing === "number" ? node.operator_timing * 1000 : undefined;
    const cardinality =
      typeof node.operator_cardinality === "number" ? node.operator_cardinality : undefined;
    const children = ((node.children as Record<string, unknown>[] | undefined) ?? []).map((c) =>
      this.convertDuckDbAnalyzeNode(c, nextId),
    );

    // Cumulative time = self + sum of children's cumulative times.
    // Undefined if neither self nor any child has timing data.
    const childrenTotal = children.reduce(
      (acc, c) => (c.actualTotalTime !== undefined ? acc + c.actualTotalTime : acc),
      0,
    );
    const anyChildHasTiming = children.some((c) => c.actualTotalTime !== undefined);
    const cumulativeMs =
      exclusiveMs !== undefined || anyChildHasTiming
        ? (exclusiveMs ?? 0) + childrenTotal
        : undefined;

    const out: ExplainPlanNode = {
      id: nextId(),
      nodeType: ((node.operator_name as string | undefined) ?? "Operator").trim(),
      actualTotalTime: cumulativeMs,
      actualRows: cardinality,
      children,
    };
    this.applyDuckDbExtraInfo(out, extra);
    return out;
  }

  /**
   * Map DuckDB's `extra_info` fields onto the generic ExplainPlanNode shape.
   * Handles the keys DuckDB emits most commonly (Table, Join Type, Conditions,
   * Filters, Estimated Cardinality, ...).
   */
  private applyDuckDbExtraInfo(node: ExplainPlanNode, extra: Record<string, unknown>): void {
    const table = extra["Table"];
    if (typeof table === "string") node.relationName = table;

    const ec = extra["Estimated Cardinality"];
    if (typeof ec === "number") node.planRows = ec;
    else if (typeof ec === "string" && /^[\d_]+$/.test(ec)) {
      node.planRows = Number(ec.replace(/_/g, ""));
    }

    const joinType = extra["Join Type"];
    if (typeof joinType === "string") node.joinType = joinType;

    const conditions = extra["Conditions"];
    if (typeof conditions === "string") {
      if (node.nodeType.includes("HASH_JOIN") || node.nodeType.includes("HASH JOIN")) {
        node.hashCond = conditions;
      } else if (node.nodeType.includes("INDEX")) {
        node.indexCond = conditions;
      } else {
        node.filter = conditions;
      }
    }

    const filters = extra["Filters"];
    if (typeof filters === "string" && !node.filter) node.filter = filters;
    else if (Array.isArray(filters) && !node.filter) {
      node.filter = filters.filter((f) => typeof f === "string").join(", ");
    }
  }

  parseSchemaResult(rows: unknown[]): SchemaTable[] {
    return (rows as DuckDBSchemaRow[]).map((row) => ({
      name: row.table_name,
      schema: row.schema_name,
      type: row.table_type === "VIEW" ? "view" : "table",
      columns: [],
      indexes: [],
    }));
  }

  parseColumnsResult(rows: unknown[], foreignKeys?: unknown[]): SchemaColumn[] {
    const fkMap = new Map<string, ForeignKeyRef>();
    if (foreignKeys) {
      for (const fk of foreignKeys as {
        schema_name: string;
        referenced_table: string;
        source_column: string;
        referenced_column: string;
      }[]) {
        if (!fk.source_column || !fk.referenced_table || !fk.referenced_column) continue;
        fkMap.set(fk.source_column, {
          referencedSchema: fk.schema_name,
          referencedTable: fk.referenced_table,
          referencedColumn: fk.referenced_column,
        });
      }
    }

    return (rows as DuckDBColumnRow[]).map((col) => ({
      name: col.column_name,
      type: col.data_type,
      nullable: col.is_nullable === "YES",
      defaultValue: col.column_default || undefined,
      isPrimaryKey: !!col.is_primary_key,
      isForeignKey: fkMap.has(col.column_name),
      foreignKeyRef: fkMap.get(col.column_name),
    }));
  }

  parseIndexesResult(_rows: unknown[]): SchemaIndex[] {
    // DuckDB doesn't expose index information in a standard way
    return [];
  }

  // === STATISTICS METHODS ===

  getTableSizesQuery(): string {
    // Get table names from information_schema
    return `SELECT
			table_schema AS schema_name,
			table_name
		FROM information_schema.tables
		WHERE table_type = 'BASE TABLE'
			AND table_schema NOT IN ('pg_catalog', 'information_schema')
		ORDER BY table_schema, table_name`;
  }

  getTableRowCountQuery(table: string, schema: string): string {
    return `SELECT COUNT(*) AS row_count FROM "${validateIdentifier(schema)}"."${validateIdentifier(table)}"`;
  }

  getIndexUsageQuery(): string {
    // DuckDB tracks indexes via duckdb_indexes() function
    return `SELECT
			schema_name,
			table_name,
			index_name,
			is_unique
		FROM duckdb_indexes()
		ORDER BY schema_name, table_name, index_name`;
  }

  getDatabaseOverviewQuery(): string {
    // Get basic database stats
    return `SELECT
			(SELECT COUNT(*) FROM information_schema.tables WHERE table_type = 'BASE TABLE' AND table_schema NOT IN ('pg_catalog', 'information_schema')) AS table_count,
			(SELECT COUNT(*) FROM duckdb_indexes()) AS index_count,
			0 AS total_size_bytes`;
  }

  parseTableSizesResult(rows: unknown[]): TableSizeInfo[] {
    return (rows as { schema_name: string; table_name: string }[]).map((row) => ({
      schema: row.schema_name || "main",
      name: row.table_name,
      rowCount: 0, // Filled in separately via getTableRowCountQuery
      totalSize: "N/A",
      totalSizeBytes: 0,
    }));
  }

  parseIndexUsageResult(rows: unknown[]): IndexUsageInfo[] {
    return (
      rows as { schema_name: string; table_name: string; index_name: string; is_unique: boolean }[]
    ).map((row) => ({
      schema: row.schema_name || "main",
      table: row.table_name,
      indexName: row.index_name,
      size: "N/A",
      scans: 0,
      unused: false,
    }));
  }

  parseDatabaseOverviewResult(rows: unknown[]): DatabaseOverview {
    const row = (
      rows as { table_count: number; index_count: number; total_size_bytes: number }[]
    )[0];
    return {
      databaseName: "DuckDB Database",
      totalSize: "In-memory",
      totalSizeBytes: 0,
      tableCount: Number(row?.table_count) || 0,
      indexCount: Number(row?.index_count) || 0,
    };
  }

  // === CREATE TABLE METHODS ===

  getColumnTypes(): ColumnTypeInfo[] {
    return [
      // String
      { name: "VARCHAR", category: "String", hasLength: true },
      { name: "TEXT", category: "String" },
      // Numeric
      { name: "INTEGER", category: "Numeric" },
      { name: "BIGINT", category: "Numeric" },
      { name: "HUGEINT", category: "Numeric" },
      { name: "SMALLINT", category: "Numeric" },
      { name: "TINYINT", category: "Numeric" },
      { name: "UINTEGER", category: "Numeric" },
      { name: "UBIGINT", category: "Numeric" },
      { name: "UHUGEINT", category: "Numeric" },
      { name: "USMALLINT", category: "Numeric" },
      { name: "UTINYINT", category: "Numeric" },
      { name: "DOUBLE", category: "Numeric" },
      { name: "FLOAT", category: "Numeric" },
      { name: "DECIMAL", category: "Numeric", hasPrecision: true },
      { name: "BIGNUM", category: "Numeric" },
      // Date/Time
      { name: "DATE", category: "Date/Time" },
      { name: "TIME", category: "Date/Time" },
      { name: "TIMESTAMP", category: "Date/Time" },
      { name: "TIMESTAMP WITH TIME ZONE", category: "Date/Time" },
      { name: "INTERVAL", category: "Date/Time" },
      // Boolean
      { name: "BOOLEAN", category: "Boolean" },
      // JSON
      { name: "JSON", category: "JSON" },
      // Binary
      { name: "BLOB", category: "Binary" },
      { name: "BIT", category: "Binary" },
      // UUID
      { name: "UUID", category: "UUID" },
      // Other
      { name: "ARRAY", category: "Other" },
      { name: "LIST", category: "Other" },
      { name: "MAP", category: "Other" },
      { name: "STRUCT", category: "Other" },
      { name: "UNION", category: "Other" },
    ];
  }

  private static readonly quote = (n: string) => `"${n}"`;

  generateCreateTableSql(definition: CreateTableDefinition): string {
    return generateCreateTableDdl(definition, DuckDBAdapter.quote);
  }

  generateAddColumnSql(schema: string, table: string, column: CreateTableColumn): string {
    return generateAddColumnDdl(schema, table, column, DuckDBAdapter.quote);
  }

  generateAlterTableSql(originalDef: CreateTableDefinition, newDef: CreateTableDefinition): string {
    return generateAlterTableSql(originalDef, newDef, {
      quote: DuckDBAdapter.quote,
      supportsDropColumn: true,
      supportsAlterColumn: true,
      useModifyColumn: false,
    });
  }

  getSchemasQuery(): string {
    return `SELECT schema_name FROM information_schema.schemata WHERE catalog_name NOT IN ('system', 'temp') ORDER BY schema_name;`;
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
  ): SqlWithBindings {
    return buildInlineUpdate(schema, table, column, newValue, primaryKeys, row, (id) =>
      this.qi(id),
    );
  }

  buildSetDefaultSql(
    schema: string,
    table: string,
    column: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings {
    return buildInlineSetDefault(schema, table, column, primaryKeys, row, (id) => this.qi(id));
  }

  buildInsertSql(schema: string, table: string, values: Record<string, unknown>): SqlWithBindings {
    return buildInlineInsert(schema, table, values, (id) => this.qi(id));
  }

  buildDeleteSql(
    schema: string,
    table: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings {
    return buildInlineDelete(schema, table, primaryKeys, row, (id) => this.qi(id));
  }
}
