/**
 * `QueryRunner` for the demo, which has no Rust core (phase 5b, Decision 13):
 * the query runner the GUI used before 5b, moved out of
 * `QueryExecutionManager` and made to speak Core's `RunEvent`s. Phase 8
 * deletes it, when the demo runs Core in the browser.
 *
 * It plans with the wasm SQL module (`$lib/sql`), runs on a
 * `DatabaseProvider` (DuckDB-WASM in the demo), pages through the engine
 * client's `paginate`, and appends history through the storage client (the
 * demo's sql.js). It follows Core's rules where the fixtures in
 * `crates/seaquel-workspace/tests/fixtures/run` pin them, `changes.json`
 * included:
 *
 * - it refuses an unconfirmed run holding a destructive statement with
 *   `CONFIRM_REQUIRED`, checked on the text before substitution;
 * - a comment-only buffer at the cursor runs nothing (`done`, 0 statements);
 * - paging re-runs the SQL after substitution with its binds (Task 1);
 * - a count that isn't a whole number is a failed count, estimated;
 * - history records only a run whose statements all ran without failing;
 * - an `other` statement that returns columns shows its rows (Decision 18),
 *   except DuckDB's lone status column (`Count` with one integer row).
 *
 * Where it can't follow Core: a provider's `select` returns row objects, so
 * an empty page or an empty `other` result carries no column names.
 */
import { cancelledEvent, errorEvent, StreamQueue } from "$lib/core/client";
import { CoreCallError } from "$lib/storage/rust-client";
import {
  columnRefs,
  countQuery,
  detectQueryTypeOrThrow,
  extractTableFromSelect,
  findDestructiveStatements,
  getStatementAtOffsetOrThrow,
  hasRowLimit,
  isDestructiveStatement,
  ParameterSubstitutionError,
  splitSqlStatementsOrThrow,
  substituteParameters,
  type ParsedStatement,
} from "$lib/sql";
import { cellText, decodeCell, encodeParam, SqlDecimal } from "$lib/values";
import { extractErrorMessage } from "$lib/errors";
import { log } from "$lib/utils/logger";
import type { DatabaseType, ParameterValue } from "$lib/types";
import type { DatabaseProvider } from "$lib/providers/types";
import type { DestructiveStatement } from "$lib/types/generated/DestructiveStatement";
import type { HistoryContext } from "$lib/types/generated/HistoryContext";
import type { PageSource } from "$lib/types/generated/PageSource";
import type { PersistedQueryHistoryItem } from "$lib/types/generated/PersistedQueryHistoryItem";
import type { QueryType } from "$lib/types/generated/QueryType";
import type { StatementKind } from "$lib/types/generated/StatementKind";
import {
  CONFIRM_REQUIRED,
  MAX_DESTRUCTIVE_LISTED,
  INVALID_ARGUMENT,
  INVALID_PARAMETERS,
  type PageParams,
  type QueryRunner,
  type RunEvent,
  type RunParams,
} from "./types";

/** The code for a failure of the wasm SQL module on the run path. */
export const SQL_CHECK_FAILED = "SQL_CHECK_FAILED";
/** The code for a driver failure that carried none (DuckDB-WASM throws plain errors). */
const QUERY_ERROR = "QUERY_ERROR";

export interface TsRunnerContext {
  /** Runs the SQL; its connection id is the run's `connectionId`. */
  provider: DatabaseProvider;
  /** The connection's type: the rules the wasm module scans with. */
  engine: DatabaseType;
  /** The engine client's `paginate` (dialect work stays in `$lib/engine`). */
  paginate: (sql: string, limit: number, offset: number) => Promise<string>;
  /** Stores a history row (`getStorage().queryHistory.append`). */
  appendHistory: (item: PersistedQueryHistoryItem) => Promise<void>;
}

/** What one statement came to, for the run's history row (Core's `Outcome`). */
interface Outcome {
  elapsedMs: number;
  rowCount: number;
  hiddenUtility: boolean;
}

