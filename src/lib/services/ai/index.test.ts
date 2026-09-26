import { describe, expect, it, vi } from "vitest";
import type { SendAIMessageParams } from "./index";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

const { handleToolCall } = await import("./index");

const REFUSAL = "Only read-only SELECT queries are permitted";
const LOCAL = { id: "conn-1", type: "postgres", name: "Local" } as const;

function params(overrides: Partial<SendAIMessageParams> = {}) {
  const runQuery = vi.fn(async (_sql: string, _signal?: AbortSignal) => [{ n: 1 }]);
  const onApprovalRequired = vi.fn<SendAIMessageParams["onApprovalRequired"]>((_q, _c, approve) =>
    approve(),
  );
  const onAddWidget = vi.fn(async () => ({ widgetId: "w-1" }));
  const onUpdateWidget = vi.fn(async () => {});
  const p = {
    connection: LOCAL,
    runQuery,
    activeConnection: () => ({ id: LOCAL.id, name: LOCAL.name }),
    aiAllowAllQueries: false,
    onApprovalRequired,
    onCreateDashboard: vi.fn(async () => ({ dashboardId: "d-1" })),
    onAddWidget,
    onGetDashboard: vi.fn(() => null),
    onUpdateWidget,
    onRemoveWidget: vi.fn(async () => {}),
    ...overrides,
  } as unknown as SendAIMessageParams;
  return { p, runQuery, onApprovalRequired, onAddWidget, onUpdateWidget };
}

describe("run_query", () => {
  it("asks for approval, then runs a read-only query", async () => {
    const { p, runQuery, onApprovalRequired } = params();
    const out = await handleToolCall("run_query", { query: "SELECT 1 AS n" }, p);
    expect(onApprovalRequired).toHaveBeenCalledOnce();
    expect(onApprovalRequired.mock.calls[0][1]).toEqual(LOCAL);
    expect(runQuery).toHaveBeenCalledWith("SELECT 1 AS n", undefined);
    expect(out).toContain("| n |");
  });

  it("refuses a write even with allow-all on", async () => {
    const { p, runQuery } = params({ aiAllowAllQueries: true });
    const out = await handleToolCall("run_query", { query: "SELECT 1; DELETE FROM t" }, p);
    expect(out).toBe(REFUSAL);
    expect(runQuery).not.toHaveBeenCalled();
  });

  it("skips approval with allow-all on", async () => {
    const { p, runQuery, onApprovalRequired } = params({ aiAllowAllQueries: true });
    await handleToolCall("run_query", { query: "SELECT 1 AS n" }, p);
    expect(onApprovalRequired).not.toHaveBeenCalled();
    expect(runQuery).toHaveBeenCalledOnce();
  });

  it("checks the query with the chat's connection type", async () => {
    // `#` starts a comment on MySQL, so this is one SELECT there; on
    // Postgres the DELETE is a second statement.
    const sql = "SELECT 1 # ; DELETE FROM t";
    const pg = params({ aiAllowAllQueries: true });
    expect(await handleToolCall("run_query", { query: sql }, pg.p)).toBe(REFUSAL);
    const my = params({
      aiAllowAllQueries: true,
      connection: { id: "conn-2", type: "mysql", name: "My" },
    });
    await handleToolCall("run_query", { query: sql }, my.p);
    expect(my.runQuery).toHaveBeenCalledOnce();
  });

  it("passes the signal to the query", async () => {
    const controller = new AbortController();
    const { p, runQuery } = params({ aiAllowAllQueries: true, signal: controller.signal });
    await handleToolCall("run_query", { query: "SELECT 1" }, p);
    expect(runQuery).toHaveBeenCalledWith("SELECT 1", controller.signal);
  });

  it("returns the runner's refusal as the tool result", async () => {
    const { p, runQuery } = params({ aiAllowAllQueries: true });
    runQuery.mockRejectedValueOnce(
      new Error('The connection "Local" is disconnected; reconnect it and try again'),
    );
    expect(await handleToolCall("run_query", { query: "SELECT 1" }, p)).toBe(
      'Query error: The connection "Local" is disconnected; reconnect it and try again',
    );
  });

  it("reports a query stopped by the signal as cancelled", async () => {
    const controller = new AbortController();
    const { p, runQuery } = params({ aiAllowAllQueries: true, signal: controller.signal });
    runQuery.mockImplementationOnce(async () => {
      controller.abort();
      throw new DOMException("Aborted", "AbortError");
    });
    expect(await handleToolCall("run_query", { query: "SELECT 1" }, p)).toBe("Query cancelled");
  });

  it("resolves a pending approval as cancelled when the signal aborts", async () => {
    const controller = new AbortController();
    let approve: () => void = () => {};
    const { p, runQuery } = params({
      signal: controller.signal,
      onApprovalRequired: (_q, _c, a) => {
        approve = a;
      },
    });
    const out = handleToolCall("run_query", { query: "SELECT 1" }, p);
    controller.abort();
    expect(await out).toBe("Query cancelled");
    // A late click on Allow doesn't run the query.
    approve();
    await Promise.resolve();
    expect(runQuery).not.toHaveBeenCalled();
  });

  it("doesn't ask for approval when the signal already aborted", async () => {
    const controller = new AbortController();
    controller.abort();
    const { p, onApprovalRequired } = params({ signal: controller.signal });
    expect(await handleToolCall("run_query", { query: "SELECT 1" }, p)).toBe("Query cancelled");
    expect(onApprovalRequired).not.toHaveBeenCalled();
  });

  it("runs an approval on the chat's connection after the active one changed", async () => {
    let approve: () => void = () => {};
    const { p, runQuery } = params({
      activeConnection: () => ({ id: "conn-2", name: "Other" }),
      onApprovalRequired: (_q, _c, a) => {
        approve = a;
      },
    });
    const out = handleToolCall("run_query", { query: "SELECT 1 AS n" }, p);
    approve();
    expect(await out).toContain("| n |");
    // The runner is bound to the chat's connection; nothing else runs it.
    expect(runQuery).toHaveBeenCalledOnce();
  });

  it("resolves a denied approval", async () => {
    const { p, runQuery } = params({ onApprovalRequired: (_q, _c, _a, deny) => deny() });
    expect(await handleToolCall("run_query", { query: "SELECT 1" }, p)).toBe(
      "User denied query execution",
    );
    expect(runQuery).not.toHaveBeenCalled();
  });
});

