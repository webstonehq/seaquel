import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
import type { DatabaseConnection, StatementResult, ParameterValue } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { QueryHistoryManager } from "./query-history.svelte.js";
import { columnSourcesFromRefs, sourceTableFromRef, type DestructiveStatement } from "$lib/sql";
import { m } from "$lib/paraglide/messages.js";
import type { ProviderRegistry } from "$lib/providers";
import { extractErrorMessage } from "$lib/errors";
import { log } from "$lib/utils/logger";
import type { PendingChangesManager } from "./pending-changes.svelte.js";
import { QueryCrudManager } from "./query-crud.svelte.js";
import { errorText } from "./error-text.js";
import { dedupeColumnNames, rowToObject } from "$lib/utils/row-access";
import { decodeRows, encodeParam } from "$lib/values";
import { licenseNudgeStore } from "$lib/stores/license-nudge.svelte.js";
import { CANCELLED } from "$lib/core/client";
import type { ParamValue } from "$lib/types/generated/ParamValue";
import type { RunTarget } from "$lib/types/generated/RunTarget";
import type { StatementKind } from "$lib/types/generated/StatementKind";
import {
  CONFIRM_REQUIRED,
  getQueryRunner,
  INVALID_PARAMETERS,
  type QueryRunner,
  type RunEvent,
  type RunParams,
} from "./query-runner/index.js";

/** How `execute` and `executeCurrent` run. */
export interface RunOptions {
  /** Rows per page; defaults to the tab's last SELECT result's, else 100. 0 streams. */
  pageSize?: number;
  /** The parameter dialog's values. Without them `{{name}}` goes to the database as typed. */
  params?: ParameterValue[];
  /** The user confirmed the run's destructive statements. */
  confirmed?: boolean;
  /**
   * The saved connection to run on instead of the active one: the rerun
   * after an edit goes to the connection the edited result came from.
   */
  connectionId?: string;
}

/** A run Core refused with `CONFIRM_REQUIRED`: the editor's destructive dialog shows it. */
export interface PendingConfirm {
  tabId: string;
  /** The first destructive statements, as Core lists them. */
  statements: DestructiveStatement[];
  /** How many the run holds, listed or not. */
  total: number;
  /** The saved connection and Core connection it was refused on; Confirm runs only there. */
  connectionId: string;
  providerConnectionId: string;
}

/**
 * One run or page on a tab. A tab has at most one: a new run cancels it, and
 * a page waits for no run (it's refused while one is going).
 */
interface Operation {
  controller: AbortController;
  projectId: string;
  mode: "run" | "page";
  /** Settles its results when it's cancelled or replaced. */
  consumer: Consumer | null;
}

/** What an operation's events are applied to. */
interface Consumer {
  op: Operation;
  tabId: string;
  connection: DatabaseConnection;
  /** A run appends results; a page rewrites the one at `position`. */
  mode: "run" | "page";
  target: RunTarget;
  pageSize: number;
  text: string;
  /** The result the latest `statementStart` is filling, if any. */
  position: number | null;
  kind: StatementKind | null;
  batched: boolean;
  startedAt: number;
  /** A run replaced the tab's results (its first result arrived). */
  fresh: boolean;
  deferred: number;
  /** The run asked Core to record history. */
  recording: boolean;
  /** Results whose statement started and hasn't finished or failed yet. */
  open: Set<number>;
  /** A page's page and size, applied with its first rows or its end. */
  pendingPage?: { page: number; pageSize: number } | null;
}

const round = (ms: number) => Math.round(ms * 100) / 100;

export { errorText } from "./error-text.js";

/**
 * The editor's query results as a view model over run events (phase 5b).
 *
 * `execute` and `executeCurrent` send the tab's text to a `QueryRunner`
 * (Core's `db.run` on desktop and web, the TypeScript runner in the demo),
 * and `goToPage`/`setPageSize` re-page one result through `db.page` with
 * the source its run sent. The runner plans, runs, times and records
 * history; this turns its events into the tab's results:
 *
 * - `statementStart` adds a result (a page rewrites its own);
 * - `batch` fills it, `statementDone` sets its totals and Core's time,
 *   `statementError` makes it an error result (a stream keeps its rows);
 * - `statementDeferred` goes to pending changes;
 * - `done` hides utility results unless every result is one, shows the
 *   deferred toast, and puts Core's history row in the cache;
 * - `error`: `CONFIRM_REQUIRED` goes to `pendingConfirm`,
 *   `INVALID_PARAMETERS` to a toast, `CANCELLED` ends quietly, anything
 *   else becomes an error result.
 *
 * A tab has one operation at a time: a new run or page on it, Stop, and
 * closing the tab cancel it in Core. Events are applied to the tab the
 * operation started on, looked up by id each time, and an operation's
 * events are dropped once it was replaced or cancelled.
 */
export class QueryExecutionManager {
  private readonly DEFAULT_PAGE_SIZE = 100;
  readonly crud: QueryCrudManager;

