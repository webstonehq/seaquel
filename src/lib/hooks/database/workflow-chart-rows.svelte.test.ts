/**
 * Chart nodes are stored without the rows their source holds:
 * the save drops the copy, the load fills it back from the
 * source. Workflows saved before 5d-2 keep their copies until saved again.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  SavedWorkflow,
  SavedWorkflowSummary,
  SerializedWorkflowNode,
  WorkflowChartNodeData,
  WorkflowResultNodeData,
} from "$lib/types/workflow";
import type { DatabaseState } from "./state.svelte.js";
import { WorkflowState } from "./workflow-state.svelte.js";

const { WorkflowManager } = await import("./workflow-manager.svelte.js");
const { RecordingLibrary } = await import("./library/recording-library");
const { setLibrary } = await import("./library/index");
const { RowSeqs } = await import("./library/seqs");
const { fromStorable, toStorable } = await import("$lib/values");

vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
const { errorToast } = await import("$lib/utils/toast");

let library: InstanceType<typeof RecordingLibrary>;
beforeEach(() => {
  library = new RecordingLibrary();
  library.seedProject("p");
  setLibrary(library);
});

const chartConfig = {
  type: "bar",
  xAxis: "n",
  yAxis: ["v"],
} as unknown as WorkflowChartNodeData["chartConfig"];

const result = (id: string, rows: unknown[][]): SerializedWorkflowNode => ({
  id,
  type: "result",
  position: { x: 0, y: 0 },
  data: {
    type: "result",
    sourceQueryNodeId: "q",
    columns: ["n", "v"],
    rows,
    totalRows: rows.length,
  } satisfies WorkflowResultNodeData,
});

const chart = (id: string, sourceNodeId: string, rows: unknown[][]): SerializedWorkflowNode => ({
  id,
  type: "chart",
  position: { x: 400, y: 0 },
  data: {
    type: "chart",
    sourceNodeId,
    columns: rows.length ? ["n", "v"] : [],
    rows,
    chartConfig,
  } satisfies WorkflowChartNodeData,
});

const ROWS = [
  [1, 10n],
  [2, 20n],
];

const summary = (w: SavedWorkflow): SavedWorkflowSummary => ({
  id: w.id,
  name: w.name,
  projectId: w.projectId,
  createdAt: w.createdAt,
  updatedAt: w.updatedAt,
});

function setup(saved: SavedWorkflow[] = []) {
  const state = $state({
    activeProjectId: "p",
    activeConnectionId: "conn",
    // The page lists workflows without their bodies.
    savedWorkflowsByProject: { p: saved.map(summary) } as Record<string, SavedWorkflowSummary[]>,
    librarySeqs: new RowSeqs(),
  });
  // Saves go through the library (5d-2): it holds the bodies.
  library.workflows.set(
    "p",
    saved.map((w) => toStorable(w)),
  );
  const workflowState = new WorkflowState();
  const manager = new WorkflowManager(
    state as unknown as DatabaseState,
    workflowState,
    async () => ({ rows: [], truncated: false }),
  );
  /** The workflow as stored, decoded. */
  const stored = (id: string) =>
    fromStorable(
      (library.workflows.get("p") ?? []).find((w) => (w as { id: string }).id === id),
    ) as SavedWorkflow;
  const storedNode = (workflowId: string, nodeId: string) =>
    stored(workflowId).nodes.find((n) => n.id === nodeId)!.data;
  const liveNode = (nodeId: string) => workflowState.getNode(nodeId)!.data;
  return { state, workflowState, manager, stored, storedNode, liveNode };
}

function workflow(nodes: SerializedWorkflowNode[]): SavedWorkflow {
  return {
    id: "workflow-1",
    name: "W",
    projectId: "p",
    nodes,
    edges: [],
    viewport: { x: 0, y: 0, zoom: 1 },
    createdAt: "2026-01-01T00:00:00.000Z",
    updatedAt: "2026-01-01T00:00:00.000Z",
  };
}

