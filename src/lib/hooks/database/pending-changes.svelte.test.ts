/**
 * The pending-changes queue over `db.applyChanges` (phase 5c): what the
 * queue holds after each outcome, confirmation, history rows from Core, the
 * reloads after an apply, and an apply that ended without an answer.
 */
import { describe, expect, it, vi } from "vitest";
import type { PendingChange } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "./state.svelte.js";
import type { QueryHistoryManager } from "./query-history.svelte.js";
import type { ApplyChangesParams, ApplyOutcome, Edit } from "./edit-service/types";
import type { PersistedQueryHistoryItem } from "$lib/types/generated/PersistedQueryHistoryItem";

const settings = vi.hoisted(() => ({ enabled: true }));
vi.mock("$lib/stores/pending-changes-settings.svelte.js", () => ({
  pendingChangesSettingsStore: settings,
}));
const logs = vi.hoisted(() => ({ error: vi.fn(), warn: vi.fn() }));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: logs.error, info: vi.fn(), warn: logs.warn },
}));

const { PendingChangesManager, listDestructive, confirmedFor } =
  await import("./pending-changes.svelte.js");
const { CoreEditService } = await import("./edit-service/core-service.js");
const { scriptedCore, refusal, plannedFor } = await import("./edit-service/scripted-core.js");

const target = { schema: "public", table: "users" };
const updateEdit = (id: number, value: string): Edit => ({
  type: "updateCell",
  target,
  key: [["id", id]],
  column: "name",
  value,
});

/** A state with the queue under `conn-1`, and a manager over a scripted Core. */
function setup(script: Parameters<typeof scriptedCore>[0] = {}) {
  const state = $state({
    connections: [{ id: "conn-1", type: "postgres", name: "Local", providerConnectionId: "pc-1" }],
    pendingChangesByConnection: {} as Record<string, PendingChange[]>,
    pendingChangesInterrupted: {} as Record<string, boolean>,
    isPendingChangesOpen: false,
  });
  const core = scriptedCore(script);
  const service = new CoreEditService(() => core.client);
  const history = {
    contextFor: (connectionId: string) => ({
      connectionId,
      connectionName: "Local",
      connectionLabels: [],
    }),
    insertRecorded: vi.fn(),
  };
  const manager = new PendingChangesManager(
    state as unknown as DatabaseState,
    {} as ProviderRegistry,
    history as unknown as QueryHistoryManager,
    async () => service,
  );
  const effects = { reloadSchema: vi.fn(async () => {}), refreshDataTabs: vi.fn(async () => {}) };
  manager.setEffects(effects);
  const queue = () => state.pendingChangesByConnection["conn-1"] ?? [];
  const ids = () => queue().map((c) => c.id);
  const applies = () => core.of("applyChanges") as ApplyChangesParams[];
  /** Queue an edit as the grid does (planned by the script). */
  const edit = (e: Edit, origin: PendingChange["origin"] = "inline-edit") => {
    manager.addPlanned("conn-1", e, plannedFor(e), origin, {
      ...target,
      column: "name",
      primaryKeyValues: Object.fromEntries(e.type === "updateCell" ? e.key : []),
    });
    return queue().at(-1)!.id;
  };
  return { manager, state, core, history, effects, queue, ids, applies, edit };
}

function outcome(extra: Partial<Extract<ApplyOutcome, { outcome: "applied" }>>): ApplyOutcome {
  return {
    outcome: "applied",
    mode: "single",
    applied: 0,
    results: [],
    ddl: false,
    history: [],
    ...extra,
  };
}

