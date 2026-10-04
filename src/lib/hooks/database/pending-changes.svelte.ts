import type {
  DatabaseConnection,
  DatabaseType,
  PendingChange,
  PendingChangeOrigin,
  PendingChangeTarget,
  QueryHistoryItem,
} from "$lib/types";
import { detectQueryType, isDestructiveStatement, type QueryType } from "$lib/sql";
import type { DatabaseState } from "./state.svelte.js";
import type { QueryHistoryManager } from "./query-history.svelte.js";
import type { ProviderRegistry } from "$lib/providers";
import { describeChange, describePendingChange } from "./pending-change-description.js";
import { pendingChangesSettingsStore } from "$lib/stores/pending-changes-settings.svelte.js";
import { log } from "$lib/utils/logger";
import { toast } from "svelte-sonner";
import { m } from "$lib/paraglide/messages.js";
import { cellKey, decodeCell } from "$lib/values";
import { noRowMatchedMessage } from "./stale-edit.js";
import { callError, errorText } from "./error-text.js";
import {
  getEditService,
  NO_ROWS_AFFECTED,
  type Change,
  type Edit,
  type EditService,
  type PlannedChange,
} from "./edit-service/index.js";
import type { ApplyMode } from "$lib/types/generated/ApplyMode";
import type { DestructiveStatement } from "$lib/types/generated/DestructiveStatement";

/** What applying a connection's queue came to, for the sheet. */
export type ApplyResult =
  /** Nothing was queued. */
  | { kind: "empty" }
  /** Every change applied; the queue is cleared. */
  | { kind: "applied"; applied: number; mode: ApplyMode; ddl: boolean }
  /**
   * A change failed or was refused. After an atomic failure or a refusal
   * nothing applied and the queue is whole; after an in-order failure the
   * applied ones left the queue and the failed one and the rest stay.
   */
  | {
      kind: "failed";
      applied: number;
      mode: ApplyMode;
      ddl: boolean;
      /** The change that failed, when it's known. */
      changeId?: string;
      /** Its position in what was sent. */
      index?: number;
      error: string;
    }
  /** The batch holds destructive statements: confirm, then apply again with `confirmed`. */
  | { kind: "confirmRequired"; destructive: DestructiveStatement[]; total: number }
  /** Core refused the call before anything ran; the queue is whole. */
  | { kind: "refused"; error: string }
  /**
   * The apply ended without an answer, whatever its mode: some changes (an
   * atomic batch: all of them, if it committed before the answer was lost)
   * may have run. The queue is kept and marked, and the schema and data
   * tabs reload.
   */
  | { kind: "interrupted"; error: string };

/**
 * The queue's destructive statements for the sheet's confirm dialog, found
 * by Core's check (`$lib/sql`, the same `destructive_reason`) in the SQL
 * each change runs. `null` when the connection's engine is unknown or the
 * check failed: the dialog then lists nothing and lets Core decide.
 */
export function listDestructive(
  changes: readonly PendingChange[],
  engine: DatabaseType | undefined,
): DestructiveStatement[] | null {
  if (!engine) return null;
  try {
    return changes.flatMap((change, index) => {
      const reason = isDestructiveStatement(change.sql, engine);
      return reason ? [{ index, sql: change.sql, reason }] : [];
    });
  } catch {
    return null;
  }
}

/**
 * Whether the sheet's apply goes `confirmed` (Decision 7): only when the user
 * saw destructive statements listed, the dialog's own (`listDestructive`) or
 * the ones Core asked about. With nothing listed it goes unconfirmed, so a
 * destructive statement the list missed makes Core ask instead of running.
 */
export function confirmedFor(
  listed: readonly DestructiveStatement[] | null,
  coreAsked: boolean,
): boolean {
  return coreAsked || (listed?.length ?? 0) > 0;
}

/** Core's code for an apply stopped because the web server closed the workspace. */
const WORKSPACE_CLOSED = "WORKSPACE_CLOSED";

/**
 * Core's code for a connection lost mid-apply (an engine out of process
 * stopped): the change in flight may have committed. Only the terminal
 * binaries' DuckDB helper gives it today, but any connection may.
 */
const CONNECTION_CLOSED = "CONNECTION_CLOSED";

