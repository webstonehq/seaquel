/**
 * `EditService` for the demo, which has no Rust core (phase 5c, Decision 16):
 * the edit builders, apply loop and data tab query the GUI used before 5c,
 * moved out of `QueryCrudManager`, `PendingChangesManager` and
 * `DataTabManager` and made to speak the generated types. Phase 8 deletes
 * it with `TsQueryRunner`, when the demo runs Core in the browser.
 *
 * It builds with the demo's DuckDB adapter (`$lib/db/duckdb.ts`: values
 * inlined as literals, no casts), runs on a `DatabaseProvider` (DuckDB-WASM)
 * and appends history through the storage client (sql.js). Where the
 * fixtures in `crates/seaquel-workspace/tests/fixtures/edits` pin Core's
 * rules, it follows them:
 *
 * - one change runs on its own (`single`); two or more DML changes run in
 *   one `BEGIN TRANSACTION … COMMIT` on DuckDB-WASM's connection
 *   (`atomic`, rolled back on the first failure); anything else runs in
 *   order and stops at the first failure (`inOrder`);
 * - a keyed edit that affects no row fails with `NO_ROWS_AFFECTED`;
 * - a typed change must be one statement, and an unconfirmed batch holding
 *   a destructive statement is `confirmRequired`; both are checked before
 *   anything runs;
 * - history gets one row per applied change, with its rows affected.
 *
 * Where it can't follow Core (gaps phase 8 closes): it reads no table
 * metadata, so it doesn't check the key against the primary key; and the
 * data tab's query is the pre-5c one (a count before every page, 0 when the
 * count fails), with filter values inlined as escaped literals, since the
 * DuckDB-WASM provider ignores bind values.
 */
import { cancelledEvent, errorEvent, StreamQueue } from "$lib/core/client";
import { CoreCallError } from "$lib/storage/rust-client";
import type { DatabaseAdapter } from "$lib/db";
import { formatLiteralValue } from "$lib/db/crud-helpers";
import {
  changeSummary,
  detectQueryTypeOrThrow,
  isDestructiveStatement,
  splitSqlStatementsOrThrow,
} from "$lib/sql";
import { decodeCell, encodeParam } from "$lib/values";
import { extractErrorMessage } from "$lib/errors";
import { log } from "$lib/utils/logger";
import { m } from "$lib/paraglide/messages.js";
import { plainQualifiedTable, quoteIdent } from "$lib/engine/qualified-table";
import type { DatabaseType } from "$lib/types";
import type { DatabaseProvider } from "$lib/providers/types";
import type { ApplyFailure } from "$lib/types/generated/ApplyFailure";
import type { ApplyMode } from "$lib/types/generated/ApplyMode";
import type { ChangeResult } from "$lib/types/generated/ChangeResult";
import type { DestructiveStatement } from "$lib/types/generated/DestructiveStatement";
import type { HistoryContext } from "$lib/types/generated/HistoryContext";
import type { PersistedQueryHistoryItem } from "$lib/types/generated/PersistedQueryHistoryItem";
import type { QueryType } from "$lib/types/generated/QueryType";
import type {
  ApplyChangesParams,
  ApplyOutcome,
  Change,
  Edit,
  EditService,
  ExtensionAction,
  PlanEditsParams,
  PlannedChange,
  RunEvent,
  TablePageParams,
  TableQuery,
} from "./types";
import { NO_ROWS_AFFECTED } from "./types";

/** Core's codes for a refusal before anything runs, and for a failed statement. */
const INVALID_ARGUMENT = "INVALID_ARGUMENT";
const EXECUTE_ERROR = "EXECUTE_ERROR";
const QUERY_ERROR = "QUERY_ERROR";
/** How many destructive statements a `confirmRequired` lists (`MAX_DESTRUCTIVE_LISTED`). */
const MAX_DESTRUCTIVE_LISTED = 100;

export interface TsEditServiceContext {
  /** Runs the SQL; its connection id is each call's `connectionId`. */
  provider: DatabaseProvider;
  /** The connection's type: the dialect and the rules the wasm module scans with. */
  engine: DatabaseType;
  /** The demo's dialect (`getAdapter(engine)`). */
  adapter: DatabaseAdapter;
  /** A table's columns from the schema cache (SQL Server's select list; empty elsewhere). */
  columnsOf?: (schema: string, table: string) => Array<{ name: string; type: string }>;
  /** Stores a history row (`getStorage().queryHistory.append`). */
  appendHistory: (item: PersistedQueryHistoryItem) => Promise<void>;
}

