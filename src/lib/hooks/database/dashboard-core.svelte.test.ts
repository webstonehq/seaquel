/**
 * Dashboards through Core (phase 5d-2): Core makes the id,
 * each edit is a patch of what it changed, and the GUI says which edits
 * are versioned (`captureVersion`); Core numbers and prunes the versions,
 * and the page shows the version and the prune from the answer. A save
 * refused for a web limit, or because storage is full, says so.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DashboardWidget } from "$lib/types";

const toasts = vi.hoisted(() => [] as string[]);
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.push(m) }));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { DashboardManager } = await import("./dashboard-manager.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");
const { RecordingLibrary } = await import("./library/recording-library");
const { setLibrary, LibraryCallError } = await import("./library/index");

let library: InstanceType<typeof RecordingLibrary>;

const widget = (id: string): DashboardWidget =>
  ({
    id,
    type: "chart",
    title: id,
    x: 0,
    y: 0,
    width: 4,
    height: 3,
    query: "SELECT 1",
    querySource: "inline",
  }) as unknown as DashboardWidget;

function setup() {
  const state = new DatabaseState();
  state.activeProjectId = "p1";
  state.dashboardsByProject = { p1: [] };
  const manager = new DashboardManager(
    state,
    async () => [],
    () => {},
  );
  return { state, manager };
}

/** The patches the page sent, in order. */
const patches = () =>
  library.callsOf("updateDashboard").map(([, patch]) => patch as Record<string, unknown>);

beforeEach(() => {
  toasts.length = 0;
  library = new RecordingLibrary();
  library.seedProject("p1");
  setLibrary(library);
});

describe("dashboards through Core", () => {
  it("a dashboard move records no version, a widget edit does", async () => {
    const { state, manager } = setup();
    const dashboard = (await manager.createDashboard("Sales"))!;
    expect(dashboard.id).toMatch(/^dashboard-/);
    expect(library.callsOf("createDashboard")).toHaveLength(1);

    await manager.addWidget(dashboard.id, widget("w1"));
    await manager.moveWidget(dashboard.id, "w1", { x: 5, y: 5 });
    await manager.resizeWidget(dashboard.id, "w1", { width: 6, height: 4 });
    await manager.updateViewport(dashboard.id, { x: 1, y: 2, zoom: 1.5 });
    await manager.updateWidget(dashboard.id, "w1", { title: "Revenue" });
    await manager.renameDashboard(dashboard.id, "Sales 2");
    await manager.toggleDashboardStarred(dashboard.id);

    expect(patches().map((p) => [Object.keys(p).sort().join(","), !!p.captureVersion])).toEqual([
      ["captureVersion,widgets", true],
      ["widgets", false],
      ["widgets", false],
      ["viewport", false],
      ["captureVersion,widgets", true],
      ["captureVersion,name", true],
      ["starred", false],
    ]);
    // Widgets go without their run state.
    const sent = patches()[0].widgets as Record<string, unknown>[];
    expect(sent[0]).not.toHaveProperty("result");

    // Core numbered the versions; the page shows them from the answers.
    expect(state.dashboardVersionsByProject.p1.map((v) => v.version)).toEqual([1, 2, 3]);
  });

  it("a dashboard too large to save says so once, and a full storage says so", async () => {
    const { manager } = setup();
    const dashboard = (await manager.createDashboard("Big"))!;
    library.failures.set("updateDashboard", {
      error: new LibraryCallError(
        "INVALID_ARGUMENT",
        "The dashboard's widgets, viewport and filter are larger than allowed here (max_dashboard_bytes: 4194304 bytes).",
      ),
      sticky: true,
    });

    await manager.addWidget(dashboard.id, widget("w1"));
    await manager.addWidget(dashboard.id, widget("w2"));
    expect(toasts).toHaveLength(1);
    expect(toasts[0]).toContain("Big");
    expect(toasts[0]).toContain("max_dashboard_bytes");

    library.failures.set("updateDashboard", {
      error: new LibraryCallError("STORAGE_FULL", "The user's storage is full."),
    });
    await manager.renameDashboard(dashboard.id, "Bigger");
    expect(toasts).toHaveLength(2);
    expect(toasts[1]).toMatch(/size limit/);
  });
});
