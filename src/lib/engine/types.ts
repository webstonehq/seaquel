/**
 * `EngineClient`: the one async interface every dialect-dependent call site
 * uses (introspection, EXPLAIN, statistics, pagination, CRUD and DDL SQL).
 *
 * Two implementations:
 * - `RustEngineClient` sends an `EngineCall` to the Rust core
 *   (`invoke("db_engine")` on desktop, `POST /api/db/engine` on web).
 * - `TsEngineClient` runs the demo's TypeScript DuckDB adapter against its
 *   DuckDB-WASM provider (the browser demo has no Rust core).
 *
 * `getEngineClient(connection)` in `./index` picks one. One method per
 * `EngineRequest` variant, taking and returning the generated wire types.
 * Rows are plain objects here; the Rust client turns them into the ordered
 * `[column, value]` pairs of the wire format.
 */

import type { ColumnTypeInfo } from "$lib/types/generated/ColumnTypeInfo";
import type { CreateTableDefinition } from "$lib/types/generated/CreateTableDefinition";
import type { DatabaseStatistics } from "$lib/types/generated/DatabaseStatistics";
import type { ExplainResult } from "$lib/types/generated/ExplainResult";
import type { SchemaColumn } from "$lib/types/generated/SchemaColumn";
import type { SchemaIndex } from "$lib/types/generated/SchemaIndex";
import type { SchemaTable } from "$lib/types/generated/SchemaTable";
import type { SqlWithBindings } from "$lib/types/generated/SqlWithBindings";

/** Column name → SQL type for `CAST($n AS type)`; columns that need no cast are absent. */
export type CastMap = Record<string, string>;

/** A row keyed by column name. Key order is the column order. */
export type RowRecord = Record<string, unknown>;

export interface TableMetadata {
  columns: SchemaColumn[];
  indexes: SchemaIndex[];
}

export interface EngineClient {
  /** Schema names (for the create-table schema picker). */
  listSchemas(): Promise<string[]>;

  /** Tables and views, without column or index metadata. */
  schemaTables(): Promise<SchemaTable[]>;

  /** Columns (with foreign keys) and indexes of one table. */
  tableMetadata(schema: string, table: string): Promise<TableMetadata>;

  statistics(): Promise<DatabaseStatistics>;

  /** Run EXPLAIN (or EXPLAIN ANALYZE) on `sql` with its bind values. */
  explain(sql: string, params: unknown[] | undefined, analyze: boolean): Promise<ExplainResult>;

  /** Column types offered by the create-table editor. */
  columnTypes(): Promise<ColumnTypeInfo[]>;

  /** `sql` wrapped with this dialect's LIMIT/OFFSET. */
  paginate(sql: string, limit: number, offset: number): Promise<string>;

  /**
   * One identifier (a column, an index), quoted and escaped the way the
   * dialect's `quote_ident` does. Local, like `paginate`; see `./qualified-table`.
   */
  quoteIdent(name: string): string;

  /**
   * `"schema"."table"` for a table as `schemaTables` lists it. Build every
   * table name from a listed schema with this, never by quoting the schema
   * as one identifier (DuckDB's `catalog.schema` is two). Local, like
   * `paginate`; see `./qualified-table`.
   */
  qualifiedTable(schema: string, table: string): string;

  buildUpdate(
    schema: string,
    table: string,
    column: string,
    value: unknown,
    primaryKeys: string[],
    row: RowRecord,
    casts?: CastMap,
  ): Promise<SqlWithBindings>;

  /**
   * `casts` wraps the primary-key placeholders (Rust dialects only).
   * `columnDefault` is the column's default expression from its metadata
   * (`defaultValue`, or `"NULL"` when it has none): SQLite has no `DEFAULT`
   * in `UPDATE` and assigns it instead. Other engines ignore it.
   */
  buildSetDefault(
    schema: string,
    table: string,
    column: string,
    primaryKeys: string[],
    row: RowRecord,
    casts?: CastMap,
    columnDefault?: string,
  ): Promise<SqlWithBindings>;

  buildInsert(
    schema: string,
    table: string,
    values: RowRecord,
    casts?: CastMap,
  ): Promise<SqlWithBindings>;

  /** `casts` wraps the primary-key placeholders (Rust dialects only). */
  buildDelete(
    schema: string,
    table: string,
    primaryKeys: string[],
    row: RowRecord,
    casts?: CastMap,
  ): Promise<SqlWithBindings>;

  /** CREATE TABLE DDL. */
  createTable(definition: CreateTableDefinition): Promise<string>;

  /** ALTER TABLE DDL turning `from` into `to` (`"-- No changes detected"` when equal). */
  alterTable(from: CreateTableDefinition, to: CreateTableDefinition): Promise<string>;
}
