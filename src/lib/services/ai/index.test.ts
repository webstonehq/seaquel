import { describe, expect, it, vi } from "vitest";
import type { SendAIMessageParams } from "./index";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

const { handleToolCall } = await import("./index");

const REFUSAL = "Only read-only SELECT queries are permitted";

function params(overrides: Partial<SendAIMessageParams> = {}) {
  const executeQuery = vi.fn(async () => [{ n: 1 }]);
  const onApprovalRequired = vi.fn<SendAIMessageParams["onApprovalRequired"]>((_q, _c, approve) =>
    approve(),
  );
  const onAddWidget = vi.fn(async () => ({ widgetId: "w-1" }));
  const onUpdateWidget = vi.fn(async () => {});
  const p = {
    connectionName: "Local",
    databaseType: "postgres",
    executeQuery,
    aiAllowAllQueries: false,
    onApprovalRequired,
    onCreateDashboard: vi.fn(async () => ({ dashboardId: "d-1" })),
    onAddWidget,
    onGetDashboard: vi.fn(() => null),
    onUpdateWidget,
    onRemoveWidget: vi.fn(async () => {}),
    ...overrides,
  } as unknown as SendAIMessageParams;
  return { p, executeQuery, onApprovalRequired, onAddWidget, onUpdateWidget };
}

describe("run_query", () => {
  it("asks for approval, then runs a read-only query", async () => {
    const { p, executeQuery, onApprovalRequired } = params();
    const out = await handleToolCall("run_query", { query: "SELECT 1 AS n" }, p);
    expect(onApprovalRequired).toHaveBeenCalledOnce();
    expect(executeQuery).toHaveBeenCalledWith("SELECT 1 AS n");
    expect(out).toContain("| n |");
  });

  it("refuses a write even with allow-all on", async () => {
    const { p, executeQuery } = params({ aiAllowAllQueries: true });
    const out = await handleToolCall("run_query", { query: "SELECT 1; DELETE FROM t" }, p);
    expect(out).toBe(REFUSAL);
    expect(executeQuery).not.toHaveBeenCalled();
  });

  it("fails closed without a connection type", async () => {
    const { p, executeQuery } = params({ databaseType: undefined, aiAllowAllQueries: true });
    expect(await handleToolCall("run_query", { query: "SELECT 1" }, p)).toBe(REFUSAL);
    expect(executeQuery).not.toHaveBeenCalled();
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
    const { p, onAddWidget } = params({ databaseType: undefined });
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
    const { p, onUpdateWidget } = params({ databaseType: undefined });
    await handleToolCall("update_widget", { dashboard_id: "d-1", widget_id: "w-1", x: 5 }, p);
    expect(onUpdateWidget).toHaveBeenCalledOnce();
  });

  it("fails closed without a connection type", async () => {
    const { p, onAddWidget } = params({ databaseType: undefined });
    const out = await handleToolCall("add_widget", { ...widget, query: "SELECT 1" }, p);
    expect(JSON.parse(out)).toEqual({ error: REFUSAL });
    expect(onAddWidget).not.toHaveBeenCalled();
  });
});
