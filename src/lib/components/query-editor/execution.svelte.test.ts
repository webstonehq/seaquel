import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DatabaseType } from "$lib/types";
import type { QueryEditorContext } from "./types.js";
import type { ParamDialog } from "./param-dialog.svelte.js";
import type { SeaquelWasm } from "$lib/wasm";

const errorToast = vi.fn();
vi.mock("$lib/utils/toast", () => ({ errorToast: (msg: string) => errorToast(msg) }));

// The real module, with the exports named in `failing` trapping, as a panic
// in seaquel-wasm would.
const failing = new Set<string>();
vi.mock("$lib/wasm", async (importOriginal) => {
  const w = await importOriginal<typeof import("$lib/wasm")>();
  const trap = () => {
    throw new WebAssembly.RuntimeError("unreachable");
  };
  return {
    ...w,
    callWasm: <T>(fn: (m: SeaquelWasm) => T): T =>
      w.callWasm((m) =>
        fn(
          new Proxy(m, {
            get: (t, k) => (typeof k === "string" && failing.has(k) ? trap : Reflect.get(t, k)),
          }),
        ),
      ),
  };
});

const { createExecution } = await import("./execution.svelte.js");

function setup(query: string, type: DatabaseType = "postgres", cursorOffset = 0) {
  const queries = { execute: vi.fn(), executeCurrent: vi.fn() };
  const ctx = {
    db: { state: { activeConnection: { type } }, queries },
    getActiveTab: () => ({ id: "tab-1", query }),
    getActiveTabId: () => "tab-1",
    getMonacoRef: () => ({ getCursorOffset: () => cursorOffset, insertText: () => {} }),
  } as unknown as QueryEditorContext;
  const paramDialog = {
    show: false,
    params: [],
    action: null,
    getParameterDefinitions: () => [],
  } as unknown as ParamDialog;
  const execution = createExecution(ctx, paramDialog, () => {});
  return { execution, queries };
}

beforeEach(() => {
  failing.clear();
  errorToast.mockClear();
});

describe("destructive check before running", () => {
  // Fix 11: the check skips comments, so the WHERE in the comment doesn't count.
  it("asks before a DELETE whose WHERE is commented out", () => {
    const { execution, queries } = setup("DELETE FROM t -- WHERE id = 1");
    execution.handleExecute();
    expect(execution.showDestructiveConfirm).toBe(true);
    expect(execution.destructiveStatements.map((s) => s.reason)).toEqual(["delete_no_where"]);
    expect(queries.execute).not.toHaveBeenCalled();
  });

  it("asks for the statement at the cursor too", () => {
    const { execution, queries } = setup(
      "SELECT 1;\nDELETE FROM t -- WHERE id = 1",
      "postgres",
      15,
    );
    execution.handleExecuteCurrent();
    expect(execution.showDestructiveConfirm).toBe(true);
    expect(queries.executeCurrent).not.toHaveBeenCalled();
  });

  it("follows the engine: a MySQL # comment doesn't hide the DELETE", () => {
    const { execution } = setup("# note\nDELETE FROM t", "mysql");
    execution.handleExecute();
    expect(execution.destructiveStatements.map((s) => s.reason)).toEqual(["delete_no_where"]);
  });

  it("runs a script with nothing destructive", () => {
    const { execution, queries } = setup("SELECT 1; UPDATE t SET a = 1 WHERE id = 2");
    execution.handleExecute();
    expect(execution.showDestructiveConfirm).toBe(false);
    expect(queries.execute).toHaveBeenCalledWith("tab-1");
  });

  it("reports a failed check and runs nothing", () => {
    failing.add("destructive_reason");
    const { execution, queries } = setup("DELETE FROM t");
    execution.handleExecute();
    execution.handleExecuteCurrent();
    expect(errorToast).toHaveBeenCalledTimes(2);
    expect(errorToast.mock.calls[0][0]).toContain("unreachable");
    expect(execution.showDestructiveConfirm).toBe(false);
    expect(queries.execute).not.toHaveBeenCalled();
    expect(queries.executeCurrent).not.toHaveBeenCalled();
  });

  // The review's probe: a `null` statement at the cursor used to skip the
  // check and run the whole buffer, DROP included.
  it("runs nothing when the statement at the cursor can't be found", () => {
    failing.add("statement_at");
    const { execution, queries } = setup("SELECT 1;\nDROP TABLE users", "postgres", 3);
    execution.handleExecuteCurrent();
    expect(errorToast).toHaveBeenCalledTimes(1);
    expect(queries.executeCurrent).not.toHaveBeenCalled();
  });

  it("runs nothing when the script can't be split", () => {
    failing.add("split_statements");
    const { execution, queries } = setup("SELECT 1;\nDROP TABLE users");
    execution.handleExecute();
    expect(errorToast).toHaveBeenCalledTimes(1);
    expect(queries.execute).not.toHaveBeenCalled();
  });
});

describe("EXPLAIN at the cursor", () => {
  it("runs nothing when the statement at the cursor can't be found", async () => {
    const { createExplainVisualize } = await import("./explain-visualize.svelte.js");
    failing.add("statement_at");
    const explainTabs = { executeEmbedded: vi.fn() };
    const ctx = {
      db: { state: { activeConnection: { type: "postgres" } }, explainTabs },
      getActiveTab: () => ({ id: "tab-1", query: "SELECT 1;\nDELETE FROM users" }),
      getActiveTabId: () => "tab-1",
      getMonacoRef: () => ({ getCursorOffset: () => 3, insertText: () => {} }),
    } as unknown as QueryEditorContext;
    const paramDialog = { getParameterDefinitions: () => [] } as unknown as ParamDialog;
    const viewState = { syncVisualBuilderSql: vi.fn(), handleViewModeChange: vi.fn() };
    const ev = createExplainVisualize(ctx, paramDialog, viewState as never);
    ev.handleExplain(true);
    expect(errorToast).toHaveBeenCalledTimes(1);
    expect(explainTabs.executeEmbedded).not.toHaveBeenCalled();
  });
});