  /** The run Core refused until the user confirms; the editor shows it for its tab. */
  pendingConfirm = $state<PendingConfirm | null>(null);
  /** Reruns `pendingConfirm`'s run, confirmed. */
  private confirmRetry: (() => Promise<void>) | null = null;

  /** The operation running on each tab. */
  private readonly operations = new Map<string, Operation>();

  constructor(
    private state: DatabaseState,
    private queryHistory: QueryHistoryManager,
    private providers: ProviderRegistry,
    private pendingChanges: PendingChangesManager,
    private runnerFor: (connection: DatabaseConnection) => Promise<QueryRunner> = (connection) =>
      getQueryRunner(connection, state, providers),
  ) {
    this.crud = new QueryCrudManager(state, providers, pendingChanges);
  }

  // -------- Tabs and results, always through the `$state` proxy --------

  private findTab(projectId: string, tabId: string) {
    return this.state.queryTabsByProject[projectId]?.find((t) => t.id === tabId);
  }

  /**
   * Look up a StatementResult *through* the Svelte 5 `$state` proxy so that
   * mutations on the returned reference are tracked and fire reactivity.
   * Event handlers must use this helper — never mutate a raw seed reference,
   * because Svelte's proxy only tracks writes that go through it.
   */
  private getProxiedResult(
    projectId: string,
    tabId: string,
    position: number,
  ): StatementResult | undefined {
    return this.findTab(projectId, tabId)?.results?.[position];
  }

  /**
   * Update a query tab's state with proper Svelte 5 reactivity.
   */
  private updateQueryTabState(
    projectId: string,
    tabId: string,
    updates: Partial<{
      results: StatementResult[];
      activeResultIndex: number;
      isExecuting: boolean;
    }>,
  ): void {
    const tab = this.findTab(projectId, tabId);
    if (!tab) return;
    Object.assign(tab, updates);
    // Trigger Svelte 5 reactivity by reassigning the top-level object
    this.state.queryTabsByProject = { ...this.state.queryTabsByProject };
  }

  // -------- Operations --------

  /** Start an operation on `tabId`, cancelling the one running there. */
  private begin(tabId: string, projectId: string, mode: Operation["mode"]): Operation {
    const previous = this.operations.get(tabId);
    const op: Operation = { controller: new AbortController(), projectId, mode, consumer: null };
    this.operations.set(tabId, op);
    if (previous) {
      previous.controller.abort();
      // Its events are dropped from here on: settle what it showed now
      // (cancelled statements, utility results hidden), so nothing is left
      // spinning if the new run shows nothing (a refusal, say).
      if (previous.consumer) this.finish(previous.consumer);
    }
    return op;
  }

  /** Stop: cancel the tab's run or page in Core and stop its spinners now. */
  cancelStream(tabId: string): void {
    const op = this.operations.get(tabId);
    op?.controller.abort();
    const projectId = op?.projectId ?? this.state.activeProjectId;
    if (!projectId) return;
    for (const r of this.findTab(projectId, tabId)?.results ?? []) {
      if (r.isStreaming) r.isStreaming = false;
    }
  }

  /** The tab closed: cancel its operation and drop its pending confirmation. */
  forgetTab(tabId: string): void {
    this.operations.get(tabId)?.controller.abort();
    this.operations.delete(tabId);
    if (this.pendingConfirm?.tabId === tabId) this.clearPendingConfirm();
  }

  /**
   * A project's tabs are about to be replaced (reloaded from storage when
   * it becomes active again): cancel their operations and settle what they
   * showed, so none is left executing or holding the page guard.
   */
  cancelProject(projectId: string): void {
    for (const [tabId, op] of this.operations) {
      if (op.projectId !== projectId) continue;
      this.operations.delete(tabId);
      op.controller.abort();
      if (op.consumer) this.finish(op.consumer);
    }
  }

  /** A project was deleted: cancel every operation on its tabs. */
  forgetProject(projectId: string): void {
    for (const [tabId, op] of this.operations) {
      if (op.projectId === projectId) this.forgetTab(tabId);
    }
    this.forgetOrphans();
  }

  /**
   * Cancel the operations whose tab is gone however it went (a bulk close,
   * a project's tabs replaced), not only through `queryTabs.remove`.
   */
  forgetOrphans(): void {
    for (const [tabId, op] of this.operations) {
      if (!this.findTab(op.projectId, tabId)) this.forgetTab(tabId);
    }
    const pending = this.pendingConfirm;
    if (
      pending &&
      !Object.values(this.state.queryTabsByProject).some((tabs) =>
        tabs?.some((t) => t.id === pending.tabId),
      )
    ) {
      this.clearPendingConfirm();
    }
  }

  /** Another tab became active: a pending confirmation belongs to the one it came from. */
  activeTabChanged(tabId: string | null): void {
    if (this.pendingConfirm && this.pendingConfirm.tabId !== tabId) this.clearPendingConfirm();
  }

