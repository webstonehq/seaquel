/**
 * `EngineClient` backed by the Rust core. Each method sends one
 * `EngineRequest` as a `db.engine` call on the connection (through the
 * page's `CoreClient`: `core_call` on desktop, `POST /api/rpc` on web) and
 * unwraps the `{kind, data}` response.
 *
 * Values follow the cell wire format (`$lib/values`): EXPLAIN's `params` go
 * out through `encodeParams`.
 */

import { callDb, getCoreClient, type CoreClient } from "$lib/core";
import type { ColumnTypeInfo, CreateTableDefinition, DatabaseStatistics } from "$lib/types";
import type { EngineRequest } from "$lib/types/generated/EngineRequest";
import type { EngineResponse } from "$lib/types/generated/EngineResponse";
import type { ExplainResult } from "$lib/types/generated/ExplainResult";
import type { SchemaTable } from "$lib/types/generated/SchemaTable";
import { encodeParams } from "$lib/values";
import { duckdbQualifiedTable, plainQualifiedTable, quoteIdent } from "./qualified-table";
import type { EngineClient, TableMetadata } from "./types";

/** Sends one call and returns the raw response; rejects with a `"CODE: message"` Error. */
export type EngineTransport = (
  connectionId: string,
  request: EngineRequest,
) => Promise<EngineResponse>;

/** `db.engine` through `client`: the connection must be this workspace's. */
export function coreEngineTransport(client: () => CoreClient = getCoreClient): EngineTransport {
  return (connectionId, request) => callDb(client(), "engine", { connectionId, request });
}

type Kind = EngineResponse["kind"];
type DataOf<K extends Kind> = Extract<EngineResponse, { kind: K }>["data"];

/**
 * Connection types whose dialect runs in Rust. Each needs a local
 * `qualifiedTable` in `QUALIFIED_TABLE`. MariaDB connects through the MySQL
 * engine (driver `"mysql"`) and shares its dialect.
 */
export type RustEngine = "postgres" | "mysql" | "mariadb" | "sqlite" | "mssql" | "duckdb";

/**
 * `qualifiedTable`, local. DuckDB splits its listed `catalog.schema`
 * (see `./qualified-table`); the other engines quote the schema as one name.
 */
const QUALIFIED_TABLE: Record<RustEngine, (schema: string, table: string) => string> = {
  postgres: (s, t) => plainQualifiedTable("postgres", s, t),
  mysql: (s, t) => plainQualifiedTable("mysql", s, t),
  mariadb: (s, t) => plainQualifiedTable("mariadb", s, t),
  sqlite: (s, t) => plainQualifiedTable("sqlite", s, t),
  mssql: (s, t) => plainQualifiedTable("mssql", s, t),
  duckdb: duckdbQualifiedTable,
};

export class RustEngineClient implements EngineClient {
  constructor(
    /** Picks the local dialect code (`quoteIdent`, `qualifiedTable`); the core knows it from the connection. */
    private readonly engine: RustEngine,
    /**
     * Returns the current id from `provider.connect` (the Rust core's
     * connection id). Read on every call, so a client created before a
     * reconnect uses the new id.
     */
    private readonly getConnectionId: () => string | undefined,
    private readonly transport: EngineTransport = coreEngineTransport(),
  ) {}

  private async call<K extends Kind>(expected: K, request: EngineRequest): Promise<DataOf<K>> {
    const id = this.getConnectionId();
    if (!id) throw new Error("No connection established");
    const response = await this.transport(id, request);
    if (response?.kind !== expected) {
      throw new Error(
        `ENGINE_PROTOCOL: expected "${expected}" response, got "${String(response?.kind)}"`,
      );
    }
    return response.data as DataOf<K>;
  }

  listSchemas(): Promise<string[]> {
    return this.call("schemas", { method: "listSchemas" });
  }

  schemaTables(): Promise<SchemaTable[]> {
    return this.call("tables", { method: "schemaTables" });
  }

  tableMetadata(schema: string, table: string): Promise<TableMetadata> {
    return this.call("tableMetadata", { method: "tableMetadata", params: { schema, table } });
  }

  statistics(): Promise<DatabaseStatistics> {
    return this.call("statistics", { method: "statistics" });
  }

  explain(sql: string, params: unknown[] | undefined, analyze: boolean): Promise<ExplainResult> {
    return this.call("explain", {
      method: "explain",
      params: { sql, params: encodeParams(params), analyze },
    });
  }

  columnTypes(): Promise<ColumnTypeInfo[]> {
    return this.call("columnTypes", { method: "columnTypes" });
  }

  quoteIdent(name: string): string {
    return quoteIdent(this.engine, name);
  }

  qualifiedTable(schema: string, table: string): string {
    return QUALIFIED_TABLE[this.engine](schema, table);
  }

  createTable(definition: CreateTableDefinition): Promise<string> {
    return this.call("sql", { method: "createTable", params: { definition } });
  }

  alterTable(from: CreateTableDefinition, to: CreateTableDefinition): Promise<string> {
    return this.call("sql", { method: "alterTable", params: { from, to } });
  }
}
