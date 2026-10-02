/**
 * The DuckDB engine in the browser (phase 8, Tasks 4 and 5), live: the
 * browser module's test build (`npm run wasm:build:browser-test`) in Node,
 * driving DuckDB-WASM 1.32's Node build (DuckDB 1.4.3) through the page's
 * bridge (`$lib/core/browser/duckdb-bridge`), with every call going through
 * Core's RPC as the demo's GUI sends it: `db.query`, `db.execute`,
 * `db.transaction`, `db.queryStream` (`readOnly`, `maxRows`, `maxBytes`),
 * `db.engine`, `db.planEdits`, `db.applyChanges` and `db.tablePage`.
 *
 * What the wire can't reach goes through the test build's hooks: a call or
 * stream dropped mid-flight (as Core drops one), a second Core over a bridge
 * that misbehaves or answers late, a `restricted` connect and Core's
 * read-only EXPLAIN (both the MCP server's).
 *
 * Ported from Task 4's driver suite (scratchpad `p8t4/run.test.cjs`, 35
 * tests), plus the DuckDB cases of `crates/seaquel-workspace/tests/fixtures/
 * edits` replayed live, the attached-catalog table page the demo's twin
 * skipped included.
 */
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { AsyncDuckDB } from "@duckdb/duckdb-wasm";
import { callDb, type CoreClient } from "$lib/core/client";
import { makeDuckDbBridge, type DuckDbBridge } from "$lib/core/browser/duckdb-bridge";
import { openBrowserCore, type OpenedBrowserCore } from "$lib/core/browser";
import {
  body as rpcBody,
  bootDuckDb,
  loadTestModule,
  testModuleMissing,
  type TestModule,
} from "$lib/core/browser/testing/node";
import cellsFile from "../../../crates/seaquel-engine-duckdb/tests/fixtures/cells.json";
import applyCases from "../../../crates/seaquel-workspace/tests/fixtures/edits/apply.json";
import changesFile from "../../../crates/seaquel-workspace/tests/fixtures/edits/changes.json";
import planDuckdb from "../../../crates/seaquel-workspace/tests/fixtures/edits/plan-duckdb.json";
import pageDuckdb from "../../../crates/seaquel-workspace/tests/fixtures/edits/table-page-duckdb.json";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

const missing = testModuleMissing();
if (missing && process.env.CI) throw new Error(missing);

type Json = any; // eslint-disable-line @typescript-eslint/no-explicit-any
const P = (v: unknown) => JSON.stringify(v);
const LONG = "SELECT sum(range) FROM range(30000000000)";
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const timed = async <T>(f: () => Promise<T>): Promise<[T, number]> => {
  const t0 = Date.now();
  const r = await f();
  return [r, Date.now() - t0];
};

/** Delays answers, never requests: each call still posts before it returns. */
function delayed(
  bridge: DuckDbBridge,
  hooks: { connectDelay?: number; rollbackDelay?: number },
): DuckDbBridge {
  const later = <T>(p: Promise<T>, ms?: number) =>
    ms ? p.then((v) => new Promise<T>((resolve) => setTimeout(() => resolve(v), ms))) : p;
  return {
    ...bridge,
    connect: () => later(bridge.connect(), hooks.connectDelay),
    startPending: (c, sql) =>
      later(bridge.startPending(c, sql), sql === "ROLLBACK" ? hooks.rollbackDelay : 0),
  };
}

