/**
 * The demo runner's events match the Core fixtures (phase 5b, Task 6).
 *
 * Every case in `crates/seaquel-workspace/tests/fixtures/run` runs through
 * `TsQueryRunner` over `CoreProvider` and a scripted `CoreClient` that
 * answers from the case's `driver` list, so the provider calls are checked
 * too. The events are folded into results the way Core's replay folds its
 * own (`crates/seaquel-core/tests/run.rs`, `view_of`) and compared under
 * the README's replay rules, with `changes.json` applied: the demo is meant
 * to behave like Core, not like the recording. Timings aren't compared.
 *
 * `TS_ONLY` lists where the demo's runner can't follow Core, and why.
 */
import { describe, expect, it, vi } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import type { DatabaseType } from "$lib/types";
import type { CoreClient } from "$lib/core";
import type { PersistedQueryHistoryItem } from "$lib/types/generated/PersistedQueryHistoryItem";
import type { RunEvent } from "$lib/types/generated/RunEvent";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

const { TsQueryRunner } = await import("./ts-runner.js");
const { CoreProvider } = await import("$lib/providers/core-provider");
const { CoreCallError } = await import("$lib/storage/rust-client");
const { dedupeColumnNames } = await import("$lib/utils/row-access");

/**
 * Cases where the demo's runner differs from Core, with the reason. The
 * rest must match exactly.
 */
const TS_ONLY: Record<string, string> = {
  // A provider's `select` returns row objects: an empty page has no column
  // names to give (Core reads them from the statement).
  "exec/empty-page": "an empty page carries no column names",
};

type Json = Record<string, unknown>;
const DIR = join(process.cwd(), "crates/seaquel-workspace/tests/fixtures/run");
const changes = JSON.parse(readFileSync(join(DIR, "changes.json"), "utf8")) as Record<
  string,
  { expected: Json }
>;
const cases: Json[] = readdirSync(DIR)
  .filter((f) => f.endsWith(".json") && f !== "changes.json")
  .flatMap((f) => JSON.parse(readFileSync(join(DIR, f), "utf8")) as Json[])
  .map((c) => ({ ...c, ...changes[c.name as string]?.expected }));

// ---------------------------------------------------------------- the client

const PAGE = "\u0001PAGE\u0001";
const PAGE_RE = new RegExp(`^${PAGE}(\\d+):(\\d+)${PAGE}([\\s\\S]*)$`);

interface Op {
  op: string;
  sql: string;
  paginate?: { limit: number; offset: number };
  params: unknown[];
  answer: Json;
}

/** Answers `db` calls from a case's `driver` list, in order, noting any difference. */
class ScriptedClient {
  problems: string[] = [];
  constructor(private ops: Op[]) {}

  load(ops: Op[]) {
    this.ops.push(...structuredClone(ops));
  }

  private next(op: string, sql: string, params: unknown[], paginate?: Op["paginate"]): Op {
    const want = this.ops.shift();
    const got = JSON.stringify({ op, sql, params, paginate });
    if (!want) {
      this.problems.push(`unexpected ${got}`);
      throw new CoreCallError({ code: "SCRIPT", message: "unexpected call" });
    }
    // A count's SQL is seaquel-sql's; only its kind and binds are compared.
    const same =
      want.op === op &&
      (op === "count" || want.sql === sql) &&
      JSON.stringify(want.params) === JSON.stringify(params) &&
      JSON.stringify(want.paginate) === JSON.stringify(paginate);
    if (!same) this.problems.push(`call ${got} vs ${JSON.stringify(want)}`);
    return want;
  }

  left(): number {
    return this.ops.length;
  }

