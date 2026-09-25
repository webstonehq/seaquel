import { describe, expect, it } from "vitest";
import { toPersistedDashboard } from "./dashboard-serialize";
import type { Dashboard } from "$lib/types";

describe("toPersistedDashboard", () => {
  it("drops widget results so bigint rows can't break the save", () => {
    const dashboard = {
      id: "d1",
      projectId: "p1",
      name: "Sales",
      viewport: { x: 0, y: 0, zoom: 1 },
      widgets: [
        {
          id: "w1",
          query: "select 1",
          result: [{ total: 9007199254740993n }],
          isLoading: false,
          error: undefined,
          lastRefreshed: new Date(0),
        },
      ],
      starred: false,
      shared: false,
      createdAt: new Date(0),
      updatedAt: new Date(0),
    } as unknown as Dashboard;

    const persisted = toPersistedDashboard(dashboard);
    expect(JSON.parse(persisted.widgets)).toEqual([{ id: "w1", query: "select 1" }]);
    expect(persisted.dateFilter).toBeNull();
    expect(persisted.createdAt).toBe(new Date(0).toISOString());
  });
});
