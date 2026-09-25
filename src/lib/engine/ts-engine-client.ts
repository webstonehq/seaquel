/**
 * `EngineClient` over the TypeScript `DatabaseAdapter` plus a provider: the
 * dialect runs here and only the generated SQL goes to the database. Every
 * method reproduces what its call site did before `EngineClient` existed
 * (get the query, select, parse; builders called directly).
 */

import { getAdapter, type CastLookup, type DatabaseAdapter } from "$lib/db";
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
import type { CastMap, EngineClient, RowRecord, TableMetadata } from "./types";

export interface TsEngineClientOptions {
  type: DatabaseType;
  /** Shown as the statistics overview's database name when the adapter has no overview query. */
  connectionName: string;
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

/** `CastMap` → the adapters' `CastLookup` (only own keys count). */
function toCastLookup(casts: CastMap | undefined): CastLookup | undefined {
  if (!casts) return undefined;
  return (col) => (Object.hasOwn(casts, col) ? casts[col] : undefined);
}

export class TsEngineClient implements EngineClient {
  private readonly type: DatabaseType;
  private readonly connectionName: string;
  private readonly getConnectionId: () => string | undefined;
  private resolvedAdapter: DatabaseAdapter | undefined;
  private readonly getProvider: () => Promise<DatabaseProvider>;

  constructor(options: TsEngineClientOptions) {
    this.type = options.type;
    this.connectionName = options.connectionName;
    this.getConnectionId = options.getConnectionId;
    this.resolvedAdapter = options.adapter;
    this.getProvider = options.getProvider ?? (() => defaultProvider(options.type));
  }