describe("PendingChangesManager queueing", () => {
  it("a queued edit holds its intent (the change apply sends back) and the planned display", () => {
    const { manager, queue } = setup();
    const e = updateEdit(7, "b");
    manager.addPlanned("conn-1", e, plannedFor(e), "inline-edit", {
      ...target,
      column: "name",
      primaryKeyValues: { id: 7 },
      newValue: "b",
    });
    const [entry] = queue();
    expect(entry.change).toEqual({ type: "edit", id: entry.id, edit: e });
    expect(entry).toMatchObject({
      connectionId: "conn-1",
      sql: plannedFor(e).sql,
      bindValues: ["b", 7],
      queryType: "update",
      dml: true,
      description: "Update users.name",
      origin: "inline-edit",
    });
  });

  it("a repeated edit of a cell replaces the queued one in place, whole", () => {
    const { manager, queue, edit } = setup();
    const first = edit(updateEdit(7, "b1"));
    edit(updateEdit(8, "other"));
    manager.addPlanned(
      "conn-1",
      { type: "setDefault", target, key: [["id", 7]], column: "name" },
      plannedFor({ type: "setDefault", target, key: [["id", 7]], column: "name" }),
      "set-default",
      { ...target, column: "name", primaryKeyValues: { id: 7 } },
    );
    expect(queue()).toHaveLength(2);
    expect(queue()[0].id).toBe(first);
    expect(queue()[0].origin).toBe("set-default");
    expect(queue()[0].change).toEqual({
      type: "edit",
      id: first,
      edit: { type: "setDefault", target, key: [["id", 7]], column: "name" },
    });
  });

  it("typed SQL the editor deferred holds its text and wire params", () => {
    const { manager, queue } = setup();
    manager.addSql(
      "conn-1",
      "DELETE FROM t WHERE id = $1",
      [{ $sq: "bigint", v: "9" }],
      "delete",
      "query-editor",
      "tab-1",
    );
    const [entry] = queue();
    expect(entry.change).toEqual({
      type: "sql",
      id: entry.id,
      sql: "DELETE FROM t WHERE id = $1",
      params: [{ $sq: "bigint", v: "9" }],
    });
    expect(entry).toMatchObject({
      bindValues: [9n],
      dml: true,
      sourceTabId: "tab-1",
      description: "Delete row from t",
    });
  });
});