describe("chart rows in saved workflows", () => {
  it("a saved chart node stores no rows when its source holds them", async () => {
    const { workflowState, manager, stored } = setup();
    workflowState.nodes = [result("r", ROWS), chart("c", "r", ROWS)].map((n) => ({ ...n }));

    const saved = stored((await manager.saveWorkflow("W"))!.id);

    const storedChart = saved.nodes.find((n) => n.id === "c")!.data as WorkflowChartNodeData;
    expect(storedChart.rows).toEqual([]);
    // Its columns and config stay; the source keeps its rows.
    expect(storedChart.columns).toEqual(["n", "v"]);
    expect(storedChart.chartConfig).toEqual(chartConfig);
    expect((saved.nodes.find((n) => n.id === "r")!.data as WorkflowResultNodeData).rows).toEqual(
      ROWS,
    );
    // The canvas on screen keeps showing the chart.
    expect((workflowState.getNode("c")!.data as WorkflowChartNodeData).rows).toEqual(ROWS);
  });

  it("a chart node reopens with its source's rows", async () => {
    const { manager, workflowState, liveNode } = setup();
    workflowState.nodes = [result("r", ROWS), chart("c", "r", ROWS)].map((n) => ({ ...n }));
    const saved = (await manager.saveWorkflow("W"))!;
    manager.clearWorkflow();

    await manager.loadWorkflow(saved.id);

    const c = liveNode("c") as WorkflowChartNodeData;
    expect(c.rows).toEqual(ROWS);
    expect(c.columns).toEqual(["n", "v"]);
    expect(c.chartConfig).toEqual(chartConfig);
  });

  it("a workflow saved with chart copies still loads, and its next save drops them", async () => {
    const old = workflow([result("r", ROWS), chart("c", "r", [[9, 99n]])]);
    const { manager, liveNode, storedNode } = setup([old]);

    await manager.loadWorkflow("workflow-1");
    // Loaded as stored: the copy is used as it is.
    expect((liveNode("c") as WorkflowChartNodeData).rows).toEqual([[9, 99n]]);

    await manager.saveWorkflow("W");
    expect((storedNode("workflow-1", "c") as WorkflowChartNodeData).rows).toEqual([]);
  });

  it("a chart whose source isn't in the workflow keeps its rows", async () => {
    const { workflowState, manager, stored } = setup();
    workflowState.nodes = [chart("c", "gone", ROWS), chart("d", "r", ROWS), result("r", [])].map(
      (n) => ({ ...n }),
    );

    const saved = stored((await manager.saveWorkflow("W"))!.id);

    const data = (id: string) =>
      (saved.nodes.find((n) => n.id === id)!.data as WorkflowChartNodeData).rows;
    expect(data("c")).toEqual(ROWS);
    // A source with no rows has nothing to rebuild the chart from either.
    expect(data("d")).toEqual(ROWS);
  });

  it("a chart fed by another chart keeps its rows", async () => {
    const { workflowState, manager, liveNode, stored } = setup();
    workflowState.nodes = [result("r", ROWS), chart("c", "r", ROWS), chart("cc", "c", ROWS)].map(
      (n) => ({ ...n }),
    );

    const saved = stored((await manager.saveWorkflow("W"))!.id);

    const rows = (id: string) =>
      (saved.nodes.find((n) => n.id === id)!.data as WorkflowChartNodeData).rows;
    expect(rows("c")).toEqual([]);
    expect(rows("cc")).toEqual(ROWS);
    manager.clearWorkflow();
    await manager.loadWorkflow(saved.id);
    expect((liveNode("cc") as WorkflowChartNodeData).rows).toEqual(ROWS);
  });
});

/**
 * 5d-2 Task 7 probe fix: `workflowsList` answers no bodies, so the page
 * lists workflows without them and reads one when it's opened or renamed.
 */