/** Codes of a rejected apply that says nothing about what ran: no answer came. */
const NO_ANSWER = new Set([
  "NETWORK_ERROR",
  "PROTOCOL_ERROR",
  "UNKNOWN",
  "CANCELLED",
  "WS_CLOSED",
  CONNECTION_CLOSED,
]);

/** An entry as it was sent: its id and its change, by value. */
function fingerprint(change: PendingChange): string {
  return JSON.stringify([change.id, change.change]);
}

function noAnswer(code: string): boolean {
  return NO_ANSWER.has(code) || /^HTTP_5\d\d$/.test(code);
}

/** Reloads a connection's schema and its data tabs after an apply (set by `UseDatabase`). */
export interface ApplyEffects {
  reloadSchema(connectionId: string): Promise<void>;
  refreshDataTabs(connectionId: string): Promise<void>;
}

/**
 * The pending-changes queue: UI state, per saved connection (phase 5c,
 * Q1). Entries hold what applying sends back (`change`: an edit intent or
 * typed SQL) and what the sheet shows. `apply` sends the queue to the
 * connection's `EditService` (`db.applyChanges` on desktop and web), which
 * validates, classifies and runs it; this keeps the queue in step with the
 * outcome and puts Core's history rows in the cache.
 */
export class PendingChangesManager {
  private effects: ApplyEffects | null = null;

  constructor(
    private state: DatabaseState,
    private providers: ProviderRegistry,
    private queryHistory: QueryHistoryManager,
    private editServiceFor: (connection: DatabaseConnection) => Promise<EditService> = (c) =>
      getEditService(c, state, providers),
  ) {}

  setEffects(effects: ApplyEffects): void {
    this.effects = effects;
  }

  isEnabled(): boolean {
    return pendingChangesSettingsStore.enabled;
  }

  private queue(connectionId: string): PendingChange[] {
    return this.state.pendingChangesByConnection[connectionId] ?? [];
  }

  private setQueue(connectionId: string, changes: PendingChange[]): void {
    this.state.pendingChangesByConnection = {
      ...this.state.pendingChangesByConnection,
      [connectionId]: changes,
    };
  }

  private push(change: PendingChange): void {
    const existing = this.queue(change.connectionId);
    this.setQueue(change.connectionId, [...existing, change]);
    if (existing.length === 0) this.openSheet();
  }

  /**
   * Queue a planned edit. An update or Set default of a cell that is already
   * queued replaces that entry in place (its id and position stay), holding
   * the new change whole: its intent, SQL, binds, origin and target.
   */
  addPlanned(
    connectionId: string,
    edit: Edit,
    planned: PlannedChange,
    origin: PendingChangeOrigin,
    target?: PendingChangeTarget,
  ): void {
    const existing =
      (edit.type === "updateCell" || edit.type === "setDefault") && target?.primaryKeyValues
        ? this.findForCell(
            connectionId,
            target.schema,
            target.table,
            edit.column,
            target.primaryKeyValues,
          )
        : undefined;
    const id = existing?.id ?? crypto.randomUUID();
    const entry: PendingChange = {
      id,
      connectionId,
      change: { type: "edit", id, edit },
      sql: planned.sql,
      bindValues: planned.params.length > 0 ? planned.params.map(decodeCell) : undefined,
      queryType: planned.queryType,
      dml: planned.dml,
      addedAt: new Date(),
      description: describeChange(planned.summary, planned.sql, origin, planned.queryType),
      origin,
      target,
    };
    if (existing) {
      this.setQueue(
        connectionId,
        this.queue(connectionId).map((c) => (c.id === existing.id ? entry : c)),
      );
      return;
    }
    this.push(entry);
  }

  /**
   * Queue typed SQL: a statement the editor deferred (`params` in the cell
   * wire format, as Core sent them) or one the table editor generated.
   */
  addSql(
    connectionId: string,
    sql: string,
    params: unknown[],
    queryType: QueryType,
    origin: PendingChangeOrigin,
    sourceTabId?: string,
  ): void {
    const engine = this.state.connections.find((c) => c.id === connectionId)?.type ?? "postgres";
    const id = crypto.randomUUID();
    this.push({
      id,
      connectionId,
      change: { type: "sql", id, sql, params },
      sql,
      // `decodeCell` copies: `params` stays the wire values apply sends back.
      bindValues: params.length > 0 ? params.map(decodeCell) : undefined,
      queryType,
      dml: queryType === "insert" || queryType === "update" || queryType === "delete",
      addedAt: new Date(),
      description: describePendingChange(sql, origin, engine),
      sourceTabId,
      origin,
    });
  }