/** A change ready to run: its SQL, what it is and whether it must match a row. */
interface Ready {
  id: string;
  sql: string;
  queryType: QueryType;
  dml: boolean;
  keyed: boolean;
}

/** A refusal before anything runs, as Core's `PlanError`. */
class Refused extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/** A driver failure as `{code, message}`: DuckDB-WASM throws plain errors. */
function failureOf(error: unknown, code = EXECUTE_ERROR): { code: string; message: string } {
  if (error instanceof CoreCallError) {
    return { code: error.code, message: error.message.replace(`${error.code}: `, "") };
  }
  return { code, message: extractErrorMessage(error) };
}

/** The CAST-to-text type for a data tab filter. */
function filterDialect(dbType: string): { textType: string } {
  if (dbType === "mssql") return { textType: "NVARCHAR(MAX)" };
  if (dbType === "mysql" || dbType === "mariadb") return { textType: "CHAR" };
  return { textType: "TEXT" };
}

/** An `IN`/`NOT IN` filter's items: comma-separated, trimmed, empty ones skipped. */
function inListItems(value: string): string[] {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter((item) => item !== "");
}

export class TsEditService implements EditService {
  constructor(private readonly ctx: TsEditServiceContext) {}

  private qi = (name: string) => quoteIdent(this.ctx.engine, name);

  // -------- Planning --------

  /** An edit's SQL (values inlined) and what it is. */
  private build(edit: Edit): { sql: string; queryType: QueryType; dml: boolean; keyed: boolean } {
    const { adapter } = this.ctx;
    const { schema, table } = edit.target;
    const keyed = (key: Array<[string, unknown]>) => ({
      primaryKeys: key.map(([column]) => column),
      row: Object.fromEntries(key.map(([column, value]) => [column, decodeCell(value)])),
    });
    switch (edit.type) {
      case "updateCell": {
        const { primaryKeys, row } = keyed(edit.key);
        const { sql } = adapter.buildUpdateSql(
          schema,
          table,
          edit.column,
          decodeCell(edit.value),
          primaryKeys,
          row,
        );
        return { sql, queryType: "update", dml: true, keyed: true };
      }
      case "setDefault": {
        const { primaryKeys, row } = keyed(edit.key);
        const { sql } = adapter.buildSetDefaultSql(schema, table, edit.column, primaryKeys, row);
        return { sql, queryType: "update", dml: true, keyed: true };
      }
      case "insertRow": {
        const values = Object.fromEntries(edit.values.map(([c, v]) => [c, decodeCell(v)]));
        const { sql } = adapter.buildInsertSql(schema, table, values);
        return { sql, queryType: "insert", dml: true, keyed: false };
      }
      case "deleteRow": {
        const { primaryKeys, row } = keyed(edit.key);
        const { sql } = adapter.buildDeleteSql(schema, table, primaryKeys, row);
        return { sql, queryType: "delete", dml: true, keyed: true };
      }
      case "truncateTable": {
        const from = plainQualifiedTable(this.ctx.engine, schema, table);
        // SQLite has no TRUNCATE: a DELETE FROM, which counts as DML.
        return this.ctx.engine === "sqlite"
          ? { sql: `DELETE FROM ${from}`, queryType: "delete", dml: true, keyed: false }
          : { sql: `TRUNCATE TABLE ${from}`, queryType: "other", dml: false, keyed: false };
      }
      case "dropObject": {
        const keyword =
          edit.kind === "materializedView"
            ? "MATERIALIZED VIEW"
            : edit.kind === "view"
              ? "VIEW"
              : "TABLE";
        const from = plainQualifiedTable(this.ctx.engine, schema, table);
        return { sql: `DROP ${keyword} ${from}`, queryType: "other", dml: false, keyed: false };
      }
    }
  }

  async plan(params: PlanEditsParams): Promise<PlannedChange[]> {
    return params.edits.map((edit) => {
      const { sql, queryType, dml } = this.build(edit);
      const summary = changeSummary(sql, this.ctx.engine);
      return { sql, params: [], queryType, dml, ...(summary ? { summary } : {}) };
    });
  }

