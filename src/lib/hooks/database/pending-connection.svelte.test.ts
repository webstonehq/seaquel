/**
 * The pending-changes sheet and the header badge show the queue of the
 * connection the user is looking at: the focused data tab's, or the
 * focused query result's, else the active connection's (phase 5c, Task 1
 * follow-up). Edits queued from a data tab on another connection used to be
 * invisible there.
 */
import { describe, expect, it } from "vitest";
import type { DataTab, PendingChange, QueryTab, StatementResult } from "$lib/types";

const { DatabaseState } = await import("./state.svelte.js");

const change = (id: string, connectionId: string) =>
  ({
    id,
    connectionId,
    sql: "UPDATE",
    queryType: "update",
    origin: "inline-edit",
  }) as PendingChange;

function setup() {
  const state = new DatabaseState();
  state.activeProjectId = "p";
  state.activeConnectionIdByProject = { p: "conn-b" };
  state.pendingChangesByConnection = {
    "conn-a": [change("a1", "conn-a"), change("a2", "conn-a")],
    "conn-b": [change("b1", "conn-b")],
  };
  state.dataTabsByProject = {
    p: [{ id: "data-1", connectionId: "conn-a" } as DataTab],
  };
  state.activeDataTabIdByProject = { p: "data-1" };
  state.queryTabsByProject = {
    p: [
      {
        id: "q-1",
        results: [{ connectionId: "conn-a" } as StatementResult],
        activeResultIndex: 0,
      } as QueryTab,
    ],
  };
  state.activeQueryTabIdByProject = { p: "q-1" };
  return state;
}

describe("pendingConnectionId", () => {
  it("is the focused data tab's connection", () => {
    const state = setup();
    state.activeView = "data";
    expect(state.pendingConnectionId).toBe("conn-a");
    expect(state.activePendingChanges.map((c) => c.id)).toEqual(["a1", "a2"]);
    expect(state.activePendingChangesCount).toBe(2);
  });

  it("is the focused query result's connection", () => {
    const state = setup();
    state.activeView = "query";
    expect(state.pendingConnectionId).toBe("conn-a");
    expect(state.activePendingChangesCount).toBe(2);
  });

  it("falls back to the active connection", () => {
    const state = setup();
    state.activeView = "schema";
    expect(state.pendingConnectionId).toBe("conn-b");
    expect(state.activePendingChanges.map((c) => c.id)).toEqual(["b1"]);

    // A query tab with no result yet, or a result without a connection.
    state.activeView = "query";
    state.queryTabsByProject = { p: [{ id: "q-1", results: [] } as unknown as QueryTab] };
    expect(state.pendingConnectionId).toBe("conn-b");

    state.activeView = "data";
    state.activeDataTabIdByProject = { p: null };
    expect(state.pendingConnectionId).toBe("conn-b");
  });
});

describe("a history re-run's focus (cleanup pass B review)", () => {
  it("shows the history row's queue while the sheet is open, over a data tab on another connection", () => {
    const state = setup();
    state.activeView = "data";
    expect(state.pendingConnectionId).toBe("conn-a");
    state.pendingFocusConnectionId = "conn-b";
    state.isPendingChangesOpen = true;
    expect(state.pendingConnectionId).toBe("conn-b");
    expect(state.activePendingChanges.map((c) => c.id)).toEqual(["b1"]);
  });

  it("ends when the sheet closes", () => {
    const state = setup();
    state.activeView = "data";
    state.pendingFocusConnectionId = "conn-b";
    state.isPendingChangesOpen = true;
    state.activeRightPanel = null;
    expect(state.pendingFocusConnectionId).toBeNull();
    state.isPendingChangesOpen = true;
    expect(state.pendingConnectionId).toBe("conn-a");
  });

  it("ends when the AI panel replaces the sheet", () => {
    const state = setup();
    state.pendingFocusConnectionId = "conn-b";
    state.isPendingChangesOpen = true;
    state.isAIOpen = true;
    expect(state.pendingFocusConnectionId).toBeNull();
  });
});