  /**
   * Queue a history row recorded with values (an applied grid edit) to run
   * again: its SQL with those binds, as typed SQL, and open the sheet. The
   * editor can't run it (its runs only fill `{{param}}`s), and going through
   * the queue keeps the sheet's review, its destructive-statement dialog and
   * Core's `confirmRequired`, and records the run in history again.
   */
  addFromHistory(item: QueryHistoryItem): void {
    const connection = this.state.connections.find((c) => c.id === item.connectionId);
    const params = item.params ?? [];
    // A second click on the same row: it's already waiting.
    const key = JSON.stringify([item.query, params]);
    const queued = this.queue(item.connectionId).some(
      (c) =>
        c.origin === "history" &&
        c.change.type === "sql" &&
        JSON.stringify([c.change.sql, c.change.params]) === key,
    );
    if (!queued) {
      const engine = connection?.type;
      const queryType = engine ? detectQueryType(item.query, engine) : "other";
      this.addSql(item.connectionId, item.query, params, queryType, "history");
    }
    // The sheet shows this connection's queue until it closes, whatever tab is focused.
    this.state.pendingFocusConnectionId = item.connectionId;
    this.openSheet();
    const name = connection?.name ?? item.connectionNameSnapshot;
    toast.info(
      queued
        ? m.history_rerun_already_queued({ connection: name })
        : m.history_rerun_queued({ connection: name }),
    );
  }

  /** Find a queued update or Set default of the same cell (same table, column, key values). */
  findForCell(
    connectionId: string,
    schema: string,
    table: string,
    column: string,
    primaryKeyValues: Record<string, unknown>,
  ): PendingChange | undefined {
    return this.queue(connectionId).find((c) => {
      if (c.origin !== "inline-edit" && c.origin !== "set-default") return false;
      const t = c.target;
      if (!t || t.schema !== schema || t.table !== table || t.column !== column) return false;
      if (!t.primaryKeyValues) return false;
      const keys = Object.keys(primaryKeyValues);
      if (keys.length !== Object.keys(t.primaryKeyValues).length) return false;
      return keys.every((pk) => cellKey(t.primaryKeyValues![pk]) === cellKey(primaryKeyValues[pk]));
    });
  }

  remove(connectionId: string, changeId: string): void {
    this.setQueue(
      connectionId,
      this.queue(connectionId).filter((c) => c.id !== changeId),
    );
  }

  clear(connectionId: string): void {
    this.setQueue(connectionId, []);
    this.setInterrupted(connectionId, false);
  }

  private setInterrupted(connectionId: string, value: boolean): void {
    if (!!this.state.pendingChangesInterrupted[connectionId] === value) return;
    this.state.pendingChangesInterrupted = {
      ...this.state.pendingChangesInterrupted,
      [connectionId]: value,
    };
  }