describe("dashboard widget queries", () => {
  const widget = { dashboard_id: "d-1", widget_type: "kpi", title: "n" };

  it("adds a widget with a read-only query", async () => {
    const { p, onAddWidget } = params();
    const out = await handleToolCall("add_widget", { ...widget, query: "SELECT 1 AS n" }, p);
    expect(JSON.parse(out)).toEqual({ widget_id: "w-1" });
    expect(onAddWidget).toHaveBeenCalledOnce();
  });

  it("adds a text widget without a query", async () => {
    const { p, onAddWidget } = params();
    await handleToolCall("add_widget", { dashboard_id: "d-1", widget_type: "text" }, p);
    expect(onAddWidget).toHaveBeenCalledOnce();
  });

  it("refuses a widget whose query writes, allow-all or not", async () => {
    for (const aiAllowAllQueries of [false, true]) {
      const { p, onAddWidget } = params({ aiAllowAllQueries });
      const out = await handleToolCall("add_widget", { ...widget, query: "DROP TABLE t" }, p);
      expect(JSON.parse(out)).toEqual({ error: REFUSAL });
      expect(onAddWidget).not.toHaveBeenCalled();
    }
  });

  it("refuses an update that sets a writing query", async () => {
    const { p, onUpdateWidget } = params();
    const out = await handleToolCall(
      "update_widget",
      { dashboard_id: "d-1", widget_id: "w-1", query: "SELECT 1; UPDATE t SET a = 1" },
      p,
    );
    expect(JSON.parse(out)).toEqual({ error: REFUSAL });
    expect(onUpdateWidget).not.toHaveBeenCalled();
  });

  it("allows an update that doesn't touch the query", async () => {
    const { p, onUpdateWidget } = params();
    await handleToolCall("update_widget", { dashboard_id: "d-1", widget_id: "w-1", x: 5 }, p);
    expect(onUpdateWidget).toHaveBeenCalledOnce();
  });

  it("refuses add_widget and update_widget when the chat's connection isn't active", async () => {
    const switched = { id: "conn-2", name: "Other" };
    const { p, onAddWidget, onUpdateWidget } = params({ activeConnection: () => switched });
    const refusal = {
      error: 'The active connection is "Other"; switch back to "Local" to change this dashboard',
    };
    const add = await handleToolCall("add_widget", { ...widget, query: "SELECT 1" }, p);
    expect(JSON.parse(add)).toEqual(refusal);
    const update = await handleToolCall(
      "update_widget",
      { dashboard_id: "d-1", widget_id: "w-1", x: 5 },
      p,
    );
    expect(JSON.parse(update)).toEqual(refusal);
    expect(onAddWidget).not.toHaveBeenCalled();
    expect(onUpdateWidget).not.toHaveBeenCalled();
  });

  it("refuses add_widget when no connection is active", async () => {
    const { p, onAddWidget } = params({ activeConnection: () => null });
    const out = await handleToolCall("add_widget", { ...widget, query: "SELECT 1" }, p);
    expect(JSON.parse(out)).toEqual({
      error: 'No connection is active; switch back to "Local" to change this dashboard',
    });
    expect(onAddWidget).not.toHaveBeenCalled();
  });

  it("still reads and removes widgets after a switch", async () => {
    const { p } = params({ activeConnection: () => ({ id: "conn-2", name: "Other" }) });
    const out = await handleToolCall("remove_widget", { dashboard_id: "d-1", widget_id: "w" }, p);
    expect(JSON.parse(out)).toEqual({ success: true });
  });
});