describe("a saved workflow's body is read when it's opened", () => {
  const nodes = () => [result("r", ROWS), chart("c", "r", [])];

  it("opening a workflow fetches its body", async () => {
    const { manager, state, workflowState, liveNode } = setup([workflow(nodes())]);
    expect(state.savedWorkflowsByProject.p[0]).not.toHaveProperty("nodes");

    expect(await manager.loadWorkflow("workflow-1")).toBe(true);

    expect(library.callsOf("getWorkflow")).toEqual([["workflow-1"]]);
    expect(workflowState.nodes.map((n) => n.id)).toEqual(["r", "c"]);
    // bigint cells come back decoded; the chart is filled from its source.
    expect((liveNode("c") as WorkflowChartNodeData).rows).toEqual(ROWS);
    expect(workflowState.activeWorkflowId).toBe("workflow-1");
  });

  it("only the latest open lands", async () => {
    const second = { ...workflow([result("x", [])]), id: "workflow-2", name: "Other" };
    const { manager, workflowState } = setup([workflow(nodes()), second]);
    const get = library.getWorkflow.bind(library);
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    library.getWorkflow = async (id: string) => {
      if (id === "workflow-1") await held;
      return get(id);
    };
    const first = manager.loadWorkflow("workflow-1");
    expect(await manager.loadWorkflow("workflow-2")).toBe(true);
    release();
    expect(await first).toBe(false);
    expect(workflowState.activeWorkflowId).toBe("workflow-2");
    expect(workflowState.nodes.map((n) => n.id)).toEqual(["x"]);
  });

  it("a workflow deleted while it's being opened never becomes the open one", async () => {
    const { manager, workflowState } = setup([workflow(nodes())]);
    workflowState.nodes = [{ ...result("mine", []) }];
    const get = library.getWorkflow.bind(library);
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    library.getWorkflow = async (id: string) => {
      const read = await get(id);
      await held;
      return read;
    };
    const open = manager.loadWorkflow("workflow-1");
    await manager.deleteWorkflow("workflow-1");
    release();

    expect(await open).toBe(false);
    expect(workflowState.activeWorkflowId).toBeNull();
    expect(workflowState.nodes.map((n) => n.id)).toEqual(["mine"]);
  });

  it("one deleted elsewhere says so, keeps the canvas and leaves the list", async () => {
    const { manager, state, workflowState } = setup([workflow(nodes())]);
    workflowState.nodes = [{ ...result("mine", []) }];
    library.workflows.set("p", []);

    expect(await manager.loadWorkflow("workflow-1")).toBe(false);

    expect(errorToast).toHaveBeenCalled();
    expect(workflowState.nodes.map((n) => n.id)).toEqual(["mine"]);
    await vi.waitFor(() => expect(state.savedWorkflowsByProject.p).toEqual([]));
  });

  it("a workflow deleted elsewhere while open still unlinks", async () => {
    const { manager, state, workflowState } = setup([workflow(nodes())]);
    await manager.loadWorkflow("workflow-1");
    library.workflows.set("p", []);
    library.n += 1;

    await manager.refreshFromLibrary("p", ["workflow-1"]);

    expect(state.savedWorkflowsByProject.p).toEqual([]);
    expect(workflowState.activeWorkflowId).toBeNull();
    // The canvas keeps what it showed.
    expect(workflowState.nodes.map((n) => n.id)).toEqual(["r", "c"]);
  });

  it("a rename changes only the name, without reading or writing the body", async () => {
    const old = workflow([result("r", ROWS), chart("c", "r", [[9, 99n]])]);
    const { manager, state, storedNode } = setup([old]);

    await manager.renameWorkflow("workflow-1", "Renamed");

    expect(library.callsOf("renameWorkflow")).toEqual([["workflow-1", "Renamed"]]);
    expect(library.callsOf("getWorkflow")).toEqual([]);
    expect(library.callsOf("updateWorkflow")).toEqual([]);
    expect(state.savedWorkflowsByProject.p.map((w) => w.name)).toEqual(["Renamed"]);
    // As stored: a pre-5d-2 chart copy stays (only saveWorkflow drops it).
    expect((storedNode("workflow-1", "c") as WorkflowChartNodeData).rows).toEqual([[9, 99n]]);
  });

  it("a save another tab makes between two renames isn't lost", async () => {
    const { manager, state, stored } = setup([workflow(nodes())]);
    await manager.renameWorkflow("workflow-1", "First");
    /** Another tab's save of new nodes, landing while the next rename runs. */
    const otherSave = () =>
      library.updateWorkflow(
        "workflow-1",
        toStorable({ name: "First", nodes: [result("other", [])], edges: [], viewport: {} }),
      );
    // Whatever the rename sends, the other save lands after it read anything.
    const get = library.getWorkflow.bind(library);
    library.getWorkflow = async (id: string) => {
      const read = await get(id);
      await otherSave();
      return read;
    };
    const rename = library.renameWorkflow.bind(library);
    library.renameWorkflow = async (id: string, name: string) => {
      await otherSave();
      return rename(id, name);
    };

    await manager.renameWorkflow("workflow-1", "Second");

    expect(stored("workflow-1").nodes.map((n) => n.id)).toEqual(["other"]);
    expect(stored("workflow-1").name).toBe("Second");
    expect(state.savedWorkflowsByProject.p.map((w) => w.name)).toEqual(["Second"]);
  });

  it("saving the open workflow updates it under its listed name", async () => {
    const { manager, state, workflowState } = setup([workflow(nodes())]);
    await manager.loadWorkflow("workflow-1");
    workflowState.nodes = [...workflowState.nodes, { ...result("r2", []) }];

    await manager.saveWorkflow();

    expect(library.callsOf("createWorkflow")).toEqual([]);
    const [[id, body]] = library.callsOf("updateWorkflow") as [string, { name: string }][];
    expect([id, body.name]).toEqual(["workflow-1", "W"]);
    expect(state.savedWorkflowsByProject.p.map((w) => w.id)).toEqual(["workflow-1"]);
  });
});
