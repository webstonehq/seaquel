/**
 * The tutorial's database in the browser (web and the demo): DuckDB-WASM in
 * the page, on an instance of its own (`tutorialDuckDb`), never the demo
 * connection's. Phase 8 trimmed it to what the tutorial uses: connect,
 * run its seed and its queries, disconnect. The demo's own connection runs
 * in Core.
 *
 * `select` and `execute` take no parameters: the tutorial sends none.
 */

import { dedupeColumnNames } from "$lib/utils/row-access";
import type { DatabaseProvider, ExecuteResult } from "./types";
import { tutorialDuckDb } from "./duckdb-wasm";

type AsyncDuckDBConnection = import("@duckdb/duckdb-wasm").AsyncDuckDBConnection;

/** What the tutorial needs of a provider (`CoreProvider` on desktop, this in the browser). */
export type TutorialProvider = Pick<
  DatabaseProvider,
  "connect" | "disconnect" | "select" | "execute"
>;

/** The parts of an Arrow result `rowsAffected` reads. */
interface CountResult {
  numRows: number;
  schema: { fields: { name: string }[] };
  getChildAt(index: number): { get(index: number): unknown } | null;
}

/**
 * The rows an INSERT, UPDATE or DELETE affected. DuckDB answers them with one
 * row in one column, `Count`; `numRows` is the size of that answer (always 1),
 * which would hide an UPDATE that matched nothing. Anything else (DDL) keeps
 * reporting `numRows`.
 */
export function rowsAffected(result: CountResult): number {
  const fields = result.schema.fields;
  if (fields.length === 1 && fields[0].name === "Count" && result.numRows === 1) {
    const count = result.getChildAt(0)?.get(0);
    if (typeof count === "bigint" || typeof count === "number") return Number(count);
  }
  return result.numRows;
}

/**
 * An Arrow result as row objects. Duplicate column names (e.g.
 * `SELECT a.id, b.id FROM a JOIN b`) are deduped as `id`, `id_2`: Arrow's
 * `toJSON()` iterates by field name and would silently overwrite them.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function tableRows(result: any): Record<string, unknown>[] {
  const fieldNames: string[] = result.schema?.fields?.map((f: { name: string }) => f.name) ?? [];
  const hasDupes = fieldNames.length > 0 && new Set(fieldNames).size !== fieldNames.length;
  if (!hasDupes) {
    // Fast path — Arrow's JSON serializer handles this correctly and preserves
    // its own type coercions (BigInt→string, timestamp formatting, etc.).
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    return result.toArray().map((row: any) => row.toJSON() as Record<string, unknown>);
  }
  const columns = dedupeColumnNames(fieldNames);
  const vectors = columns.map((_, i) => result.getChildAt(i));
  const numRows = Number(result.numRows);
  return Array.from({ length: numRows }, (_, r) => {
    const obj: Record<string, unknown> = {};
    for (let c = 0; c < columns.length; c++) {
      obj[columns[c]] = vectors[c]?.get(r) ?? null;
    }
    return obj;
  });
}

export class DuckDBProvider implements TutorialProvider {
  private connections = new Map<string, AsyncDuckDBConnection>();
  private next = 0;

  async connect(): Promise<string> {
    const conn = await (await tutorialDuckDb()).connect();
    const connectionId = `duckdb-${++this.next}`;
    this.connections.set(connectionId, conn);
    return connectionId;
  }

  async disconnect(connectionId: string): Promise<void> {
    const conn = this.connections.get(connectionId);
    if (conn) {
      this.connections.delete(connectionId);
      await conn.close();
    }
  }

  private connection(connectionId: string): AsyncDuckDBConnection {
    const conn = this.connections.get(connectionId);
    if (!conn) throw new Error(`Connection not found: ${connectionId}`);
    return conn;
  }

  async select<T = Record<string, unknown>>(connectionId: string, sql: string): Promise<T[]> {
    return tableRows(await this.connection(connectionId).query(sql)) as T[];
  }

  async execute(connectionId: string, sql: string): Promise<ExecuteResult> {
    return { rowsAffected: rowsAffected(await this.connection(connectionId).query(sql)) };
  }
}
