/**
 * `EngineClient` over the TypeScript `DatabaseAdapter` plus a provider, for
 * the browser demo only: its DuckDB-WASM has no Rust core, so the dialect
 * (`src/lib/db/duckdb.ts`) runs here and only the generated SQL goes to the
 * database. Desktop and web use `RustEngineClient` for every engine.
 */

import { getAdapter, type DatabaseAdapter } from "$lib/db";
import { ProviderRegistry } from "$lib/providers/provider-registry";
import type { DatabaseProvider } from "$lib/providers/types";
import type {
  ColumnTypeInfo,
  CreateTableDefinition,
  DatabaseStatistics,
  DatabaseType,
  ExplainResult,
  SchemaTable,
} from "$lib/types";
import type { SqlWithBindings } from "$lib/types/generated/SqlWithBindings";
import { plainQualifiedTable, quoteIdent } from "./qualified-table";
import type { CastMap, EngineClient, RowRecord, TableMetadata } from "./types";

export interface TsEngineClientOptions {
  type: DatabaseType;
  /**
   * Returns the current id from `provider.connect`, read on every query so a
   * client created before a reconnect uses the new id. Queries throw without
   * one (builders don't need it).
   */
  getConnectionId: () => string | undefined;
  /** Defaults to `getAdapter(type)`, looked up on first use. */
  adapter?: DatabaseAdapter;
  /** Defaults to the shared provider for `type` (the same instance the app's `ProviderRegistry` uses). */
  getProvider?: () => Promise<DatabaseProvider>;
}

// A fresh registry still hands out the module-level provider singletons.
let sharedRegistry: ProviderRegistry | null = null;
function defaultProvider(type: DatabaseType): Promise<DatabaseProvider> {
  sharedRegistry ??= new ProviderRegistry();
  return sharedRegistry.getForType(type);
}

export class TsEngineClient implements EngineClient {
  private readonly type: DatabaseType;
  private readonly getConnectionId: () => string | undefined;
  private resolvedAdapter: DatabaseAdapter | undefined;
  private readonly getProvider: () => Promise<DatabaseProvider>;

  constructor(options: TsEngineClientOptions) {
    this.type = options.type;
    this.getConnectionId = options.getConnectionId;
    this.resolvedAdapter = options.adapter;
    this.getProvider = options.getProvider ?? (() => defaultProvider(options.type));
  }

  /**
   * Resolved on first use, not in the constructor: `getAdapter` throws for
   * every engine but DuckDB, and a client for one must still be
   * constructible (the demo's registry path).
   */
  private get adapter(): DatabaseAdapter {
    this.resolvedAdapter ??= getAdapter(this.type);
    return this.resolvedAdapter;
  }

  /** The provider and connection id, or the error the old call sites threw. */
  private async connection(): Promise<{ provider: DatabaseProvider; id: string }> {
    const id = this.getConnectionId();
    if (!id) throw new Error("No connection established");
    return { provider: await this.getProvider(), id };
  }

  async listSchemas(): Promise<string[]> {
    const { provider, id } = await this.connection();
    const rows = await provider.select<{ schema_name: string }>(id, this.adapter.getSchemasQuery());
    return rows.map((r) => r.schema_name);
  }

  async schemaTables(): Promise<SchemaTable[]> {
    const { provider, id } = await this.connection();
    const rows = await provider.select(id, this.adapter.getSchemaQuery());
    return this.adapter.parseSchemaResult(rows as unknown[]);
  }

  async tableMetadata(schema: string, table: string): Promise<TableMetadata> {
    const { provider, id } = await this.connection();
    const adapter = this.adapter;
    const columnsResult = await provider.select(id, adapter.getColumnsQuery(table, schema));
    const indexesResult = await provider.select(id, adapter.getIndexesQuery(table, schema));
    const foreignKeysResult = await provider.select(id, adapter.getForeignKeysQuery(table, schema));

    return {
      columns: adapter.parseColumnsResult(columnsResult || [], foreignKeysResult),
      indexes: adapter.parseIndexesResult(indexesResult || []),
    };
  }

  async statistics(): Promise<DatabaseStatistics> {
    const adapter = this.adapter;
    const { provider, id } = await this.connection();
    const run = (query: string) => provider.select<Record<string, unknown>>(id, query);

    const [tableSizesRows, indexUsageRows, overviewRows] = await Promise.all([
      run(adapter.getTableSizesQuery()),
      run(adapter.getIndexUsageQuery()),
      run(adapter.getDatabaseOverviewQuery()),
    ]);

    // DuckDB has no cheap row estimates, so each table is counted.
    const tableSizes = await Promise.all(
      adapter.parseTableSizesResult(tableSizesRows).map(async (table) => {
        try {
          const result = await run(adapter.getTableRowCountQuery(table.name, table.schema));
          const rowCount = Number((result[0] as { row_count?: number })?.row_count) || 0;
          return { ...table, rowCount };
        } catch {
          return table; // Keep the original entry if the count fails
        }
      }),
    );

    return {
      overview: adapter.parseDatabaseOverviewResult(overviewRows),
      tableSizes,
      indexUsage: adapter.parseIndexUsageResult(indexUsageRows),
    };
  }

  /** DuckDB's EXPLAIN inlines its literals, so the bind values aren't sent. */
  async explain(
    sql: string,
    _params: unknown[] | undefined,
    analyze: boolean,
  ): Promise<ExplainResult> {
    const adapter = this.adapter;
    const explainQuery = adapter.getExplainQuery(sql, analyze);
    const { provider, id } = await this.connection();
    const queryResult = await provider.select(id, explainQuery);
    return adapter.parseExplainResult(queryResult, analyze);
  }

  async columnTypes(): Promise<ColumnTypeInfo[]> {
    return this.adapter.getColumnTypes();
  }

  async paginate(sql: string, limit: number, offset: number): Promise<string> {
    return this.adapter.paginateQuery(sql, limit, offset);
  }

  quoteIdent(name: string): string {
    return quoteIdent(this.type, name);
  }

  /** The schema as one name (the demo's DuckDB lists its schemas bare). */
  qualifiedTable(schema: string, table: string): string {
    return plainQualifiedTable(this.type, schema, table);
  }

  /** Ignores `casts`: values are inlined as literals. */
  async buildUpdate(
    schema: string,
    table: string,
    column: string,
    value: unknown,
    primaryKeys: string[],
    row: RowRecord,
    _casts?: CastMap,
  ): Promise<SqlWithBindings> {
    return this.adapter.buildUpdateSql(schema, table, column, value, primaryKeys, row);
  }

  async buildSetDefault(
    schema: string,
    table: string,
    column: string,
    primaryKeys: string[],
    row: RowRecord,
  ): Promise<SqlWithBindings> {
    return this.adapter.buildSetDefaultSql(schema, table, column, primaryKeys, row);
  }

  /** Ignores `casts`: values are inlined as literals. */
  async buildInsert(
    schema: string,
    table: string,
    values: RowRecord,
    _casts?: CastMap,
  ): Promise<SqlWithBindings> {
    return this.adapter.buildInsertSql(schema, table, values);
  }

  async buildDelete(
    schema: string,
    table: string,
    primaryKeys: string[],
    row: RowRecord,
  ): Promise<SqlWithBindings> {
    return this.adapter.buildDeleteSql(schema, table, primaryKeys, row);
  }

  async createTable(definition: CreateTableDefinition): Promise<string> {
    return this.adapter.generateCreateTableSql(definition);
  }

  async alterTable(from: CreateTableDefinition, to: CreateTableDefinition): Promise<string> {
    return this.adapter.generateAlterTableSql(from, to);
  }
}