describe.skipIf(missing !== null)("DuckDB in the browser, through Core", () => {
  let module: TestModule;
  let db: AsyncDuckDB;
  let opened: OpenedBrowserCore;
  let client: CoreClient;
  let connectionId: string;
  const log: string[] = [];
  let streamSeq = 0;

  beforeAll(async () => {
    module = (await loadTestModule())!;
    db = await bootDuckDb();
    opened = await openBrowserCore({
      module,
      bridge: makeDuckDbBridge(db, { note: (line) => log.push(line) }),
      store: { load: async () => null, save: async () => {}, moveAside: async () => {} },
      localStorage: null,
      window: null,
      document: null,
    });
    client = opened.client;
    await opened.core.ensureDemoConnection();
    ({ connectionId } = await callDb(client, "connect", {
      target: { type: "saved", id: "demo-connection" },
    } as never));
  }, 60_000);

  afterAll(async () => {
    opened?.close();
    await db?.terminate();
  });

  /** A Core call's result, or `{error: {code, message}}`. */
  async function attempt<T = Json>(
    f: () => Promise<T>,
  ): Promise<T | { error: { code: string; message: string } }> {
    try {
      return await f();
    } catch (error) {
      const e = error as { code?: string; message?: string };
      return {
        error: { code: e.code ?? "UNKNOWN", message: (e.message ?? "").replace(`${e.code}: `, "") },
      };
    }
  }
  const q = (sql: string, params: unknown[] = []): Promise<Json> =>
    attempt(() => callDb(client, "query", { connectionId, sql, params } as never));
  const ex = async (sql: string, params: unknown[] = []): Promise<Json> => {
    const r: Json = await attempt(() =>
      callDb(client, "execute", { connectionId, sql, params } as never),
    );
    return r.error ? r : { rowsAffected: r.rows_affected };
  };
  const tx = (statements: { sql: string; params?: unknown[]; min?: number }[]): Promise<Json> =>
    attempt(async () => {
      await callDb(client, "transaction", {
        connectionId,
        statements: statements.map((s) => ({
          sql: s.sql,
          params: s.params ?? [],
          ...(s.min ? { expectRows: { min: s.min } } : {}),
        })),
      } as never);
      return { ok: true };
    });
  const engine = (request: unknown): Promise<Json> =>
    attempt(
      async () =>
        ((await callDb(client, "engine", { connectionId, request } as never)) as Json).data,
    );

  /** Every event of a query stream. */
  async function streamEvents(
    params: Record<string, unknown>,
    signal?: AbortSignal,
  ): Promise<Json[]> {
    const events: Json[] = [];
    for await (const event of client.stream(
      {
        method: "db",
        params: {
          method: "queryStream",
          params: { connectionId, streamId: `s${++streamSeq}`, params: [], ...params },
        },
      } as never,
      { signal },
    )) {
      events.push(event);
    }
    return events;
  }

  /** The read-only path: `{columns, rows, truncated}` or `{error}`. */
  async function ro(
    sql: string,
    maxRows?: number,
    maxBytes?: number,
    params: unknown[] = [],
  ): Promise<Json> {
    const events = await streamEvents({ sql, params, readOnly: true, maxRows, maxBytes });
    const last = events.at(-1);
    if (last.type === "error") return { error: { code: last.code, message: last.message } };
    const batches = events.filter((e) => e.type === "batch");
    return {
      columns: batches[0]?.columns ?? null,
      rows: batches.flatMap((b) => b.rows),
      truncated: batches.some((b) => b.truncated === true),
    };
  }

  const dropped = async (
    group: string,
    method: string,
    params: unknown,
    ms: number,
    side = false,
  ) => JSON.parse(await module.__test_call_dropped_after(rpcBody(group, method, params), ms, side));
  const streamDropped = async (params: Record<string, unknown>, ms: number, side = false) =>
    JSON.parse(
      await module.__test_stream_dropped_after(
        rpcBody("db", "queryStream", { streamId: `d${++streamSeq}`, params: [], ...params }),
        ms,
        side,
      ),
    );

  // ── The first check: DuckDB-WASM's pending queries, raw ──────────────────

  it("pending queries give IPC stream chunks, and cancelPendingQuery stops a long query", async () => {
    const c = await db.connectInternal();
    let header = await db.startPendingQuery(c, "SELECT range AS i FROM range(5000)", true);
    while (header == null) header = await db.pollPendingQuery(c);
    // A stream's schema message: continuation marker, then its length.
    expect(Buffer.from(header.slice(0, 4)).toString("hex")).toBe("ffffffff");
    let chunks = 0;
    for (;;) {
      const b = await db.fetchQueryResults(c);
      if (b == null) continue;
      if (b.length === 0) break;
      chunks++;
    }
    expect(chunks).toBeGreaterThanOrEqual(2);

    header = await db.startPendingQuery(c, LONG, true);
    expect(header).toBeNull();
    const polling = (async () => {
      try {
        while (header == null) header = await db.pollPendingQuery(c);
        return "finished";
      } catch (e) {
        return (e as Error).message;
      }
    })();
    await sleep(300);
    const [cancelled, ms] = await timed(async () => [
      await db.cancelPendingQuery(c),
      await polling,
    ]);
    expect(cancelled).toEqual([true, "query was canceled"]);
    expect(ms).toBeLessThan(2000);
    await db.disconnect(c);
  });

  it("a long pending query keeps answering the bridge's liveness ping (Task 7 probe, item 5)", async () => {
    // The ping is checked every 200 ms and must answer within 1 s: a query
    // running for 3 s must never be taken for a dead worker.
    const bridge = makeDuckDbBridge(db, { liveness: { checkAfterMs: 200, pingTimeoutMs: 1000 } });
    const c = await bridge.connect();
    let header = await bridge.startPending(c, LONG);
    const until = Date.now() + 3000;
    while (header == null && Date.now() < until) header = await bridge.pollPending(c);
    expect(header).toBeNull(); // still running, and not failed
    expect(await bridge.cancel(c)).toBe(true);
    await bridge.pollPending(c).catch(() => null);
    await bridge.close(c);
  }, 15_000);

  // ── Values ───────────────────────────────────────────────────────────────

  /** `same_value`: floats by value (NaN equal to NaN), arrays element-wise. */
  function same(a: Json, b: Json): boolean {
    if (typeof a === "number" && typeof b === "number")
      return a === b || (Number.isNaN(a) && Number.isNaN(b));
    if (Array.isArray(a) && Array.isArray(b))
      return a.length === b.length && a.every((x, i) => same(x, b[i]));
    return JSON.stringify(a) === JSON.stringify(b);
  }

  type Problem =
    | { kind: "setup" | "select" | "bindBackError"; message: string }
    | { kind: "decoded"; actual: Json; expected: Json }
    | { kind: "bindBack"; actual: Json; answer: Json };

  // The differences from the native driver on DuckDB-WASM 1.4.3, by case
  // name (Task 4's notes). Any other difference fails; each of these must
  // still differ this way.
  const KNOWN: Record<string, (p: Problem) => boolean> = {
    "-7::BIGNUM": (p) => p.kind === "bindBack" && p.actual === -7 && p.answer === false,
    "'101010'::BIT": (p) =>
      p.kind === "decoded" && p.actual?.$sq === "bytes" && p.expected === "101010",
    "'{\"a\": [1, 2.5, null]}'::JSON": (p) =>
      p.kind === "decoded" && p.actual === '{"a": [1, 2.5, null]}',
    "['{\"x\": 1}'::JSON]": (p) =>
      p.kind === "decoded" && Array.isArray(p.actual) && typeof p.actual[0] === "string",
    "{'i': 1, 'big': 9007199254740993, 'd': 1.50, 'f': 'inf'::DOUBLE, 'b': '\\x00a'::BLOB, 'dt': DATE '2024-01-01', 'l': [1, 2], 's': {'x': NULL}, 'j': '[1]'::JSON, 'h': 170141183460469231731687303715884105727::HUGEINT}":
      (p) => p.kind === "decoded" && p.actual?.v?.j === "[1]",
    "TIME_NS '12:00:00.123456789'": (p) =>
      p.kind === "select" && /Unsupported Arrow type TIME_NS/.test(p.message),
    "[TIME_NS '12:00:00.123456789']": (p) =>
      p.kind === "select" && /Unsupported Arrow type TIME_NS/.test(p.message),
    "{'i': INTERVAL 1 YEAR, 't': TIME_NS '12:00:00.123456789'}": (p) =>
      p.kind === "select" && /Unsupported Arrow type TIME_NS/.test(p.message),
    "union_value(t := TIME_NS '12:00:00')::UNION(n INTEGER, t TIME_NS)": (p) =>
      p.kind === "select" && /Unsupported Arrow type TIME_NS/.test(p.message),
    "TIMETZ '12:00:00+02'": (p) =>
      p.kind === "decoded" && p.actual === "12:00:00" && p.expected === "12:00:00+02",
    "TIMETZ '12:00:00.25-05:30'": (p) => p.kind === "decoded" && p.actual === "12:00:00.25",
    "[TIMETZ '12:00:00+02', TIME '00:00:01'::TIMETZ]": (p) =>
      p.kind === "decoded" && P(p.actual) === P(["12:00:00", "00:00:01"]),
    "TIMETZ key": (p) =>
      p.kind === "decoded" && p.actual === "12:00:00" && p.expected === "12:00:00-05:30",
    "'POINT(1 2)'::GEOMETRY": (p) =>
      (p.kind === "select" || p.kind === "setup") &&
      /Type with name "GEOMETRY" is not in the catalog/.test(p.message),
  };

  it("every native typed-cell case decodes and binds back as natively, but for the known 1.4.3 differences", async () => {
    const cases = (cellsFile as Json).cases as Json[];
    expect(cases.length).toBe(119);
    const failed: Record<string, Problem> = {};
    for (const c of cases) {
      let problem: Problem | null = null;
      for (const sql of c.setup) {
        const r = await ex(sql);
        if (r.error) {
          problem = { kind: "setup", message: r.error.message };
          break;
        }
      }
      if (!problem) {
        const r = await q(c.select);
        if (r.error) problem = { kind: "select", message: r.error.message };
        else if (!r.rows?.[0]) problem = { kind: "select", message: "no cell" };
        else {
          const actual = r.rows[0][0];
          if (!same(actual, c.expected))
            problem = { kind: "decoded", actual, expected: c.expected };
          else if (c.bindBack) {
            const back = await q(c.bindBack, [actual]);
            const answer = back.error ? null : back.rows?.[0]?.[0];
            if (back.error) problem = { kind: "bindBackError", message: back.error.message };
            else if (answer !== true && answer !== 1)
              problem = { kind: "bindBack", actual, answer };
          }
        }
      }
      for (const sql of c.teardown) await ex(sql);
      if (problem) failed[c.name] = problem;
    }
    const unexpected = Object.keys(failed).filter((n) => !(n in KNOWN));
    expect(unexpected, unexpected.map((n) => `${n}: ${P(failed[n])}`).join("\n")).toEqual([]);
    for (const [name, matches] of Object.entries(KNOWN)) {
      expect(name in failed, `${name} no longer differs: drop it from KNOWN`).toBe(true);
      expect(matches(failed[name]), `${name}: ${P(failed[name])}`).toBe(true);
    }
  }, 60_000);

  it("HUGEINT reads as natively, 39 digits included; a DECIMAL(38, 0) reads as an Int when it fits", async () => {
    const r = await q(
      "SELECT 12::HUGEINT AS a, '170141183460469231731687303715884105727'::HUGEINT AS b, sum(range) AS c, 5::DECIMAL(38,0) AS d FROM range(3)",
    );
    expect(r.rows).toEqual([
      [12, { $sq: "decimal", v: "170141183460469231731687303715884105727" }, 3, 5],
    ]);
  });

  // ── Binds ────────────────────────────────────────────────────────────────

  it("a bound value of every kind round-trips through its literal", async () => {
    const kinds: unknown[] = [
      null,
      true,
      false,
      0,
      -5,
      { $sq: "bigint", v: "-9223372036854775808" },
      { $sq: "bigint", v: "9007199254740993" },
      1.5,
      0.1,
      { $sq: "float", v: "NaN" },
      { $sq: "float", v: "inf" },
      { $sq: "float", v: "-inf" },
      { $sq: "decimal", v: "1.50" },
      { $sq: "decimal", v: "-0.05" },
      { $sq: "decimal", v: "12345678901234567890123456789012345678" },
      "héllo €",
      'it\'s \\ "q" ? -- /* $$',
      "",
      { $sq: "bytes", v: Buffer.from([0, 255, 97]).toString("base64") },
      { $sq: "bytes", v: "" },
      [1, -2, null],
      [],
    ];
    for (const v of kinds) {
      const r = await q("SELECT ? AS v", [v]);
      expect(r.error, `${P(v)}: ${P(r.error)}`).toBeUndefined();
      expect(r.rows, P(v)).toEqual([[v]]);
    }
    // A NUL inside a value: written as chr(0), read back whole.
    expect((await q("SELECT ? AS v, length(?) AS n", ["a\u0000b", "a\u0000b"])).rows).toEqual([
      ["a\u0000b", 3],
    ]);
    // JSON: bound as JSON; DuckDB-WASM sends JSON back as text (1.4.3).
    const json = await q("SELECT ? AS v, json_type(?) AS t", [
      { $sq: "json", v: { a: [1, "it's"] } },
      { $sq: "json", v: [1] },
    ]);
    expect(json.rows).toEqual([['{"a":[1,"it\'s"]}', "ARRAY"]]);
  });

  it("placeholders inside strings, comments and quoted names are left alone", async () => {
    const none = await q(
      "SELECT '?' AS \"a?\", E'\\'?' AS e, $$?$$ AS d, ? AS v -- ?\n /* ? */ , ?2 + 0 AS w",
    );
    expect(none.error.code).toBe("QUERY_ERROR"); // no values: passed on, DuckDB refuses `?`
    const ok = await q(
      "SELECT '?' AS \"a?\", E'\\'?' AS e, $$?$$ AS d, ? AS v -- ?\n /* ? */ , ? AS w",
      ["x'); DROP TABLE nope; --", 7],
    );
    expect(ok.columns).toEqual(["a?", "e", "d", "v", "w"]);
    expect(ok.rows).toEqual([["?", "'?", "?", "x'); DROP TABLE nope; --", 7]]);
    expect((await q("SELECT ?, ?", [1])).error.message).toMatch(/1 value for 2 placeholders/);
  });

  it("SQL holding a NUL is refused, not cut short", async () => {
    await ex("CREATE TABLE nul_t AS SELECT range AS i FROM range(3)");
    const r = await ex("DELETE FROM nul_t WHERE i = 1\u0000 OR true");
    expect(r.error.code).toBe("QUERY_ERROR");
    expect(r.error.message).toMatch(/NUL/);
    expect((await q("SELECT count(*) FROM nul_t")).rows).toEqual([[3]]);
  });

  // ── Results ──────────────────────────────────────────────────────────────

  it("an empty result keeps its column names, in query and in a stream", async () => {
    const r = await q("SELECT 1 AS a, 'x' AS b WHERE false");
    expect([r.columns, r.rows]).toEqual([["a", "b"], []]);
    const events = await streamEvents({ sql: "SELECT 1 AS a, 'x' AS b WHERE false" });
    expect(events).toEqual([
      { type: "batch", columns: ["a", "b"], rows: [], is_final: true },
      { type: "done" },
    ]);
  });

  it("a stream crosses in batches of 5000 rows", async () => {
    const events = await streamEvents({ sql: "SELECT range AS i FROM range(100000)" });
    const batches = events.filter((e) => e.type === "batch");
    expect(events.at(-1).type).toBe("done");
    expect(batches.reduce((n, b) => n + b.rows.length, 0)).toBe(100000);
    expect(batches.length).toBeGreaterThanOrEqual(20);
    expect(batches[0].columns).toEqual(["i"]);
    expect(batches.slice(1).every((b) => b.columns === null)).toBe(true);
  });

  it("past 100,000 rows a query is RESULT_TOO_LARGE", async () => {
    expect((await q("SELECT range FROM range(100001)")).error.code).toBe("RESULT_TOO_LARGE");
    expect((await q("SELECT range FROM range(100000)")).rows.length).toBe(100000);
  });

  it("rows affected come from DuckDB's Count, for UPDATE and DELETE alike", async () => {
    expect(await ex("CREATE TABLE ra AS SELECT range AS i FROM range(10)")).toEqual({
      rowsAffected: 10,
    });
    expect(await ex("UPDATE ra SET i = i + 100 WHERE i < ?", [3])).toEqual({ rowsAffected: 3 });
    expect(await ex("DELETE FROM ra WHERE i >= ?", [100])).toEqual({ rowsAffected: 3 });
    expect(await ex("INSERT INTO ra VALUES (?), (?)", [1, 2])).toEqual({ rowsAffected: 2 });
    expect(await ex("CREATE TABLE ra2 (a INT)")).toEqual({ rowsAffected: 0 });
    expect((await ex("SELEC 1")).error.code).toBe("EXECUTE_ERROR");
  });

  // ── Cancel ───────────────────────────────────────────────────────────────

  async function nextAnswersAtOnce() {
    const [next, ms] = await timed(() => q("SELECT 42 AS x"));
    expect(next.rows).toEqual([[42]]);
    expect(ms).toBeLessThan(2000);
  }

  for (const kind of ["query", "execute"] as const) {
    it(`dropping a ${kind} call stops its query in DuckDB`, async () => {
      const before = log.length;
      const [r, ms] = await timed(() =>
        dropped("db", kind, { connectionId, sql: LONG, params: [] }, 300),
      );
      expect(r).toEqual({ dropped: true });
      expect(ms).toBeLessThan(1000);
      expect(log.slice(before).some((l) => l.startsWith("cancel"))).toBe(true);
      await nextAnswersAtOnce();
    });
  }

  it("dropping a stream stops its query in DuckDB", async () => {
    const before = log.length;
    const r = await streamDropped({ connectionId, sql: LONG }, 300);
    expect(r).toEqual({ events: [], dropped: true });
    expect(log.slice(before).some((l) => l.startsWith("cancel"))).toBe(true);
    await nextAnswersAtOnce();
  });

  it("dropping a read-only stream stops it, rolls back and closes its own connection", async () => {
    const before = log.length;
    const r = await streamDropped({ connectionId, sql: LONG, readOnly: true }, 300);
    expect(r).toEqual({ events: [], dropped: true });
    await sleep(100);
    const sent = log.slice(before);
    expect(
      sent.some((l) => l.startsWith("cancel")),
      sent.join("\n"),
    ).toBe(true);
    expect(
      sent.some((l) => /runQuery \d+ ROLLBACK/.test(l)),
      sent.join("\n"),
    ).toBe(true);
    expect(
      sent.some((l) => l.startsWith("close")),
      sent.join("\n"),
    ).toBe(true);
    await nextAnswersAtOnce();
  });

  it("a stream aborted from the page (db.cancel) ends with no more rows", async () => {
    const controller = new AbortController();
    setTimeout(() => controller.abort(), 300);
    const [events, ms] = await timed(() => streamEvents({ sql: LONG }, controller.signal));
    expect(events).toEqual([{ type: "error", code: "CANCELLED", message: expect.any(String) }]);
    expect(ms).toBeLessThan(2000);
    expect((await q("SELECT 43 AS x")).rows).toEqual([[43]]);
  });

  it("a stream left half-read leaves the connection usable", async () => {
    let first: Json = null;
    for await (const event of client.stream({
      method: "db",
      params: {
        method: "queryStream",
        params: {
          connectionId,
          streamId: `s${++streamSeq}`,
          sql: "SELECT range AS i FROM range(10000000)",
          params: [],
        },
      },
    } as never)) {
      first = event;
      break;
    }
    expect(first.rows.length).toBe(5000);
    expect((await q("SELECT 44 AS x")).rows).toEqual([[44]]);
  });

  it("a dropped execute doesn't leave its table behind", async () => {
    const r = await dropped(
      "db",
      "execute",
      {
        connectionId,
        sql: "CREATE TABLE never AS SELECT sum(range) AS s FROM range(30000000000)",
        params: [],
      },
      300,
    );
    expect(r).toEqual({ dropped: true });
    expect(
      (await q("SELECT count(*) FROM duckdb_tables() WHERE table_name = 'never'")).rows,
    ).toEqual([[0]]);
  });

  // ── Transactions ─────────────────────────────────────────────────────────

  it("a transaction commits every statement", async () => {
    await ex("CREATE TABLE tx (k INT PRIMARY KEY, v VARCHAR)");
    expect(
      await tx([
        { sql: "INSERT INTO tx VALUES (?, ?), (?, ?)", params: [1, "a", 2, "b"] },
        { sql: "UPDATE tx SET v = ? WHERE k = ?", params: ["z", 1], min: 1 },
      ]),
    ).toEqual({ ok: true });
    expect((await q("SELECT k, v FROM tx ORDER BY k")).rows).toEqual([
      [1, "z"],
      [2, "b"],
    ]);
  });

  it("a failing statement rolls everything back (its index is the apply suite's failedAt)", async () => {
    const r = await tx([
      { sql: "INSERT INTO tx VALUES (3, 'c')" },
      { sql: "INSERT INTO tx VALUES (1, 'dup')" },
    ]);
    expect(r.error.code).toBe("EXECUTE_ERROR");
    const short = await tx([
      { sql: "INSERT INTO tx VALUES (4, 'd')" },
      { sql: "UPDATE tx SET v = 'x' WHERE k = 99", min: 1 },
    ]);
    expect(short.error.code).toBe("NO_ROWS_AFFECTED");
    expect((await q("SELECT count(*) FROM tx")).rows).toEqual([[2]]);
    expect((await tx([{ sql: "SELECT ?, ?", params: [1] }])).error.code).toBeDefined();
  });

  it("a transaction opened by hand is TRANSACTION_OPEN, before BEGIN", async () => {
    await ex("BEGIN");
    await ex("INSERT INTO tx VALUES (5, 'mine')");
    expect((await tx([{ sql: "INSERT INTO tx VALUES (6, 'f')" }])).error.code).toBe(
      "TRANSACTION_OPEN",
    );
    // The user's transaction is still open and theirs to end.
    await ex("ROLLBACK");
    expect((await q("SELECT count(*) FROM tx WHERE k IN (5, 6)")).rows).toEqual([[0]]);
  });

  it("a dropped transaction is rolled back, and none is left open", async () => {
    const r = await dropped(
      "db",
      "transaction",
      {
        connectionId,
        statements: [
          { sql: "CREATE TABLE IF NOT EXISTS drop_tx (a INT)", params: [] },
          { sql: "INSERT INTO drop_tx VALUES (1)", params: [] },
          { sql: LONG, params: [] },
        ],
      },
      300,
    );
    expect(r).toEqual({ dropped: true });
    await sleep(100);
    expect((await q("SELECT count(*) FROM drop_tx")).error?.code).toBe("QUERY_ERROR");
    expect(await tx([{ sql: "CREATE TABLE after_drop (a INT)" }])).toEqual({ ok: true });
  });

  // ── The read-only path ───────────────────────────────────────────────────

  it("the read-only path runs one SELECT and refuses writes", async () => {
    await ex("CREATE TABLE ro_t AS SELECT range AS i FROM range(5)");
    await ex("CREATE SEQUENCE ro_s");
    expect((await ro("SELECT count(*) AS n FROM ro_t")).rows).toEqual([[5]]);
    for (const sql of [
      "DELETE FROM ro_t",
      "SELECT 1; DELETE FROM ro_t",
      "SELECT nextval('ro_s')",
      "CREATE TABLE ro_x (a INT)",
      "INSERT INTO ro_t VALUES (9)",
    ]) {
      const r = await ro(sql);
      expect(r.error?.code, `${sql}: ${P(r)}`).toBe("READ_ONLY");
      expect(r.error.message).not.toMatch(/LINE 1: SELECT \* FROM query\(/);
    }
    expect((await q("SELECT count(*) FROM ro_t")).rows).toEqual([[5]]);
    expect(
      (await q("SELECT count(*) FROM duckdb_tables() WHERE table_name = 'ro_x'")).rows,
    ).toEqual([[0]]);
    expect((await ro("SELECT 1", undefined, undefined, [1])).error.code).toBe("READ_ONLY");
    expect((await ro("SELECT 1\u0000")).error.code).toMatch(/QUERY_ERROR|READ_ONLY/);
    expect((await ro("SELEC 1")).error.code).toMatch(/QUERY_ERROR|READ_ONLY/);
  });

  it("the read-only path honours max_rows and max_bytes", async () => {
    const r = await ro("SELECT range AS i FROM range(1000)", 10);
    expect([r.rows.length, r.truncated]).toEqual([10, true]);
    const all = await ro("SELECT range AS i FROM range(10)", 10);
    expect([all.rows.length, all.truncated]).toEqual([10, false]);
    const bytes = await ro("SELECT repeat('x', 100) AS s FROM range(1000)", undefined, 1000);
    expect(bytes.truncated && bytes.rows.length < 20, `${bytes.rows.length} rows`).toBe(true);
    expect((await ro("SELECT range FROM range(100001)")).error.code).toBe("RESULT_TOO_LARGE");
  });

  it("the read-only path rolls back and closes its connection on every outcome", async () => {
    const before = log.length;
    await ro("SELECT 1");
    await ro("SELECT * FROM no_such_table");
    const calls = log.slice(before);
    expect(calls.filter((l) => l === "connect").length).toBe(2);
    expect(calls.filter((l) => l.startsWith("close")).length).toBe(2);
    expect(calls.filter((l) => /ROLLBACK/.test(l)).length).toBe(2);
  });

  // ── EXPLAIN and introspection ────────────────────────────────────────────

  it("EXPLAIN, EXPLAIN ANALYZE and the read-only EXPLAIN", async () => {
    const plain = await engine({
      method: "explain",
      params: { sql: "SELECT * FROM ro_t WHERE i > ?", params: [1], analyze: false },
    });
    expect(plain.plan, P(plain)).toBeTruthy();
    const analyzed = await engine({
      method: "explain",
      params: { sql: "SELECT count(*) FROM ro_t", params: [], analyze: true },
    });
    expect(analyzed.plan, P(analyzed)).toBeTruthy();
    const ro1 = JSON.parse(
      await module.__test_explain_read_only(connectionId, "SELECT * FROM ro_t"),
    );
    expect(ro1.plan, P(ro1)).toBeTruthy();
    const two = JSON.parse(
      await module.__test_explain_read_only(connectionId, "SELECT 1; DELETE FROM ro_t"),
    );
    expect(two.error.code).toBe("READ_ONLY");
  });

  it("introspection, attached catalogs included", async () => {
    await ex("CREATE SCHEMA app");
    await ex("CREATE TABLE app.parent (id INTEGER PRIMARY KEY, code VARCHAR UNIQUE)");
    await ex(
      "CREATE TABLE app.child (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES app.parent(id), n INTEGER)",
    );
    await ex("CREATE INDEX child_n ON app.child (n)");
    await ex("ATTACH ':memory:' AS aux");
    await ex("CREATE TABLE aux.main.extra (k INTEGER PRIMARY KEY)");
    const schemas: string[] = await engine({ method: "listSchemas" });
    expect(schemas).toEqual(expect.arrayContaining(["app", "aux.main"]));
    const tables: Json[] = await engine({ method: "schemaTables" });
    expect(tables.map((t) => `${t.schema}.${t.name}`)).toEqual(
      expect.arrayContaining(["app.child", "aux.main.extra"]),
    );
    const meta = await engine({
      method: "tableMetadata",
      params: { schema: "app", table: "child" },
    });
    const parent = meta.columns.find((c: Json) => c.name === "parent_id");
    expect(
      parent.isForeignKey && parent.foreignKeyRef.referencedTable === "parent",
      P(meta.columns),
    ).toBe(true);
    expect(meta.columns.find((c: Json) => c.name === "id").isPrimaryKey).toBe(true);
    expect(
      meta.indexes.some((i: Json) => i.name === "child_n"),
      P(meta.indexes),
    ).toBe(true);
    const pmeta = await engine({
      method: "tableMetadata",
      params: { schema: "app", table: "parent" },
    });
    expect(pmeta.columns.find((c: Json) => c.name === "code").isUnique).toBe(true);
    const extra = await engine({
      method: "tableMetadata",
      params: { schema: "aux.main", table: "extra" },
    });
    expect(extra.columns.find((c: Json) => c.name === "k").isPrimaryKey).toBe(true);
    const stats = await engine({ method: "statistics" });
    expect(
      stats.tableSizes.some((t: Json) => t.name === "extra" && t.schema === "aux.main"),
      P(stats.tableSizes),
    ).toBe(true);
  });

  // ── ENUM ─────────────────────────────────────────────────────────────────

  it("a SELECT of an ENUM shows its values, through query, stream and the read-only path", async () => {
    await ex("CREATE TYPE mood AS ENUM ('sad', 'ok', 'happy')");
    await ex("CREATE TABLE en (k INT, m mood, l mood[])");
    await ex(
      "INSERT INTO en VALUES (1, 'ok', ['sad', 'happy']), (2, 'happy', []), (3, NULL, NULL)",
    );
    const before = log.length;
    expect((await q("SELECT k, m, l FROM en ORDER BY k")).rows).toEqual([
      [1, "ok", ["sad", "happy"]],
      [2, "happy", []],
      [3, null, null],
    ]);
    expect(log.slice(before).some((l) => /^runQuery \d+ SELECT k, m, l FROM en/.test(l))).toBe(
      true,
    );
    const events = await streamEvents({ sql: "SELECT m FROM en ORDER BY k" });
    expect(events[0]).toEqual({
      type: "batch",
      columns: ["m"],
      rows: [["ok"], ["happy"], [null]],
      is_final: true,
    });
    const capped = await ro("SELECT m FROM en ORDER BY k", 2);
    expect([capped.rows, capped.truncated]).toEqual([[["ok"], ["happy"]], true]);
    const bytes = await ro("SELECT m, repeat('x', 100) AS pad FROM en, range(100)", undefined, 500);
    expect(bytes.truncated && bytes.rows.length < 10, `${bytes.rows.length} rows`).toBe(true);
    expect((await q("SELECT m FROM en, range(100001)")).error.code).toBe("RESULT_TOO_LARGE");
  });

  it("an ENUM a non-SELECT returns gives the message (it already ran)", async () => {
    const r = await q("INSERT INTO en VALUES (4, 'sad', []) RETURNING m");
    expect(r.error?.code, P(r)).toBe("UNSUPPORTED_TYPE");
    expect(r.error.message).toMatch(
      /^Column "m" is an ENUM, which DuckDB in the browser can't send yet\. Cast it in the query, e\.g\. to VARCHAR\. \(.+\)$/,
    );
    expect(r.error.message).not.toMatch(/INSERT/);
    expect((await q("SELECT count(*) FROM en WHERE k = 4")).rows).toEqual([[1]]);
  });

  // ── Calls a second Core makes over its own bridge ────────────────────────

  const formTarget = {
    type: "form",
    form: {
      name: "Side",
      type: "duckdb",
      host: "",
      port: 0,
      databaseName: ":memory:",
      username: "",
      connectionString: "",
      sshEnabled: false,
      sshHost: "",
      sshPort: 22,
      sshUsername: "",
      sshAuthMethod: "password",
      sshKeyPath: "",
      savePassword: false,
      saveSshPassword: false,
      saveSshKeyPassphrase: false,
    },
  };

  async function side<T = Json>(
    group: string,
    method: string,
    params?: unknown,
  ): Promise<T | { error: Json }> {
    try {
      return (JSON.parse(await module.__test_side_call(rpcBody(group, method, params))) as Json)
        .result.result;
    } catch (error) {
      return {
        error:
          typeof error === "string" ? JSON.parse(error) : { code: "THREW", message: String(error) },
      };
    }
  }

  it("a read-only call dropped during its ROLLBACK still closes its connection", async () => {
    const mine: string[] = [];
    await module.__test_side_open(
      delayed(makeDuckDbBridge(db, { note: (l) => mine.push(l) }), { rollbackDelay: 600 }),
    );
    const { connectionId: sideId } = (await side("db", "connect", { target: formTarget })) as Json;
    const r = await streamDropped(
      { connectionId: sideId, sql: "SELECT 1", readOnly: true },
      300,
      true,
    );
    expect(r.dropped).toBe(true);
    await sleep(900);
    const own = mine
      .filter((l) => l.startsWith("connected"))
      .map((l) => l.split(" ")[1])
      .at(-1);
    expect(own, mine.join("\n")).toBeTruthy();
    expect(mine, mine.join("\n")).toContain(`close ${own}`);
    expect(mine, mine.join("\n")).toContain(`start ${own} ROLLBACK`);
  });

  it("a dropped connect closes the connection once it arrives", async () => {
    const mine: string[] = [];
    await module.__test_side_open(
      delayed(makeDuckDbBridge(db, { note: (l) => mine.push(l) }), { connectDelay: 300 }),
    );
    expect(await dropped("db", "connect", { target: formTarget }, 50, true)).toEqual({
      dropped: true,
    });
    await sleep(600);
    const id = mine.find((l) => l.startsWith("connected"))?.split(" ")[1];
    expect(id, mine.join("\n")).toBeTruthy();
    expect(mine, mine.join("\n")).toContain(`close ${id}`);
  });

  it("concurrent calls on one connection take turns and each gets its own answer", async () => {
    await ex("CREATE TABLE cc (i INT)");
    const calls: Promise<Json>[] = [];
    for (let i = 0; i < 10; i++) {
      calls.push(q("SELECT ? AS v, count(*) AS n FROM range(?)", [i, 1000 * i]));
      calls.push(ex("INSERT INTO cc VALUES (?)", [i]));
      calls.push(ro(`SELECT ${i} AS v`));
    }
    const streams = [1, 2, 3].map(async () => {
      const events = await streamEvents({ sql: "SELECT range AS i FROM range(20000)" });
      const last = events.at(-1);
      return last.type === "error"
        ? last
        : events.filter((e) => e.type === "batch").reduce((n, b) => n + b.rows.length, 0);
    });
    const out = await Promise.all(calls);
    for (let i = 0; i < 10; i++) {
      expect(out[3 * i].rows).toEqual([[i, 1000 * i]]);
      expect(out[3 * i + 1]).toEqual({ rowsAffected: 1 });
      expect(out[3 * i + 2].rows).toEqual([[i]]);
    }
    expect(await Promise.all(streams)).toEqual([20000, 20000, 20000]);
    expect((await q("SELECT count(*), sum(i) FROM cc")).rows).toEqual([[10, 45]]);
  });

  it("a multi-statement execute runs every statement and counts the last", async () => {
    expect(
      await ex("CREATE TABLE ms (a INT); INSERT INTO ms VALUES (1), (2); UPDATE ms SET a = a + 1"),
    ).toEqual({
      rowsAffected: 2,
    });
    expect((await q("SELECT a FROM ms ORDER BY a")).rows).toEqual([[2], [3]]);
  });

  it("a bridge that misbehaves gives errors, never a trap", async () => {
    const real = () => makeDuckDbBridge(db);
    const boom = () => {
      throw new Error("boom");
    };
    const garbage = () =>
      Promise.resolve(new Uint8Array([0xff, 0xff, 0xff, 0xff, 0x10, 0, 0, 0, 1, 2, 3]));
    const halfHeader = (c: number, sql: string) =>
      db.startPendingQuery(c, sql, true).then(async (hd) => {
        while (hd == null) hd = await db.pollPendingQuery(c);
        return hd.slice(0, hd.length >> 1);
      });
    const bridges: Record<string, unknown> = {
      empty: {},
      throws: {
        connect: boom,
        runQuery: boom,
        startPending: boom,
        pollPending: boom,
        fetchChunk: boom,
        cancel: boom,
        close: boom,
      },
      connectNotANumber: { ...real(), connect: () => Promise.resolve("x") },
      connectNegative: { ...real(), connect: () => Promise.resolve(-1) },
      startNotBytes: { ...real(), startPending: () => Promise.resolve(42) },
      startThrowsLater: { ...real(), startPending: () => Promise.reject("not an Error") },
      pollForever: {
        ...real(),
        startPending: () => Promise.resolve(null),
        pollPending: () => Promise.reject(new Error("poll failed")),
      },
      fetchNotBytes: { ...real(), fetchChunk: () => Promise.resolve({}) },
      garbageHeader: { ...real(), startPending: garbage },
      truncatedHeader: { ...real(), startPending: halfHeader },
      garbageChunk: { ...real(), fetchChunk: garbage },
      cancelThrows: { ...real(), cancel: boom, close: boom },
    };
    for (const [name, bridge] of Object.entries(bridges)) {
      await module.__test_side_open(bridge);
      const connected: Json = await side("db", "connect", { target: formTarget });
      if (connected.error) {
        expect(connected.error.code, `${name}: ${P(connected)}`).not.toBe("THREW");
        continue;
      }
      const id = connected.connectionId;
      const outcomes: Json[] = [
        await side("db", "query", { connectionId: id, sql: "SELECT 1 AS a" }),
        await side("db", "execute", { connectionId: id, sql: "SELECT 1" }),
        await side("db", "transaction", {
          connectionId: id,
          statements: [{ sql: "SELECT 1", params: [] }],
        }),
      ];
      const events: Json[] = [];
      await module.__test_side_stream(
        rpcBody("db", "queryStream", {
          connectionId: id,
          streamId: `m${++streamSeq}`,
          sql: "SELECT 1",
          params: [],
          readOnly: true,
        }),
        (json) => events.push(JSON.parse(json)),
      );
      outcomes.push(events.at(-1)?.event);
      if (name === "cancelThrows") {
        expect(outcomes[0].rows, name).toEqual([[1]]);
        continue;
      }
      for (const o of outcomes) {
        const code = o?.error?.code ?? (o?.type === "error" ? o.code : undefined);
        expect(code, `${name}: ${P(outcomes)}`).toBeTruthy();
        expect(code).not.toBe("THREW");
      }
    }
    // The page's own Core is fine afterwards.
    expect((await q("SELECT 45 AS x")).rows).toEqual([[45]]);
  });

  it("a restricted connection is NOT_SUPPORTED", async () => {
    expect(JSON.parse(await module.__test_connect_restricted(false)).code).toBe("NOT_SUPPORTED");
  });

  // ── The DuckDB edit fixtures, live ───────────────────────────────────────

  const changes = changesFile as Record<string, { expected: Json }>;
  const history = {
    connectionId: "demo-connection",
    connectionName: "Demo Database",
    connectionLabels: [],
  };
  /** DuckDB-WASM 1.4.3 sends JSON as text (Task 4): a JSON cell is its text. */
  const asWasm = (cell: Json): Json => (cell?.$sq === "json" ? JSON.stringify(cell.v) : cell);

  async function plan(edits: Json[]): Promise<Json[]> {
    return (await callDb(client, "planEdits", { connectionId, edits } as never)) as Json[];
  }
  async function apply(changesList: Json[]): Promise<Json> {
    return callDb(client, "applyChanges", {
      connectionId,
      changes: changesList,
      confirmed: true,
      history,
    } as never);
  }

  async function catalogTable(rows: string) {
    await ex("DETACH DATABASE IF EXISTS cat");
    await ex("ATTACH ':memory:' AS cat");
    await ex("CREATE TABLE cat.main.t (id BIGINT PRIMARY KEY, v VARCHAR DEFAULT 'd', j JSON)");
    if (rows) await ex(`INSERT INTO cat.main.t VALUES ${rows}`);
  }
  async function mainTable(rows: string) {
    await ex("DROP TABLE IF EXISTS main.m");
    await ex("CREATE TABLE main.m (k INTEGER PRIMARY KEY, h HUGEINT, s STRUCT(a INTEGER))");
    if (rows) await ex(`INSERT INTO main.m VALUES ${rows}`);
  }

  const fields = (c: Json) => ({
    sql: c.sql,
    params: c.params,
    queryType: c.queryType,
    dml: c.dml,
    summary: c.summary ?? null,
  });

  it("plan-duckdb: the attached-catalog queue plans as recorded, and applies", async () => {
    const c = (planDuckdb as Json[]).find((x) => x.name === "duckdb/attached-catalog-queued");
    const want = changes[c.name]?.expected?.queue ?? c.queue;
    await catalogTable(`(9007199254740993, 'a', '{"x": 1}')`);
    const planned = await plan(c.steps.map((s: Json) => s.edit));
    expect(planned.map(fields)).toEqual(want.map(fields));
    const outcome = await apply(want.map((entry: Json) => entry.change));
    expect(outcome).toMatchObject({ outcome: "applied", mode: "atomic", applied: 4, ddl: false });
    expect((await q("SELECT id, v, j FROM cat.main.t ORDER BY id")).rows).toEqual([[5, "b", null]]);
  });

  it("plan-duckdb: the default catalog's immediate edits apply one at a time", async () => {
    const c = (planDuckdb as Json[]).find((x) => x.name === "duckdb/default-catalog-immediate");
    await mainTable(`(1, 170141183460469231731687303715884105727, {'a': 1})`);
    for (const step of c.steps) {
      const outcome = await apply([{ type: "edit", id: "c1", edit: step.edit }]);
      expect(outcome).toMatchObject({ outcome: "applied", mode: "single", applied: 1 });
      expect(outcome.results[0].rowsAffected).toBe(1);
    }
    expect((await q("SELECT k, h, s FROM main.m")).rows).toEqual([
      [1, -5, { $sq: "json", v: { a: 2 } }],
    ]);
  });

  it("plan-duckdb: an edit, then a delete of a row that went away is NO_ROWS_AFFECTED", async () => {
    const c = (planDuckdb as Json[]).find(
      (x) => x.name === "duckdb/query-tab-edit-and-stale-delete",
    );
    await mainTable("(2, NULL, NULL)");
    const [update, del] = c.steps;
    expect(await apply([{ type: "edit", id: "c1", edit: update.edit }])).toMatchObject({
      applied: 1,
    });
    await ex("DELETE FROM main.m WHERE k = 2"); // another writer
    const outcome = await apply([{ type: "edit", id: "c2", edit: del.edit }]);
    // The GUI words NO_ROWS_AFFECTED itself (`stale-edit.ts`), as Core's
    // replay knows (`compares_text`): the code is what's pinned.
    expect(outcome.failed).toMatchObject({ code: del.outcome.code, index: 0, id: "c2" });
  });

  const applyCase = (name: string) => (applyCases as Json[]).find((x) => x.name === name);

  function checkApply(name: string, outcome: Json) {
    const c = applyCase(name);
    const want = { ...c.outcome, ...changes[name]?.expected?.outcome };
    const wantHistory = changes[name]?.expected?.history ?? c.history;
    expect(outcome.outcome).toBe("applied");
    expect(outcome.mode).toBe(c.mode);
    expect(outcome.applied).toBe(want.executed);
    expect(outcome.ddl).toBe(want.hasDdl);
    expect(outcome.failed ? 1 : 0).toBe(want.failed);
    if (outcome.failed) {
      expect(outcome.failed.index).toBe(want.failedAt);
      expect(outcome.failed.id).toBe(want.failedChangeId);
      expect(outcome.failed.code).toBe(want.code);
      // As Core's replay: NO_ROWS_AFFECTED and NOT_EDITABLE are worded by the GUI.
      // DuckDB-WASM 1.4.3 puts "Execute failed: " before the error of a
      // statement that fails inside a transaction; the rest is DuckDB's.
      if (want.error && !["NO_ROWS_AFFECTED", "NOT_EDITABLE"].includes(want.code)) {
        expect(
          `${outcome.failed.code}: ${outcome.failed.message}`.replace(": Execute failed: ", ": "),
        ).toBe(want.error);
      }
    }
    expect(outcome.history.map((h: Json) => ({ query: h.query, rowCount: h.rowCount }))).toEqual(
      wantHistory.map((h: Json) => ({ query: h.query, rowCount: h.rowCount })),
    );
  }

  it("apply/duckdb-catalog-atomic", async () => {
    await catalogTable("(1, 'a', NULL), (2, 'b', NULL)");
    const c = applyCase("apply/duckdb-catalog-atomic");
    checkApply(c.name, await apply(c.queue.map((e: Json) => e.change)));
    expect((await q("SELECT id, v FROM cat.main.t ORDER BY id")).rows).toEqual([
      [1, "x"],
      [2, "d"],
    ]);
  });

  it("apply/duckdb-sidebar-catalog-truncate-and-drop", async () => {
    await catalogTable("(1, 'a', NULL), (2, 'b', NULL), (3, 'c', NULL)");
    await ex("CREATE VIEW cat.main.v AS SELECT * FROM cat.main.t");
    await mainTable("");
    const c = applyCase("apply/duckdb-sidebar-catalog-truncate-and-drop");
    checkApply(c.name, await apply(c.queue.map((e: Json) => e.change)));
    expect(
      (
        await q(
          "SELECT count(*) FROM duckdb_tables() WHERE (database_name = 'cat' AND table_name = 't') OR table_name = 'm'",
        )
      ).rows,
    ).toEqual([[0]]);
  });

  it("apply/duckdb-stale-middle", async () => {
    await catalogTable("(1, 'a', NULL), (3, 'c', NULL)");
    const c = applyCase("apply/duckdb-stale-middle");
    checkApply(c.name, await apply(c.queue.map((e: Json) => e.change)));
    expect((await q("SELECT id, v FROM cat.main.t ORDER BY id")).rows).toEqual([
      [1, "a"],
      [3, "c"],
    ]);
  });

  it("apply/duckdb-error-middle", async () => {
    await catalogTable("(1, 'a', NULL), (3, 'c', NULL)");
    const c = applyCase("apply/duckdb-error-middle");
    checkApply(c.name, await apply(c.queue.map((e: Json) => e.change)));
    expect((await q("SELECT id, v FROM cat.main.t ORDER BY id")).rows).toEqual([
      [1, "a"],
      [3, "c"],
    ]);
  });

  async function tablePage(c: Json): Promise<Json[]> {
    const events: Json[] = [];
    for await (const event of client.stream({
      method: "db",
      params: {
        method: "tablePage",
        params: {
          connectionId,
          streamId: `tp${++streamSeq}`,
          query: c.input.tableQuery,
          page: c.input.page,
          pageSize: c.input.pageSize,
        },
      },
    } as never)) {
      events.push(event);
    }
    return events;
  }

  for (const name of ["tp/duckdb-attached-catalog", "tp/duckdb-default-catalog-full-page"]) {
    it(name, async () => {
      const c = (pageDuckdb as Json[]).find((x) => x.name === name);
      const want = { ...c.result, ...changes[name]?.expected?.result };
      if (name === "tp/duckdb-attached-catalog") {
        // The matching row and one the filters leave out.
        await catalogTable(`(1, 'a', '{"x":1}'), (2, 'b', NULL), (3, 'a', NULL)`);
      } else {
        await mainTable("(1, NULL, {'a': 1}), (2, NULL, {'a': 2})");
      }
      const events = await tablePage(c);
      expect(events.at(-1).type, P(events.at(-1))).toBe("done");
      const rows = events.filter((e) => e.type === "batch").flatMap((e) => e.rows);
      // Column `j` is JSON, which DuckDB-WASM sends as text; `s` is a STRUCT,
      // which still decodes as JSON.
      const jsonColumn = want.columns.indexOf("j");
      expect(rows).toEqual(
        want.rows.map((r: Json[]) => r.map((cell, i) => (i === jsonColumn ? asWasm(cell) : cell))),
      );
      const done = events.find((e) => e.type === "statementDone");
      expect(done.totalRows).toBe(want.totalRows);
      expect(done.totalPages).toBe(want.totalPages);
      const counted =
        changes[name]?.expected && "count" in changes[name].expected
          ? changes[name].expected.count
          : c.count;
      expect(done.countEstimated ?? false).toBe(false);
      if (counted === null) expect(want.totalRows).toBe(rows.length);
    });
  }

  it("close closes the DuckDB-WASM connection", async () => {
    const before = log.length;
    await callDb(client, "disconnect", { connectionId } as never);
    expect(log.slice(before).some((l) => l.startsWith("close"))).toBe(true);
  });
});
