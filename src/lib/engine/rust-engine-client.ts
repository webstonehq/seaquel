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
import { duckdbQualifiedTable, plainQualifiedTable, quoteIdent } from "./qualified-table";
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

/**
 * Connection types whose dialect runs in Rust. Each needs a local `paginate`
 * in `PAGINATE` and a `qualifiedTable` in `QUALIFIED_TABLE`. MariaDB connects
 * through the MySQL engine (driver `"mysql"`, see `toRustConfig`) and shares
 * its dialect.
 */
export type RustEngine = "postgres" | "mysql" | "mariadb" | "sqlite" | "mssql" | "duckdb";

function checkRowCount(name: string, n: number): void {
  // The Rust params are u64; stop at 2^53 so the number prints the same.
  if (!Number.isSafeInteger(n) || n < 0) {
    throw new Error(`INVALID_ARGUMENT: ${name} must be a non-negative integer, got ${n}`);
  }
}

/**
 * Pagination runs here, not in the core, to save a round trip per page (on
 * web: browser → Node → Rust). Each mirrors its Rust dialect's `paginate`
 * byte for byte, checked against the same parity fixture: `paginate` in
 * crates/seaquel-engine-postgres/src/dialect.rs (tests/fixtures/paginate.json),
 * crates/seaquel-engine-mysql/src/dialect.rs (tests/fixtures/mysql/paginate.json),
 * crates/seaquel-engine-sqlite/src/dialect.rs (tests/fixtures/paginate.json),
 * crates/seaquel-engine-mssql/src/dialect.rs (tests/fixtures/paginate.json
 * with the `paginate` cases of bugfixes.json)
 * and crates/seaquel-engine-duckdb/src/dialect.rs (tests/fixtures/paginate.json).
 * A later phase should replace this with
 * the Rust dialect compiled to wasm (seaquel-wasm).
 */
const limitOffset = (sql: string, limit: number, offset: number) =>
  `${sql} LIMIT ${limit} OFFSET ${offset}`;

/** `char::is_whitespace` in Rust: Unicode White_Space (JS `\s` differs at U+0085 and U+FEFF). */
function isRustWhitespace(c: string): boolean {
  return /^[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]$/.test(c);
}

/** A T-SQL word character: identifier, keyword, number or `@variable`. */
function isTsqlWordChar(c: string): boolean {
  return /^[A-Za-z0-9_@#$]$/.test(c);
}

/**
 * `MssqlDialect::paginate`: `OFFSET … ROWS FETCH NEXT … ROWS ONLY` after
 * `sql`, with `ORDER BY (SELECT NULL)` first unless the query has its own
 * top-level ORDER BY (outside parentheses, strings, quoted names and
 * comments). Whatever follows the last token that isn't `;` is dropped.
 * Works on code points, as the Rust works on chars.
 */
function mssqlPaginate(sql: string, limit: number, offset: number): string {
  const s = Array.from(sql);
  const n = s.length;
  const starts = (i: number, pat: string) => s[i] === pat[0] && s[i + 1] === pat[1];
  let i = 0;
  let depth = 0;
  // Top-level words, ASCII upper-cased; `null` for any other top-level token.
  const words: Array<string | null> = [];
  // End (in code points) of the last significant token that isn't `;`.
  let cut = 0;
  while (i < n) {
    const ch = s[i];
    if (isRustWhitespace(ch)) {
      i += 1;
    } else if (starts(i, "--")) {
      const j = s.indexOf("\n", i);
      i = j === -1 ? n : j + 1;
    } else if (starts(i, "/*")) {
      let level = 1;
      i += 2;
      while (i < n && level > 0) {
        if (starts(i, "/*")) {
          level += 1;
          i += 2;
        } else if (starts(i, "*/")) {
          level -= 1;
          i += 2;
        } else {
          i += 1;
        }
      }
    } else if (ch === "'" || ch === '"' || ch === "[") {
      const close = ch === "[" ? "]" : ch;
      i += 1;
      while (i < n) {
        if (s[i] === close) {
          if (i + 1 < n && s[i + 1] === close) {
            i += 2;
            continue;
          }
          i += 1;
          break;
        }
        i += 1;
      }
      cut = i;
      if (depth === 0) words.push(null);
    } else if (isTsqlWordChar(ch)) {
      let j = i;
      while (j < n && isTsqlWordChar(s[j])) j += 1;
      if (depth === 0) words.push(s.slice(i, j).join("").toUpperCase());
      i = j;
      cut = j;
    } else {
      if (ch === "(") depth += 1;
      else if (ch === ")") depth -= 1;
      if (ch !== ";") cut = i + 1;
      if (depth === 0 && ch !== "(" && ch !== ")") words.push(null);
      i += 1;
    }
  }
  const hasOrder = words.some((w, k) => w === "ORDER" && words[k + 1] === "BY");
  const body = s.slice(0, cut).join("");
  const order = hasOrder ? "" : " ORDER BY (SELECT NULL)";
  return `${body}${order} OFFSET ${offset} ROWS FETCH NEXT ${limit} ROWS ONLY`;
}

const PAGINATE: Record<RustEngine, (sql: string, limit: number, offset: number) => string> = {
  postgres: limitOffset,
  mysql: limitOffset,
  mariadb: limitOffset,
  sqlite: limitOffset,
  mssql: mssqlPaginate,
  duckdb: limitOffset,
};

/**
 * `qualifiedTable`, also local. DuckDB splits its listed `catalog.schema`
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

  quoteIdent(name: string): string {
    return quoteIdent(this.engine, name);
  }

  qualifiedTable(schema: string, table: string): string {
    return QUALIFIED_TABLE[this.engine](schema, table);
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
    columnDefault?: string,
  ): Promise<SqlWithBindings> {
    return this.sqlWithBindings({
      method: "buildSetDefault",
      params: {
        schema,
        table,
        column,
        ...(columnDefault !== undefined ? { column_default: columnDefault } : {}),
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
