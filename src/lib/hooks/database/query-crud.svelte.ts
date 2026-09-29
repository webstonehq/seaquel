import type { DatabaseConnection, PendingChangeOrigin, PendingChangeTarget } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { ProviderRegistry, ReadOnlyRows } from "$lib/providers";
import type { PendingChangesManager } from "./pending-changes.svelte.js";
import { log } from "$lib/utils/logger";
import { noRowMatchedMessage } from "./stale-edit.js";
import { readOnlyError } from "$lib/services/ai/context.js";
import { callErrorText, errorText } from "./error-text.js";
import { CONFIRM_REQUIRED } from "./query-runner/types";
import {
  deleteRowEdit,
  dropObjectEdit,
  getEditService,
  insertRowEdit,
  NO_ROWS_AFFECTED,
  setDefaultEdit,
  truncateTableEdit,
  updateCellEdit,
  type Edit,
  type EditService,
  type EditTable,
} from "./edit-service/index.js";
import type { ObjectKind } from "$lib/types/generated/ObjectKind";

/**
 * The cell a queued update or Set default writes, as a key; `null` for the
 * other edits, which never replace a queued one.
 */
function cellOf(connectionId: string, edit: Edit): string | null {
  if (edit.type !== "updateCell" && edit.type !== "setDefault") return null;
  return JSON.stringify([connectionId, edit.target, edit.column, edit.key]);
}

type CrudResult = { success: boolean; error?: string; queued?: boolean };
type InsertResult = CrudResult & { lastInsertId?: number };

/** The refusal for an edit whose connection is disconnected or was removed. */
const NO_CONNECTION: CrudResult = { success: false, error: "No connection established" };

/** What `submit` came to: queued, applied (with its insert id and DDL flag), or an error. */
type Submitted = InsertResult & { ddl?: boolean };

/**
 * The grid's edits as a view model over the connection's `EditService`
 * (phase 5c): each edit is an intent (a table, the key picked out of the
 * row, a column and a value; `./edit-service/intents`) that Core plans
 * and runs with the connection's dialect, after reading the table's
 * metadata and checking the key against its primary key.
 *
 * With pending changes on, `db.planEdits` fills the queue entry (a repeated
 * edit of a cell replaces the queued one); otherwise `db.applyChanges` runs
 * it at once as one change. A keyed edit that matched no row fails with the
 * translated "no row matched" naming the table and key. Every edit runs on
 * the saved connection it's given (the one its row came from), looked up
 * again after each await, never on whichever connection is active.
 */
export class QueryCrudManager {
  /**
   * The latest queued edit of each cell (connection, table, column, key):
   * a plan that lands after a later edit of the same cell is dropped, so
   * two quick edits can't queue out of order.
   */
  private cellEdits = new Map<string, number>();
  private editSeq = 0;

  constructor(
    private state: DatabaseState,
    private providers: ProviderRegistry,
    private pendingChanges: PendingChangesManager,
    private editServiceFor: (connection: DatabaseConnection) => Promise<EditService> = (c) =>
      getEditService(c, state, providers),
  ) {}

  /**
   * A saved connection as it is now, with its Core id: `null` when it was
   * removed or is disconnected. Read at the time of each edit (and again
   * after each await before it's sent), so a reconnect's new Core id is
   * used and an edit never goes to whichever connection is active.
   */
  private lookUp(
    connectionId: string,
  ): { connection: DatabaseConnection; providerConnectionId: string } | null {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    const providerConnectionId = connection?.providerConnectionId;
    return connection && providerConnectionId ? { connection, providerConnectionId } : null;
  }