  /** A queue entry ready to run, or a `Refused`. */
  private ready(change: Change): Ready {
    if (change.type === "edit") return { id: change.id, ...this.build(change.edit) };
    const { engine } = this.ctx;
    let statements: number;
    let queryType: QueryType;
    try {
      statements = splitSqlStatementsOrThrow(change.sql, engine).length;
      queryType = detectQueryTypeOrThrow(change.sql, engine);
    } catch (error) {
      throw new Refused(INVALID_ARGUMENT, extractErrorMessage(error));
    }
    if (statements !== 1) {
      throw new Refused(INVALID_ARGUMENT, "A pending change must be exactly one statement");
    }
    const dml = queryType === "insert" || queryType === "update" || queryType === "delete";
    return { id: change.id, sql: change.sql, queryType, dml, keyed: false };
  }

  // -------- Applying --------

  async apply(params: ApplyChangesParams): Promise<ApplyOutcome> {
    const { connectionId, changes, confirmed, history } = params;
    const { engine, provider } = this.ctx;
    const mode: ApplyMode =
      changes.length === 1 ? "single" : changes.every((c) => this.isDml(c)) ? "atomic" : "inOrder";

    // Validation, before anything runs.
    const ready: Ready[] = [];
    for (const [index, change] of changes.entries()) {
      try {
        ready.push(this.ready(change));
      } catch (error) {
        const { code, message } =
          error instanceof Refused ? error : failureOf(error, INVALID_ARGUMENT);
        return applied(mode, 0, [], { id: change.id, index, code, message }, false, []);
      }
    }
    const destructive: DestructiveStatement[] = [];
    for (const [index, r] of ready.entries()) {
      let reason;
      try {
        reason = isDestructiveStatement(r.sql, engine);
      } catch (error) {
        const { message } = failureOf(error);
        return applied(
          mode,
          0,
          [],
          { id: r.id, index, code: INVALID_ARGUMENT, message },
          false,
          [],
        );
      }
      if (reason) destructive.push({ index, sql: r.sql, reason });
    }
    if (destructive.length > 0 && !confirmed) {
      return {
        outcome: "confirmRequired",
        destructive: destructive.slice(0, MAX_DESTRUCTIVE_LISTED),
        destructiveTotal: destructive.length,
      };
    }

    const ran: Array<{ sql: string; rows: number; elapsedMs: number }> = [];
    const results: ChangeResult[] = [];
    let failed: ApplyFailure | undefined;
    let ddl = false;

    if (mode === "atomic") {
      const started = performance.now();
      const counts: number[] = [];
      try {
        await provider.execute(connectionId, "BEGIN TRANSACTION");
      } catch (error) {
        failed = failureOf(error);
      }
      if (!failed) {
        for (const [index, r] of ready.entries()) {
          try {
            const { rowsAffected } = await provider.execute(connectionId, r.sql);
            if (r.keyed && rowsAffected === 0) {
              failed = noRowMatched(index, r.id);
              break;
            }
            counts.push(rowsAffected);
          } catch (error) {
            failed = { id: r.id, index, ...failureOf(error) };
            break;
          }
        }
        try {
          await provider.execute(connectionId, failed ? "ROLLBACK" : "COMMIT");
        } catch (error) {
          failed ??= failureOf(error);
        }
      }
      if (!failed) {
        const elapsedMs = round(performance.now() - started);
        ready.forEach((r, i) => ran.push({ sql: r.sql, rows: counts[i] ?? 0, elapsedMs }));
      }
    } else {
      for (const [index, r] of ready.entries()) {
        const started = performance.now();
        try {
          const { rowsAffected, lastInsertId } = await provider.execute(connectionId, r.sql);
          if (r.keyed && rowsAffected === 0) {
            failed = noRowMatched(index, r.id);
            break;
          }
          ddl ||= !r.dml;
          ran.push({
            sql: r.sql,
            rows: rowsAffected,
            elapsedMs: round(performance.now() - started),
          });
          results.push({
            id: r.id,
            rowsAffected,
            ...(lastInsertId === undefined ? {} : { lastInsertId }),
          });
        } catch (error) {
          failed = { id: r.id, index, ...failureOf(error) };
          break;
        }
      }
    }

    if (failed) void log.info(`Pending changes (demo) stopped: ${failed.code}`);
    const rows = history && ran.length > 0 ? await this.appendHistory(history, ran) : [];
    return applied(mode, ran.length, results, failed, ddl, rows);
  }

  private isDml(change: Change): boolean {
    try {
      return this.ready(change).dml;
    } catch {
      return false;
    }
  }