  /**
   * Apply the connection's queue as it is now (`db.applyChanges`, Decision
   * 5), with history. Keeps the queue in step with the outcome (see
   * `ApplyResult`), puts the history rows Core appended in the cache, and
   * reloads the schema (after DDL) and the connection's data tabs (after
   * anything ran, or an apply that ended without an answer).
   *
   * @param confirmed The user confirmed the destructive statements.
   */
  async apply(connectionId: string, confirmed = false): Promise<ApplyResult> {
    const sent = this.queue(connectionId);
    if (sent.length === 0) return { kind: "empty" };
    const lookUp = () => this.state.connections.find((c) => c.id === connectionId);
    const connection = lookUp();
    if (!connection?.providerConnectionId) {
      return { kind: "refused", error: "No connection established" };
    }

    let service: EditService;
    try {
      service = await this.editServiceFor(connection);
    } catch (error) {
      const { code, message } = callError(error);
      return { kind: "refused", error: errorText(code, message) };
    }
    // Looked up again after the await: a reconnect gives a new Core id.
    const now = lookUp();
    if (!now?.providerConnectionId || now.type !== connection.type) {
      return { kind: "refused", error: "No connection established" };
    }

    const changes: Change[] = sent.map((c) => c.change);
    let outcome;
    try {
      outcome = await service.apply({
        connectionId: now.providerConnectionId,
        changes,
        ...(confirmed ? { confirmed: true } : {}),
        history: this.queryHistory.contextFor(connectionId),
      });
    } catch (error) {
      const { code, message } = callError(error);
      const text = errorText(code, message);
      if (noAnswer(code)) {
        // Some changes may have run, whatever the mode: an atomic batch may
        // have committed before its answer was lost, and a retry would run
        // it again. Nothing says which ran. Keep the queue, say so, and
        // show the database as it is now.
        void log.warn(`Pending changes on ${connectionId} ended without an answer: ${code}`);
        this.setInterrupted(connectionId, true);
        await this.reload(connectionId, true);
        return { kind: "interrupted", error: text };
      }
      void log.warn(`Pending changes on ${connectionId} refused: ${code}`);
      return { kind: "refused", error: text };
    }

    if (outcome.outcome === "confirmRequired") {
      return {
        kind: "confirmRequired",
        destructive: outcome.destructive,
        total: outcome.destructiveTotal,
      };
    }

    if (outcome.mode === "atomic" && outcome.failed?.code === WORKSPACE_CLOSED) {
      // The web server closed the workspace mid-apply: the transaction was
      // dropped, and a commit that raced it may still have landed. Treat it
      // as no answer: keep the queue, mark it and reload.
      void log.warn(`Pending changes on ${connectionId} ended with the workspace closed`);
      this.setInterrupted(connectionId, true);
      await this.reload(connectionId, true);
      return {
        kind: "interrupted",
        error: errorText(outcome.failed.code, outcome.failed.message),
      };
    }

    for (const item of outcome.history) this.queryHistory.insertRecorded(item);

    const { applied, mode, ddl, failed } = outcome;
    // What ran leaves the queue, however the queue changed meanwhile: an
    // entry replaced by a new edit of its cell during the apply stays.
    const ran = new Set(sent.slice(0, applied).map(fingerprint));
    this.setQueue(
      connectionId,
      this.queue(connectionId).filter((c) => !ran.has(fingerprint(c))),
    );

    if (failed?.code === CONNECTION_CLOSED) {
      // The connection went during the apply (DuckDB helper probe F1): what
      // ran before it is gone from the queue, but the change in flight (an
      // atomic batch's COMMIT included) may have landed. Keep the rest,
      // mark it and show the database as it is now.
      void log.warn(
        `Pending changes on ${connectionId} ended with the connection closed (${mode})`,
      );
      this.setInterrupted(connectionId, true);
      await this.reload(connectionId, true);
      return { kind: "interrupted", error: errorText(failed.code, failed.message) };
    }
    this.setInterrupted(connectionId, false);

    if (applied > 0 || ddl) await this.reload(connectionId, ddl);

    if (!failed) return { kind: "applied", applied, mode, ddl };

    // The index and the code only: the message can hold key or row values.
    void log.error(
      `Pending changes on ${connectionId} stopped (${mode}) at index ${failed.index ?? "?"}: ${failed.code}`,
    );
    const change = failed.id ? sent.find((c) => c.id === failed.id) : undefined;
    const target = change?.target;
    const error =
      failed.code === NO_ROWS_AFFECTED && target?.primaryKeyValues
        ? noRowMatchedMessage(target.schema, target.table, target.primaryKeyValues)
        : errorText(failed.code, failed.message);
    return {
      kind: "failed",
      applied,
      mode,
      ddl,
      ...(change ? { changeId: change.id } : {}),
      ...(failed.index === undefined ? {} : { index: failed.index }),
      error,
    };
  }

  /** The schema (after DDL, or when nothing says what ran) and the connection's data tabs. */
  private async reload(connectionId: string, schema: boolean): Promise<void> {
    if (!this.effects) return;
    try {
      if (schema) await this.effects.reloadSchema(connectionId);
      await this.effects.refreshDataTabs(connectionId);
    } catch (error) {
      void log.warn(`Reloading after pending changes failed: ${callError(error).code}`);
    }
  }

  toggleSheet(): void {
    this.state.isPendingChangesOpen = !this.state.isPendingChangesOpen;
  }

  openSheet(): void {
    this.state.isPendingChangesOpen = true;
  }

  closeSheet(): void {
    this.state.isPendingChangesOpen = false;
  }
}