  /**
   * Queue `edit` (pending changes on) or apply it now, on `connectionId`.
   * `target` is the queue entry's (the grid's overlays) and names the table
   * and key in the "no row matched" error.
   */
  private async submit(
    connectionId: string,
    edit: Edit,
    origin: PendingChangeOrigin,
    target: PendingChangeTarget,
    options: { confirmed?: boolean } = {},
  ): Promise<Submitted> {
    const found = this.lookUp(connectionId);
    if (!found) return NO_CONNECTION;
    let service: EditService;
    try {
      service = await this.editServiceFor(found.connection);
    } catch (error) {
      return { success: false, error: callErrorText(error) };
    }
    // The connection as it is after the await (a reconnect, a disconnect).
    const now = this.lookUp(connectionId);
    if (!now || now.connection.type !== found.connection.type) return NO_CONNECTION;

    if (this.pendingChanges.isEnabled()) {
      const cell = cellOf(connectionId, edit);
      const seq = ++this.editSeq;
      if (cell) this.cellEdits.set(cell, seq);
      let planned;
      let failed: string | null = null;
      try {
        [planned] = await service.plan({ connectionId: now.providerConnectionId, edits: [edit] });
      } catch (error) {
        failed = callErrorText(error);
      }
      // A later edit of the same cell was made meanwhile: it replaces this one.
      if (cell && this.cellEdits.get(cell) !== seq) return { success: true, queued: true };
      if (cell) this.cellEdits.delete(cell);
      if (failed !== null) return { success: false, error: failed };
      if (!planned) return { success: false, error: "The edit wasn't planned" };
      // The plan awaited: the connection may have gone meanwhile.
      if (!this.lookUp(connectionId)) return NO_CONNECTION;
      this.pendingChanges.addPlanned(connectionId, edit, planned, origin, target);
      return { success: true, queued: true };
    }

    let outcome;
    try {
      outcome = await service.apply({
        connectionId: now.providerConnectionId,
        changes: [{ type: "edit", id: crypto.randomUUID(), edit }],
        ...(options.confirmed ? { confirmed: true } : {}),
      });
    } catch (error) {
      return { success: false, error: callErrorText(error) };
    }
    if (outcome.outcome === "confirmRequired") {
      // Grid edits aren't destructive and the sidebar sends `confirmed`.
      return { success: false, error: errorText(CONFIRM_REQUIRED, "") };
    }
    const { failed } = outcome;
    if (failed) {
      if (failed.code === NO_ROWS_AFFECTED && target.primaryKeyValues) {
        // The table only: the key is row data.
        void log.error(`Keyed edit of ${target.schema}.${target.table} matched no row`);
        return {
          success: false,
          error: noRowMatchedMessage(target.schema, target.table, target.primaryKeyValues),
        };
      }
      void log.error(`Edit of ${target.schema}.${target.table} failed: ${failed.code}`);
      return { success: false, error: errorText(failed.code, failed.message) };
    }
    const lastInsertId = outcome.results[0]?.lastInsertId;
    return {
      success: true,
      ddl: outcome.ddl,
      ...(lastInsertId === undefined ? {} : { lastInsertId }),
    };
  }

  /** The row's primary-key values, for the queue entry and the "no row matched" error. */
  private keyValues(table: EditTable, row: Record<string, unknown>): Record<string, unknown> {
    return Object.fromEntries(table.primaryKeys.map((pk) => [pk, row[pk]]));
  }

  /**
   * Update one cell, on the saved connection `connectionId` (the one the row
   * came from, never the active one). A queued edit of the same cell is
   * replaced.
   */
  async updateCellDirect(
    connectionId: string,
    sourceTable: EditTable,
    row: Record<string, unknown>,
    column: string,
    newValue: unknown,
  ): Promise<CrudResult> {
    if (sourceTable.primaryKeys.length === 0) {
      return { success: false, error: "No primary key found" };
    }
    const { success, error, queued } = await this.submit(
      connectionId,
      updateCellEdit(sourceTable, row, column, newValue),
      "inline-edit",
      {
        schema: sourceTable.schema,
        table: sourceTable.name,
        column,
        primaryKeyValues: this.keyValues(sourceTable, row),
        newValue,
      },
    );
    return { success, error, queued };
  }

  /** Set one cell to its column's default, on the saved connection `connectionId`. */
  async setCellDefaultDirect(
    connectionId: string,
    sourceTable: EditTable,
    row: Record<string, unknown>,
    column: string,
  ): Promise<CrudResult> {
    if (sourceTable.primaryKeys.length === 0) {
      return { success: false, error: "No primary key found" };
    }
    const { success, error, queued } = await this.submit(
      connectionId,
      setDefaultEdit(sourceTable, row, column),
      "set-default",
      {
        schema: sourceTable.schema,
        table: sourceTable.name,
        column,
        primaryKeyValues: this.keyValues(sourceTable, row),
      },
    );
    return { success, error, queued };
  }

  /** Insert a row into the table on the saved connection `connectionId`. */
  async insertRow(
    connectionId: string,
    sourceTable: { schema: string; name: string },
    values: Record<string, unknown>,
  ): Promise<InsertResult> {
    if (Object.keys(values).length === 0) {
      return { success: false, error: "No values provided" };
    }
    void log.debug(`Row insert on ${connectionId}`);
    const { success, error, queued, lastInsertId } = await this.submit(
      connectionId,
      insertRowEdit(sourceTable, values),
      "insert-row",
      { schema: sourceTable.schema, table: sourceTable.name, insertValues: values },
    );
    return {
      success,
      error,
      queued,
      ...(lastInsertId === undefined ? {} : { lastInsertId }),
    };
  }

  /** Delete a row from the table on the saved connection `connectionId`. */
  async deleteRow(
    connectionId: string,
    sourceTable: EditTable,
    row: Record<string, unknown>,
  ): Promise<CrudResult> {
    if (sourceTable.primaryKeys.length === 0) {
      return { success: false, error: "No primary key found" };
    }
    void log.debug(`Row delete on ${connectionId}`);
    const { success, error, queued } = await this.submit(
      connectionId,
      deleteRowEdit(sourceTable, row),
      "delete-row",
      {
        schema: sourceTable.schema,
        table: sourceTable.name,
        primaryKeyValues: this.keyValues(sourceTable, row),
      },
    );
    return { success, error, queued };
  }

