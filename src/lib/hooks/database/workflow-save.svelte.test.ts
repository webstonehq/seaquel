/**
 * Saved workflows through Core (phase 5d-2): a save is
 * `workflowCreate` (Core's id) or `workflowUpdate`, rename and delete are
 * their own calls, and a workflow the web refuses as too large says
 * so once and stays open.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SavedWorkflow } from "$lib/types/workflow";

const toasts = vi.hoisted(() => [] as string[]);
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.push(m) }));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { WorkflowManager } = await import("./workflow-manager.svelte.js");
const { WorkflowState } = await import("./workflow-state.svelte.js");
const { RecordingLibrary } = await import("./library/recording-library");
const { setLibrary, LibraryCallError } = await import("./library/index");
const { RowSeqs } = await import("./library/seqs");
import type { DatabaseState } from "./state.svelte.js";

let library: InstanceType<typeof RecordingLibrary>;

function setup() {
  const state = $state({
    activeProjectId: "p1" as string | null,
    activeConnectionId: "c1",
    savedWorkflowsByProject: { p1: [], p2: [] } as Record<string, SavedWorkflow[]>,
    librarySeqs: new RowSeqs(),
  });
  const workflowState = new WorkflowState();
  const manager = new WorkflowManager(
    state as unknown as DatabaseState,
    workflowState,
    async () => ({ rows: [], truncated: false }),
  );
  return { state, workflowState, manager };
}

const TOO_LARGE = () =>
  new LibraryCallError(
    "INVALID_ARGUMENT",
    "The workflow is larger than allowed here (max_workflow_bytes: 16777216 bytes).",
  );

beforeEach(() => {
  toasts.length = 0;
  library = new RecordingLibrary();
  library.seedProject("p1");
  library.seedProject("p2");
  setLibrary(library);
});

describe("saved workflows through Core", () => {
  it("a save gets Core's id, and rename and delete are their own calls", async () => {
    const { state, workflowState, manager } = setup();
    manager.addQueryNode("SELECT 1");

    const saved = await manager.saveWorkflow("Flow");
    expect(saved?.id).toMatch(/^workflow-\d+$/);
    expect(workflowState.activeWorkflowId).toBe(saved!.id);
    expect(library.callsOf("createWorkflow")).toHaveLength(1);
    const [projectId, body] = library.callsOf("createWorkflow")[0] as [string, SavedWorkflow];
    expect(projectId).toBe("p1");
    // Core sets these.
    expect(body).not.toHaveProperty("id");
    expect(body).not.toHaveProperty("createdAt");
    expect(state.savedWorkflowsByProject.p1.map((w) => w.name)).toEqual(["Flow"]);

    await manager.saveWorkflow("Flow");
    expect(library.callsOf("updateWorkflow").map(([id]) => id)).toEqual([saved!.id]);

    await manager.renameWorkflow(saved!.id, "Renamed");
    expect(state.savedWorkflowsByProject.p1.map((w) => w.name)).toEqual(["Renamed"]);

    await manager.deleteWorkflow(saved!.id);
    expect(library.callsOf("removeWorkflow")).toEqual([[saved!.id]]);
    expect(state.savedWorkflowsByProject.p1).toEqual([]);
    expect(workflowState.activeWorkflowId).toBeNull();
  });

  it("a stale activeWorkflowId after a project switch updates that workflow, not a copy", async () => {
    const { state, workflowState, manager } = setup();
    manager.addQueryNode("SELECT 1");
    const saved = await manager.saveWorkflow("Flow");

    // Another project is opened; the canvas (global) still shows the workflow.
    state.activeProjectId = "p2";
    await manager.saveWorkflow("Flow");

    expect(library.callsOf("createWorkflow")).toHaveLength(1);
    expect(library.callsOf("updateWorkflow").map(([id]) => id)).toEqual([saved!.id]);
    expect(state.savedWorkflowsByProject.p2).toEqual([]);
    expect(state.savedWorkflowsByProject.p1.map((w) => w.id)).toEqual([saved!.id]);
    expect(workflowState.activeWorkflowId).toBe(saved!.id);
  });

  it("saving after a project switch keeps the workflow's own name", async () => {
    const { state, manager } = setup();
    manager.addQueryNode("SELECT 1");
    const saved = (await manager.saveWorkflow("Flow"))!;
    state.activeProjectId = "p2";

    // The sidebar's Save names nothing: the workflow's own name is kept.
    await manager.saveWorkflow();

    expect(state.savedWorkflowsByProject.p1.map((w) => [w.id, w.name])).toEqual([
      [saved.id, "Flow"],
    ]);
  });

  it("a new workflow saved without a name gets one", async () => {
    const { state, manager } = setup();
    manager.addQueryNode("SELECT 1");
    await manager.saveWorkflow();
    expect(state.savedWorkflowsByProject.p1[0]?.name).toMatch(/^Workflow /);
  });

  it("a workflow past a count limit says so in its own words", async () => {
    const { manager } = setup();
    manager.addQueryNode("SELECT 1");
    library.failures.set("createWorkflow", {
      error: new LibraryCallError(
        "INVALID_ARGUMENT",
        "There are more saved workflows than allowed here (max_workflows: 1000).",
      ),
    });
    await manager.saveWorkflow("Many");
    expect(toasts).toHaveLength(1);
    expect(toasts[0]).toContain("max_workflows");
    expect(toasts[0]).not.toMatch(/results/);
  });

  it("a workflow too large to save says so and keeps it open", async () => {
    const { state, workflowState, manager } = setup();
    manager.addQueryNode("SELECT 1");
    library.failures.set("createWorkflow", { error: TOO_LARGE(), sticky: true });

    expect(await manager.saveWorkflow("Big")).toBeNull();
    expect(toasts).toHaveLength(1);
    expect(toasts[0]).toContain("Big");
    expect(toasts[0]).toContain("max_workflow_bytes");
    // Still open, not saved, nothing lost.
    expect(workflowState.nodes).toHaveLength(1);
    expect(state.savedWorkflowsByProject.p1).toEqual([]);

    // Once per workflow: saving it again doesn't toast again.
    expect(await manager.saveWorkflow("Big")).toBeNull();
    expect(toasts).toHaveLength(1);
  });
});