describe("PendingChangesManager.apply", () => {
  it("sends the queue in order with history, and clears it after success", async () => {
    const { manager, edit, applies, ids, effects } = setup();
    const c1 = edit(updateEdit(1, "a"));
    const c2 = edit(updateEdit(2, "b"));
    expect(await manager.apply("conn-1")).toEqual({
      kind: "applied",
      applied: 2,
      mode: "atomic",
      ddl: false,
    });
    const [sent] = applies();
    expect(sent.connectionId).toBe("pc-1");
    expect(sent.changes.map((c) => c.id)).toEqual([c1, c2]);
    expect(sent.history).toEqual({
      connectionId: "conn-1",
      connectionName: "Local",
      connectionLabels: [],
    });
    expect(sent.confirmed).toBeUndefined();
    expect(ids()).toEqual([]);
    // Something ran: the connection's data tabs reload; no DDL, no schema reload.
    expect(effects.refreshDataTabs).toHaveBeenCalledWith("conn-1");
    expect(effects.reloadSchema).not.toHaveBeenCalled();
  });

  it("keeps the queue whole after an atomic failure and marks the change", async () => {
    const { manager, edit, ids, effects } = setup({
      applyChanges: (p) =>
        outcome({
          mode: "atomic",
          failed: { id: p.changes[1].id, index: 1, code: "QUERY_ERROR", message: "deadlock" },
        }),
    });
    const c1 = edit(updateEdit(1, "a"));
    const c2 = edit(updateEdit(2, "b"));
    const c3 = edit(updateEdit(3, "c"));
    expect(await manager.apply("conn-1")).toEqual({
      kind: "failed",
      applied: 0,
      mode: "atomic",
      ddl: false,
      changeId: c2,
      index: 1,
      error: "deadlock",
    });
    expect(ids()).toEqual([c1, c2, c3]);
    expect(effects.refreshDataTabs).not.toHaveBeenCalled();
  });

  it("removes the applied prefix after an in-order failure", async () => {
    const { manager, edit, ids, effects } = setup({
      applyChanges: (p) =>
        outcome({
          mode: "inOrder",
          applied: 2,
          ddl: true,
          results: p.changes.slice(0, 2).map((c) => ({ id: c.id, rowsAffected: 1 })),
          failed: { id: p.changes[2].id, index: 2, code: "EXECUTE_ERROR", message: "no" },
        }),
    });
    edit(updateEdit(1, "a"));
    manager.addSql("conn-1", "CREATE TABLE z (i int)", [], "other", "query-editor");
    const c3 = edit(updateEdit(3, "c"));
    const c4 = edit(updateEdit(4, "d"));
    const result = await manager.apply("conn-1");
    expect(result).toMatchObject({ kind: "failed", applied: 2, mode: "inOrder", changeId: c3 });
    expect(ids()).toEqual([c3, c4]);
    // DDL ran: the schema reloads, then the data tabs.
    expect(effects.reloadSchema).toHaveBeenCalledWith("conn-1");
    expect(effects.refreshDataTabs).toHaveBeenCalledWith("conn-1");
  });

  it("keeps the queue whole after a refusal before anything ran", async () => {
    const { manager, edit, ids } = setup({
      applyChanges: (p) =>
        outcome({
          mode: "atomic",
          failed: { id: p.changes[1].id, index: 1, code: "NOT_EDITABLE", message: "not the key" },
        }),
    });
    const c1 = edit(updateEdit(1, "a"));
    const c2 = edit(updateEdit(2, "b"));
    expect(await manager.apply("conn-1")).toMatchObject({
      kind: "failed",
      error: "NOT_EDITABLE: not the key",
    });
    expect(ids()).toEqual([c1, c2]);
  });

  it("NO_ROWS_AFFECTED shows the i18n message with the change's table and key", async () => {
    const { manager, edit } = setup({
      applyChanges: (p) =>
        outcome({
          failed: { id: p.changes[0].id, index: 0, code: "NO_ROWS_AFFECTED", message: "x" },
        }),
    });
    edit(updateEdit(7, "a"));
    const result = await manager.apply("conn-1");
    expect(result.kind === "failed" && result.error).toContain("public.users");
    expect(result.kind === "failed" && result.error).toContain("id = 7");
  });

  it("confirmRequired lists the statements and applies nothing; confirmed resends", async () => {
    let asked = 0;
    const { manager, edit, applies, ids } = setup({
      applyChanges: (p) => {
        if (!p.confirmed) {
          asked += 1;
          return {
            outcome: "confirmRequired",
            destructive: [{ index: 0, sql: "TRUNCATE TABLE k", reason: "truncate" }],
            destructiveTotal: 1,
          };
        }
        return outcome({ applied: 1, results: [{ id: p.changes[0].id, rowsAffected: 0 }] });
      },
    });
    edit({ type: "truncateTable", target }, "truncate-table");
    expect(await manager.apply("conn-1")).toEqual({
      kind: "confirmRequired",
      destructive: [{ index: 0, sql: "TRUNCATE TABLE k", reason: "truncate" }],
      total: 1,
    });
    expect(ids()).toHaveLength(1);
    expect(await manager.apply("conn-1", true)).toMatchObject({ kind: "applied", applied: 1 });
    expect(applies().map((p) => p.confirmed)).toEqual([undefined, true]);
    expect(asked).toBe(1);
    expect(ids()).toEqual([]);
  });

  it("apply history rows go to the cache once each", async () => {
    const rows: PersistedQueryHistoryItem[] = [1, 2].map((n) => ({
      id: `hist-${n}`,
      query: `UPDATE ${n}`,
      timestamp: "2026-10-03T00:00:00Z",
      executionTime: 1,
      rowCount: n,
      connectionId: "conn-1",
      favorite: false,
      connectionLabelsSnapshot: [],
      connectionNameSnapshot: "Local",
    }));
    const { manager, edit, history } = setup({
      applyChanges: (p) => outcome({ mode: "atomic", applied: p.changes.length, history: rows }),
    });
    edit(updateEdit(1, "a"));
    edit(updateEdit(2, "b"));
    await manager.apply("conn-1");
    expect(history.insertRecorded.mock.calls).toEqual([[rows[0]], [rows[1]]]);
  });

  it("an entry replaced while the apply was in flight stays queued", async () => {
    const holder: { replace?: () => void } = {};
    const { manager, edit, queue } = setup({
      applyChanges: (p) => {
        holder.replace?.();
        return outcome({ applied: 1, results: [{ id: p.changes[0].id, rowsAffected: 1 }] });
      },
    });
    const c1 = edit(updateEdit(7, "old"));
    holder.replace = () => edit(updateEdit(7, "new"));
    expect(await manager.apply("conn-1")).toMatchObject({ kind: "applied" });
    expect(queue().map((c) => [c.id, c.target?.primaryKeyValues])).toEqual([[c1, { id: 7 }]]);
    expect(queue()[0].change).toMatchObject({ edit: { value: "new" } });
  });

  it("a refused call keeps the queue whole and shows TRANSACTION_OPEN translated", async () => {
    const { manager, edit, ids, state } = setup({
      applyChanges: () => {
        throw refusal("TRANSACTION_OPEN", "open");
      },
    });
    const c1 = edit(updateEdit(1, "a"));
    const result = await manager.apply("conn-1");
    expect(result.kind).toBe("refused");
    expect(result.kind === "refused" && result.error).toMatch(/Commit or roll it back first/);
    expect(ids()).toEqual([c1]);
    expect(state.pendingChangesInterrupted["conn-1"]).toBeFalsy();
  });

  it("an in-order apply that ends without an answer keeps the queue, marks it and reloads", async () => {
    const { manager, edit, ids, state, effects } = setup({
      applyChanges: () => {
        throw refusal("NETWORK_ERROR", "Failed to fetch");
      },
    });
    const c1 = edit(updateEdit(1, "a"));
    manager.addSql("conn-1", "CREATE TABLE z (i int)", [], "other", "query-editor");
    const result = await manager.apply("conn-1");
    expect(result.kind).toBe("interrupted");
    expect(ids()).toHaveLength(2);
    expect(ids()[0]).toBe(c1);
    expect(state.pendingChangesInterrupted["conn-1"]).toBe(true);
    expect(effects.reloadSchema).toHaveBeenCalledWith("conn-1");
    expect(effects.refreshDataTabs).toHaveBeenCalledWith("conn-1");

    // Clearing the queue clears the mark.
    manager.clear("conn-1");
    expect(state.pendingChangesInterrupted["conn-1"]).toBe(false);
  });

  it("an atomic batch that ends without an answer is interrupted too: it may have committed", async () => {
    const { manager, edit, ids, state, effects } = setup({
      applyChanges: () => {
        throw refusal("HTTP_502", "Bad Gateway");
      },
    });
    const c1 = edit(updateEdit(1, "a"));
    const c2 = edit(updateEdit(2, "b"));
    expect((await manager.apply("conn-1")).kind).toBe("interrupted");
    expect(ids()).toEqual([c1, c2]);
    expect(state.pendingChangesInterrupted["conn-1"]).toBe(true);
    expect(effects.reloadSchema).toHaveBeenCalledWith("conn-1");
    expect(effects.refreshDataTabs).toHaveBeenCalledWith("conn-1");
  });

  it("an atomic batch stopped by the server closing the workspace is interrupted", async () => {
    const message =
      "The server closed this workspace during the apply, so its changes may not have been saved.";
    const { manager, edit, ids, state, effects } = setup({
      applyChanges: () =>
        outcome({ mode: "atomic", failed: { code: "WORKSPACE_CLOSED", message } }),
    });
    const c1 = edit(updateEdit(1, "a"));
    const c2 = edit(updateEdit(2, "b"));
    const result = await manager.apply("conn-1");
    expect(result.kind).toBe("interrupted");
    expect(result.kind === "interrupted" && result.error).toContain("may not have been saved");
    expect(ids()).toEqual([c1, c2]);
    expect(state.pendingChangesInterrupted["conn-1"]).toBe(true);
    expect(effects.reloadSchema).toHaveBeenCalledWith("conn-1");
    expect(effects.refreshDataTabs).toHaveBeenCalledWith("conn-1");
  });

  it("a disconnected connection sends nothing", async () => {
    const { manager, edit, state, core } = setup();
    edit(updateEdit(1, "a"));
    state.connections = [{ ...state.connections[0], providerConnectionId: undefined as never }];
    expect(await manager.apply("conn-1")).toEqual({
      kind: "refused",
      error: "No connection established",
    });
    expect(core.calls).toEqual([]);
  });

  it("a failed apply logs no message, only the index and the error's code", async () => {
    const canary = "CANARY-7f3a";
    const { manager, edit } = setup({
      applyChanges: (p) =>
        outcome({
          failed: {
            id: p.changes[0].id,
            index: 0,
            code: "QUERY_ERROR",
            message: `duplicate key (email)=(${canary})`,
          },
        }),
    });
    logs.error.mockClear();
    edit(updateEdit(1, canary));
    const result = await manager.apply("conn-1");
    expect(result.kind === "failed" && result.error).toContain(canary);
    const lines = logs.error.mock.calls.map((c) => String(c[0]));
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain("QUERY_ERROR");
    expect(lines[0]).toContain("index 0");
    expect(lines.join("\n")).not.toContain(canary);
  });
});

describe("the sheet's confirmation rule", () => {
  const entry = (sql: string) => ({ sql }) as PendingChange;

  it("lists the destructive statements each change runs, by its position", () => {
    expect(
      listDestructive(
        [
          entry('UPDATE "t" SET "a" = $1 WHERE "id" = $2'),
          entry("DELETE FROM k"),
          entry('TRUNCATE TABLE "t"'),
        ],
        "postgres",
      ),
    ).toEqual([
      { index: 1, sql: "DELETE FROM k", reason: "delete_no_where" },
      { index: 2, sql: 'TRUNCATE TABLE "t"', reason: "truncate" },
    ]);
    expect(listDestructive([entry("DELETE FROM k")], undefined)).toBeNull();
  });

  it("sends confirmed only when the user saw something listed", () => {
    const listed = [{ index: 0, sql: "DELETE FROM k", reason: "delete_no_where" as const }];
    expect(confirmedFor(listed, false)).toBe(true);
    // Nothing listed, or the check failed: Core decides and asks.
    expect(confirmedFor([], false)).toBe(false);
    expect(confirmedFor(null, false)).toBe(false);
    // Core asked and the dialog showed its list.
    expect(confirmedFor(null, true)).toBe(true);
  });
});
