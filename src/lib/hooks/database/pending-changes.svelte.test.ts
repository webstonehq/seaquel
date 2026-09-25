import { describe, expect, it, vi } from "vitest";
import type { PendingChange } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "./state.svelte.js";
import type { QueryHistoryManager } from "./query-history.svelte.js";

vi.mock("$lib/stores/pending-changes-settings.svelte.js", () => ({
  pendingChangesSettingsStore: { enabled: true },
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn() },
}));

const { PendingChangesManager } = await import("./pending-changes.svelte.js");

function change(id: string, origin: PendingChange["origin"], extra: Partial<PendingChange> = {}) {
  return {
    id,
    connectionId: "conn-1",
    sql: `SQL ${id}`,
    queryType: origin === "delete-row" ? "delete" : "update",
    addedAt: new Date(),
    description: id,
    origin,
    ...extra,
  } as PendingChange;
}

const keyed = (id: string, origin: PendingChange["origin"] = "inline-edit") =>
  change(id, origin, {
    target: { schema: "public", table: "users", column: "name", primaryKeyValues: { id: 7 } },
  });

function makeManager(changes: PendingChange[], rowsAffected: number[]) {
  const state = {
    connections: [{ id: "conn-1", type: "postgres", providerConnectionId: "pc-1" }],
    pendingChangesByConnection: { "conn-1": changes },
  } as unknown as DatabaseState;
  const execute = vi.fn(async () => ({ rowsAffected: rowsAffected.shift() ?? 1 }));
  const providers = {
    getForType: vi.fn(async () => ({ execute })),
  } as unknown as ProviderRegistry;
  const history = { addToHistory: vi.fn() } as unknown as QueryHistoryManager;
  return { manager: new PendingChangesManager(state, providers, history), state, execute };
}

describe("PendingChangesManager.executeAll", () => {
  it("stops at a keyed edit that matched no row and keeps it and the rest pending", async () => {
    const changes = [keyed("a"), keyed("b", "set-default"), keyed("c", "delete-row")];
    const { manager, state, execute } = makeManager(changes, [1, 0]);

    const result = await manager.executeAll("conn-1");

    expect(result).toMatchObject({ executed: 1, failed: 1, failedAt: 1, failedChangeId: "b" });
    expect(result.error).toContain("public.users");
    expect(result.error).toContain("id = 7");
    expect(execute).toHaveBeenCalledTimes(2);
    expect(state.pendingChangesByConnection["conn-1"].map((c) => c.id)).toEqual(["b", "c"]);
  });

  it("fails a keyed delete that matched no row", async () => {
    const { manager } = makeManager([keyed("d", "delete-row")], [0]);
    expect(await manager.executeAll("conn-1")).toMatchObject({ failed: 1, failedChangeId: "d" });
  });

  it("lets 0-row statements through when no row was expected", async () => {
    const changes = [
      change("q", "query-editor"),
      change("i", "insert-row"),
      change("t", "alter-table", { queryType: "other" }),
    ];
    const { manager, state } = makeManager(changes, [0, 0, 0]);

    expect(await manager.executeAll("conn-1")).toEqual({ executed: 3, failed: 0, hasDdl: true });
    expect(state.pendingChangesByConnection["conn-1"]).toHaveLength(3);
  });
});