type Emit = (event: RunEvent) => void;

const round = (ms: number) => Math.round(ms * 100) / 100;
const strip = (sql: string) => sql.replace(/;$/, "").trim();

/** A driver failure as Core's `{code, message}`. */
function failure(error: unknown): { code: string; message: string } {
  if (error instanceof CoreCallError) {
    return { code: error.code, message: error.message.replace(`${error.code}: `, "") };
  }
  return { code: QUERY_ERROR, message: extractErrorMessage(error) };
}

/** A stream's `"CODE: message"` error text as `{code, message}`. */
function splitStreamError(text: string): { code: string; message: string } {
  const m = /^([A-Z][A-Z0-9_]*): ([\s\S]*)$/.exec(text);
  return m ? { code: m[1], message: m[2] } : { code: QUERY_ERROR, message: text };
}

/** A count's first cell as a whole number, or `null` (Core's `count_of`). */
function countOf(value: unknown): number | null {
  let text: string;
  if (typeof value === "number") {
    return Number.isInteger(value) && value >= 0 ? value : null;
  } else if (typeof value === "bigint") {
    text = value.toString();
  } else if (typeof value === "string" || value instanceof SqlDecimal) {
    text = cellText(value).trim();
  } else {
    return null;
  }
  return /^\d+$/.test(text) ? Number(text) : null;
}

/** Row objects as columnar rows in the wire format. */
function columnar(rows: Record<string, unknown>[]): { columns: string[]; rows: unknown[][] } {
  const columns = rows.length > 0 ? Object.keys(rows[0]) : [];
  return { columns, rows: rows.map((row) => columns.map((c) => encodeParam(row[c]))) };
}

/**
 * DuckDB's answer to a statement that returns nothing (`CREATE TABLE … AS`):
 * one `Count` column with one integer row. The other status answers carry
 * no row, so `select` gives no column names for them anyway.
 */
function isDuckdbStatus(engine: DatabaseType, columns: string[], rows: unknown[][]): boolean {
  if (engine !== "duckdb" || columns.length !== 1 || columns[0] !== "Count") return false;
  if (rows.length !== 1) return false;
  const cell = decodeCell(rows[0][0]);
  return typeof cell === "number" || typeof cell === "bigint";
}

export class TsQueryRunner implements QueryRunner {
  constructor(private readonly ctx: TsRunnerContext) {}

  run(params: RunParams, signal: AbortSignal): AsyncIterable<RunEvent> {
    return this.drive(signal, (emit, stop) => this.runAll(params, emit, stop));
  }

  page(params: PageParams, signal: AbortSignal): AsyncIterable<RunEvent> {
    return this.drive(signal, (emit, stop) => this.pageOne(params, emit, stop));
  }

  /**
   * The events of `body`, as the Core clients hand them out: aborting
   * `signal` (or the consumer stopping) cancels it and ends with
   * `CANCELLED`; a body that throws ends with its error.
   */
  private drive(
    signal: AbortSignal,
    body: (emit: Emit, stop: AbortSignal) => Promise<void>,
  ): AsyncIterable<RunEvent> {
    const stop = new AbortController();
    const queue = new StreamQueue<RunEvent>(() => stop.abort());
    if (signal.aborted) {
      queue.pushError(cancelledEvent());
      return queue;
    }
    const onAbort = () => {
      stop.abort();
      queue.pushError(cancelledEvent());
    };
    signal.addEventListener("abort", onAbort, { once: true });
    queue.onEnd(() => signal.removeEventListener("abort", onAbort));
    const emit: Emit = (event) => {
      if (!stop.signal.aborted) queue.push(event);
    };
    body(emit, stop.signal).then(
      () => queue.finish(),
      (error: unknown) => queue.pushError(errorEvent(error)),
    );
    return queue;
  }

