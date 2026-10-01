/**
 * Workflow query nodes (phase 5c, Decision 10): read-only on the node's own
 * saved connection, at most `WORKFLOW_MAX_ROWS` rows, cancelled when the
 * node is re-run or deleted.
 */
import { describe, expect, it, vi } from "vitest";
import type { WorkflowQueryNodeData, WorkflowResultNodeData } from "$lib/types/workflow";
import type { ReadOnlyRows } from "$lib/providers";
import type { DatabaseState } from "./state.svelte.js";
import { WorkflowState } from "./workflow-state.svelte.js";
import { READ_ONLY_REFUSAL } from "$lib/sql";

const { WorkflowManager, WORKFLOW_MAX_ROWS } = await import("./workflow-manager.svelte.js");

type Run = (
  connectionId: string,
  sql: string,
  signal: AbortSignal,
  maxRows: number,
) => Promise<ReadOnlyRows>;

function setup(run: Run) {
  const state = $state({ activeConnectionId: "conn-pg" as string | null });
  const workflowState = new WorkflowState();
  const executeQuery = vi.fn(run);
  const manager = new WorkflowManager(
    state as unknown as DatabaseState,
    workflowState,
    executeQuery,
  );
  const data = (id: string) => workflowState.getNode(id)?.data;
  const result = (queryNodeId: string) =>
    workflowState.nodes.find(
      (n) =>
        n.data.type === "result" &&
        (n.data as WorkflowResultNodeData).sourceQueryNodeId === queryNodeId,
    )?.data as WorkflowResultNodeData | undefined;
  return { state, manager, executeQuery, data, result };
}

describe("workflow query nodes", () => {
  it("a workflow node runs read-only on its own connection and shows truncated", async () => {
    const { state, manager, executeQuery, result } = setup(async () => ({
      rows: [{ n: 1 }, { n: 2 }],
      truncated: true,
    }));
    const nodeId = manager.addQueryNode("SELECT n FROM big");
    // Another connection becomes active: the node keeps the one it was made on.
    state.activeConnectionId = "conn-mysql";
    await manager.executeQueryNode(nodeId);

    expect(executeQuery).toHaveBeenCalledOnce();
    const [connectionId, sql, signal, maxRows] = executeQuery.mock.calls[0];
    expect([connectionId, sql, maxRows]).toEqual(["conn-pg", "SELECT n FROM big", 10_000]);
    expect(WORKFLOW_MAX_ROWS).toBe(10_000);
    expect(signal).toBeInstanceOf(AbortSignal);
    expect(result(nodeId)).toMatchObject({
      columns: ["n"],
      rows: [[1], [2]],
      totalRows: 2,
      truncated: true,
    });
  });

  it("a write in a node is refused as read-only, with a message saying so", async () => {
    const { manager, data } = setup(async () => {
      throw new Error(READ_ONLY_REFUSAL);
    });
    const nodeId = manager.addQueryNode("DELETE FROM e");
    await manager.executeQueryNode(nodeId);
    const node = data(nodeId) as WorkflowQueryNodeData;
    expect(node.isExecuting).toBe(false);
    expect(node.error).toMatch(/read-only/);

    // The database's own refusal reads the same.
    const other = setup(async () => {
      throw new Error("READ_ONLY: cannot execute INSERT in a read-only transaction");
    });
    const second = other.manager.addQueryNode("INSERT INTO e VALUES (1)");
    await other.manager.executeQueryNode(second);
    expect((other.data(second) as WorkflowQueryNodeData).error).toMatch(/read-only/);
  });

  it("any other error shows as it is", async () => {
    const { manager, data } = setup(async () => {
      throw new Error('The connection "PG" is disconnected; reconnect it and try again');
    });
    const nodeId = manager.addQueryNode("SELECT 1");
    await manager.executeQueryNode(nodeId);
    expect((data(nodeId) as WorkflowQueryNodeData).error).toBe(
      'The connection "PG" is disconnected; reconnect it and try again',
    );
  });

  it("re-running a node cancels the previous run, whose answer is dropped", async () => {
    const answers: Array<(rows: ReadOnlyRows) => void> = [];
    const { manager, executeQuery, result } = setup(
      (_c, _s, _signal) => new Promise((resolve) => answers.push(resolve)),
    );
    const nodeId = manager.addQueryNode("SELECT n");
    const first = manager.executeQueryNode(nodeId);
    const second = manager.executeQueryNode(nodeId);
    const [firstSignal, secondSignal] = executeQuery.mock.calls.map((c) => c[2]);
    expect(firstSignal.aborted).toBe(true);
    expect(secondSignal.aborted).toBe(false);

    answers[1]({ rows: [{ n: "new" }], truncated: false });
    await second;
    answers[0]({ rows: [{ n: "old" }], truncated: false });
    await first;
    expect(result(nodeId)?.rows).toEqual([["new"]]);
  });

  it("deleting a node cancels its run", async () => {
    const { manager, executeQuery } = setup(() => new Promise(() => {}));
    const nodeId = manager.addQueryNode("SELECT n");
    void manager.executeQueryNode(nodeId);
    manager.removeNode(nodeId);
    expect(executeQuery.mock.calls[0][2].aborted).toBe(true);
  });
});