  async call(request: { params: { method: string; params: Json } }) {
    const { method, params } = request.params;
    const text = params.sql as string;
    const binds = params.params as unknown[];
    if (method === "query") {
      const m = PAGE_RE.exec(text);
      let a: Op;
      if (m) {
        a = this.next("page", m[3], binds, { limit: Number(m[1]), offset: Number(m[2]) });
      } else {
        const kind = this.ops[0]?.op === "count" ? "count" : "utility";
        a = this.next(kind, text, binds);
      }
      const answer = a.answer as { columns?: string[]; rows?: unknown[][]; error?: Json };
      if (answer.error) throw new CoreCallError(answer.error as never);
      return {
        method: "db",
        result: {
          method: "query",
          result: { columns: answer.columns ?? [], rows: structuredClone(answer.rows ?? []) },
        },
      };
    }
    if (method === "execute") {
      const a = this.next("write", text, binds).answer as {
        rowsAffected?: number;
        lastInsertId?: number;
        error?: Json;
      };
      if (a.error) throw new CoreCallError(a.error as never);
      return {
        method: "db",
        result: {
          method: "execute",
          result: { rows_affected: a.rowsAffected ?? 0, last_insert_id: a.lastInsertId ?? null },
        },
      };
    }
    this.problems.push(`unexpected db.${method}`);
    throw new CoreCallError({ code: "SCRIPT", message: `unexpected db.${method}` });
  }

  async *stream(request: { params: { params: { sql: string; params: unknown[] } } }) {
    const p = request.params.params;
    let a: Op;
    try {
      a = this.next("stream", p.sql, p.params);
    } catch (e) {
      yield { type: "error", code: "SCRIPT", message: String(e) };
      return;
    }
    const s = a.answer as {
      columns?: string[];
      rows?: unknown[][];
      batches?: { columns?: string[]; rows: unknown[][] }[];
      error?: Json;
    };
    const batches =
      s.batches ?? (s.rows || s.columns ? [{ columns: s.columns, rows: s.rows ?? [] }] : []);
    for (let i = 0; i < batches.length; i++) {
      yield {
        type: "batch",
        columns: batches[i].columns ?? null,
        rows: structuredClone(batches[i].rows),
        is_final: !s.error && i === batches.length - 1,
      };
    }
    if (s.error) yield { type: "error", ...s.error };
    else yield { type: "done" };
  }

  events() {
    return () => {};
  }
}

// ---------------------------------------------------------------- the fold

interface View {
  results: Json[];
  deferred: Json[];
  history: Json | null;
  toasts: Json[];
  problems: string[];
  error: Extract<RunEvent, { type: "error" }> | null;
}

/** Core's replay `apply`/`view_of`, over the runner's events. */
function viewOf(events: RunEvent[], current: boolean): View {
  const v: View = {
    results: [],
    deferred: [],
    history: null,
    toasts: [],
    problems: [],
    error: null,
  };
  const last = () => v.results.at(-1);
  for (const e of events) {
    switch (e.type) {
      case "statementStart":
        v.results.push({
          index: e.index,
          sql: e.sql,
          source: e.source,
          kind: e.kind,
          queryType: e.queryType,
          columns: [],
          rows: [],
          rowCount: 0,
          totalRows: 0,
          page: e.page,
          pageSize: e.pageSize,
          totalPages: 1,
          error: null,
          affectedRows: null,
          lastInsertId: null,
          table: e.table ?? null,
          columnRefs: e.columnRefs ?? null,
          countEstimated: null,
          _batch: false,
        });
        break;
      case "batch": {
        const r = last();
        if (!r) {
          v.problems.push("batch before any statementStart");
          break;
        }
        if (e.columns) r.columns = dedupeColumnNames(e.columns);
        (r.rows as unknown[][]).push(...e.rows);
        r.rowCount = (r.rows as unknown[][]).length;
        r._batch = true;
        break;
      }
      case "statementDone": {
        const r = last();
        if (!r) {
          v.problems.push("statementDone without a start");
          break;
        }
        r.totalRows = e.totalRows;
        r.totalPages = e.totalPages;
        if (r.kind === "page") r.countEstimated = e.countEstimated;
        if (e.rowsAffected !== undefined) r.affectedRows = e.rowsAffected;
        if (e.lastInsertId !== undefined) r.lastInsertId = e.lastInsertId;
        break;
      }
      case "statementError": {
        if (e.sql !== undefined) {
          v.results.push({
            index: e.index,
            sql: e.sql,
            source: null,
            kind: null,
            queryType: null,
            error: e.message,
            _planned: true,
          });
          break;
        }
        const r = last();
        if (!r) {
          v.problems.push("statementError without a start");
          break;
        }
        r.error = `${e.code}: ${e.message}`;
        r.totalRows = r.rowCount;
        break;
      }
      case "statementDeferred":
        v.deferred.push({ index: e.index, sql: e.sql, source: e.source, queryType: e.queryType });
        break;
      case "done":
        if (e.statements === 0) {
          v.toasts.push({
            kind: "info",
            message: "No executable statements found (only comments)",
          });
        }
        v.history = e.history ? { query: e.history.query, rowCount: e.history.rowCount } : null;
        break;
      case "error":
        v.error = e;
        if (e.code === "INVALID_PARAMETERS") v.toasts.push({ kind: "error", message: e.message });
        else v.problems.push(`run failed: ${e.code}: ${e.message}`);
        break;
    }
  }
  const isUtility = (r: Json) => r.kind === "utility" && r.error === null && r._batch === false;
  const allUtility = v.results.every(isUtility);
  for (const r of v.results) r.shown = !(isUtility(r) && !allUtility);
  const n = v.deferred.length;
  if (n > 0) {
    v.toasts.push({
      kind: "info",
      message: current
        ? "Statement added to pending changes"
        : `${n} statement${n > 1 ? "s" : ""} added to pending changes`,
    });
  }
  return v;
}

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
const numEq = (a: unknown, b: unknown) =>
  typeof a === "number" || typeof b === "number" ? Number(a) === Number(b) : same(a, b);