  private async runAll(p: RunParams, emit: Emit, stop: AbortSignal): Promise<void> {
    const { engine } = this.ctx;
    const values: ParameterValue[] | undefined = p.params?.map((v) => ({
      name: v.name,
      value: decodeCell(v.value ?? null),
    }));

    // Plan: the statements, then the destructive check before anything runs.
    let chosen: ParsedStatement[];
    const destructive: DestructiveStatement[] = [];
    try {
      if (p.target.type === "all") {
        chosen = splitSqlStatementsOrThrow(p.text, engine);
        for (const d of findDestructiveStatements(chosen, engine)) {
          destructive.push({ index: d.index, sql: d.sql, reason: d.reason });
        }
      } else {
        const s = getStatementAtOffsetOrThrow(p.text, p.target.cursor, engine);
        chosen = s ? [s] : [];
        const reason = s ? isDestructiveStatement(s.sql, engine) : null;
        if (s && reason) destructive.push({ index: s.index, sql: s.sql, reason });
      }
    } catch (error) {
      emit({ type: "error", code: SQL_CHECK_FAILED, message: extractErrorMessage(error) });
      return;
    }
    if (destructive.length > 0 && !p.confirmed) {
      const n = destructive.length;
      emit({
        type: "error",
        code: CONFIRM_REQUIRED,
        message: `${n} destructive statement${n === 1 ? "" : "s"} must be confirmed before this run`,
        destructive: destructive.slice(0, MAX_DESTRUCTIVE_LISTED),
        destructiveTotal: n,
      });
      return;
    }

    let ran = 0;
    let failed = false;
    let first: Outcome | null = null;
    let firstShown: Outcome | null = null;
    for (const statement of chosen) {
      if (stop.aborted) return;
      const index = p.target.type === "all" ? statement.index : 0;
      const typed = statement.sql;

      let source: PageSource;
      let binds: unknown[] = [];
      if (values) {
        try {
          const out = substituteParameters(typed, values, engine);
          binds = out.bindValues;
          source = { sql: out.sql, params: binds.map(encodeParam) };
        } catch (error) {
          const message =
            error instanceof ParameterSubstitutionError
              ? error.message
              : extractErrorMessage(error);
          if (p.target.type === "current") {
            emit({ type: "error", code: INVALID_PARAMETERS, message });
            return;
          }
          failed = true;
          emit({
            type: "statementError",
            index,
            code: INVALID_PARAMETERS,
            message,
            elapsedMs: 0,
            sql: typed,
          });
          continue;
        }
      } else {
        source = { sql: typed, params: [] };
      }

      let queryType: QueryType;
      let kind: StatementKind;
      try {
        queryType = detectQueryTypeOrThrow(source.sql, engine);
        kind = this.kindOf(queryType, source.sql, p.pageSize);
      } catch (error) {
        failed = true;
        emit({
          type: "statementError",
          index,
          code: SQL_CHECK_FAILED,
          message: extractErrorMessage(error),
          elapsedMs: 0,
          sql: typed,
        });
        continue;
      }

      if (p.deferWrites && queryType !== "select") {
        emit({ type: "statementDeferred", index, sql: typed, source, queryType });
        continue;
      }

      emit({
        type: "statementStart",
        index,
        sql: typed,
        source,
        queryType,
        kind,
        page: 1,
        pageSize: p.pageSize,
        ...this.refs(queryType, source.sql),
      });
      const result = await this.execute(
        p.connectionId,
        index,
        kind,
        source.sql,
        binds,
        1,
        p.pageSize,
        emit,
        stop,
      );
      if (stop.aborted) return;
      ran += 1;
      if (!result) {
        failed = true;
        continue;
      }
      first ??= result;
      if (!firstShown && !result.hiddenUtility) firstShown = result;
    }
    if (stop.aborted) return;

    const succeeded = ran > 0 && !failed;
    let history: PersistedQueryHistoryItem | undefined;
    const outcome = firstShown ?? first;
    if (succeeded && p.history && outcome) {
      const query = p.target.type === "all" ? p.text : (chosen[0]?.sql ?? "");
      history = await this.record(p.history, query, outcome);
    }
    emit({
      type: "done",
      statements: chosen.length,
      succeeded,
      ...(history ? { history } : {}),
    });
  }