  private async appendHistory(
    ctx: HistoryContext,
    ran: Array<{ sql: string; rows: number; elapsedMs: number }>,
  ): Promise<PersistedQueryHistoryItem[]> {
    const stored: PersistedQueryHistoryItem[] = [];
    for (const r of ran) {
      const item: PersistedQueryHistoryItem = {
        id: `hist-${crypto.randomUUID()}`,
        query: r.sql,
        timestamp: new Date().toISOString(),
        executionTime: r.elapsedMs,
        rowCount: r.rows,
        connectionId: ctx.connectionId,
        favorite: false,
        connectionLabelsSnapshot: ctx.connectionLabels,
        connectionNameSnapshot: ctx.connectionName,
      };
      try {
        await this.ctx.appendHistory(item);
        stored.push(item);
      } catch (error) {
        void log.error(`Recording the apply in history failed: ${failureOf(error).code}`);
      }
    }
    return stored;
  }

  // -------- The data tab --------

  tablePage(params: TablePageParams, signal: AbortSignal): AsyncIterable<RunEvent> {
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
    const emit = (event: RunEvent) => {
      if (!stop.signal.aborted) queue.push(event);
    };
    this.pageOne(params, emit, stop.signal).then(
      () => queue.finish(),
      (error: unknown) => queue.pushError(errorEvent(error)),
    );
    return queue;
  }

  private async pageOne(
    p: TablePageParams,
    emit: (event: RunEvent) => void,
    stop: AbortSignal,
  ): Promise<void> {
    const { provider } = this.ctx;
    let page: { sql: string; params: unknown[] };
    let count: { sql: string; params: unknown[] };
    try {
      page = this.buildQuery(p.query, p.page, p.pageSize);
      count = this.buildCountQuery(p.query);
    } catch (error) {
      emit({ type: "error", code: INVALID_ARGUMENT, message: extractErrorMessage(error) });
      return;
    }
    const started = performance.now();
    emit({
      type: "statementStart",
      index: 0,
      sql: page.sql,
      source: { sql: page.sql, params: page.params.map(encodeParam) },
      queryType: "select",
      kind: "page",
      page: p.page,
      pageSize: p.pageSize,
    });

    let totalRows = 0;
    try {
      const counted = await provider.select<Record<string, unknown>>(
        p.connectionId,
        count.sql,
        count.params,
      );
      totalRows = Number(Object.values(counted[0] ?? {})[0] ?? 0);
    } catch {
      // Count query failed, proceed without total
    }
    if (stop.aborted) return;

    let rows: Record<string, unknown>[];
    try {
      rows = await provider.select<Record<string, unknown>>(p.connectionId, page.sql, page.params);
    } catch (error) {
      if (stop.aborted) return;
      const { code, message } = failureOf(error, QUERY_ERROR);
      const elapsedMs = round(performance.now() - started);
      emit({ type: "statementError", index: 0, code, message, elapsedMs });
      emit({ type: "done", statements: 1, succeeded: false });
      return;
    }
    if (stop.aborted) return;
    const columns = rows.length > 0 ? Object.keys(rows[0]) : [];
    emit({
      type: "batch",
      columns,
      rows: rows.map((row) => columns.map((c) => encodeParam(row[c]))),
      is_final: true,
    });
    emit({
      type: "statementDone",
      index: 0,
      elapsedMs: round(performance.now() - started),
      totalRows,
      totalPages: Math.max(1, Math.ceil(totalRows / p.pageSize)),
      countEstimated: false,
    });
    emit({ type: "done", statements: 1, succeeded: true });
  }

  /** The page's SELECT, with today's paging. */
  buildQuery(
    query: TableQuery,
    page: number,
    pageSize: number,
  ): { sql: string; params: unknown[] } {
    const { engine } = this.ctx;
    const from = plainQualifiedTable(engine, query.target.schema, query.target.table);
    const params: unknown[] = [];
    const base = `SELECT ${this.buildSelectClause(query)} FROM ${from}`;
    const where = this.buildWhere(query);

    let orderBy = "";
    if (query.sort.length > 0) {
      orderBy = ` ORDER BY ${query.sort.map((s) => `${this.qi(s.column)} ${s.direction}`).join(", ")}`;
    }

    const offset = (page - 1) * pageSize;
    let pagination: string;
    if (engine === "mssql") {
      if (!orderBy) orderBy = " ORDER BY (SELECT NULL)";
      pagination = ` OFFSET ${offset} ROWS FETCH NEXT ${pageSize} ROWS ONLY`;
    } else {
      pagination = ` LIMIT ${pageSize} OFFSET ${offset}`;
    }
    return { sql: `${base}${where}${orderBy}${pagination}`, params };
  }