  /**
   * The sidebar's DROP (Decision 11): an intent Core builds with the
   * dialect. Queued with pending changes on; otherwise applied, confirmed
   * (the sidebar's own dialog asked). Throws the error when it fails.
   */
  async dropObject(
    connectionId: string,
    table: { schema: string; name: string },
    kind: ObjectKind,
  ): Promise<{ queued?: boolean }> {
    const result = await this.submit(
      connectionId,
      dropObjectEdit(table, kind),
      kind === "table" ? "drop-table" : "drop-view",
      { schema: table.schema, table: table.name },
      { confirmed: true },
    );
    if (!result.success) throw new Error(result.error);
    return result.queued ? { queued: true } : {};
  }

  /**
   * The sidebar's TRUNCATE (Decision 11; SQLite's is a `DELETE FROM`, which
   * Core builds). Queued or applied like `dropObject`.
   */
  async truncateTable(
    connectionId: string,
    table: { schema: string; name: string },
  ): Promise<{ queued?: boolean }> {
    const result = await this.submit(
      connectionId,
      truncateTableEdit(table),
      "truncate-table",
      { schema: table.schema, table: table.name },
      { confirmed: true },
    );
    if (!result.success) throw new Error(result.error);
    return result.queued ? { queued: true } : {};
  }

  /**
   * Statements the table editor generated (`createTable`/`alterTable`), as
   * typed changes: queued one per statement with pending changes on,
   * otherwise applied in one call, confirmed (the user asked for the
   * change in the editor; an ALTER may drop a column). DDL applies in order
   * and stops at the first failure.
   */
  async applyStatements(
    connectionId: string,
    statements: string[],
    origin: PendingChangeOrigin,
  ): Promise<{ queued: true } | { queued: false; applied: number; error?: string }> {
    const found = this.lookUp(connectionId);
    if (!found) return { queued: false, applied: 0, error: NO_CONNECTION.error };
    if (this.pendingChanges.isEnabled()) {
      for (const sql of statements) {
        this.pendingChanges.addSql(connectionId, sql, [], "other", origin);
      }
      return { queued: true };
    }
    let service: EditService;
    try {
      service = await this.editServiceFor(found.connection);
    } catch (error) {
      return { queued: false, applied: 0, error: callErrorText(error) };
    }
    const now = this.lookUp(connectionId);
    if (!now || now.connection.type !== found.connection.type) {
      return { queued: false, applied: 0, error: NO_CONNECTION.error };
    }
    try {
      const outcome = await service.apply({
        connectionId: now.providerConnectionId,
        changes: statements.map((sql) => ({
          type: "sql" as const,
          id: crypto.randomUUID(),
          sql,
          params: [],
        })),
        confirmed: true,
      });
      if (outcome.outcome === "confirmRequired") {
        // Sent confirmed: Core shouldn't ask.
        return { queued: false, applied: 0, error: errorText(CONFIRM_REQUIRED, "") };
      }
      const { failed, applied } = outcome;
      if (failed) {
        void log.error(`Table editor statements stopped at ${failed.index ?? "?"}: ${failed.code}`);
        return { queued: false, applied, error: errorText(failed.code, failed.message) };
      }
      return { queued: false, applied };
    } catch (error) {
      return { queued: false, applied: 0, error: callErrorText(error) };
    }
  }

  /**
   * Run one query in the read-only mode the database enforces
   * (`DatabaseProvider.selectReadOnly`). The only way AI and dashboard SQL
   * reaches a database.
   *
   * Runs on `connectionId`, whatever is active. The connection is looked up
   * on every call, so it follows `reconnect()` (which replaces the object and
   * its provider id). It refuses, naming the connection, when it was removed
   * or is disconnected, and runs the token check (`readOnlyError`) against the
   * connection it looked up, so a connection whose type was edited is checked
   * as what it is now.
   *
   * @param connectionName The name for the "was removed" refusal, which has
   *   no connection to read it from.
   * @param signal Aborting it cancels the query and rejects the promise.
   * @param maxRows Return at most this many rows, `truncated` when there
   *   were more (the AI's `run_query`). Without it a result past the
   *   engine's row cap fails (dashboard widgets).
   */
  async executeReadOnly(
    connectionId: string,
    sql: string,
    signal?: AbortSignal,
    connectionName?: string,
    maxRows?: number,
  ): Promise<ReadOnlyRows> {
    const lookUp = () => {
      const connection = this.state.connections.find((c) => c.id === connectionId);
      if (!connection) {
        throw new Error(
          connectionName === undefined
            ? "The connection was removed"
            : `The connection "${connectionName}" was removed`,
        );
      }
      const providerConnectionId = connection.providerConnectionId;
      if (!providerConnectionId) {
        throw new Error(
          `The connection "${connection.name}" is disconnected; reconnect it and try again`,
        );
      }
      return { connection, providerConnectionId };
    };

    // Everything that decides where and how the query runs is read after the
    // last await, so a reconnect or an edit during it can't send the query
    // to a stale provider id or check it as the old engine.
    const { type } = lookUp().connection;
    const provider = await this.providers.getForType(type);
    const { connection, providerConnectionId } = lookUp();
    if (connection.type !== type) {
      throw new Error(`The connection "${connection.name}" changed; try again`);
    }
    const refusal = readOnlyError(sql, connection.type);
    if (refusal) throw new Error(refusal);

    return await provider.selectReadOnly(providerConnectionId, sql, signal, maxRows);
  }
}