/** Core's replay `compare_result`, under the README's rules. */
function compare(at: string, got: Json, fix: Json, out: string[]) {
  const check = (field: string, ok: boolean) => {
    if (!ok)
      out.push(`${at}.${field}: ${JSON.stringify(got[field])} vs ${JSON.stringify(fix[field])}`);
  };
  check("index", got.index === fix.index);
  check("sql", got.sql === fix.sql);
  check("shown", got.shown === fix.shown);
  check("kind", got.kind === fix.kind);
  if (fix.source !== null) check("source", same(got.source, fix.source));
  if (fix.queryType !== null) check("queryType", got.queryType === fix.queryType);
  check("error", got.error === fix.error);
  for (const field of ["table", "columnRefs", "countEstimated"]) {
    if (field in fix) check(field, same(got[field], fix[field]));
  }
  const error = fix.error !== null;
  const write = fix.kind === "write";
  if (write) {
    check("affectedRows", numEq(got.affectedRows, fix.affectedRows));
    check("lastInsertId", numEq(got.lastInsertId, fix.lastInsertId));
  }
  if (!write && (!error || fix.kind === "stream")) {
    for (const field of [
      "columns",
      "rows",
      "rowCount",
      "totalRows",
      "page",
      "pageSize",
      "totalPages",
    ]) {
      check(field, numEq(got[field], fix[field]));
    }
  }
}

async function collect(iterable: AsyncIterable<RunEvent>): Promise<RunEvent[]> {
  const out: RunEvent[] = [];
  for await (const e of iterable) out.push(e);
  return out;
}

const HISTORY = { connectionId: "saved-1", connectionName: "Local", connectionLabels: [] };

