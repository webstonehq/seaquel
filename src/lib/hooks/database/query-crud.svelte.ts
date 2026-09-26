import type { PendingChangeOrigin, PendingChangeTarget } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { ProviderRegistry } from "$lib/providers";
import type { PendingChangesManager } from "./pending-changes.svelte.js";
import { extractErrorMessage } from "$lib/errors";
import { log } from "$lib/utils/logger";
import { getEngineClient, usesRustEngine, type CastMap, type TableMetadata } from "$lib/engine";
import { describePendingChange } from "./pending-change-description.js";
import type { SchemaColumn } from "$lib/types";
import { storeTableMetadata } from "./schema-cache.js";
import { noRowMatchedMessage } from "./stale-edit.js";

type CrudResult = { success: boolean; error?: string; queued?: boolean };

/** Without a `castType`, text-like, user-defined and array columns bind without a cast. */
const UNCAST_TYPES = new Set(["text", "character varying", "user-defined", "array"]);

/**
 * Types information_schema reports without their length. `CAST(… AS bit)`
 * is bit(1) and `CAST(… AS character)` is char(1), which truncate the value,
 * so a key compared that way matches no row. The unbounded types compare
 * (and assign) at the column's own length. Postgres only: no other engine
 * gets a cast map (see `buildCastMap`).
 */
const UNBOUNDED_CAST: Record<string, string> = { bit: "bit varying", character: "bpchar" };

/**
 * Column name → type for Postgres, whose builders wrap bind placeholders in
 * `CAST($N AS type)`. Columns that need no cast are absent.
 * The Rust dialects also cast primary-key placeholders with it (update,
 * set default, delete), so uuid/date/timestamp keys sent as text compare.
 *
 * A column with a `castType` (Postgres reports one for every column from its
 * catalog: enums and arrays by their real type, user types schema-qualified)
 * casts to it as it is; the builders interpolate it into the SQL, so it must
 * never come from user input. Without one, the type comes from `type` by the rules above.
 */
export function castMapForColumns(columns: readonly SchemaColumn[]): CastMap {
  return Object.fromEntries(
    columns.flatMap((c): [string, string][] => {
      if (c.castType) return [[c.name, c.castType]];
      const type = c.type.toLowerCase();
      return UNCAST_TYPES.has(type) ? [] : [[c.name, UNBOUNDED_CAST[type] ?? c.type]];
    }),
  );
}

/**
 * The result of a keyed edit (update, set default, delete) that ran: an
 * error naming the table and key when it matched no row, so a stale key
 * doesn't lose the edit silently.
 */
function matchedRow(
  sourceTable: { schema: string; name: string; primaryKeys: string[] },
  row: Record<string, unknown>,
  rowsAffected: number,
): CrudResult {
  if (rowsAffected !== 0) return { success: true };
  const key = Object.fromEntries(sourceTable.primaryKeys.map((pk) => [pk, row[pk]]));
  const error = noRowMatchedMessage(sourceTable.schema, sourceTable.name, key);
  void log.error(`Keyed edit matched no row: ${error}`);
  return { success: false, error };
}

/**
 * Handles CRUD operations (insert, update, delete) and raw query execution.
 * Extracted from QueryExecutionManager for readability.
 */
export class QueryCrudManager {
  /**
   * Table metadata loaded for cast maps (see `buildCastMap`), per connection,
   * then by provider connection id, schema and table. Holds the promise, so
   * concurrent edits share one load; a failed load is dropped so the next
   * edit retries. Keying by the provider connection id forgets everything on
   * reconnect and disconnect; `forgetLoadedColumns` does it on schema reloads.
   */
  private loadedColumns = new Map<string, Map<string, Promise<TableMetadata>>>();

  constructor(
    private state: DatabaseState,
    private providers: ProviderRegistry,
    private pendingChanges: PendingChangesManager,
  ) {}

  /** Drop the metadata loaded for cast maps on a connection, e.g. when its schema reloads. */
  forgetLoadedColumns(connectionId: string): void {
    this.loadedColumns.delete(connectionId);
  }

