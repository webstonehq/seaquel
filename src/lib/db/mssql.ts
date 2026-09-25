import type { DatabaseAdapter } from "./index";
import { validateIdentifier } from "./index";
import type { ExplainPlanNode, ExplainResult } from "$lib/types";
import { makeNodeIdFactory } from "./explain-helpers";
import { generateAlterTableSql, generateCreateTableDdl, generateAddColumnDdl } from "./alter-table";
import type { SqlWithBindings, ValueFormatter } from "./crud-helpers";
import {
  buildInlineUpdate,
  buildInlineSetDefault,
  buildInlineInsert,
  buildInlineDelete,
  formatLiteralValue,
  formatMssqlBinary,
} from "./crud-helpers";
import type {
  SchemaTable,
  SchemaColumn,
  SchemaIndex,
  ForeignKeyRef,
  ColumnTypeInfo,
  CreateTableDefinition,
  CreateTableColumn,
} from "$lib/types";

interface MssqlSchemaRow {
  schema_name: string;
  table_name: string;
  object_type: string;
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
    return `SELECT s.name AS schema_name, t.name AS table_name, 'TABLE' AS object_type
		FROM sys.tables t
		INNER JOIN sys.schemas s ON t.schema_id = s.schema_id
		WHERE t.is_ms_shipped = 0
		UNION ALL
		SELECT s.name AS schema_name, v.name AS table_name, 'VIEW' AS object_type
		FROM sys.views v
		INNER JOIN sys.schemas s ON v.schema_id = s.schema_id
		WHERE v.is_ms_shipped = 0
		ORDER BY schema_name, table_name`;
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
    // SQL Server emits XML plans via session-level SET statements.
    // SHOWPLAN_XML = estimated plan; STATISTICS XML = actual plan with runtime counters.
    if (analyze) {
      return `SET STATISTICS XML ON;\n${baseQuery};\nSET STATISTICS XML OFF;`;
    }
    return `SET SHOWPLAN_XML ON;\n${baseQuery};\nSET SHOWPLAN_XML OFF;`;
  }

  parseExplainResult(rows: unknown[], analyze: boolean): ExplainResult {
    const nextId = makeNodeIdFactory();

    // SQL Server returns the XML plan in a single cell — column name is
    // "Microsoft SQL Server 2005 XML Showplan" (or similar across versions).
    const xml = this.extractMssqlXml(rows);
    if (!xml) {
      return {
        plan: {
          id: nextId(),
          nodeType: "Query Plan",
          filter: "No XML plan returned. Check permissions for SHOWPLAN.",
          children: [],
        },
        planningTime: 0,
        executionTime: undefined,
        isAnalyze: analyze,
      };
    }

    try {
      const doc = new DOMParser().parseFromString(xml, "application/xml");
      const parserError = doc.querySelector("parsererror");
      if (parserError) throw new Error(parserError.textContent ?? "Failed to parse plan XML");

      // The root operator lives under StmtSimple/QueryPlan/RelOp.
      const rootRelOp =
        doc.querySelector("StmtSimple > QueryPlan > RelOp") ?? doc.querySelector("RelOp");
      if (!rootRelOp) {
        return {
          plan: { id: nextId(), nodeType: "Query Plan", children: [] },
          planningTime: 0,
          executionTime: undefined,
          isAnalyze: analyze,
        };
      }

      return {
        plan: this.convertMssqlRelOp(rootRelOp, nextId),
        planningTime: 0,
        executionTime: undefined,
        isAnalyze: analyze,
      };
    } catch (err) {
      return {
        plan: {
          id: nextId(),
          nodeType: "Query Plan",
          filter: err instanceof Error ? err.message : String(err),
          children: [],
        },
        planningTime: 0,
        executionTime: undefined,
        isAnalyze: analyze,
      };
    }
  }

  private extractMssqlXml(rows: unknown[]): string | null {
    for (const row of rows as Record<string, unknown>[]) {
      for (const value of Object.values(row)) {
        if (typeof value === "string" && value.includes("<ShowPlanXML")) return value;
      }
    }
    return null;
  }

  private convertMssqlRelOp(relOp: Element, nextId: () => string): ExplainPlanNode {
    const node: ExplainPlanNode = {
      id: nextId(),
      nodeType: relOp.getAttribute("LogicalOp") || relOp.getAttribute("PhysicalOp") || "RelOp",
      children: [],
    };

    const estTotalCost = relOp.getAttribute("EstimatedTotalSubtreeCost");
    if (estTotalCost) node.totalCost = Number(estTotalCost);
    const estRows = relOp.getAttribute("EstimateRows");
    if (estRows) node.planRows = Number(estRows);
    const estWidth = relOp.getAttribute("EstimateRowSize") ?? relOp.getAttribute("AvgRowSize");
    if (estWidth) node.planWidth = Number(estWidth);

    // Relation / index come from the first scan-like child element.
    const scanObject = relOp.querySelector(":scope > * > Object");
    if (scanObject) {
      const table = scanObject.getAttribute("Table");
      const index = scanObject.getAttribute("Index");
      if (table) node.relationName = this.stripBrackets(table);
      if (index) node.indexName = this.stripBrackets(index);
    }

    // Predicate / filter wording varies by operator type.
    const predicate = relOp.querySelector(":scope > * > Predicate > ScalarOperator");
    if (predicate) node.filter = predicate.getAttribute("ScalarString") ?? undefined;

    // Actual runtime counters (STATISTICS XML) — sum across thread entries.
    const runtimeRows = relOp.querySelectorAll(
      ":scope > RunTimeInformation > RunTimeCountersPerThread",
    );
    if (runtimeRows.length > 0) {
      let actualRows = 0;
      let actualMs = 0;
      let actualExecs = 0;
      for (const rt of runtimeRows) {
        actualRows += Number(rt.getAttribute("ActualRows") ?? 0);
        actualMs = Math.max(actualMs, Number(rt.getAttribute("ActualElapsedms") ?? 0));
        actualExecs += Number(rt.getAttribute("ActualExecutions") ?? 0);
      }
      node.actualRows = actualRows;
      if (actualMs > 0) node.actualTotalTime = actualMs;
      if (actualExecs > 0) node.actualLoops = actualExecs;
    }

    // Child RelOps appear under operator-specific wrapper elements (NestedLoops, Hash, etc.).
    const childRelOps = relOp.querySelectorAll(":scope > * > RelOp");
    for (const child of childRelOps) {
      node.children.push(this.convertMssqlRelOp(child, nextId));
    }

    return node;
  }

  private stripBrackets(id: string): string {
    return id.replace(/^\[|\]$/g, "");
  }

  parseSchemaResult(rows: unknown[]): SchemaTable[] {
    return (rows as MssqlSchemaRow[]).map((row) => ({
      name: row.table_name,
      schema: row.schema_name,
      type: row.object_type === "VIEW" ? "view" : "table",
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

  // === CREATE TABLE METHODS ===

  getColumnTypes(): ColumnTypeInfo[] {
    return [
      // String
      { name: "VARCHAR", category: "String", hasLength: true },
      { name: "NVARCHAR", category: "String", hasLength: true },
      { name: "CHAR", category: "String", hasLength: true },
      { name: "NCHAR", category: "String", hasLength: true },
      { name: "TEXT", category: "String" },
      { name: "NTEXT", category: "String" },
      // Numeric
      { name: "INT", category: "Numeric" },
      { name: "BIGINT", category: "Numeric" },
      { name: "SMALLINT", category: "Numeric" },
      { name: "TINYINT", category: "Numeric" },
      { name: "DECIMAL", category: "Numeric", hasPrecision: true },
      { name: "NUMERIC", category: "Numeric", hasPrecision: true },
      { name: "MONEY", category: "Numeric" },
      { name: "SMALLMONEY", category: "Numeric" },
      { name: "FLOAT", category: "Numeric" },
      { name: "REAL", category: "Numeric" },
      { name: "BIT", category: "Numeric" },
      // Date/Time
      { name: "DATE", category: "Date/Time" },
      { name: "DATETIME", category: "Date/Time" },
      { name: "DATETIME2", category: "Date/Time" },
      { name: "SMALLDATETIME", category: "Date/Time" },
      { name: "TIME", category: "Date/Time" },
      { name: "DATETIMEOFFSET", category: "Date/Time" },
      // Binary
      { name: "BINARY", category: "Binary", hasLength: true },
      { name: "VARBINARY", category: "Binary", hasLength: true },
      { name: "IMAGE", category: "Binary" },
      // UUID
      { name: "UNIQUEIDENTIFIER", category: "UUID" },
      // Other
      { name: "XML", category: "Other" },
    ];
  }

  private static readonly quote = (n: string) => `[${n}]`;

  generateCreateTableSql(definition: CreateTableDefinition): string {
    return generateCreateTableDdl(definition, MssqlAdapter.quote);
  }

  generateAddColumnSql(schema: string, table: string, column: CreateTableColumn): string {
    return generateAddColumnDdl(schema, table, column, MssqlAdapter.quote, false);
  }

  generateAlterTableSql(originalDef: CreateTableDefinition, newDef: CreateTableDefinition): string {
    return generateAlterTableSql(originalDef, newDef, {
      quote: MssqlAdapter.quote,
      supportsDropColumn: true,
      supportsAlterColumn: true,
      useModifyColumn: false,
    });
  }

  getSchemasQuery(): string {
    return `SELECT name as schema_name FROM sys.schemas WHERE name NOT IN ('sys', 'guest', 'INFORMATION_SCHEMA') ORDER BY name;`;
  }

  // === CRUD SQL GENERATION ===

  quoteIdentifier(id: string): string {
    return `[${id.replace(/\]/g, "]]")}]`;
  }

  paginateQuery(baseQuery: string, limit: number, offset: number): string {
    if (!/\bORDER\s+BY\b/i.test(baseQuery)) {
      return `${baseQuery} ORDER BY (SELECT NULL) OFFSET ${offset} ROWS FETCH NEXT ${limit} ROWS ONLY`;
    }
    return `${baseQuery} OFFSET ${offset} ROWS FETCH NEXT ${limit} ROWS ONLY`;
  }

  // T-SQL's BIT column doesn't accept `TRUE`/`FALSE` keywords — only `1`/`0`.
  // Everything else defers to the shared literal formatter.
  private static readonly formatValue: ValueFormatter = (v) => {
    if (typeof v === "boolean") return v ? "1" : "0";
    if (v instanceof Uint8Array) return formatMssqlBinary(v);
    return formatLiteralValue(v);
  };

  buildUpdateSql(
    schema: string,
    table: string,
    column: string,
    newValue: unknown,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings {
    return buildInlineUpdate(
      schema,
      table,
      column,
      newValue,
      primaryKeys,
      row,
      (id) => this.quoteIdentifier(id),
      MssqlAdapter.formatValue,
    );
  }

  buildSetDefaultSql(
    schema: string,
    table: string,
    column: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings {
    return buildInlineSetDefault(
      schema,
      table,
      column,
      primaryKeys,
      row,
      (id) => this.quoteIdentifier(id),
      MssqlAdapter.formatValue,
    );
  }

  buildInsertSql(schema: string, table: string, values: Record<string, unknown>): SqlWithBindings {
    return buildInlineInsert(
      schema,
      table,
      values,
      (id) => this.quoteIdentifier(id),
      MssqlAdapter.formatValue,
    );
  }

  buildDeleteSql(
    schema: string,
    table: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings {
    return buildInlineDelete(
      schema,
      table,
      primaryKeys,
      row,
      (id) => this.quoteIdentifier(id),
      MssqlAdapter.formatValue,
    );
  }
}