  /**
   * Resolved on first use, not in the constructor: `getAdapter` throws for
   * engines with no TypeScript adapter (Postgres since phase 1), and a client
   * for one must still be constructible (e.g. the demo's registry path).
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
    const query = this.adapter.getSchemasQuery?.();
    if (!query) return [];
    const { provider, id } = await this.connection();
    const rows = await provider.select<{ schema_name: string }>(id, query);
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

    let foreignKeysResult: unknown[] | undefined;
    if (adapter.getForeignKeysQuery) {
      foreignKeysResult = await provider.select(id, adapter.getForeignKeysQuery(table, schema));
    }

    return {
      columns: adapter.parseColumnsResult(columnsResult || [], foreignKeysResult),
      indexes: adapter.parseIndexesResult(indexesResult || []),
    };
  }

  async statistics(): Promise<DatabaseStatistics> {
    const adapter = this.adapter;
    const sizesQuery = adapter.getTableSizesQuery?.();
    const usageQuery = adapter.getIndexUsageQuery?.();
    const overviewQuery = adapter.getDatabaseOverviewQuery?.();

    let run: (query: string) => Promise<Record<string, unknown>[]> = () => Promise.resolve([]);
    if (sizesQuery || usageQuery || overviewQuery || adapter.getTableRowCountQuery) {
      const { provider, id } = await this.connection();
      run = (query) => provider.select<Record<string, unknown>>(id, query);
    }

    const [tableSizesRows, indexUsageRows, overviewRows] = await Promise.all([
      sizesQuery ? run(sizesQuery) : Promise.resolve([]),
      usageQuery ? run(usageQuery) : Promise.resolve([]),
      overviewQuery ? run(overviewQuery) : Promise.resolve([]),
    ]);

    let tableSizes = adapter.parseTableSizesResult?.(tableSizesRows) ?? [];

    // Engines without cheap row estimates (SQLite, DuckDB) count per table.
    const rowCountQuery = adapter.getTableRowCountQuery?.bind(adapter);
    if (rowCountQuery && tableSizes.length > 0) {
      tableSizes = await Promise.all(
        tableSizes.map(async (table) => {
          try {
            const result = await run(rowCountQuery(table.name, table.schema));
            const rowCount = Number((result[0] as { row_count?: number })?.row_count) || 0;
            return { ...table, rowCount };
          } catch {
            return table; // Keep the original entry if the count fails
          }
        }),
      );
    }

    return {
      overview: adapter.parseDatabaseOverviewResult?.(overviewRows) ?? {
        databaseName: this.connectionName,
        totalSize: "N/A",
        tableCount: 0,
        indexCount: 0,
      },
      tableSizes,
      indexUsage: adapter.parseIndexUsageResult?.(indexUsageRows) ?? [],
    };
  }

  async explain(
    sql: string,
    params: unknown[] | undefined,
    analyze: boolean,
  ): Promise<ExplainResult> {
    const adapter = this.adapter;
    const dbType = this.type;
    const explainQuery = adapter.getExplainQuery(sql, analyze);
    const { provider, id } = await this.connection();

    let actualRowCount: number | undefined;
    let executionTime: number | undefined;

    if (dbType === "sqlite" && analyze) {
      const startTime = performance.now();
      const queryResult = await provider.select(id, sql, params);
      executionTime = performance.now() - startTime;
      actualRowCount = queryResult.length;
    }

    // MSSQL and DuckDB EXPLAIN inline their literals and take no bind values.
    const useBindValues = dbType !== "mssql" && dbType !== "duckdb";
    const queryResult = await provider.select(id, explainQuery, useBindValues ? params : undefined);
    const explainResult = adapter.parseExplainResult(queryResult, analyze);

    // SQLite has no native ANALYZE: populate the root with measured execution stats.
    // Only the root gets actuals — there is no per-operator breakdown — and we
    // deliberately leave planRows undefined so the UI doesn't invent an "estimate"
    // that equals the actual count.
    if (dbType === "sqlite" && analyze) {
      if (actualRowCount !== undefined) {
        explainResult.plan.actualRows = actualRowCount;
      }
      if (executionTime !== undefined) {
        explainResult.plan.actualTotalTime = executionTime;
        explainResult.executionTime = executionTime;
      }
    }
    return explainResult;
  }

  async columnTypes(): Promise<ColumnTypeInfo[]> {
    return this.adapter.getColumnTypes?.() ?? [];
  }

  async paginate(sql: string, limit: number, offset: number): Promise<string> {
    return this.adapter.paginateQuery(sql, limit, offset);
  }

  async buildUpdate(
    schema: string,
    table: string,
    column: string,
    value: unknown,
    primaryKeys: string[],
    row: RowRecord,
    casts?: CastMap,
  ): Promise<SqlWithBindings> {
    return this.adapter.buildUpdateSql(
      schema,
      table,
      column,
      value,
      primaryKeys,
      row,
      toCastLookup(casts),
    );
  }

  /** Takes no `casts`: the TS helpers never cast primary keys. */
  async buildSetDefault(
    schema: string,
    table: string,
    column: string,
    primaryKeys: string[],
    row: RowRecord,
  ): Promise<SqlWithBindings> {
    return this.adapter.buildSetDefaultSql(schema, table, column, primaryKeys, row);
  }

  async buildInsert(
    schema: string,
    table: string,
    values: RowRecord,
    casts?: CastMap,
  ): Promise<SqlWithBindings> {
    return this.adapter.buildInsertSql(schema, table, values, toCastLookup(casts));
  }

  /** Takes no `casts`: the TS helpers never cast primary keys. */
  async buildDelete(
    schema: string,
    table: string,
    primaryKeys: string[],
    row: RowRecord,
  ): Promise<SqlWithBindings> {
    return this.adapter.buildDeleteSql(schema, table, primaryKeys, row);
  }

  async createTable(definition: CreateTableDefinition): Promise<string> {
    if (!this.adapter.generateCreateTableSql) throw this.unsupported("CREATE TABLE generation");
    return this.adapter.generateCreateTableSql(definition);
  }

  async alterTable(from: CreateTableDefinition, to: CreateTableDefinition): Promise<string> {
    if (!this.adapter.generateAlterTableSql) throw this.unsupported("ALTER TABLE generation");
    return this.adapter.generateAlterTableSql(from, to);
  }

  private unsupported(what: string): Error {
    return new Error(`${what} is not supported for ${this.type}`);
  }
}