  private async pageOne(p: PageParams, emit: Emit, stop: AbortSignal): Promise<void> {
    const { engine } = this.ctx;
    if (p.page < 1) {
      emit({ type: "error", code: INVALID_ARGUMENT, message: "Pages start at 1" });
      return;
    }
    const sql = p.source.sql;
    let queryType: QueryType;
    let kind: StatementKind;
    try {
      queryType = detectQueryTypeOrThrow(sql, engine);
      if (queryType !== "select") {
        emit({ type: "error", code: INVALID_ARGUMENT, message: "Only a SELECT can be paged" });
        return;
      }
      kind = this.kindOf(queryType, sql, p.pageSize);
    } catch (error) {
      emit({ type: "error", code: SQL_CHECK_FAILED, message: extractErrorMessage(error) });
      return;
    }
    emit({
      type: "statementStart",
      index: 0,
      sql,
      source: p.source,
      queryType,
      kind,
      page: p.page,
      pageSize: p.pageSize,
      ...this.refs(queryType, sql),
    });
    const binds = p.source.params.map((v) => decodeCell(v));
    const result = await this.execute(
      p.connectionId,
      0,
      kind,
      sql,
      binds,
      p.page,
      p.pageSize,
      emit,
      stop,
    );
    if (stop.aborted) return;
    emit({ type: "done", statements: 1, succeeded: result !== null });
  }

  /** How a statement runs (Decision 5). Throws if the module fails. */
  private kindOf(queryType: QueryType, sql: string, pageSize: number): StatementKind {
    if (queryType === "select") {
      return pageSize === 0 || hasRowLimit(strip(sql), this.ctx.engine) ? "stream" : "page";
    }
    if (queryType === "insert" || queryType === "update" || queryType === "delete") return "write";
    return "utility";
  }

  /** A SELECT's table and column references, for inline editing. */
  private refs(queryType: QueryType, sql: string) {
    if (queryType !== "select") return {};
    const base = strip(sql);
    const table = extractTableFromSelect(base, this.ctx.engine);
    const refs = columnRefs(base, this.ctx.engine);
    return { ...(table ? { table } : {}), ...(refs ? { columnRefs: refs } : {}) };
  }