  /**
   * The user confirmed `pendingConfirm` on `tabId`: run it again, confirmed,
   * on the connection it was refused on. If the tab's connection changed
   * meanwhile (another connection made active, reconnected), nothing runs.
   */
  async confirmPending(tabId: string): Promise<void> {
    const pending = this.pendingConfirm;
    if (pending?.tabId !== tabId) return;
    const retry = this.confirmRetry;
    this.clearPendingConfirm();
    // The refused run's own connection, as it is now: removed, disconnected
    // or reconnected (a new Core id) means it isn't what was confirmed.
    const connection = this.state.connections.find((c) => c.id === pending.connectionId);
    if (!connection || connection.providerConnectionId !== pending.providerConnectionId) {
      errorToast(m.query_confirm_connection_changed());
      return;
    }
    await retry?.();
  }

  /** The user cancelled the destructive dialog. */
  clearPendingConfirm(): void {
    this.pendingConfirm = null;
    this.confirmRetry = null;
  }

  // -------- Running --------

  /** Run every statement in the tab. */
  execute(tabId: string, options: RunOptions = {}): Promise<void> {
    return this.run(tabId, { type: "all" }, options);
  }

  /** Run the statement at the cursor (a UTF-16 offset into the tab's text). */
  executeCurrent(tabId: string, cursorOffset: number, options: RunOptions = {}): Promise<void> {
    return this.run(tabId, { type: "current", cursor: cursorOffset }, options);
  }

  private async run(
    tabId: string,
    target: RunTarget,
    options: RunOptions,
    text?: string,
  ): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;
    const connection =
      options.connectionId === undefined
        ? this.state.activeConnection
        : this.state.connections.find((c) => c.id === options.connectionId);
    if (!connection?.providerConnectionId) {
      errorToast("Not connected to database. Please reconnect.");
      return;
    }
    const tab = this.findTab(projectId, tabId);
    if (!tab) return;
    if (this.pendingConfirm?.tabId === tabId) this.clearPendingConfirm();

    // The tab's last SELECT result's page size. Not an error or utility
    // result's: an error result's placeholder would contaminate re-runs.
    const previousSelect = tab.results?.find(
      (r) => !r.isError && !r.isUtility && r.queryType === "select",
    );
    const pageSize = options.pageSize ?? previousSelect?.pageSize ?? this.DEFAULT_PAGE_SIZE;

    let params: ParamValue[] | undefined;
    try {
      params = options.params?.map((p) => ({
        name: p.name,
        // Every substituter treats a Date as its ISO text.
        value: encodeParam(p.value instanceof Date ? p.value.toISOString() : p.value),
      }));
    } catch (error) {
      // An invalid Date: nothing runs, as for a value Core can't substitute.
      errorToast(extractErrorMessage(error));
      return;
    }

    const runText = text ?? tab.query;
    const deferWrites = this.pendingChanges.isEnabled();
    const runner = await this.runnerOrToast(connection);
    if (!runner) return;
    const request: RunParams = {
      connectionId: connection.providerConnectionId,
      streamId: crypto.randomUUID(),
      text: runText,
      target,
      pageSize,
      ...(params ? { params } : {}),
      ...(options.confirmed ? { confirmed: true } : {}),
      ...(deferWrites ? { deferWrites: true } : {}),
      history: this.queryHistory.contextFor(connection.id),
    };

