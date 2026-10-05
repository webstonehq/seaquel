/**
 * The DuckDB extensions tab sends typed actions (`db.duckdbExtension` on desktop)
 * on the connection the tab was opened for, whatever
 * is active later.
 */
import { describe, expect, it, vi } from "vitest";
import type { ExtensionsDuckdbTab } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import type { ExtensionAction } from "$lib/types/generated/ExtensionAction";

const { ExtensionsDuckdbTabManager } = await import("./extensions-duckdb-tabs.svelte.js");

describe("ExtensionsDuckdbTabManager", () => {
  it("the extensions tab sends typed actions on the tab's connection, not the active one", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({ ok: true, text: async () => "" })),
    );
    try {
      const a = { id: "duck-a", type: "duckdb", name: "A", providerConnectionId: "pc-a" };
      const b = { id: "duck-b", type: "duckdb", name: "B", providerConnectionId: "pc-b" };
      const state = $state({
        activeProjectId: "p",
        activeConnectionId: "duck-a",
        activeConnection: a as typeof a | typeof b,
        connections: [a, b],
        extensionsDuckdbTabsByProject: {} as Record<string, ExtensionsDuckdbTab[]>,
        activeExtensionsDuckdbTabIdByProject: {} as Record<string, string | null>,
      });
      const runAction = vi.fn(async (_connectionId: string, action: ExtensionAction) =>
        action.type === "list" ? [{ extension_name: "json", loaded: true, installed: true }] : null,
      );
      const manager = new ExtensionsDuckdbTabManager(
        state as unknown as DatabaseState,
        { add: vi.fn() } as unknown as TabOrderingManager,
        () => {},
        () => {},
        runAction,
      );

      const tabId = (await manager.add())!;
      state.activeConnectionId = "duck-b";
      state.activeConnection = b;
      runAction.mockClear();

      await manager.installExtension(tabId, "httpfs");
      await manager.loadExtension(tabId, "httpfs");
      await manager.updateExtension(tabId, "httpfs");
      await manager.installCommunityExtension(tabId, "h3");
      await manager.installAndLoadExtension(tabId, "json");
      await manager.refresh(tabId);

      for (const [connectionId] of runAction.mock.calls) expect(connectionId).toBe("duck-a");
      // Each action, then the listing it refreshes; then refresh's own listing.
      const list = ["duck-a", { type: "list" }];
      expect(runAction.mock.calls).toEqual([
        ["duck-a", { type: "install", name: "httpfs" }],
        list,
        ["duck-a", { type: "load", name: "httpfs" }],
        list,
        ["duck-a", { type: "update", name: "httpfs" }],
        list,
        ["duck-a", { type: "installCommunity", name: "h3" }],
        list,
        ["duck-a", { type: "installAndLoad", name: "json" }],
        list,
        list,
      ]);
      expect(state.extensionsDuckdbTabsByProject.p[0].extensions?.[0]).toMatchObject({
        extension_name: "json",
        loaded: true,
      });
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("shows an action's refusal on the tab", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({ ok: true, text: async () => "" })),
    );
    try {
      const a = { id: "duck-a", type: "duckdb", name: "A", providerConnectionId: "pc-a" };
      const state = $state({
        activeProjectId: "p",
        activeConnectionId: "duck-a",
        activeConnection: a,
        connections: [a],
        extensionsDuckdbTabsByProject: {} as Record<string, ExtensionsDuckdbTab[]>,
        activeExtensionsDuckdbTabIdByProject: {} as Record<string, string | null>,
      });
      const runAction = vi.fn(async (_c: string, action: ExtensionAction) => {
        if (action.type === "list") return [];
        throw new Error("INVALID_ARGUMENT: Invalid extension name: a;b");
      });
      const manager = new ExtensionsDuckdbTabManager(
        state as unknown as DatabaseState,
        { add: vi.fn() } as unknown as TabOrderingManager,
        () => {},
        () => {},
        runAction,
      );
      const tabId = (await manager.add())!;
      await manager.installExtension(tabId, "a;b");
      expect(state.extensionsDuckdbTabsByProject.p[0].error).toBe(
        "INVALID_ARGUMENT: Invalid extension name: a;b",
      );
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
