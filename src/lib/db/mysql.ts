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

/** MySQL information_schema may return VARBINARY columns as byte arrays; decode to string */
function decodeValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return String.fromCharCode(...value);
  // oxlint-disable-next-line typescript/no-base-to-string
  return String(value ?? "");
}

interface MysqlSchemaRow {
  schema_name: string;
  table_name: string;
  table_type: string;
}

interface MysqlColumnRow {
  column_name: string;
  data_type: string;
  is_nullable: string;
  column_default: string | null;
  is_primary_key: number;
  is_foreign_key: number;
  foreign_key_ref: string | null;
}

interface MysqlIndexRow {
  index_name: string;
  column_name: string;
  non_unique: number;
  index_type: string;
}

interface MysqlTableSizeRow {
  schema_name: string;
  table_name: string;
  row_count: number;
  total_size: string;
  total_size_bytes: number;
  data_size: string;
  index_size: string;
}

interface MysqlIndexUsageRow {
  schema_name: string;
  table_name: string;
  index_name: string;
  size: string;
  scans: number;
  rows_read: number;
  unused: boolean;
}

interface MysqlDatabaseOverviewRow {
  database_name: string;
  total_size: string;
  total_size_bytes: number;
  table_count: number;
  index_count: number;
  connection_count: number;
}

export class MysqlAdapter implements DatabaseAdapter {
  getSchemaQuery(): string {
    return `SELECT
			table_schema AS schema_name,
			table_name AS table_name,
			table_type
		FROM
			information_schema.tables
		WHERE
			table_type IN ('BASE TABLE', 'VIEW')
			AND table_schema = DATABASE()
		ORDER BY
			table_schema, table_name`;
  }

  getColumnsQuery(table: string, schema: string): string {
    return `SELECT
			c.COLUMN_NAME AS column_name,
			c.COLUMN_TYPE AS data_type,
			c.IS_NULLABLE AS is_nullable,
			c.COLUMN_DEFAULT AS column_default,
			IF(c.COLUMN_KEY = 'PRI', 1, 0) AS is_primary_key,
			IF(c.COLUMN_KEY = 'MUL' AND EXISTS (
				SELECT 1 FROM information_schema.KEY_COLUMN_USAGE kcu
				WHERE kcu.TABLE_SCHEMA = c.TABLE_SCHEMA
					AND kcu.TABLE_NAME = c.TABLE_NAME
					AND kcu.COLUMN_NAME = c.COLUMN_NAME
					AND kcu.REFERENCED_TABLE_NAME IS NOT NULL
			), 1, 0) AS is_foreign_key,
			(SELECT CONCAT(kcu.REFERENCED_TABLE_SCHEMA, '.', kcu.REFERENCED_TABLE_NAME, '.', kcu.REFERENCED_COLUMN_NAME)
				FROM information_schema.KEY_COLUMN_USAGE kcu
				WHERE kcu.TABLE_SCHEMA = c.TABLE_SCHEMA
					AND kcu.TABLE_NAME = c.TABLE_NAME
					AND kcu.COLUMN_NAME = c.COLUMN_NAME
					AND kcu.REFERENCED_TABLE_NAME IS NOT NULL
				LIMIT 1
			) AS foreign_key_ref
		FROM information_schema.COLUMNS c
		WHERE c.TABLE_NAME = '${validateIdentifier(table)}' AND c.TABLE_SCHEMA = '${validateIdentifier(schema)}'
		ORDER BY c.ORDINAL_POSITION`;
  }

  getIndexesQuery(table: string, schema: string): string {
    return `SELECT
			INDEX_NAME AS index_name,
			COLUMN_NAME AS column_name,
			NON_UNIQUE AS non_unique,
			INDEX_TYPE AS index_type
		FROM information_schema.STATISTICS
		WHERE TABLE_NAME = '${validateIdentifier(table)}' AND TABLE_SCHEMA = '${validateIdentifier(schema)}'
		ORDER BY INDEX_NAME, SEQ_IN_INDEX`;
  }

