/**
 * The demo's read-only query against real DuckDB-WASM (the Node build of the
 * same 1.32 package the demo loads in the browser). Every attack is checked
 * from a normal connection afterwards: a refusal alone doesn't pass.
 */
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import path from "node:path";
import { Worker } from "node:worker_threads";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { selectReadOnlyOn } from "./duckdb-provider";

type AsyncDuckDB = import("@duckdb/duckdb-wasm").AsyncDuckDB;
type AsyncDuckDBConnection = import("@duckdb/duckdb-wasm").AsyncDuckDBConnection;

const require = createRequire(import.meta.url);
const dist = path.dirname(require.resolve("@duckdb/duckdb-wasm/dist/duckdb-node.cjs"));
const duckdb =
  require("@duckdb/duckdb-wasm/dist/duckdb-node.cjs") as typeof import("@duckdb/duckdb-wasm");

/**
 * DuckDB-WASM's Node worker talks through `globalThis.onmessage` and
 * `postMessage`, as in a browser; this gives it those on a worker thread and
 * the Web Worker interface `AsyncDuckDB` expects on this side.
 */
class NodeWorker {
  private readonly worker: Worker;
  private readonly handlers = new Map<unknown, (data: unknown) => void>();

  constructor(file: string) {
    const boot = [
      "const { parentPort } = require('node:worker_threads');",
      "globalThis.postMessage = (m, t) => parentPort.postMessage(m, t);",
      "parentPort.on('message', (data) => globalThis.onmessage && globalThis.onmessage({ data }));",
      `require(${JSON.stringify(file)});`,
    ].join("\n");
    // The worker prints every query error with its stack; keep that out of
    // the test output.
    this.worker = new Worker(boot, { eval: true, stdout: true, stderr: true });
  }
  postMessage(message: unknown, transfer?: Transferable[]) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    this.worker.postMessage(message, transfer as any);
  }
  addEventListener(type: string, fn: (event: unknown) => void) {
    const handler = (data: unknown) => fn(type === "message" ? { data } : data);
    this.handlers.set(fn, handler);
    this.worker.on(type, handler);
  }
  removeEventListener(type: string, fn: (event: unknown) => void) {
    const handler = this.handlers.get(fn);
    if (handler) this.worker.off(type, handler);
  }
  terminate() {
    void this.worker.terminate();
  }
}

let db: AsyncDuckDB;
let conn: AsyncDuckDBConnection;
/** The Node build writes to the real filesystem; attacks aim in here. */
let scratch: string;

beforeAll(async () => {
  scratch = mkdtempSync(path.join(tmpdir(), "seaquel-demo-ro-"));
  const worker = new NodeWorker(path.join(dist, "duckdb-node-eh.worker.cjs"));
  db = new duckdb.AsyncDuckDB(new duckdb.VoidLogger(), worker as unknown as globalThis.Worker);
  await db.instantiate(path.join(dist, "duckdb-eh.wasm"));
  conn = await db.connect();
}, 60_000);

afterAll(async () => {
  await conn?.close();
  await db?.terminate();
  if (scratch) rmSync(scratch, { recursive: true, force: true });
});

beforeEach(async () => {
  await conn.query(`
    DROP TABLE IF EXISTS t; DROP TABLE IF EXISTS x; DROP SEQUENCE IF EXISTS s;
    CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1);
    CREATE SEQUENCE s;
  `);
});

/** One value from the normal connection. */
async function scalar(sql: string): Promise<unknown> {
  const row = (await conn.query(sql)).toArray()[0]?.toJSON() as Record<string, unknown>;
  return Object.values(row)[0];
}

async function rowsInT(): Promise<number> {
  return Number(await scalar("SELECT count(*) FROM t"));
}

async function tableExists(name: string): Promise<boolean> {
  return (
    Number(await scalar(`SELECT count(*) FROM duckdb_tables() WHERE table_name = '${name}'`)) > 0
  );
}

