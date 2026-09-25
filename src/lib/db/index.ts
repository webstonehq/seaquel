/**
 * The TypeScript dialect adapter, for the browser demo only: its DuckDB-WASM
 * has no Rust core, so `TsEngineClient` runs `duckdb.ts` against it. Every
 * other engine, and DuckDB on desktop and web, runs in Rust
 * (`crates/seaquel-engine-*`).
 */

import type {
  ColumnTypeInfo,
  CreateTableColumn,
  CreateTableDefinition,
  DatabaseType,
  SchemaTable,
  SchemaColumn,
  SchemaIndex,
  TableSizeInfo,
  IndexUsageInfo,
  DatabaseOverview,
  ExplainResult,
} from "$lib/types";
import { DuckDBAdapter } from "./duckdb";
import type { SqlWithBindings } from "./crud-helpers";

export type { SqlWithBindings } from "./crud-helpers";

export interface DatabaseAdapter {
  /** SQL query to list all tables in the database */
  getSchemaQuery(): string;

  /** SQL query to get column metadata for a table */
  getColumnsQuery(table: string, schema: string): string;

  /** SQL query to get index information for a table */
  getIndexesQuery(table: string, schema: string): string;

  /** SQL query to get foreign key information for a table */
  getForeignKeysQuery(table: string, schema: string): string;

  /** Build the EXPLAIN query (it inlines its literals and takes no bind values) */
  getExplainQuery(query: string, analyze: boolean): string;

  /** Parse EXPLAIN results into the renderer-ready ExplainResult */
  parseExplainResult(rows: unknown[], analyze: boolean): ExplainResult;

  /** Transform raw schema query results to SchemaTable[] */
  parseSchemaResult(rows: unknown[]): SchemaTable[];

  /** Transform raw columns query results to SchemaColumn[] */
  parseColumnsResult(rows: unknown[], foreignKeys?: unknown[]): SchemaColumn[];

  /** Transform raw indexes query results to SchemaIndex[] */
  parseIndexesResult(rows: unknown[]): SchemaIndex[];

  // === STATISTICS ===

  /** SQL query listing the tables whose sizes the statistics tab shows */
  getTableSizesQuery(): string;

  /** SQL query to get index usage statistics */
  getIndexUsageQuery(): string;

  /** SQL query to get database overview statistics */
  getDatabaseOverviewQuery(): string;

  /** Parse table sizes query results (row counts are filled in per table) */
  parseTableSizesResult(rows: unknown[]): TableSizeInfo[];

  /** Parse index usage query results */
  parseIndexUsageResult(rows: unknown[]): IndexUsageInfo[];

  /** Parse database overview query results */
  parseDatabaseOverviewResult(rows: unknown[]): DatabaseOverview;

  /** SQL query to get the row count of one table */
  getTableRowCountQuery(table: string, schema: string): string;

  // === DDL ===

  /** Available column types */
  getColumnTypes(): ColumnTypeInfo[];

  /** Generate CREATE TABLE DDL from a table definition */
  generateCreateTableSql(definition: CreateTableDefinition): string;

  /** Generate ALTER TABLE ADD COLUMN DDL (only the fixture recorder calls it) */
  generateAddColumnSql(schema: string, table: string, column: CreateTableColumn): string;

  /** SQL query to list available schemas */
  getSchemasQuery(): string;

  /** Generate ALTER TABLE statements to transform originalDef into newDef */
  generateAlterTableSql(originalDef: CreateTableDefinition, newDef: CreateTableDefinition): string;

  // === CRUD SQL GENERATION (values are inlined, so there are no casts) ===

  /** Quote a SQL identifier (only the fixture recorder calls it) */
  quoteIdentifier(id: string): string;

  /** Append LIMIT/OFFSET to a base SELECT query */
  paginateQuery(baseQuery: string, limit: number, offset: number): string;

  /** Build an UPDATE SET column = value WHERE pk = pk_value statement. */
  buildUpdateSql(
    schema: string,
    table: string,
    column: string,
    newValue: unknown,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings;

  /** Build an UPDATE SET column = DEFAULT WHERE pk = pk_value statement. */
  buildSetDefaultSql(
    schema: string,
    table: string,
    column: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings;

  /** Build an INSERT INTO statement. */
  buildInsertSql(schema: string, table: string, values: Record<string, unknown>): SqlWithBindings;

  /** Build a DELETE FROM WHERE pk = pk_value statement. */
  buildDeleteSql(
    schema: string,
    table: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): SqlWithBindings;
}

/**
 * Validates and sanitizes a SQL identifier (table name, schema name, column name).
 * Throws an error if the identifier contains invalid characters.
 * Allows Unicode letters/digits for international table names.
 */
export function validateIdentifier(name: string): string {
  // Allow alphanumeric, underscore, and common international characters
  // Also allow dollar sign which PostgreSQL supports
  if (!/^[\p{L}\p{N}_$][\p{L}\p{N}_$]*$/u.test(name)) {
    throw new Error(`Invalid SQL identifier: "${name}"`);
  }
  return name;
}

const duckdbAdapter = new DuckDBAdapter();

/** The demo's DuckDB adapter. Every other type throws: it has no TypeScript adapter. */
export function getAdapter(type: DatabaseType): DatabaseAdapter {
  if (type !== "duckdb") {
    throw new Error(`Database type "${type}" is not supported yet`);
  }
  return duckdbAdapter;
}