    const op = this.begin(tabId, projectId, "run");
    this.updateQueryTabState(projectId, tabId, { isExecuting: true });
    const consumer: Consumer = {
      op,
      tabId,
      connection,
      mode: "run",
      target,
      pageSize,
      text: runText,
      position: null,
      kind: null,
      batched: false,
      startedAt: 0,
      fresh: false,
      deferred: 0,
      recording: true,
      open: new Set(),
    };
    op.consumer = consumer;
    // Confirm reruns on the connection it was refused on, whatever is active by then.
    await this.consume(consumer, runner.run(request, op.controller.signal), () =>
      this.run(
        tabId,
        target,
        { ...options, confirmed: true, connectionId: connection.id },
        runText,
      ),
    );
  }

  /**
   * Navigate to a specific page for a specific result.
   */
  async goToPage(tabId: string, page: number, resultIndex?: number): Promise<void> {
    const tab = this.activeTab(tabId);
    if (!tab?.results) return;
    const index = resultIndex ?? tab.activeResultIndex ?? 0;
    const result = tab.results[index];
    if (!result) return;
    const target = Math.max(1, Math.min(page, result.totalPages));
    await this.page(tabId, index, target, result.pageSize);
  }

  /**
   * Set page size and re-execute query.
   */
  async setPageSize(tabId: string, pageSize: number, resultIndex?: number): Promise<void> {
    const tab = this.activeTab(tabId);
    if (!tab?.results) return;
    await this.page(tabId, resultIndex ?? tab.activeResultIndex ?? 0, 1, pageSize);
  }

  private activeTab(tabId: string) {
    const projectId = this.state.activeProjectId;
    return projectId ? this.findTab(projectId, tabId) : undefined;
  }

  /** Re-page one result through `db.page`, with the source its run sent. */
  private async page(
    tabId: string,
    position: number,
    page: number,
    pageSize: number,
  ): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;
    // A page never cancels a run: the rest of it would silently not run.
    // The pagination controls are disabled while it goes.
    if (this.operations.get(tabId)?.mode === "run") return;
    const result = this.getProxiedResult(projectId, tabId, position);
    // The result's own connection, as it is now (a reconnect's new Core id).
    const connection = this.state.connections.find((c) => c.id === result?.connectionId);
    if (!connection?.providerConnectionId) return;
    // Only a SELECT pages (Core refuses anything else); an error result
    // that never started has no source.
    if (!result?.pageSource || result.queryType !== "select") return;
    const source = {
      sql: result.pageSource.sql,
      params: [...result.pageSource.params],
    };

    const runner = await this.runnerOrToast(connection);
    if (!runner) return;
    // A run may have started while the runner was looked up.
    if (this.operations.get(tabId)?.mode === "run") return;
    const op = this.begin(tabId, projectId, "page");
    this.updateQueryTabState(projectId, tabId, { isExecuting: true });
    const consumer: Consumer = {
      op,
      tabId,
      connection,
      mode: "page",
      target: { type: "all" },
      pageSize,
      text: source.sql,
      position,
      kind: null,
      batched: false,
      startedAt: 0,
      fresh: false,
      deferred: 0,
      recording: false,
      open: new Set(),
    };
    op.consumer = consumer;
    const events = runner.page(
      {
        connectionId: connection.providerConnectionId,
        streamId: crypto.randomUUID(),
        source,
        page,
        pageSize,
      },
      op.controller.signal,
    );
    await this.consume(consumer, events, null);
  }

  /**
   * The connection's runner, or `null` after an error toast (the demo's
   * provider failed to start, say). Nothing is executing yet when it fails.
   */
  private async runnerOrToast(connection: DatabaseConnection): Promise<QueryRunner | null> {
    try {
      return await this.runnerFor(connection);
    } catch (error) {
      errorToast(extractErrorMessage(error));
      return null;
    }
  }

  /**
   * Apply an operation's events to its tab until it ends. Events of an
   * operation that was replaced or cancelled, or whose tab closed, are
   * dropped; only a `done` still puts its history row in the cache (Core
   * stored it).
   */
  private async consume(
    c: Consumer,
    events: AsyncIterable<RunEvent>,
    retryConfirmed: (() => Promise<void>) | null,
  ): Promise<void> {
    const { op, tabId } = c;
    try {
      for await (const event of events) {
        if (!op.controller.signal.aborted && !this.findTab(op.projectId, tabId)) {
          // The tab closed: stop the run in Core.
          op.controller.abort();
        }
        if (op.controller.signal.aborted) {
          if (event.type === "done" && event.history) {
            this.queryHistory.insertRecorded(event.history);
          }
          continue;
        }
        this.apply(c, event, retryConfirmed);
      }
    } catch (error) {
      // A runner that threw instead of ending with an error event.
      if (!op.controller.signal.aborted) {
        this.apply(
          c,
          { type: "error", code: "UNKNOWN", message: extractErrorMessage(error) },
          null,
        );
      }
    } finally {
      if (this.operations.get(tabId) === op) {
        this.operations.delete(tabId);
        this.finish(c);
      }
    }
  }

  /**
   * The operation ended, or was cancelled or replaced: stop its spinners,
   * mark statements that never finished as cancelled, and, for a run, hide
   * utility results. Runs once per operation.
   */
  private finish(c: Consumer): void {
    if (c.op.consumer !== c) return;
    c.op.consumer = null;
    const { projectId } = c.op;
    const tab = this.findTab(projectId, c.tabId);
    if (!tab) return;
    for (const position of c.open) {
      const r = this.getProxiedResult(projectId, c.tabId, position);
      if (r) this.cancelled(c, r);
    }
    c.open.clear();
    for (const r of tab.results ?? []) {
      if (r.isStreaming) r.isStreaming = false;
    }
    if (c.mode === "run" && c.fresh) {
      this.updateQueryTabState(projectId, c.tabId, {
        results: this.filterAndIndexResults(tab.results ?? []),
        isExecuting: false,
      });
    } else {
      this.updateQueryTabState(projectId, c.tabId, { isExecuting: false });
    }
  }

  /**
   * A statement that never finished. A stream keeps the rows it got (as a
   * stopped stream always did); a page that got nothing new keeps the rows
   * it showed; anything else says it was cancelled rather than show an
   * empty result as if it had run.
   */
  private cancelled(c: Consumer, r: StatementResult): void {
    r.isStreaming = false;
    if (c.mode === "page" && c.kind !== "stream") return;
    if (c.kind === "stream" && r.rows.length > 0) return;
    const text = m.query_statement_cancelled();
    Object.assign(r, {
      columns: ["Error"],
      rows: [[text]],
      rowCount: 1,
      totalRows: 1,
      totalPages: 1,
      error: text,
      isError: true,
    });
  }

  /** Append a result to a run's tab; the run's first result replaces the tab's old ones. */
  private append(c: Consumer, result: StatementResult): number {
    const { projectId } = c.op;
    const tab = this.findTab(projectId, c.tabId);
    if (!tab) return -1;
    const results = c.fresh ? [...(tab.results ?? []), result] : [result];
    if (!c.fresh) {
      c.fresh = true;
      this.updateQueryTabState(projectId, c.tabId, { results, activeResultIndex: 0 });
    } else {
      this.updateQueryTabState(projectId, c.tabId, { results });
    }
    return results.length - 1;
  }

  private current(c: Consumer): StatementResult | undefined {
    if (c.position === null || c.position < 0) return undefined;
    return this.getProxiedResult(c.op.projectId, c.tabId, c.position);
  }

  private schemas(c: Consumer) {
    return this.state.schemas[c.connection.id] ?? [];
  }

  private apply(c: Consumer, event: RunEvent, retryConfirmed: (() => Promise<void>) | null): void {
    switch (event.type) {
      case "statementStart": {
        c.kind = event.kind;
        c.batched = false;
        c.startedAt = performance.now();
        const fields = {
          queryType: event.queryType,
          kind: event.kind,
          sourceTable: sourceTableFromRef(event.table, this.schemas(c)),
          columnSources: columnSourcesFromRefs(event.columnRefs, this.schemas(c)),
          page: event.page,
          pageSize: event.pageSize,
          totalPages: 1,
          isError: false,
          isStreaming: true,
        };
        if (c.mode === "page") {
          const target = this.current(c);
          if (!target || c.position === null) return;
          c.open.add(c.position);
          c.pendingPage = { page: event.page, pageSize: event.pageSize };
          // A page keeps the rows, page and page size it shows until the new
          // rows arrive, so a page cancelled before then leaves them as they
          // were; a stream starts empty.
          target.isStreaming = true;
          if (event.kind === "stream") {
            Object.assign(target, fields, {
              error: undefined,
              countEstimated: undefined,
              columns: [],
              rows: [],
              rowCount: 0,
              totalRows: 0,
            });
            c.pendingPage = null;
          }
          return;
        }
        c.position = this.append(c, {
          ...fields,
          columns: [],
          rows: [],
          rowCount: 0,
          totalRows: 0,
          executionTime: 0,
          statementIndex: event.index,
          statementSql: event.sql,
          connectionId: c.connection.id,
          pageSource: { sql: event.source.sql, params: event.source.params },
        });
        if (c.position >= 0) c.open.add(c.position);
        return;
      }
      case "batch": {
        const target = this.current(c);
        if (!target) return;
        this.applyPendingPage(c, target);
        const streaming = c.kind === "stream";
        if (event.columns && (!streaming || target.columns.length === 0)) {
          // Duplicate names (`SELECT a.id, b.id`) break every
          // `columns.indexOf(name)` downstream: `id`, `id_2`.
          target.columns = dedupeColumnNames(event.columns);
        }
        const rows = decodeRows(event.rows);
        if (streaming) {
          // `.push` on the proxied array goes through Svelte's array trap:
          // O(batch) per call, where a spread reassignment is O(N²) over
          // the stream.
          for (let i = 0; i < rows.length; i += 10_000) {
            target.rows.push(...rows.slice(i, i + 10_000));
          }
          target.totalRows = target.rows.length;
        } else {
          target.rows = rows;
        }
        target.rowCount = target.rows.length;
        c.batched = true;
        // A live counter until Core's time arrives with `statementDone`.
        target.executionTime = round(performance.now() - c.startedAt);
        return;
      }
      case "statementDone": {
        const target = this.current(c);
        if (!target) return;
        this.applyPendingPage(c, target);
        if (c.position !== null) c.open.delete(c.position);
        target.executionTime = event.elapsedMs;
        target.isStreaming = false;
        target.totalPages = event.totalPages;
        switch (c.kind) {
          case "page":
            target.totalRows = event.totalRows;
            target.countEstimated = event.countEstimated;
            break;
          case "write": {
            const affected = event.rowsAffected ?? 0;
            Object.assign(target, {
              columns: ["Result"],
              rows: [[`${affected} row(s) affected`]],
              rowCount: 1,
              totalRows: 1,
              affectedRows: affected,
              lastInsertId: event.lastInsertId,
              pageSize: 1,
            });
            break;
          }
          case "utility":
            // Rows when it returned columns; otherwise a
            // utility result, hidden when another result shows.
            if (c.batched) {
              target.totalRows = event.totalRows;
            } else {
              Object.assign(target, { isUtility: true, totalRows: 0, rowCount: 0 });
            }
            break;
          default:
            target.totalRows = event.totalRows;
        }
        c.position = c.mode === "page" ? c.position : null;
        return;
      }
      case "statementError": {
        if (event.sql !== undefined) {
          // A planned failure (a value that can't be substituted in run
          // all): no `statementStart`, nothing ran.
          // Nothing to substitute keeps the bare message, as the grid always showed it.
          const text =
            event.code === INVALID_PARAMETERS
              ? event.message
              : errorText(event.code, event.message);
          this.append(c, this.errorResult(c, event.sql, text, event.index));
          return;
        }
        const target = this.current(c);
        if (!target) return;
        if (c.position !== null) c.open.delete(c.position);
        this.fail(c, target, errorText(event.code, event.message), event.elapsedMs);
        if (c.mode === "run") c.position = null;
        return;
      }
      case "statementDeferred": {
        this.pendingChanges.addSql(
          c.connection.id,
          event.source.sql,
          event.source.params,
          event.queryType,
          "query-editor",
          c.tabId,
        );
        c.deferred += 1;
        return;
      }
      case "done": {
        if (c.mode === "page") return;
        if (event.statements === 0) toast.info(m.query_no_executable_statements());
        if (c.deferred > 0) {
          const n = c.deferred;
          toast.info(
            c.target.type === "current"
              ? "Statement added to pending changes"
              : `${n} statement${n > 1 ? "s" : ""} added to pending changes`,
          );
          this.pendingChanges.openSheet();
        }
        if (c.target.type === "all" && !c.fresh) {
          // Run all with nothing shown (no statements, or all deferred).
          this.updateQueryTabState(c.op.projectId, c.tabId, { results: [], activeResultIndex: 0 });
        }
        if (event.history) this.queryHistory.insertRecorded(event.history);
        if (event.succeeded && c.recording) licenseNudgeStore.recordQuery();
        void log.info(
          `Query run on ${c.connection.id}: ${event.statements} statements, ${event.succeeded ? "succeeded" : "not all succeeded"}`,
        );
        return;
      }
      case "error": {
        if (event.code === CANCELLED) return;
        if (event.code === CONFIRM_REQUIRED && retryConfirmed) {
          this.pendingConfirm = {
            tabId: c.tabId,
            statements: event.destructive ?? [],
            total: event.destructiveTotal ?? event.destructive?.length ?? 0,
            connectionId: c.connection.id,
            providerConnectionId: c.connection.providerConnectionId ?? "",
          };
          this.confirmRetry = retryConfirmed;
          return;
        }
        if (event.code === INVALID_PARAMETERS && c.mode === "run") {
          errorToast(event.message);
          return;
        }
        void log.warn(`Query run on ${c.connection.id} failed: ${event.code}`);
        const text = errorText(event.code, event.message);
        const target = this.current(c);
        if (target) {
          if (c.position !== null) c.open.delete(c.position);
          this.fail(c, target, text, round(performance.now() - c.startedAt));
        } else if (c.mode === "run") {
          this.append(c, this.errorResult(c, c.text, text, 0));
        }
        return;
      }
    }
  }

  /** A page's new page number and size, once its rows (or its end) arrive. */
  private applyPendingPage(c: Consumer, target: StatementResult): void {
    if (!c.pendingPage) return;
    Object.assign(target, c.pendingPage, {
      queryType: "select",
      kind: c.kind ?? undefined,
      error: undefined,
      isError: false,
      countEstimated: undefined,
    });
    c.pendingPage = null;
  }

  /** `target` failed: a stream keeps the rows it got; anything else shows the error. */
  private fail(c: Consumer, target: StatementResult, error: string, elapsedMs: number): void {
    target.isStreaming = false;
    target.executionTime = elapsedMs;
    if (c.kind === "stream") {
      Object.assign(target, { isError: true, error, totalRows: target.rows.length });
      return;
    }
    Object.assign(target, {
      columns: ["Error"],
      rows: [[error]],
      rowCount: 1,
      totalRows: 1,
      totalPages: 1,
      affectedRows: undefined,
      lastInsertId: undefined,
      error,
      isError: true,
    });
  }

  /**
   * Create a standardized error result for failed statement execution.
   */
  private errorResult(
    c: Consumer,
    statementSql: string,
    error: string,
    statementIndex: number,
  ): StatementResult {
    return {
      columns: ["Error"],
      rows: [[error]],
      rowCount: 1,
      totalRows: 1,
      executionTime: 0,
      page: 1,
      pageSize: c.pageSize,
      totalPages: 1,
      statementIndex,
      statementSql,
      connectionId: c.connection.id,
      error,
      isError: true,
    };
  }

  /**
   * Filter out utility results and re-index for display.
   * Keeps utility results only if ALL results are utility (so the user sees something).
   */
  private filterAndIndexResults(allResults: StatementResult[]): StatementResult[] {
    const displayResults = allResults.filter((r) => !r.isUtility);
    const results = displayResults.length > 0 ? displayResults : allResults;
    return results.map((r, idx) => ({ ...r, statementIndex: idx }));
  }

  /**
   * Set the active result tab index.
   */
  setActiveResult(tabId: string, resultIndex: number): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;
    const tab = this.findTab(projectId, tabId);
    if (!tab?.results || resultIndex < 0 || resultIndex >= tab.results.length) return;
    this.updateQueryTabState(projectId, tabId, { activeResultIndex: resultIndex });
  }

  /**
   * Update a cell value in the database (query tab version).
   * Resolves the row from tab results, then delegates to crud.updateCellDirect.
   */
  async updateCell(
    tabId: string,
    resultIndex: number,
    rowIndex: number,
    column: string,
    newValue: unknown,
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
  ): Promise<{ success: boolean; error?: string; queued?: boolean }> {
    const editTarget = this.resolveEditTarget(tabId, resultIndex, rowIndex, column, sourceTable);
    if (!editTarget) return { success: false, error: "Row not found" };
    if (editTarget.error) return { success: false, error: editTarget.error };
    const { connectionId } = editTarget;
    if (!connectionId) return { success: false, error: "No connection established" };

    void log.debug(`Cell update on ${connectionId}`);
    const result = await this.crud.updateCellDirect(
      connectionId,
      editTarget.sourceTable,
      editTarget.row,
      editTarget.column,
      newValue,
    );
    if (result.success) {
      // Write the new value back into the columnar store so the UI reflects
      // the edit. `row` above is a disposable materialized copy — updating
      // it alone wouldn't propagate. We replace the whole inner row array
      // rather than mutating by index: streamed rows arrive from the Tauri
      // Channel as plain arrays and are pushed into the proxied outer array,
      // so we can't assume the inner array is deeply-tracked. Replacing the
      // slot fires the outer array's set trap, which reliably re-renders
      // the affected row.
      const tabs = this.state.queryTabsByProject[this.state.activeProjectId!] ?? [];
      const tab = tabs.find((t) => t.id === tabId);
      const target = tab?.results?.[resultIndex];
      if (target) {
        const colIdx = target.columns.indexOf(column);
        const existing = target.rows[rowIndex];
        if (colIdx !== -1 && existing) {
          const next = existing.slice();
          next[colIdx] = newValue;
          target.rows[rowIndex] = next;
        }
      }
    }
    return result;
  }

  /**
   * Set a cell value to its column DEFAULT (query tab version).
   * Resolves the row from tab results, then delegates to crud.setCellDefaultDirect.
   */
  async setCellDefault(
    tabId: string,
    resultIndex: number,
    rowIndex: number,
    column: string,
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
  ): Promise<{ success: boolean; error?: string; queued?: boolean }> {
    const editTarget = this.resolveEditTarget(tabId, resultIndex, rowIndex, column, sourceTable);
    if (!editTarget) return { success: false, error: "Row not found" };
    if (editTarget.error) return { success: false, error: editTarget.error };
    const { connectionId } = editTarget;
    if (!connectionId) return { success: false, error: "No connection established" };

    void log.debug(`Cell set default on ${connectionId}`);
    const result = await this.crud.setCellDefaultDirect(
      connectionId,
      editTarget.sourceTable,
      editTarget.row,
      editTarget.column,
    );
    if (result.success && !result.queued) {
      // Re-fetch the row to get the actual default value, from the
      // connection the result came from.
      await this.execute(tabId, { connectionId });
    }
    return result;
  }

  /**
   * Delete a row of a query tab's result, on the connection the result came
   * from. The key is read through the result's column sources, as a cell
   * edit's is, so an aliased key column (`SELECT id AS order_id`) binds.
   */
  async deleteRowAt(
    tabId: string,
    resultIndex: number,
    rowIndex: number,
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
  ): Promise<{ success: boolean; error?: string; queued?: boolean }> {
    const tabs = this.state.queryTabsByProject[this.state.activeProjectId!] ?? [];
    const result = tabs.find((t) => t.id === tabId)?.results?.[resultIndex];
    if (!result) return { success: false, error: "Row not found" };
    // Any column that comes from the table routes to it; without one, the
    // fallback keys the row by display names.
    const fromTable = result.columns.find((_, i) => {
      const s = result.columnSources?.[i];
      return s?.schema === sourceTable.schema && s.table === sourceTable.name;
    });
    const editTarget = this.resolveEditTarget(
      tabId,
      resultIndex,
      rowIndex,
      fromTable ?? result.columns[0] ?? "",
      sourceTable,
    );
    if (!editTarget) return { success: false, error: "Row not found" };
    if (editTarget.error) return { success: false, error: editTarget.error };
    const { connectionId } = editTarget;
    if (!connectionId) return { success: false, error: "No connection established" };
    return await this.crud.deleteRow(connectionId, editTarget.sourceTable, editTarget.row);
  }

  // --- Delegated CRUD methods: each takes the saved connection the row came from ---

  insertRow(
    connectionId: string,
    sourceTable: { schema: string; name: string },
    values: Record<string, unknown>,
  ) {
    return this.crud.insertRow(connectionId, sourceTable, values);
  }

  deleteRow(
    connectionId: string,
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
    row: Record<string, unknown>,
  ) {
    return this.crud.deleteRow(connectionId, sourceTable, row);
  }

  updateCellDirect(
    connectionId: string,
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
    row: Record<string, unknown>,
    column: string,
    newValue: unknown,
  ) {
    return this.crud.updateCellDirect(connectionId, sourceTable, row, column, newValue);
  }

  setCellDefaultDirect(
    connectionId: string,
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
    row: Record<string, unknown>,
    column: string,
  ) {
    return this.crud.setCellDefaultDirect(connectionId, sourceTable, row, column);
  }

  /** See `QueryCrudManager.executeReadOnly`: AI, dashboard and workflow SQL only. */
  executeReadOnly(
    connectionId: string,
    sql: string,
    signal?: AbortSignal,
    connectionName?: string,
    maxRows?: number,
  ) {
    return this.crud.executeReadOnly(connectionId, sql, signal, connectionName, maxRows);
  }

  /** The sidebar's DROP: see `QueryCrudManager.dropObject`. */
  dropObject(
    connectionId: string,
    table: { schema: string; name: string },
    kind: "table" | "view" | "materializedView",
  ) {
    return this.crud.dropObject(connectionId, table, kind);
  }

  /** The sidebar's TRUNCATE: see `QueryCrudManager.truncateTable`. */
  truncateTable(connectionId: string, table: { schema: string; name: string }) {
    return this.crud.truncateTable(connectionId, table);
  }

  /**
   * Resolve the actual UPDATE/SET-DEFAULT target for a cell edit.
   *
   * The user edits a cell under its *display* column name, which may have been
   * dedupe-suffixed (e.g. `id_2` for the second `id` in a JOIN'd result) or
   * aliased via `SELECT x AS y`. We use the query's `columnSources` to route
   * the edit to the underlying table+column, and to reshape the row into a
   * `Record<actualPKName, value>` that the WHERE-clause builder can consume
   * directly.
   *
   * Falls back to the caller-supplied `sourceTable` and literal `column` name
   * when per-column info is unavailable (unparseable query, `SELECT *`, etc.),
   * preserving the pre-existing single-table behavior.
   */
  private resolveEditTarget(
    tabId: string,
    resultIndex: number,
    rowIndex: number,
    column: string,
    fallbackSourceTable: { schema: string; name: string; primaryKeys: string[] },
  ):
    | {
        sourceTable: { schema: string; name: string; primaryKeys: string[] };
        row: Record<string, unknown>;
        column: string;
        /** The saved connection the result came from. */
        connectionId: string | undefined;
        error?: string;
      }
    | undefined {
    const tabs = this.state.queryTabsByProject[this.state.activeProjectId!] ?? [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results || resultIndex >= tab.results.length) return undefined;
    const result = tab.results[resultIndex];
    const rawRow = result.rows[rowIndex];
    if (!rawRow) return undefined;
    const { connectionId } = result;

    // No per-column info parsed — fall back to single-table routing, keyed by
    // display names (which is what the WHERE builder has always seen).
    const colIdx = result.columns.indexOf(column);
    const sources = result.columnSources;
    if (!sources || colIdx === -1 || !sources[colIdx]) {
      return {
        sourceTable: fallbackSourceTable,
        row: rowToObject(rawRow, result.columns),
        column,
        connectionId,
      };
    }

    const src = sources[colIdx]!;
    // Pull the target table's PK values out of the row by scanning for any
    // output columns that map back to one of its PK columns. A query that
    // doesn't project all of the target table's PKs can't be updated safely —
    // we'd have no way to identify the specific row in a WHERE clause.
    const rowObj: Record<string, unknown> = {};
    const foundPks = new Set<string>();
    for (let i = 0; i < result.columns.length; i++) {
      const s = sources[i];
      if (!s) continue;
      if (s.schema !== src.schema || s.table !== src.table) continue;
      if (src.primaryKeys.includes(s.column)) {
        rowObj[s.column] = rawRow[i];
        foundPks.add(s.column);
      }
    }
    const missingPks = src.primaryKeys.filter((pk) => !foundPks.has(pk));
    if (missingPks.length > 0) {
      return {
        sourceTable: fallbackSourceTable,
        row: rowToObject(rawRow, result.columns),
        column,
        connectionId,
        error: `Cannot edit ${src.table}.${src.column}: primary key ${missingPks.join(", ")} is not in the result`,
      };
    }

    return {
      sourceTable: {
        schema: src.schema,
        name: src.table,
        primaryKeys: src.primaryKeys,
      },
      row: rowObj,
      column: src.column,
      connectionId,
    };
  }
}
