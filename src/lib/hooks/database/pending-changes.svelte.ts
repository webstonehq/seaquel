import type { PendingChange, PendingChangeOrigin, PendingChangeTarget } from "$lib/types";
import type { QueryType } from "$lib/db/query-utils";
import type { DatabaseState } from "./state.svelte.js";
import type { QueryHistoryManager } from "./query-history.svelte.js";
import type { ProviderRegistry } from "$lib/providers";
import { describePendingChange } from "$lib/db/pending-change-description";
import { pendingChangesSettingsStore } from "$lib/stores/pending-changes-settings.svelte.js";
import { extractErrorMessage } from "$lib/errors";
import { log } from "$lib/utils/logger";
import { cellKey } from "$lib/values";

export interface ExecuteAllResult {
  executed: number;
  failed: number;
  failedAt?: number;
  error?: string;
  hasDdl?: boolean;
}

export class PendingChangesManager {
  constructor(
    private state: DatabaseState,
    private providers: ProviderRegistry,
    private queryHistory: QueryHistoryManager,
  ) {}

  isEnabled(): boolean {
    return pendingChangesSettingsStore.enabled;
  }

  add(
    connectionId: string,
    sql: string,
    queryType: QueryType,
    origin: PendingChangeOrigin,
    sourceTabId?: string,
    bindValues?: unknown[],
    target?: PendingChangeTarget,
  ): void {
    const change: PendingChange = {
      id: crypto.randomUUID(),
      connectionId,
      sql,
      queryType,
      addedAt: new Date(),
      description: describePendingChange(sql, origin),
      sourceTabId,
      bindValues,
      origin,
      target,
    };

    const existing = this.state.pendingChangesByConnection[connectionId] ?? [];
    const isFirstChange = existing.length === 0;
    this.state.pendingChangesByConnection = {
      ...this.state.pendingChangesByConnection,
      [connectionId]: [...existing, change],
    };

    if (isFirstChange) {
      this.openSheet();
    }
  }

  /** Find an existing pending change for the same cell (same table, column, PK values). */
  findForCell(
    connectionId: string,
    schema: string,
    table: string,
    column: string,
    primaryKeyValues: Record<string, unknown>,
  ): PendingChange | undefined {
    const changes = this.state.pendingChangesByConnection[connectionId] ?? [];
    return changes.find((c) => {
      if (c.origin !== "inline-edit" && c.origin !== "set-default") return false;
      const t = c.target;
      if (!t || t.schema !== schema || t.table !== table || t.column !== column) return false;
      if (!t.primaryKeyValues) return false;
      return Object.keys(primaryKeyValues).every(
        (pk) => cellKey(t.primaryKeyValues![pk]) === cellKey(primaryKeyValues[pk]),
      );
    });
  }

  /** Update an existing pending change in-place (new SQL, bind values, target, description). */
  update(
    connectionId: string,
    changeId: string,
    updates: {
      sql: string;
      bindValues?: unknown[];
      target?: PendingChangeTarget;
      description?: string;
    },
  ): void {
    const existing = this.state.pendingChangesByConnection[connectionId] ?? [];
    this.state.pendingChangesByConnection = {
      ...this.state.pendingChangesByConnection,
      [connectionId]: existing.map((c) =>
        c.id === changeId
          ? {
              ...c,
              sql: updates.sql,
              bindValues: updates.bindValues ?? c.bindValues,
              target: updates.target ?? c.target,
              description: updates.description ?? c.description,
            }
          : c,
      ),
    };
  }

  remove(connectionId: string, changeId: string): void {
    const existing = this.state.pendingChangesByConnection[connectionId] ?? [];
    this.state.pendingChangesByConnection = {
      ...this.state.pendingChangesByConnection,
      [connectionId]: existing.filter((c) => c.id !== changeId),
    };
  }

  clear(connectionId: string): void {
    this.state.pendingChangesByConnection = {
      ...this.state.pendingChangesByConnection,
      [connectionId]: [],
    };
  }

  async executeAll(connectionId: string): Promise<ExecuteAllResult> {
    const changes = this.state.pendingChangesByConnection[connectionId] ?? [];
    if (changes.length === 0) return { executed: 0, failed: 0 };

    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection?.providerConnectionId) {
      return { executed: 0, failed: 1, failedAt: 0, error: "No connection established" };
    }

    const provider = await this.providers.getForType(connection.type);
    let executed = 0;
    let hasDdl = false;

    for (let i = 0; i < changes.length; i++) {
      const change = changes[i];
      try {
        await provider.execute(connection.providerConnectionId, change.sql, change.bindValues);
        executed++;

        if (change.queryType === "other") {
          hasDdl = true;
        }

        // Add to query history
        this.queryHistory.addToHistory(change.sql, {
          columns: ["Result"],
          rows: [["Executed from pending changes"]],
          rowCount: 1,
          totalRows: 1,
          executionTime: 0,
          page: 1,
          pageSize: 1,
          totalPages: 1,
        });
      } catch (error) {
        const errorMsg = extractErrorMessage(error);
        void log.error(`Pending change execution failed at index ${i}: ${errorMsg}`);

        // Remove executed changes, keep the failed one and remaining
        this.state.pendingChangesByConnection = {
          ...this.state.pendingChangesByConnection,
          [connectionId]: changes.slice(i),
        };

        return { executed, failed: 1, failedAt: i, error: errorMsg };
      }
    }

    return { executed, failed: 0, hasDdl };
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