async function replay(c: Json): Promise<string[]> {
  const input = c.input as Json;
  const engine = c.engine as DatabaseType;
  const client = new ScriptedClient(structuredClone(c.driver as Op[]));
  const provider = new CoreProvider(() => client as unknown as CoreClient);
  const appended: PersistedQueryHistoryItem[] = [];
  const runner = new TsQueryRunner({
    provider,
    engine,
    paginate: async (sql, limit, offset) => `${PAGE}${limit}:${offset}${PAGE}${sql}`,
    appendHistory: async (item) => {
      appended.push(item);
    },
  });
  const base = {
    connectionId: "pc-1",
    text: input.text as string,
    target: input.target as never,
    pageSize: input.pageSize as number,
    ...(input.params ? { params: input.params as never } : {}),
    ...(input.deferWrites ? { deferWrites: true } : {}),
    history: HISTORY,
  };
  const out: string[] = [];

  // Unconfirmed first: a run holding destructive statements is refused
  // with the prompt's list, and runs nothing.
  const destructive = c.destructive as Json[];
  if (destructive.length > 0) {
    const refused = await collect(
      runner.run({ ...base, streamId: "refused" }, new AbortController().signal),
    );
    const last = refused.at(-1);
    if (refused.length !== 1 || last?.type !== "error" || last.code !== "CONFIRM_REQUIRED") {
      out.push(`unconfirmed: ${JSON.stringify(refused)}`);
    } else if (!same(last.destructive, destructive)) {
      out.push(
        `destructive: ${JSON.stringify(last.destructive)} vs ${JSON.stringify(destructive)}`,
      );
    }
  }

  const events = await collect(
    runner.run(
      { ...base, streamId: "run", ...(input.confirmed ? { confirmed: true } : {}) },
      new AbortController().signal,
    ),
  );
  const current = (input.target as Json).type === "current";
  const view = viewOf(events, current);
  out.push(...view.problems, ...client.problems.splice(0).map((p) => `driver: ${p}`));

  const fixResults = c.results as Json[];
  if (view.results.length !== fixResults.length) {
    out.push(
      `${view.results.length} results vs ${fixResults.length}: ${events.map((e) => e.type).join(",")}`,
    );
  }
  view.results.forEach((r, i) => fixResults[i] && compare(`results[${i}]`, r, fixResults[i], out));
  if (!same(view.deferred, c.deferred)) {
    out.push(`deferred: ${JSON.stringify(view.deferred)} vs ${JSON.stringify(c.deferred)}`);
  }
  const fixHistory = c.history as Json | null;
  const historyOk =
    (view.history === null && fixHistory === null) ||
    (view.history !== null &&
      fixHistory !== null &&
      view.history.query === fixHistory.query &&
      numEq(view.history.rowCount, fixHistory.rowCount));
  if (!historyOk)
    out.push(`history: ${JSON.stringify(view.history)} vs ${JSON.stringify(fixHistory)}`);
  if (
    view.history &&
    !same(
      appended.map((h) => h.query),
      [view.history.query],
    )
  ) {
    out.push("history: done's row isn't the one appended");
  }
  if (!same(view.toasts, c.toasts)) {
    out.push(`toasts: ${JSON.stringify(view.toasts)} vs ${JSON.stringify(c.toasts)}`);
  }

  // Paging afterwards, through `page` with the result's source.
  const results = view.results;
  for (const [n, p] of ((c.pages as Json[] | undefined) ?? []).entries()) {
    const action = p.action as Json;
    const at = (action.resultIndex as number | undefined) ?? 0;
    const before = results[at];
    if (!before) {
      out.push(`pages[${n}]: no result ${at}`);
      continue;
    }
    const [page, pageSize] =
      action.type === "goToPage"
        ? [action.page as number, before.pageSize as number]
        : [1, action.pageSize as number];
    client.load(p.driver as Op[]);
    const pageEvents = await collect(
      runner.page(
        {
          connectionId: "pc-1",
          streamId: `page-${n}`,
          source: before.source as never,
          page,
          pageSize,
        },
        new AbortController().signal,
      ),
    );
    const pv = viewOf(pageEvents, false);
    out.push(...pv.problems.map((x) => `pages[${n}]: ${x}`));
    out.push(...client.problems.splice(0).map((x) => `pages[${n}] driver: ${x}`));
    if (pv.history !== null) out.push(`pages[${n}]: a page recorded history`);
    if (pv.results.length !== 1) {
      out.push(`pages[${n}]: ${pv.results.length} results`);
      continue;
    }
    const r = { ...pv.results[0], index: before.index, sql: before.sql, shown: before.shown };
    compare(`pages[${n}]`, r, p.result as Json, out);
    results[at] = r;
  }
  if (client.left() > 0) out.push(`${client.left()} driver answers left over`);
  if (appended.length > 1) out.push(`${appended.length} history rows appended`);
  return out;
}

describe("TsQueryRunner replays the run fixtures", () => {
  it("has the fixtures", () => {
    expect(cases.length).toBeGreaterThanOrEqual(95);
  });

  for (const c of cases) {
    const name = c.name as string;
    it(name, async () => {
      const problems = await replay(c);
      if (name in TS_ONLY) {
        // Still a difference: remove the entry once the demo catches up.
        expect(problems, TS_ONLY[name]).not.toEqual([]);
      } else {
        expect(problems).toEqual([]);
      }
    });
  }
});