  /** The count for the page's filters. */
  buildCountQuery(query: TableQuery): { sql: string; params: unknown[] } {
    const from = plainQualifiedTable(this.ctx.engine, query.target.schema, query.target.table);
    const params: unknown[] = [];
    const where = this.buildWhere(query);
    return { sql: `SELECT COUNT(*) FROM ${from}${where}`, params };
  }

  /**
   * ` WHERE …` from the filters (empty without any). Each value is compared
   * with the column cast to text. The demo inlines the values as escaped
   * literals: DuckDB-WASM's provider ignores bind values, so placeholders
   * would never be filled (the data tab's filters failed in the demo before
   * 5c). `IN`/`NOT IN` take each trimmed item; a list with no items throws.
   */
  private buildWhere(query: TableQuery): string {
    const filters = query.filters.filter((f) => f.column);
    if (filters.length === 0) return "";
    const { textType } = filterDialect(this.ctx.engine);
    const conditions = filters.map((f) => {
      const col = this.qi(f.column);
      if (f.op === "IS NULL") return `${col} IS NULL`;
      if (f.op === "IS NOT NULL") return `${col} IS NOT NULL`;
      if (f.op === "IN" || f.op === "NOT IN") {
        const items = inListItems(f.value);
        if (items.length === 0) {
          throw new Error(m.data_filter_in_empty({ operator: f.op, column: f.column }));
        }
        return `CAST(${col} AS ${textType}) ${f.op} (${items.map(formatLiteralValue).join(", ")})`;
      }
      return `CAST(${col} AS ${textType}) ${f.op} ${formatLiteralValue(f.value)}`;
    });
    return ` WHERE ${conditions.join(` ${query.logic} `)}`;
  }

  /**
   * The SELECT list: `*`, except on SQL Server for a table with columns
   * tiberius 0.12 can't read, which are listed and cast to NVARCHAR(MAX).
   */
  private buildSelectClause(query: TableQuery): string {
    if (this.ctx.engine !== "mssql") return "*";
    const columns = this.ctx.columnsOf?.(query.target.schema, query.target.table) ?? [];
    if (columns.length === 0) return "*";
    const hangs = (type: string) =>
      /^(sql_variant|geography|geometry|hierarchyid)$/i.test(type.trim());
    if (!columns.some((c) => hangs(c.type))) return "*";
    return columns
      .map((c) =>
        hangs(c.type)
          ? `CAST(${this.qi(c.name)} AS NVARCHAR(MAX)) AS ${this.qi(c.name)}`
          : this.qi(c.name),
      )
      .join(", ");
  }

  // -------- The DuckDB extensions tab --------

  /** The statements the tab ran before 5c, through the provider's `select` as `executeRaw` did. */
  async duckdbExtension(
    connectionId: string,
    action: ExtensionAction,
  ): Promise<Record<string, unknown>[] | null> {
    const { provider } = this.ctx;
    if (action.type === "list") {
      return await provider.select(connectionId, "SELECT * FROM duckdb_extensions()");
    }
    const { name } = action;
    if (!/^[A-Za-z0-9_]+$/.test(name)) {
      throw new CoreCallError({
        code: INVALID_ARGUMENT,
        message: `Invalid extension name: ${name}`,
      });
    }
    const sql = {
      install: `INSTALL '${name}'`,
      load: `LOAD '${name}'`,
      update: `UPDATE EXTENSIONS (${name})`,
      installCommunity: `INSTALL '${name}' FROM community; LOAD '${name}';`,
      installAndLoad: `INSTALL '${name}'; LOAD '${name}';`,
    }[action.type];
    await provider.select(connectionId, sql);
    return null;
  }
}

const round = (ms: number) => Math.round(ms * 100) / 100;

function noRowMatched(index: number, id: string): ApplyFailure {
  return {
    id,
    index,
    code: NO_ROWS_AFFECTED,
    message: `Change ${index + 1} matched no row. It may have been changed or deleted since it was loaded; refresh and try again.`,
  };
}

function applied(
  mode: ApplyMode,
  count: number,
  results: ChangeResult[],
  failed: ApplyFailure | undefined,
  ddl: boolean,
  history: PersistedQueryHistoryItem[],
): ApplyOutcome {
  return {
    outcome: "applied",
    mode,
    applied: count,
    results,
    ...(failed ? { failed } : {}),
    ddl,
    history,
  };
}