  /**
   * The cast map for a table on the active connection (see `castMapForColumns`).
   *
   * When the schema cache doesn't have the table's columns yet (not loaded, or
   * the table isn't listed), a Rust-engine connection loads them first (once;
   * see `loadedColumns`) and puts them in the schema cache if its entry still
   * has none: without casts, typed keys don't compare (`uuid = text`) and a
   * NULL doesn't assign to jsonb, enum or array columns. `undefined` without an
   * active connection, or when that load fails.
   *
   * Only Postgres gets a cast map. SQLite would wrap values in
   * `CAST(? AS <declared type>)`, and its affinity rules turn DATETIME, DATE,
   * BOOLEAN, JSON, UUID and NUMERIC(…) casts numeric (`'2024-01-01 10:00'`
   * becomes 2024, `'{"a":1}'` becomes 0) and the BLOB fallback turns text into
   * a blob. MySQL, MariaDB, MSSQL and DuckDB ignore casts.
   */
  async buildCastMap(schema: string, tableName: string): Promise<CastMap | undefined> {
    const connectionId = this.state.activeConnectionId;
    const connection = this.state.activeConnection;
    if (!connectionId || !connection || connection.type !== "postgres") return undefined;
    const findTable = () =>
      (this.state.schemas[connectionId] ?? []).find(
        (t) => t.name === tableName && t.schema === schema,
      );
    const table = findTable();
    if (table && (table.columns.length > 0 || !usesRustEngine(connection))) {
      return castMapForColumns(table.columns);
    }
    if (!usesRustEngine(connection)) return undefined;

    const byConnection =
      this.loadedColumns.get(connectionId) ?? new Map<string, Promise<TableMetadata>>();
    this.loadedColumns.set(connectionId, byConnection);
    const key = JSON.stringify([connection.providerConnectionId ?? null, schema, tableName]);
    let load = byConnection.get(key);
    if (!load) {
      load = getEngineClient(connection, this.state).tableMetadata(schema, tableName);
      byConnection.set(key, load);
    }

    try {
      const { columns, indexes } = await load;
      // Only fill an entry that still has no columns: a schema refresh may
      // have stored newer ones while this was loading.
      const current = findTable();
      if (current && current.columns.length === 0) {
        storeTableMetadata(this.state, connectionId, { ...current, columns, indexes });
      }
      return castMapForColumns(columns);
    } catch (error) {
      if (byConnection.get(key) === load) byConnection.delete(key);
      void log.debug(
        `No cast map for ${schema}.${tableName}: loading its columns failed: ${extractErrorMessage(error)}`,
      );
      return undefined;
    }
  }

  /**
   * The default expression Set to default assigns on SQLite, which has no
   * `DEFAULT` in `UPDATE`: the column's `defaultValue` (the SQL text of its
   * DEFAULT clause), or `"NULL"` for a column without one. Read from fresh
   * metadata, since the schema cache may predate a change to the default.
   * `undefined` for every other engine, whose dialects write `DEFAULT`.
   */
  private async setDefaultExpression(
    schema: string,
    table: string,
    column: string,
  ): Promise<string | undefined> {
    const connection = this.state.activeConnection;
    if (!connection || connection.type !== "sqlite" || !usesRustEngine(connection)) {
      return undefined;
    }
    const { columns } = await getEngineClient(connection, this.state).tableMetadata(schema, table);
    const found = columns.find((c) => c.name === column);
    if (!found) throw new Error(`Column "${column}" not found in "${table}"`);
    return found.defaultValue ?? "NULL";
  }

