/**
 * `EngineClient` backed by the Rust core. Each method sends one `EngineCall`
 * (`invoke("db_engine", { call })` on desktop, `POST /api/db/engine` on web)
 * and unwraps the `{kind, data}` response.
 *
 * Values follow the cell wire format (`$lib/values`): every value-bearing
 * field (`value`, `params`, row and insert values) goes out through
 * `encodeParam`, and `bindValues` come back through `decodeCell`, so bigint
 * primary keys, bytes and decimals round-trip exactly.
 */

import { invoke } from "@tauri-apps/api/core";
import { envApiBaseUrl, postJson } from "$lib/providers/http-provider";
import { formatError } from "$lib/providers/wire";
import type { ColumnTypeInfo, CreateTableDefinition, DatabaseStatistics } from "$lib/types";
import type { EngineCall } from "$lib/types/generated/EngineCall";
import type { EngineRequest } from "$lib/types/generated/EngineRequest";
import type { EngineResponse } from "$lib/types/generated/EngineResponse";
import type { ExplainResult } from "$lib/types/generated/ExplainResult";
import type { SchemaTable } from "$lib/types/generated/SchemaTable";
import type { SqlWithBindings } from "$lib/types/generated/SqlWithBindings";
import { isTauri } from "$lib/utils/environment";
import { decodeCell, encodeParam, encodeParams } from "$lib/values";
import type { CastMap, EngineClient, RowRecord, TableMetadata } from "./types";

/** Sends one call and returns the raw response; rejects with a `"CODE: message"` Error. */
export type EngineTransport = (call: EngineCall) => Promise<EngineResponse>;

/** Desktop: the `db_engine` Tauri command. */
export const tauriTransport: EngineTransport = async (call) => {
  try {
    return await invoke<EngineResponse>("db_engine", { call });
  } catch (error) {
    // A DbError object, or a plain string when Tauri can't deserialize the args.
    throw formatError(error);
  }
};

/**
 * Web: `POST {baseUrl}/api/db/engine`. `baseUrl` defaults to
 * `VITE_SEAQUEL_API_URL`, else same-origin, like `HttpProvider`.
 */
export function httpTransport(baseUrl: string = envApiBaseUrl() ?? ""): EngineTransport {
  return (call) => postJson<EngineResponse>(`${baseUrl}/api/db/engine`, call);
}

/** Picked per call so tests (and late environment detection) see the current mode. */
const defaultTransport: EngineTransport = (call) =>
  isTauri() ? tauriTransport(call) : httpTransport()(call);

type Kind = EngineResponse["kind"];
type DataOf<K extends Kind> = Extract<EngineResponse, { kind: K }>["data"];

/** A value for the wire; `undefined` (a missing JS key) binds as NULL, as it does in TypeScript. */
function encodeValue(v: unknown): unknown {
  return v === undefined ? null : encodeParam(v);
}

/** Row → ordered `[column, value]` pairs, keeping only `keys` when given. */
function toRowValues(row: RowRecord, keys?: string[]): Array<[string, unknown]> {
  const entries = Object.entries(row);
  const kept = keys ? entries.filter(([col]) => keys.includes(col)) : entries;
  return kept.map(([col, v]) => [col, encodeValue(v)]);
}

function decodeBindings(data: SqlWithBindings): SqlWithBindings {
  if (data.bindValues) data.bindValues = data.bindValues.map((v) => decodeCell(v));
  return data;
}

/** Engines whose dialect runs in Rust. Each needs a local `paginate` in `PAGINATE`. */
export type RustEngine = "postgres";

function checkRowCount(name: string, n: number): void {
  // The Rust params are u64; stop at 2^53 so the number prints the same.
  if (!Number.isSafeInteger(n) || n < 0) {
    throw new Error(`INVALID_ARGUMENT: ${name} must be a non-negative integer, got ${n}`);
  }
}

