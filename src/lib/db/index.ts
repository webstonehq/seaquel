import type {
  DatabaseType,
  SchemaTable,
  SchemaColumn,
  SchemaIndex,
  TableSizeInfo,
  IndexUsageInfo,
  DatabaseOverview,
  ExplainResult,
} from "$lib/types";
import { MssqlAdapter } from "./mssql";
import { MysqlAdapter } from "./mysql";
import { SqliteAdapter } from "./sqlite";
import { DuckDBAdapter } from "./duckdb";

export type { SqlWithBindings, CastLookup } from "./crud-helpers";

export interface DatabaseAdapter {
  /** SQL query to list all tables in the database */
  getSchemaQuery(): string;

  /** SQL query to get column metadata for a table */
  getColumnsQuery(table: string, schema: string): string;

  /** SQL query to get index information for a table */
  getIndexesQuery(table: string, schema: string): string;

  /** SQL query to get foreign key information for a table (optional, some DBs include in columns query) */
  getForeignKeysQuery?(table: string, schema: string): string;

  /** Build the EXPLAIN query for this database type */
  getExplainQuery(query: string, analyze: boolean): string;

  /** Parse EXPLAIN results into the renderer-ready ExplainResult */
  parseExplainResult(rows: unknown[], analyze: boolean): ExplainResult;

  /** Transform raw schema query results to SchemaTable[] */
  parseSchemaResult(rows: unknown[]): SchemaTable[];

  /** Transform raw columns query results to SchemaColumn[] */
  parseColumnsResult(rows: unknown[], foreignKeys?: unknown[]): SchemaColumn[];

  /** Transform raw indexes query results to SchemaIndex[] */
  parseIndexesResult(rows: unknown[]): SchemaIndex[];

  // === STATISTICS METHODS (optional) ===

  /** SQL query to get table sizes */
  getTableSizesQuery?(): string;

  /** SQL query to get index usage statistics */
  getIndexUsageQuery?(): string;

  /** SQL query to get database overview statistics */
  getDatabaseOverviewQuery?(): string;

  /** Parse table sizes query results */
  parseTableSizesResult?(rows: unknown[]): TableSizeInfo[];

  /** Parse index usage query results */
  parseIndexUsageResult?(rows: unknown[]): IndexUsageInfo[];

  /** Parse database overview query results */
  parseDatabaseOverviewResult?(rows: unknown[]): DatabaseOverview;

  /** SQL query to get row count for a specific table (for DBs that need per-table queries) */
  getTableRowCountQuery?(table: string, schema: string): string;

  /** Get available column types for this database engine */
  getColumnTypes?(): import("$lib/types").ColumnTypeInfo[];

  /** Generate CREATE TABLE DDL from a table definition */
  generateCreateTableSql?(definition: import("$lib/types").CreateTableDefinition): string;

  /** Generate ALTER TABLE ADD COLUMN DDL */
  generateAddColumnSql?(
    schema: string,
    table: string,
    column: import("$lib/types").CreateTableColumn,
  ): string;

  /** SQL query to list available schemas */
  getSchemasQuery?(): string;

  /** Generate ALTER TABLE statements to transform originalDef into newDef */
  generateAlterTableSql?(
    originalDef: import("$lib/types").CreateTableDefinition,
    newDef: import("$lib/types").CreateTableDefinition,
  ): string;

  // === CRUD SQL GENERATION ===

  /** Quote a SQL identifier (table name, column name, schema name) for this engine. */
  quoteIdentifier(id: string): string;

  /**
   * Generate a paginated SELECT query.
   * @param baseQuery - The base SELECT query without LIMIT/OFFSET
   * @param limit - Number of rows to fetch
   * @param offset - Number of rows to skip
   */
  paginateQuery(baseQuery: string, limit: number, offset: number): string;

  /** Build an UPDATE SET column = value WHERE pk = pk_value statement. */
  buildUpdateSql(
    schema: string,
    table: string,
    column: string,
    newValue: unknown,
    primaryKeys: string[],
    row: Record<string, unknown>,
    castLookup?: import("./crud-helpers").CastLookup,
  ): import("./crud-helpers").SqlWithBindings;

  /** Build an UPDATE SET column = DEFAULT WHERE pk = pk_value statement. */
  buildSetDefaultSql(
    schema: string,
    table: string,
    column: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): import("./crud-helpers").SqlWithBindings;

  /** Build an INSERT INTO statement. */
  buildInsertSql(
    schema: string,
    table: string,
    values: Record<string, unknown>,
    castLookup?: import("./crud-helpers").CastLookup,
  ): import("./crud-helpers").SqlWithBindings;

  /** Build a DELETE FROM WHERE pk = pk_value statement. */
  buildDeleteSql(
    schema: string,
    table: string,
    primaryKeys: string[],
    row: Record<string, unknown>,
  ): import("./crud-helpers").SqlWithBindings;
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

const mysqlAdapter = new MysqlAdapter();
const adapters: Partial<Record<DatabaseType, DatabaseAdapter>> = {
  mssql: new MssqlAdapter(),
  mysql: mysqlAdapter,
  mariadb: mysqlAdapter,
  sqlite: new SqliteAdapter(),
  duckdb: new DuckDBAdapter(),
};

export function getAdapter(type: DatabaseType): DatabaseAdapter {
  const adapter = adapters[type];
  if (!adapter) {
    throw new Error(`Database type "${type}" is not supported yet`);
  }
  return adapter;
}