  /**
   * Update a single cell value directly.
   * Used by the data viewer for inline cell editing and by updateCell (query tab version).
   */
  async updateCellDirect(
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
    row: Record<string, unknown>,
    column: string,
    newValue: unknown,
    options?: { deduplicatePending?: boolean },
  ): Promise<CrudResult> {
    if (sourceTable.primaryKeys.length === 0) {
      return { success: false, error: "No primary key found" };
    }

    const connection = this.state.activeConnection;
    if (!connection?.providerConnectionId) {
      return { success: false, error: "No connection established" };
    }

    try {
      const provider = await this.providers.getForType(connection.type);
      const client = getEngineClient(connection, this.state);
      const casts = await this.buildCastMap(sourceTable.schema, sourceTable.name);
      const { sql: query, bindValues } = await client.buildUpdate(
        sourceTable.schema,
        sourceTable.name,
        column,
        newValue,
        sourceTable.primaryKeys,
        row,
        casts,
      );

      if (this.pendingChanges.isEnabled()) {
        const pkValues = Object.fromEntries(sourceTable.primaryKeys.map((pk) => [pk, row[pk]]));
        const target: PendingChangeTarget = {
          schema: sourceTable.schema,
          table: sourceTable.name,
          column,
          primaryKeyValues: pkValues,
          newValue,
        };

        if (options?.deduplicatePending) {
          const existingChange = this.pendingChanges.findForCell(
            connection.id,
            sourceTable.schema,
            sourceTable.name,
            column,
            pkValues,
          );
          if (existingChange) {
            this.pendingChanges.update(connection.id, existingChange.id, {
              sql: query,
              bindValues,
              target,
              description: describePendingChange(query, "inline-edit"),
            });
            return { success: true, queued: true };
          }
        }

        this.pendingChanges.add(
          connection.id,
          query,
          "update",
          "inline-edit",
          undefined,
          bindValues,
          target,
        );
        return { success: true, queued: true };
      }

      const { rowsAffected } = await provider.execute(
        connection.providerConnectionId,
        query,
        bindValues,
      );
      return matchedRow(sourceTable, row, rowsAffected);
    } catch (error) {
      return { success: false, error: extractErrorMessage(error) };
    }
  }

  /**
   * Set a cell to its column DEFAULT directly.
   * Used by the data viewer and by setCellDefault (query tab version).
   */
  async setCellDefaultDirect(
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
    row: Record<string, unknown>,
    column: string,
    options?: { deduplicatePending?: boolean },
  ): Promise<CrudResult> {
    if (sourceTable.primaryKeys.length === 0) {
      return { success: false, error: "No primary key found" };
    }

    const connection = this.state.activeConnection;
    if (!connection?.providerConnectionId) {
      return { success: false, error: "No connection established" };
    }

    try {
      const provider = await this.providers.getForType(connection.type);
      const client = getEngineClient(connection, this.state);
      const columnDefault = await this.setDefaultExpression(
        sourceTable.schema,
        sourceTable.name,
        column,
      );
      const { sql: query, bindValues } = await client.buildSetDefault(
        sourceTable.schema,
        sourceTable.name,
        column,
        sourceTable.primaryKeys,
        row,
        await this.buildCastMap(sourceTable.schema, sourceTable.name),
        ...(columnDefault === undefined ? [] : [columnDefault]),
      );

      if (this.pendingChanges.isEnabled()) {
        const pkV = Object.fromEntries(sourceTable.primaryKeys.map((pk) => [pk, row[pk]]));
        const t: PendingChangeTarget = {
          schema: sourceTable.schema,
          table: sourceTable.name,
          column,
          primaryKeyValues: pkV,
        };

        if (options?.deduplicatePending) {
          const existingChange = this.pendingChanges.findForCell(
            connection.id,
            sourceTable.schema,
            sourceTable.name,
            column,
            pkV,
          );
          if (existingChange) {
            this.pendingChanges.update(connection.id, existingChange.id, {
              sql: query,
              bindValues,
              target: t,
              description: describePendingChange(query, "set-default"),
            });
            return { success: true, queued: true };
          }
        }

        this.pendingChanges.add(
          connection.id,
          query,
          "update",
          "set-default",
          undefined,
          bindValues,
          t,
        );
        return { success: true, queued: true };
      }

      const { rowsAffected } = await provider.execute(
        connection.providerConnectionId,
        query,
        bindValues,
      );
      return matchedRow(sourceTable, row, rowsAffected);
    } catch (error) {
      return { success: false, error: extractErrorMessage(error) };
    }
  }