  getExplainQuery(query: string, analyze: boolean): string {
    const baseQuery = query.replace(/;$/, "");
    return analyze ? `EXPLAIN ANALYZE ${baseQuery}` : `EXPLAIN FORMAT=JSON ${baseQuery}`;
  }

  parseExplainResult(rows: unknown[], analyze: boolean): ExplainResult {
    const nextId = makeNodeIdFactory();

    if (analyze) {
      // EXPLAIN ANALYZE returns text rows in MySQL 8.0.18+; lines begin with "-> Op"
      const text = (rows as Record<string, unknown>[])
        .map((r) => {
          const first = Object.values(r)[0];
          return typeof first === "string" ? first : "";
        })
        .join("\n");
      const plan = this.parseMysqlAnalyzeTree(text, nextId);
      return {
        plan,
        planningTime: 0,
        executionTime: undefined,
        isAnalyze: true,
      };
    }

    // EXPLAIN FORMAT=JSON returns a single row with the JSON plan
    const result = rows as Record<string, unknown>[];
    const jsonStr = Object.values(result[0] ?? {})[0];
    const parsed = typeof jsonStr === "string" ? JSON.parse(jsonStr) : jsonStr;

    // Schema v2.0 (MySQL 9.x+): root holds a `query_plan` operation tree.
    const queryPlan = (parsed as { query_plan?: Record<string, unknown> })?.query_plan;
    if (queryPlan) {
      return {
        plan: this.convertMysqlV2PlanNode(queryPlan, nextId),
        planningTime: 0,
        executionTime: undefined,
        isAnalyze: false,
      };
    }

    // Schema v1 (MySQL 8.x): root holds a `query_block`.
    const queryBlock = (parsed as { query_block?: Record<string, unknown> })?.query_block;
    if (!queryBlock) {
      return {
        plan: { id: nextId(), nodeType: "Query", children: [] },
        planningTime: 0,
        executionTime: undefined,
        isAnalyze: false,
      };
    }

    return {
      plan: this.convertMysqlPlanNode(queryBlock, nextId),
      planningTime: 0,
      executionTime: undefined,
      isAnalyze: false,
    };
  }