  /**
   * One statement's events after its `statementStart`: batches, then
   * `statementDone` or `statementError`. `null` when it failed; nothing is
   * emitted once `stop` aborted.
   */
  private async execute(
    connectionId: string,
    index: number,
    kind: StatementKind,
    sql: string,
    binds: unknown[],
    page: number,
    pageSize: number,
    emit: Emit,
    stop: AbortSignal,
  ): Promise<Outcome | null> {
    const { provider, engine } = this.ctx;
    const base = strip(sql);
    const start = performance.now();
    const elapsed = () => round(performance.now() - start);
    const done = (
      totalRows: number,
      extra: {
        totalPages?: number;
        countEstimated?: boolean;
        rowsAffected?: number;
        lastInsertId?: number;
      } = {},
    ) =>
      emit({
        type: "statementDone",
        index,
        elapsedMs: elapsed(),
        totalRows,
        totalPages: extra.totalPages ?? 1,
        countEstimated: extra.countEstimated ?? false,
        ...(extra.rowsAffected !== undefined ? { rowsAffected: extra.rowsAffected } : {}),
        ...(extra.lastInsertId !== undefined ? { lastInsertId: extra.lastInsertId } : {}),
      });
    const fail = (error: { code: string; message: string }) => {
      emit({ type: "statementError", index, ...error, elapsedMs: elapsed() });
      return null;
    };

    try {
      if (kind === "stream") {
        let rows = 0;
        const outcome = await provider.selectStream(
          connectionId,
          base,
          binds.length > 0 ? binds : undefined,
          (batch) => {
            if (stop.aborted) return false;
            rows += batch.rows.length;
            emit({
              type: "batch",
              columns: batch.columns,
              rows: batch.rows.map((row) => row.map(encodeParam)),
              is_final: batch.isFinal,
            });
            return !stop.aborted;
          },
          stop,
        );
        if (stop.aborted) return null;
        if (outcome.error) return fail(splitStreamError(outcome.error));
        done(rows);
        return { elapsedMs: elapsed(), rowCount: rows, hiddenUtility: false };
      }

      if (kind === "page") {
        const offset = (page - 1) * pageSize;
        const paged = await this.ctx.paginate(base, pageSize + 1, offset);
        let fetched = await provider.select<Record<string, unknown>>(
          connectionId,
          paged,
          binds.length > 0 ? binds : undefined,
        );
        if (stop.aborted) return null;
        let total: number;
        let estimated = false;
        if (fetched.length <= pageSize) {
          total = offset + fetched.length;
        } else {
          fetched = fetched.slice(0, pageSize);
          let counted: number | null = null;
          try {
            const rows = await provider.select<{ total: unknown }>(
              connectionId,
              countQuery(base, engine),
              binds.length > 0 ? binds : undefined,
            );
            counted = countOf(rows[0]?.total);
            if (counted === null) void log.warn("Row count failed: COUNT_NOT_NUMERIC; estimating");
          } catch (error) {
            void log.warn(`Row count failed: ${failure(error).code}; estimating`);
          }
          if (stop.aborted) return null;
          if (counted === null) {
            estimated = true;
            total = offset + pageSize + 1;
          } else {
            total = counted;
          }
        }
        const { columns, rows } = columnar(fetched);
        emit({ type: "batch", columns, rows, is_final: true });
        const totalPages = pageSize === 0 ? 1 : Math.max(1, Math.ceil(total / pageSize));
        done(total, { totalPages, countEstimated: estimated });
        return { elapsedMs: elapsed(), rowCount: total, hiddenUtility: false };
      }

      if (kind === "write") {
        const result = await provider.execute(
          connectionId,
          base,
          binds.length > 0 ? binds : undefined,
        );
        if (stop.aborted) return null;
        const rowsAffected = result?.rowsAffected ?? 0;
        done(0, { rowsAffected, lastInsertId: result?.lastInsertId });
        return { elapsedMs: elapsed(), rowCount: rowsAffected, hiddenUtility: false };
      }

      // A utility statement: `select`, as DuckDB's SET needs. Rows when it
      // returned columns (Decision 18).
      const answer = await provider.select<Record<string, unknown>>(
        connectionId,
        base,
        binds.length > 0 ? binds : undefined,
      );
      if (stop.aborted) return null;
      const { columns, rows } = columnar(answer);
      if (columns.length > 0 && !isDuckdbStatus(engine, columns, rows)) {
        emit({ type: "batch", columns, rows, is_final: true });
        done(rows.length);
        return { elapsedMs: elapsed(), rowCount: rows.length, hiddenUtility: false };
      }
      done(0);
      return { elapsedMs: elapsed(), rowCount: 0, hiddenUtility: true };
    } catch (error) {
      if (stop.aborted) return null;
      return fail(failure(error));
    }
  }

  /** Append the run's history row; `undefined` (logged) when that fails. */
  private async record(
    ctx: HistoryContext,
    query: string,
    outcome: Outcome,
  ): Promise<PersistedQueryHistoryItem | undefined> {
    const item: PersistedQueryHistoryItem = {
      id: `hist-${crypto.randomUUID()}`,
      query,
      timestamp: new Date().toISOString(),
      executionTime: outcome.elapsedMs,
      rowCount: outcome.rowCount,
      connectionId: ctx.connectionId,
      favorite: false,
      connectionLabelsSnapshot: ctx.connectionLabels,
      connectionNameSnapshot: ctx.connectionName,
    };
    try {
      await this.ctx.appendHistory(item);
      return item;
    } catch (error) {
      void log.error(`Recording the run in history failed: ${extractErrorMessage(error)}`);
      return undefined;
    }
  }
}