describe("selectReadOnlyOn (demo DuckDB-WASM)", () => {
  it("runs a SELECT and its read-only forms", async () => {
    expect(await selectReadOnlyOn(db, "SELECT a FROM t")).toEqual([{ a: 1 }]);
    expect(await selectReadOnlyOn(db, "SELECT a FROM t;")).toEqual([{ a: 1 }]);
    expect(await selectReadOnlyOn(db, "FROM t")).toEqual([{ a: 1 }]);
    expect(
      await selectReadOnlyOn(db, "WITH w AS (SELECT a + 1 AS b FROM t) SELECT b FROM w"),
    ).toEqual([{ b: 2 }]);
  });

  it("returns [] for no rows", async () => {
    expect(await selectReadOnlyOn(db, "SELECT a FROM t WHERE a > 5")).toEqual([]);
  });

  it("names duplicate columns as query() does", async () => {
    expect(await selectReadOnlyOn(db, "SELECT 1 AS a, 2 AS a")).toEqual([{ a: 1, a_1: 2 }]);
  });

  const writes: [string, string, RegExp][] = [
    ["INSERT", "INSERT INTO t VALUES (2)", /^READ_ONLY: /],
    ["INSERT … RETURNING", "INSERT INTO t VALUES (2) RETURNING a", /^READ_ONLY: /],
    ["UPDATE", "UPDATE t SET a = 2", /^READ_ONLY: /],
    ["DELETE", "DELETE FROM t", /^READ_ONLY: /],
    ["COMMIT, then a write", "COMMIT; INSERT INTO t VALUES (2); SELECT 1", /^READ_ONLY: /],
    ["a second statement that writes", "SELECT 1; INSERT INTO t VALUES (2)", /^READ_ONLY: /],
    // DuckDB's binder refuses this one before the read-only checks.
    [
      "a CTE that deletes",
      "WITH d AS (DELETE FROM t RETURNING a) SELECT * FROM d",
      /^QUERY_ERROR: .*A CTE needs a SELECT/,
    ],
  ];
  it.each(writes)("refuses %s and leaves t as it was", async (_name, sql, error) => {
    await expect(selectReadOnlyOn(db, sql)).rejects.toThrow(error);
    expect(await rowsInT()).toBe(1);
    expect(await scalar("SELECT a FROM t")).toBe(1);
  });

  it("refuses DDL and leaves no table", async () => {
    for (const sql of [
      "CREATE TABLE x AS SELECT 1 AS n",
      "CREATE TEMP TABLE x (n INTEGER)",
      "CREATE VIEW x AS SELECT 1",
    ]) {
      await expect(selectReadOnlyOn(db, sql), sql).rejects.toThrow(/^READ_ONLY: /);
    }
    expect(await tableExists("x")).toBe(false);
    expect(Number(await scalar("SELECT count(*) FROM duckdb_views() WHERE view_name = 'x'"))).toBe(
      0,
    );
    await expect(selectReadOnlyOn(db, "DROP TABLE t")).rejects.toThrow(/^READ_ONLY: /);
    expect(await tableExists("t")).toBe(true);
  });

  it("refuses nextval and leaves the sequence where it was", async () => {
    await expect(selectReadOnlyOn(db, "SELECT nextval('s')")).rejects.toThrow(/^READ_ONLY: /);
    expect(Number(await scalar("SELECT nextval('s')"))).toBe(1);
  });

  it("refuses COPY … TO and writes no file", async () => {
    const file = path.join(scratch, "copy.csv");
    await expect(selectReadOnlyOn(db, `COPY (SELECT 1) TO '${file}'`)).rejects.toThrow(
      /^READ_ONLY: /,
    );
    expect(existsSync(file)).toBe(false);
  });

  it("refuses EXPORT DATABASE and writes nothing", async () => {
    const dir = path.join(scratch, "export");
    await expect(selectReadOnlyOn(db, `EXPORT DATABASE '${dir}'`)).rejects.toThrow(/^READ_ONLY: /);
    expect(existsSync(dir)).toBe(false);
  });

  it("refuses ATTACH and attaches nothing", async () => {
    const file = path.join(scratch, "attached.duckdb");
    await expect(selectReadOnlyOn(db, `ATTACH '${file}' AS m`)).rejects.toThrow(/^READ_ONLY: /);
    expect(existsSync(file)).toBe(false);
    expect(
      Number(await scalar("SELECT count(*) FROM duckdb_databases() WHERE database_name = 'm'")),
    ).toBe(0);
  });

  it("refuses SET and leaves the setting as it was", async () => {
    const before = await scalar("SELECT current_setting('default_order')");
    for (const sql of ["SET default_order = 'desc'", "SET GLOBAL default_order = 'desc'"]) {
      await expect(selectReadOnlyOn(db, sql), sql).rejects.toThrow(/^READ_ONLY: /);
    }
    expect(await scalar("SELECT current_setting('default_order')")).toBe(before);
    // Nor did a later read-only call inherit it.
    expect(await selectReadOnlyOn(db, "SELECT current_setting('default_order') AS o")).toEqual([
      { o: before },
    ]);
  });

  it("refuses INSTALL, LOAD, CHECKPOINT and PRAGMA", async () => {
    for (const sql of ["INSTALL httpfs", "LOAD parquet", "CHECKPOINT", "PRAGMA version"]) {
      await expect(selectReadOnlyOn(db, sql), sql).rejects.toThrow(/^READ_ONLY: /);
    }
  });

  it("reports other errors with the QUERY_ERROR code", async () => {
    await expect(selectReadOnlyOn(db, "SELECT * FROM missing_table")).rejects.toThrow(
      /^QUERY_ERROR: Catalog Error/,
    );
  });

  it("leaves the user's own transaction open and unaffected", async () => {
    await conn.query("BEGIN TRANSACTION");
    await conn.query("INSERT INTO t VALUES (7)");
    // The read-only query runs on its own connection and doesn't see it.
    expect(await selectReadOnlyOn(db, "SELECT count(*)::INTEGER AS n FROM t")).toEqual([{ n: 1 }]);
    // Still in the user's transaction: their row is there, and COMMIT works.
    expect(await rowsInT()).toBe(2);
    await conn.query("COMMIT");
    expect(await rowsInT()).toBe(2);
  });

  it("rejects with an AbortError, before connecting, for an aborted signal", async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(selectReadOnlyOn(db, "SELECT 1", controller.signal)).rejects.toMatchObject({
      name: "AbortError",
    });
  });

  it("rejects at once when the signal aborts mid-query, and the next call works", async () => {
    // About 2 s on DuckDB-WASM in Node. The worker finishes it in the
    // background (see selectReadOnlyOn), so the next call waits for it.
    const controller = new AbortController();
    const slow = selectReadOnlyOn(
      db,
      "SELECT count(*) AS n FROM range(60000000) a, range(10) b WHERE (a.range + b.range) % 7 = 0",
      controller.signal,
    );
    const started = Date.now();
    setTimeout(() => controller.abort(), 50);
    await expect(slow).rejects.toMatchObject({ name: "AbortError" });
    expect(Date.now() - started).toBeLessThan(500);
    expect(await selectReadOnlyOn(db, "SELECT a FROM t")).toEqual([{ a: 1 }]);
  }, 60_000);
});