/**
 * Pagination runs here, not in the core, to save a round trip per page (on
 * web: browser → Node → Rust). Each mirrors its Rust dialect's `paginate`
 * byte for byte, checked against the same parity fixture; for Postgres see
 * `PostgresDialect::paginate` in crates/seaquel-engine-postgres/src/dialect.rs
 * and tests/fixtures/paginate.json. Phase 2 should replace this with the Rust
 * dialect compiled to wasm (seaquel-wasm).
 */
const PAGINATE: Record<RustEngine, (sql: string, limit: number, offset: number) => string> = {
  postgres: (sql, limit, offset) => `${sql} LIMIT ${limit} OFFSET ${offset}`,
};

export class RustEngineClient implements EngineClient {
  constructor(
    /** Picks the local dialect code (`paginate`); the core knows it from the connection. */
    private readonly engine: RustEngine,
    /**
     * Returns the current id from `provider.connect` (the Rust core's
     * connection id). Read on every call, so a client created before a
     * reconnect uses the new id.
     */
    private readonly getConnectionId: () => string | undefined,
    private readonly transport: EngineTransport = defaultTransport,
  ) {}

  private async call<K extends Kind>(expected: K, request: EngineRequest): Promise<DataOf<K>> {
    const id = this.getConnectionId();
    if (!id) throw new Error("No connection established");
    const response = await this.transport({ connection_id: id, request });
    if (response?.kind !== expected) {
      throw new Error(
        `ENGINE_PROTOCOL: expected "${expected}" response, got "${String(response?.kind)}"`,
      );
    }
    return response.data as DataOf<K>;
  }

  private async sqlWithBindings(request: EngineRequest): Promise<SqlWithBindings> {
    return decodeBindings(await this.call("sqlWithBindings", request));
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

  async paginate(sql: string, limit: number, offset: number): Promise<string> {
    checkRowCount("limit", limit);
    checkRowCount("offset", offset);
    return PAGINATE[this.engine](sql, limit, offset);
  }

  buildUpdate(
    schema: string,
    table: string,
    column: string,
    value: unknown,
    primaryKeys: string[],
    row: RowRecord,
    casts?: CastMap,
  ): Promise<SqlWithBindings> {
    return this.sqlWithBindings({
      method: "buildUpdate",
      params: {
        schema,
        table,
        column,
        value: encodeValue(value),
        primary_keys: primaryKeys,
        // The builder only reads primary-key values; don't ship the other cells.
        row: toRowValues(row, primaryKeys),
        ...(casts ? { casts } : {}),
      },
    });
  }

  buildSetDefault(
    schema: string,
    table: string,
    column: string,
    primaryKeys: string[],
    row: RowRecord,
    casts?: CastMap,
  ): Promise<SqlWithBindings> {
    return this.sqlWithBindings({
      method: "buildSetDefault",
      params: {
        schema,
        table,
        column,
        primary_keys: primaryKeys,
        row: toRowValues(row, primaryKeys),
        ...(casts ? { casts } : {}),
      },
    });
  }

  buildInsert(
    schema: string,
    table: string,
    values: RowRecord,
    casts?: CastMap,
  ): Promise<SqlWithBindings> {
    return this.sqlWithBindings({
      method: "buildInsert",
      params: { schema, table, values: toRowValues(values), ...(casts ? { casts } : {}) },
    });
  }

  buildDelete(
    schema: string,
    table: string,
    primaryKeys: string[],
    row: RowRecord,
    casts?: CastMap,
  ): Promise<SqlWithBindings> {
    return this.sqlWithBindings({
      method: "buildDelete",
      params: {
        schema,
        table,
        primary_keys: primaryKeys,
        row: toRowValues(row, primaryKeys),
        ...(casts ? { casts } : {}),
      },
    });
  }

  createTable(definition: CreateTableDefinition): Promise<string> {
    return this.call("sql", { method: "createTable", params: { definition } });
  }

  alterTable(from: CreateTableDefinition, to: CreateTableDefinition): Promise<string> {
    return this.call("sql", { method: "alterTable", params: { from, to } });
  }
}