  /**
   * Convert a MySQL EXPLAIN FORMAT=JSON schema v2.0 operation node.
   * v2.0 (introduced with `json_schema_version: "2.0"`) replaces the nested
   * `table`/`nested_loop` shape with a uniform tree: every node has an
   * `operation` string, optional `inputs[]` for children, and access metadata.
   */
  private convertMysqlV2PlanNode(
    node: Record<string, unknown>,
    nextId: () => string,
  ): ExplainPlanNode {
    const operation = (node["operation"] as string) ?? "Operator";
    const inputs = (node["inputs"] as Record<string, unknown>[] | undefined) ?? [];
    const estimatedRows = node["estimated_rows"];
    const estimatedCost = node["estimated_total_cost"];

    // Strip " on <alias>" and trailing parenthesized detail to get a clean
    // operator name (matches the EXPLAIN ANALYZE tree parser's convention).
    const nodeType = operation.split(/\s+on\s+|\s*\(/)[0]?.trim() || operation;

    return {
      id: nextId(),
      nodeType,
      relationName: node["table_name"] as string | undefined,
      alias: node["alias"] as string | undefined,
      indexName: node["index_name"] as string | undefined,
      joinType: node["join_type"] as string | undefined,
      indexCond: node["lookup_condition"] as string | undefined,
      filter: node["condition"] as string | undefined,
      planRows: typeof estimatedRows === "number" ? Math.round(estimatedRows) : undefined,
      totalCost: typeof estimatedCost === "number" ? estimatedCost : undefined,
      children: inputs.map((child) => this.convertMysqlV2PlanNode(child, nextId)),
    };
  }

  private static readonly ACCESS_TYPE_LABELS: Record<string, string> = {
    ALL: "Table Scan",
    const: "Const",
    eq_ref: "Index Scan",
    fulltext: "Fulltext Scan",
    index: "Index Scan",
    index_merge: "Index Merge",
    index_subquery: "Index Subquery",
    range: "Range Scan",
    ref: "Index Scan",
    ref_or_null: "Index Scan",
    system: "Const",
    unique_subquery: "Unique Subquery",
  };

  private convertMysqlPlanNode(
    node: Record<string, unknown>,
    nextId: () => string,
  ): ExplainPlanNode {
    const table = node["table"] as Record<string, unknown> | undefined;
    const nestedLoop = node["nested_loop"] as Record<string, unknown>[] | undefined;
    const grouping = node["grouping_operation"] as Record<string, unknown> | undefined;
    const ordering = node["ordering_operation"] as Record<string, unknown> | undefined;
    const duplicates = node["duplicates_removal"] as Record<string, unknown> | undefined;
    const attachedSubqueries = node["attached_subqueries"] as Record<string, unknown>[] | undefined;
    const unionResult = node["union_result"] as Record<string, unknown> | undefined;

    const queryCost = node["cost_info"]
      ? Number((node["cost_info"] as Record<string, unknown>)["query_cost"])
      : undefined;

    if (table) {
      const accessType = (table["access_type"] as string) || "ALL";
      const readCost = Number(
        (table["cost_info"] as Record<string, unknown> | undefined)?.["read_cost"] ?? 0,
      );
      const evalCost = Number(
        (table["cost_info"] as Record<string, unknown> | undefined)?.["eval_cost"] ?? 0,
      );
      const totalCost = readCost || evalCost ? readCost + evalCost : undefined;

      const node: ExplainPlanNode = {
        id: nextId(),
        nodeType: MysqlAdapter.ACCESS_TYPE_LABELS[accessType] ?? accessType,
        relationName: table["table_name"] as string | undefined,
        indexName: table["key"] as string | undefined,
        filter: table["attached_condition"] as string | undefined,
        planRows: table["rows_examined_per_scan"] as number | undefined,
        totalCost,
        children: [],
      };
      // Some MySQL plans nest a materialized subquery inside the table node.
      const materialized = table["materialized_from_subquery"] as
        | Record<string, unknown>
        | undefined;
      if (materialized?.["query_block"]) {
        node.children.push(
          this.convertMysqlPlanNode(materialized["query_block"] as Record<string, unknown>, nextId),
        );
      }
      return node;
    }

    if (nestedLoop) {
      return {
        id: nextId(),
        nodeType: "Nested Loop",
        totalCost: queryCost,
        children: nestedLoop.map((item) => this.convertMysqlPlanNode(item, nextId)),
      };
    }

    if (grouping) {
      return {
        id: nextId(),
        nodeType: "Group",
        totalCost: queryCost,
        children: [this.convertMysqlPlanNode(grouping, nextId)],
      };
    }

    if (ordering) {
      return {
        id: nextId(),
        nodeType: "Sort",
        totalCost: queryCost,
        children: [this.convertMysqlPlanNode(ordering, nextId)],
      };
    }

    if (duplicates) {
      return {
        id: nextId(),
        nodeType: "Distinct",
        totalCost: queryCost,
        children: [this.convertMysqlPlanNode(duplicates, nextId)],
      };
    }

    if (unionResult) {
      const specs = (unionResult["query_specifications"] as Record<string, unknown>[]) ?? [];
      return {
        id: nextId(),
        nodeType: "Union",
        children: specs.map((spec) => {
          const qb = spec["query_block"] as Record<string, unknown> | undefined;
          return qb
            ? this.convertMysqlPlanNode(qb, nextId)
            : { id: nextId(), nodeType: "Query Block", children: [] };
        }),
      };
    }

    // Plain query_block
    const children: ExplainPlanNode[] = [];
    if (attachedSubqueries) {
      for (const sub of attachedSubqueries) {
        const qb = sub["query_block"] as Record<string, unknown> | undefined;
        if (qb) children.push(this.convertMysqlPlanNode(qb, nextId));
      }
    }
    return {
      id: nextId(),
      nodeType: "Query Block",
      totalCost: queryCost,
      children,
    };
  }

  /**
   * Parse MySQL 8.0.18+ `EXPLAIN ANALYZE` text output into a tree.
   * Each line begins with "-> Operator  (cost=X rows=Y) (actual time=a..b rows=r loops=l)";
   * child operators are indented by 4 spaces per level relative to their parent.
   */
  private parseMysqlAnalyzeTree(text: string, nextId: () => string): ExplainPlanNode {
    const lines = text.split("\n").filter((l) => l.trim().length > 0);
    const root: ExplainPlanNode = { id: nextId(), nodeType: "Query", children: [] };
    const stack: { indent: number; node: ExplainPlanNode }[] = [{ indent: -1, node: root }];

    for (const raw of lines) {
      const indent = raw.search(/->/);
      if (indent < 0) continue;
      const body = raw.slice(indent + 2).trim();

      const node: ExplainPlanNode = { id: nextId(), nodeType: "Operator", children: [] };

      // Pull off trailing parenthesized metadata blocks left-to-right.
      let head = body;
      const estMatch = /\(cost=([\d.]+)\s+rows=(\d+)\)/.exec(body);
      if (estMatch) {
        node.totalCost = Number(estMatch[1]);
        node.planRows = Number(estMatch[2]);
        head = head.replace(estMatch[0], "");
      }
      const actualMatch = /\(actual time=([\d.]+)\.\.([\d.]+)\s+rows=(\d+)\s+loops=(\d+)\)/.exec(
        body,
      );
      if (actualMatch) {
        node.actualStartupTime = Number(actualMatch[1]);
        node.actualTotalTime = Number(actualMatch[2]);
        node.actualRows = Number(actualMatch[3]);
        node.actualLoops = Number(actualMatch[4]);
        head = head.replace(actualMatch[0], "");
      }

      // The remaining head is "OperatorName [on table] [using index]..."
      const headTrim = head.trim();
      const onMatch = /\bon\s+(\w+)(?:\s+using\s+(\w+))?/.exec(headTrim);
      if (onMatch) {
        node.relationName = onMatch[1];
        if (onMatch[2]) node.indexName = onMatch[2];
      }
      const nodeTypeText = headTrim.split(/\s+on\s+|\s*\(/)[0]?.trim() ?? "Operator";
      node.nodeType = nodeTypeText || "Operator";

      while (stack.length > 1 && stack[stack.length - 1]!.indent >= indent) {
        stack.pop();
      }
      stack[stack.length - 1]!.node.children.push(node);
      stack.push({ indent, node });
    }

    // If there's exactly one top-level operator, return it directly instead of the wrapper.
    if (root.children.length === 1) return root.children[0]!;
    return root;
  }

  parseSchemaResult(rows: unknown[]): SchemaTable[] {
    return (rows as MysqlSchemaRow[])
      .map((row) => ({
        name: decodeValue(row.table_name),
        schema: decodeValue(row.schema_name),
        type: (row.table_type === "VIEW" ? "view" : "table") as SchemaTable["type"],
        columns: [],
        indexes: [],
      }))
      .filter((t) => t.name !== "");
  }

  parseColumnsResult(rows: unknown[]): SchemaColumn[] {
    return (rows as MysqlColumnRow[]).map((col) => {
      const fkRef = col.foreign_key_ref ? decodeValue(col.foreign_key_ref) : null;
      let foreignKeyRef: ForeignKeyRef | undefined;
      if (fkRef) {
        const parts = fkRef.split(".");
        if (parts.length === 3) {
          foreignKeyRef = {
            referencedSchema: parts[0],
            referencedTable: parts[1],
            referencedColumn: parts[2],
          };
        }
      }
      return {
        name: decodeValue(col.column_name),
        type: decodeValue(col.data_type),
        nullable: decodeValue(col.is_nullable) === "YES",
        defaultValue: col.column_default ? decodeValue(col.column_default) : undefined,
        isPrimaryKey: !!col.is_primary_key,
        isForeignKey: !!col.is_foreign_key,
        foreignKeyRef,
      };
    });
  }

  parseIndexesResult(rows: unknown[]): SchemaIndex[] {
    // Group rows by index name since MySQL returns one row per column
    const indexMap = new Map<string, { columns: string[]; unique: boolean; type: string }>();
    for (const row of rows as MysqlIndexRow[]) {
      const name = decodeValue(row.index_name);
      const colName = decodeValue(row.column_name);
      const existing = indexMap.get(name);
      if (existing) {
        existing.columns.push(colName);
      } else {
        indexMap.set(name, {
          columns: [colName],
          unique: !row.non_unique,
          type: decodeValue(row.index_type).toLowerCase(),
        });
      }
    }

    return Array.from(indexMap.entries()).map(([name, info]) => ({
      name,
      columns: info.columns,
      unique: info.unique,
      type: info.type,
    }));
  }

  // === STATISTICS METHODS ===

  getTableSizesQuery(): string {
    return `SELECT
			TABLE_SCHEMA AS schema_name,
			TABLE_NAME AS table_name,
			TABLE_ROWS AS row_count,
			CONCAT(ROUND((DATA_LENGTH + INDEX_LENGTH) / 1024, 2), ' KB') AS total_size,
			(DATA_LENGTH + INDEX_LENGTH) AS total_size_bytes,
			CONCAT(ROUND(DATA_LENGTH / 1024, 2), ' KB') AS data_size,
			CONCAT(ROUND(INDEX_LENGTH / 1024, 2), ' KB') AS index_size
		FROM information_schema.TABLES
		WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE'
		ORDER BY (DATA_LENGTH + INDEX_LENGTH) DESC`;
  }

  getIndexUsageQuery(): string {
    return `SELECT
			TABLE_SCHEMA AS schema_name,
			TABLE_NAME AS table_name,
			INDEX_NAME AS index_name,
			CONCAT(ROUND(STAT_VALUE * @@innodb_page_size / 1024, 2), ' KB') AS size,
			0 AS scans,
			0 AS rows_read,
			0 AS unused
		FROM mysql.innodb_index_stats
		WHERE stat_name = 'size' AND TABLE_SCHEMA = DATABASE()
		ORDER BY STAT_VALUE DESC`;
  }

  getDatabaseOverviewQuery(): string {
    return `SELECT
			DATABASE() AS database_name,
			CONCAT(ROUND(SUM(DATA_LENGTH + INDEX_LENGTH) / 1024, 2), ' KB') AS total_size,
			SUM(DATA_LENGTH + INDEX_LENGTH) AS total_size_bytes,
			COUNT(*) AS table_count,
			(SELECT COUNT(*) FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = DATABASE()) AS index_count,
			(SELECT COUNT(*) FROM information_schema.PROCESSLIST) AS connection_count
		FROM information_schema.TABLES
		WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE'`;
  }

  parseTableSizesResult(rows: unknown[]): TableSizeInfo[] {
    return (rows as MysqlTableSizeRow[]).map((row) => ({
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
    return (rows as MysqlIndexUsageRow[]).map((row) => ({
      schema: row.schema_name,
      table: row.table_name,
      indexName: row.index_name,
      size: row.size,
      scans: Number(row.scans) || 0,
      rowsRead: Number(row.rows_read) || 0,
      unused: !!row.unused,
    }));
  }

  parseDatabaseOverviewResult(rows: unknown[]): DatabaseOverview {
    const row = (rows as MysqlDatabaseOverviewRow[])[0];
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
      { name: "VARCHAR", category: "String", hasLength: true },
      { name: "CHAR", category: "String", hasLength: true },
      { name: "TEXT", category: "String" },
      { name: "TINYTEXT", category: "String" },
      { name: "MEDIUMTEXT", category: "String" },
      { name: "LONGTEXT", category: "String" },
      { name: "ENUM", category: "String" },
      { name: "SET", category: "String" },
      // Numeric
      { name: "INT", category: "Numeric" },
      { name: "TINYINT", category: "Numeric" },
      { name: "SMALLINT", category: "Numeric" },
      { name: "MEDIUMINT", category: "Numeric" },
      { name: "BIGINT", category: "Numeric" },
      { name: "FLOAT", category: "Numeric" },
      { name: "DOUBLE", category: "Numeric" },
      { name: "DECIMAL", category: "Numeric", hasPrecision: true },
      // Date/Time
      { name: "DATE", category: "Date/Time" },
      { name: "DATETIME", category: "Date/Time" },
      { name: "TIMESTAMP", category: "Date/Time" },
      { name: "TIME", category: "Date/Time" },
      { name: "YEAR", category: "Date/Time" },
      // Boolean
      { name: "BOOLEAN", category: "Boolean" },
      // JSON
      { name: "JSON", category: "JSON" },
      // Binary
      { name: "BINARY", category: "Binary", hasLength: true },
      { name: "VARBINARY", category: "Binary", hasLength: true },
      { name: "BLOB", category: "Binary" },
      { name: "TINYBLOB", category: "Binary" },
      { name: "MEDIUMBLOB", category: "Binary" },
      { name: "LONGBLOB", category: "Binary" },
    ];
  }

  private static readonly quote = (n: string) => `\`${n}\``;

  generateCreateTableSql(definition: CreateTableDefinition): string {
    return generateCreateTableDdl(definition, MysqlAdapter.quote);
  }

  generateAddColumnSql(schema: string, table: string, column: CreateTableColumn): string {
    return generateAddColumnDdl(schema, table, column, MysqlAdapter.quote);
  }

  generateAlterTableSql(originalDef: CreateTableDefinition, newDef: CreateTableDefinition): string {
    return generateAlterTableSql(originalDef, newDef, {
      quote: MysqlAdapter.quote,
      supportsDropColumn: true,
      supportsAlterColumn: true,
      useModifyColumn: true,
    });
  }

  getSchemasQuery(): string {
    return `SELECT SCHEMA_NAME as schema_name FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY SCHEMA_NAME;`;
  }

  // === CRUD SQL GENERATION ===

  // MySQL uses backtick-quoted identifiers; double quotes only work with the
  // non-default ANSI_QUOTES SQL mode.
  private qi(id: string): string {
    return `\`${id.replace(/`/g, "``")}\``;
  }

  // MySQL (via sqlx) uses `?` placeholders, not `$N`.
  private static readonly placeholder = () => "?";

  quoteIdentifier(id: string): string {
    return this.qi(id);
  }

  paginateQuery(baseQuery: string, limit: number, offset: number): string {
    return `${baseQuery} LIMIT ${limit} OFFSET ${offset}`;
  }

  // MySQL's CAST only accepts a fixed set of target types (SIGNED, UNSIGNED,
  // CHAR, DATE, JSON, etc.); raw column types like `tinyint(1)` or
  // `varchar(255)` are not valid CAST targets. sqlx binds JS scalars with
  // correct types, so we skip the CAST wrapper entirely for MySQL.
  buildUpdateSql(
    schema: string,
    table: string,
    column: string,
    newValue: unknown,
    primaryKeys: string[],
    row: Record<string, unknown>,
    _castLookup?: CastLookup,
  ): SqlWithBindings {
    return buildParamUpdate(
      schema,
      table,
      column,
      newValue,
      primaryKeys,
      row,
      (id) => this.qi(id),
      undefined,
      MysqlAdapter.placeholder,
    );
  }

  buildSetDefaultSql(
    schema: string,
    table: string,
    column: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings {
    return buildParamSetDefault(
      schema,
      table,
      column,
      primaryKeys,
      row,
      (id) => this.qi(id),
      MysqlAdapter.placeholder,
    );
  }

  buildInsertSql(
    schema: string,
    table: string,
    values: Record<string, unknown>,
    _castLookup?: CastLookup,
  ): SqlWithBindings {
    return buildParamInsert(
      schema,
      table,
      values,
      (id) => this.qi(id),
      undefined,
      MysqlAdapter.placeholder,
    );
  }

  buildDeleteSql(
    schema: string,
    table: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings {
    return buildParamDelete(
      schema,
      table,
      primaryKeys,
      row,
      (id) => this.qi(id),
      MysqlAdapter.placeholder,
    );
  }
}