  /**
   * Insert a new row into the database.
   */
  async insertRow(
    sourceTable: { schema: string; name: string },
    values: Record<string, unknown>,
  ): Promise<{ success: boolean; error?: string; lastInsertId?: number; queued?: boolean }> {
    const columns = Object.keys(values);
    if (columns.length === 0) {
      return { success: false, error: "No values provided" };
    }

    const connection = this.state.activeConnection;
    if (!connection?.providerConnectionId) {
      return { success: false, error: "No connection established" };
    }

    void log.debug(`Row insert on ${connection?.id}`);
    try {
      const provider = await this.providers.getForType(connection.type);
      const client = getEngineClient(connection, this.state);
      const casts = await this.buildCastMap(sourceTable.schema, sourceTable.name);
      const { sql: query, bindValues } = await client.buildInsert(
        sourceTable.schema,
        sourceTable.name,
        values,
        casts,
      );

      if (this.pendingChanges.isEnabled()) {
        const target: PendingChangeTarget = {
          schema: sourceTable.schema,
          table: sourceTable.name,
          insertValues: values,
        };
        this.pendingChanges.add(
          connection.id,
          query,
          "insert",
          "insert-row",
          undefined,
          bindValues,
          target,
        );
        return { success: true, queued: true };
      }

      const result = await provider.execute(connection.providerConnectionId, query, bindValues);
      return { success: true, lastInsertId: result?.lastInsertId };
    } catch (error) {
      return { success: false, error: extractErrorMessage(error) };
    }
  }

  /**
   * Delete a row from the database.
   */
  async deleteRow(
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
    row: Record<string, unknown>,
  ): Promise<CrudResult> {
    if (sourceTable.primaryKeys.length === 0) {
      return { success: false, error: "No primary key found" };
    }

    const connection = this.state.activeConnection;
    if (!connection?.providerConnectionId) {
      return { success: false, error: "No connection established" };
    }

    void log.debug(`Row delete on ${connection?.id}`);
    try {
      const provider = await this.providers.getForType(connection.type);
      const client = getEngineClient(connection, this.state);
      const { sql: query, bindValues } = await client.buildDelete(
        sourceTable.schema,
        sourceTable.name,
        sourceTable.primaryKeys,
        row,
        await this.buildCastMap(sourceTable.schema, sourceTable.name),
      );

      if (this.pendingChanges.isEnabled()) {
        const delPk = Object.fromEntries(sourceTable.primaryKeys.map((pk) => [pk, row[pk]]));
        const delTgt: PendingChangeTarget = {
          schema: sourceTable.schema,
          table: sourceTable.name,
          primaryKeyValues: delPk,
        };
        this.pendingChanges.add(
          connection.id,
          query,
          "delete",
          "delete-row",
          undefined,
          bindValues,
          delTgt,
        );
        return { success: true, queued: true };
      }

      const { rowsAffected } = await provider.execute(
        connection.providerConnectionId,
        query,
        bindValues,
      );
      return matchedRow(sourceTable, row, rowsAffected);
    } catch (error) {
      return { success: false, error: extractErrorMessage(error) };
    }
  }

  /**
   * Execute a raw query and return results directly.
   * Used by statistics dashboard and other features that need raw query results.
   */
  async executeRaw(query: string): Promise<Record<string, unknown>[]> {
    const connection = this.state.activeConnection;
    const isConnected = !!connection?.providerConnectionId;
    if (!connection || !isConnected) {
      throw new Error("Not connected to database");
    }

    const provider = await this.providers.getForType(connection.type);
    return await provider.select<Record<string, unknown>>(connection.providerConnectionId!, query);
  }

  /**
   * Execute a raw DDL/write statement on the active connection.
   * Used for CREATE TABLE, DROP TABLE, ALTER TABLE, TRUNCATE, etc.
   */
  async executeRawDdl(query: string): Promise<{ queued?: boolean }> {
    const connection = this.state.activeConnection;
    if (!connection?.providerConnectionId) {
      throw new Error("Not connected to database");
    }

    if (this.pendingChanges.isEnabled()) {
      const upper = query.trimStart().toUpperCase();
      let origin: PendingChangeOrigin;
      if (upper.startsWith("TRUNCATE")) origin = "truncate-table";
      else if (upper.startsWith("ALTER TABLE")) origin = "alter-table";
      else if (upper.startsWith("CREATE TABLE")) origin = "create-table";
      else if (upper.startsWith("DROP TABLE")) origin = "drop-table";
      else origin = "query-editor";
      this.pendingChanges.add(connection.id, query, "other", origin);
      return { queued: true };
    }

    const provider = await this.providers.getForType(connection.type);
    await provider.execute(connection.providerConnectionId, query);
    return {};
  }
}
