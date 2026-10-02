/**
 * Opening a history row (cleanup pass B): a row recorded with values (an
 * applied grid edit) can't run from the editor, whose runs only know
 * `{{param}}`s, so it goes to the history handler (the pending-changes
 * queue) instead of a tab. A row without values opens in a tab as before.
 */
import { describe, expect, it, vi } from "vitest";
import type { QueryHistoryItem } from "$lib/types";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";

const { DatabaseState } = await import("./state.svelte.js");
const { QueryTabManager } = await import("./query-tabs.svelte.js");

function item(id: string, extra: Partial<QueryHistoryItem> = {}): QueryHistoryItem {
  return {
    id,
    query: "UPDATE t SET a = $1 WHERE id = $2",
    timestamp: new Date(0),
    executionTime: 1,
    rowCount: 1,
    connectionId: "c1",
    favorite: false,
    connectionLabelsSnapshot: [],
    connectionNameSnapshot: "C",
    ...extra,
  };
}

function setup() {
  const state = new DatabaseState();
  state.activeProjectId = "p1";
  state.activeConnectionId = "c1";
  state.queryHistoryByConnection = {
    c1: [item("with", { params: ["x", 1] }), item("without", { query: "SELECT 1" })],
  };
  const tabs = new QueryTabManager(state, {} as TabOrderingManager, () => {});
  const add = vi.spyOn(tabs, "add").mockReturnValue("tab-new");
  const rerun = vi.fn();
  tabs.setHistoryRerun(rerun);
  return { tabs, add, rerun };
}

describe("QueryTabManager.loadFromHistory", () => {
  it("hands a row with values to the history handler and opens no tab", () => {
    const { tabs, add, rerun } = setup();
    const view = vi.fn();
    tabs.loadFromHistory("with", view);
    expect(rerun).toHaveBeenCalledTimes(1);
    expect(rerun.mock.calls[0][0]).toMatchObject({ id: "with", params: ["x", 1] });
    expect(add).not.toHaveBeenCalled();
    expect(view).not.toHaveBeenCalled();
  });

  it("opens a row without values in a tab", () => {
    const { tabs, add, rerun } = setup();
    tabs.loadFromHistory("without");
    expect(rerun).not.toHaveBeenCalled();
    expect(add).toHaveBeenCalledWith(expect.stringContaining("History"), "SELECT 1");
  });
});
