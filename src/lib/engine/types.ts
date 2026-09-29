/**
 * `EngineClient`: the one async interface every dialect-dependent call site
 * uses (introspection, EXPLAIN, statistics and DDL SQL). The grid's edits,
 * the data tab's page and pagination are Core's (phase 5c: the edits
 * service; `TsEditService` and `TsQueryRunner` in the demo).
 *
 * Two implementations:
 * - `RustEngineClient` sends an `EngineRequest` to the Rust core as a
 *   `db.engine` call (`core_call` on desktop, `POST /api/rpc` on web).
 * - `TsEngineClient` runs the demo's TypeScript DuckDB adapter against its
 *   DuckDB-WASM provider (the browser demo has no Rust core).
 *
 * `getEngineClient(connection)` in `./index` picks one. One method per
 * `EngineRequest` variant, taking and returning the generated wire types.
 */

import type { ColumnTypeInfo } from "$lib/types/generated/ColumnTypeInfo";
import type { CreateTableDefinition } from "$lib/types/generated/CreateTableDefinition";
import type { DatabaseStatistics } from "$lib/types/generated/DatabaseStatistics";
import type { ExplainResult } from "$lib/types/generated/ExplainResult";
import type { SchemaColumn } from "$lib/types/generated/SchemaColumn";
import type { SchemaIndex } from "$lib/types/generated/SchemaIndex";
import type { SchemaTable } from "$lib/types/generated/SchemaTable";

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

  /**
   * One identifier (a column, an index), quoted and escaped the way the
   * dialect's `quote_ident` does. Local; see `./qualified-table`.
   */
  quoteIdent(name: string): string;

  /**
   * `"schema"."table"` for a table as `schemaTables` lists it. Build every
   * table name from a listed schema with this, never by quoting the schema
   * as one identifier (DuckDB's `catalog.schema` is two). Local; see
   * `./qualified-table`.
   */
  qualifiedTable(schema: string, table: string): string;

  /** CREATE TABLE DDL. */
  createTable(definition: CreateTableDefinition): Promise<string>;

  /** ALTER TABLE DDL turning `from` into `to` (`"-- No changes detected"` when equal). */
  alterTable(from: CreateTableDefinition, to: CreateTableDefinition): Promise<string>;
}
