# AI Dashboard Creation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Allow the AI chat to create and modify dashboards through structured tool calls, with progressive real-time rendering.

**Architecture:** Five new AI tools (`create_dashboard`, `add_widget`, `get_dashboard`, `update_widget`, `remove_widget`) are added to `src/lib/services/ai.ts`. Tool call handling is added to both the Anthropic and OpenAI-compatible streaming loops. Dashboard operations are wired through callback functions passed via `SendAIMessageParams`, constructed in `UIStateManager`.

**Tech Stack:** TypeScript, Svelte 5 runes, Anthropic/OpenAI streaming APIs

**Design doc:** `docs/plans/2026-03-24-ai-dashboard-creation-design.md`

---

### Task 1: Define the five dashboard tool schemas

**Files:**
- Modify: `src/lib/services/ai.ts:9-23` (next to `RUN_QUERY_TOOL`)

**Step 1: Add the tool definitions**

Add these constants after `RUN_QUERY_TOOL` (line 23):

```typescript
const CREATE_DASHBOARD_TOOL = {
  name: "create_dashboard",
  description:
    "Create a new empty dashboard. Returns the dashboard ID. After creating, use add_widget to populate it.",
  input_schema: {
    type: "object" as const,
    properties: {
      name: {
        type: "string",
        description: "The dashboard name (e.g. 'Sales Dashboard')",
      },
    },
    required: ["name"],
  },
};

const ADD_WIDGET_TOOL = {
  name: "add_widget",
  description:
    "Add a widget to a dashboard. The widget query executes automatically. Position widgets using pixel coordinates on the canvas.",
  input_schema: {
    type: "object" as const,
    properties: {
      dashboard_id: { type: "string", description: "The dashboard ID from create_dashboard" },
      title: { type: "string", description: "Widget title" },
      x: { type: "number", description: "X position in pixels" },
      y: { type: "number", description: "Y position in pixels" },
      width: { type: "number", description: "Width in pixels" },
      height: { type: "number", description: "Height in pixels" },
      query: { type: "string", description: "SQL SELECT query for the widget data" },
      widget_type: {
        type: "string",
        enum: ["chart", "kpi", "text"],
        description: "Widget type",
      },
      chart_config: {
        type: "object",
        description:
          "Required for chart widgets. Properties: type (bar|line|pie|scatter|area), xAxis (category column name), yAxis (array of numeric column names), colors (optional object mapping column names to hex colors)",
        properties: {
          type: { type: "string", enum: ["bar", "line", "pie", "scatter", "area"] },
          xAxis: { type: "string" },
          yAxis: { type: "array", items: { type: "string" } },
          colors: { type: "object" },
        },
      },
      kpi_config: {
        type: "object",
        description:
          "Required for kpi widgets. Properties: label (display label), valueColumn (column name with the value), format (number|percentage), prefix (e.g. '$'), suffix (e.g. '%')",
        properties: {
          label: { type: "string" },
          valueColumn: { type: "string" },
          format: { type: "string", enum: ["number", "percentage"] },
          prefix: { type: "string" },
          suffix: { type: "string" },
        },
        required: ["label", "valueColumn"],
      },
      text_config: {
        type: "object",
        description: "Required for text widgets. Properties: content (markdown text)",
        properties: {
          content: { type: "string" },
        },
        required: ["content"],
      },
    },
    required: ["dashboard_id", "title", "x", "y", "width", "height", "widget_type"],
  },
};

const GET_DASHBOARD_TOOL = {
  name: "get_dashboard",
  description:
    "Read an existing dashboard definition including all widgets. Use this before modifying a dashboard.",
  input_schema: {
    type: "object" as const,
    properties: {
      dashboard_id: { type: "string", description: "The dashboard ID" },
    },
    required: ["dashboard_id"],
  },
};

const UPDATE_WIDGET_TOOL = {
  name: "update_widget",
  description:
    "Update a single widget in a dashboard. Only pass the fields you want to change. If the query changes, it re-executes automatically.",
  input_schema: {
    type: "object" as const,
    properties: {
      dashboard_id: { type: "string", description: "The dashboard ID" },
      widget_id: { type: "string", description: "The widget ID to update" },
      title: { type: "string" },
      x: { type: "number" },
      y: { type: "number" },
      width: { type: "number" },
      height: { type: "number" },
      query: { type: "string" },
      widget_type: { type: "string", enum: ["chart", "kpi", "text"] },
      chart_config: {
        type: "object",
        properties: {
          type: { type: "string", enum: ["bar", "line", "pie", "scatter", "area"] },
          xAxis: { type: "string" },
          yAxis: { type: "array", items: { type: "string" } },
          colors: { type: "object" },
        },
      },
      kpi_config: {
        type: "object",
        properties: {
          label: { type: "string" },
          valueColumn: { type: "string" },
          format: { type: "string", enum: ["number", "percentage"] },
          prefix: { type: "string" },
          suffix: { type: "string" },
        },
      },
      text_config: {
        type: "object",
        properties: {
          content: { type: "string" },
        },
      },
    },
    required: ["dashboard_id", "widget_id"],
  },
};

const REMOVE_WIDGET_TOOL = {
  name: "remove_widget",
  description: "Remove a widget from a dashboard.",
  input_schema: {
    type: "object" as const,
    properties: {
      dashboard_id: { type: "string", description: "The dashboard ID" },
      widget_id: { type: "string", description: "The widget ID to remove" },
    },
    required: ["dashboard_id", "widget_id"],
  },
};
```

**Step 2: Verify no type errors**

Run: `npm run check`
Expected: No new errors

**Step 3: Commit**

```
feat(ai): add dashboard tool schema definitions
```

---

### Task 2: Add dashboard callbacks to SendAIMessageParams

**Files:**
- Modify: `src/lib/services/ai.ts:110-131` (`SendAIMessageParams` interface)
- Modify: `src/lib/services/ai.ts:168-169` (tools array construction)

**Step 1: Extend the params interface**

Add these fields to `SendAIMessageParams` (after `onError`):

```typescript
export interface SendAIMessageParams {
  // ... existing fields ...
  onError: (msg: string) => void;
  // Dashboard tool callbacks
  onCreateDashboard?: (name: string) => Promise<{ dashboardId: string } | null>;
  onAddWidget?: (dashboardId: string, widget: Omit<DashboardWidget, 'id' | 'result' | 'isLoading' | 'error' | 'lastRefreshed'>) => Promise<{ widgetId: string } | null>;
  onGetDashboard?: (dashboardId: string) => DashboardGetResult | null;
  onUpdateWidget?: (dashboardId: string, widgetId: string, updates: Partial<DashboardWidget>) => Promise<void>;
  onRemoveWidget?: (dashboardId: string, widgetId: string) => Promise<void>;
}
```

Add the `DashboardGetResult` type near the top of the file (after imports):

```typescript
import type { DashboardWidget } from "$lib/types/dashboard";

interface DashboardGetResult {
  id: string;
  name: string;
  widgets: Array<{
    id: string;
    title: string;
    x: number;
    y: number;
    width: number;
    height: number;
    widgetType: string;
    query: string;
    chartConfig?: ChartConfig;
    kpiConfig?: KpiConfig;
    textConfig?: TextConfig;
  }>;
}
```

Also import the needed types:

```typescript
import type { ChartConfig } from "$lib/types/chart";
import type { KpiConfig, TextConfig } from "$lib/types/dashboard";
```

**Step 2: Include dashboard tools in the tools array**

In `sendAIMessage()`, update the tools construction (around line 169). Change:

```typescript
const tools = shareData ? [RUN_QUERY_TOOL] : [];
```

To:

```typescript
const dataTools = shareData ? [RUN_QUERY_TOOL] : [];
const dashboardTools = params.onCreateDashboard
  ? [CREATE_DASHBOARD_TOOL, ADD_WIDGET_TOOL, GET_DASHBOARD_TOOL, UPDATE_WIDGET_TOOL, REMOVE_WIDGET_TOOL]
  : [];
const tools = [...dataTools, ...dashboardTools];
```

**Step 3: Verify no type errors**

Run: `npm run check`
Expected: No new errors

**Step 4: Commit**

```
feat(ai): add dashboard callbacks to SendAIMessageParams
```

---

### Task 3: Extend the system prompt with dashboard guidance

**Files:**
- Modify: `src/lib/services/ai.ts:97-107` (`buildSystemPrompt` function)

**Step 1: Update buildSystemPrompt**

Change the function signature to accept a flag for whether dashboard tools are available:

```typescript
function buildSystemPrompt(schemaCtx: string, dbType?: DatabaseType, hasDashboardTools?: boolean): string {
  const dbLabel = dbType ? DATABASE_LABELS[dbType] : "SQL";
  const parts = [
    `You are a helpful SQL assistant for a ${dbLabel} database. Always use ${dbLabel}-compatible syntax.`,
  ];
  if (schemaCtx) parts.push(schemaCtx);
  parts.push(
    "Provide clear, concise SQL queries and explanations. When writing SQL, wrap it in a markdown code block.",
  );

  if (hasDashboardTools) {
    parts.push(`You can create and modify dashboards using the dashboard tools.

Dashboard layout conventions (pixel-based canvas):
- KPI widgets: 220x140px. Place up to 4 across in a row with 20px gaps. First row starts at x=20, y=20.
- Chart widgets: 460x340px. Place 2 across per row with 20px gaps.
- Text widgets: flexible sizing, typically full-width (940x60px) for headers.
- Row spacing: KPI row height ~160px, chart row height ~360px, text row ~80px.
- Standard canvas width: ~980px (20px margin on each side).

Widget type guidance:
- kpi: single aggregate number (COUNT, SUM, AVG). Always needs kpi_config with label and valueColumn.
- chart: comparisons, trends, distributions. Always needs chart_config with type, xAxis, yAxis.
  - bar: categorical comparisons
  - line: trends over time
  - area: trends with volume emphasis
  - pie: proportional distribution (limit to <8 slices)
  - scatter: correlation between two variables
- text: section headers or notes. Needs text_config with content.

Chart config: xAxis is the category/label column, yAxis is an array of numeric column names. Use descriptive hex colors.

When creating a dashboard:
1. Call create_dashboard with a descriptive name
2. Call add_widget for each widget — the query runs automatically and data appears in real-time
3. Start with KPI widgets in the first row for key metrics
4. Follow with chart widgets for detailed analysis
5. Write correct SQL for the database type (${dbLabel})`);
  }

  return parts.join("\n\n");
}
```

**Step 2: Update the two callsites of buildSystemPrompt**

In `sendAIMessage()` (around line 168), update:

```typescript
const systemPrompt = buildSystemPrompt(schemaCtx, databaseType, !!params.onCreateDashboard);
```

In `generateSQL()` (around line 385), keep the existing call unchanged (no dashboard tools for inline SQL generation):

```typescript
const systemPrompt = buildSystemPrompt(schemaCtx, databaseType);
```

**Step 3: Verify no type errors**

Run: `npm run check`
Expected: No new errors

**Step 4: Commit**

```
feat(ai): add dashboard layout guidance to system prompt
```

---

### Task 4: Handle dashboard tool calls in the Anthropic streaming loop

**Files:**
- Modify: `src/lib/services/ai.ts:176-269` (Anthropic `while(true)` loop)

This is the most substantial change. The existing loop handles `run_query` tool calls. We need to add handling for the 5 dashboard tools.

**Step 1: Extract a tool dispatch helper**

Add this function before `sendAIMessage` (around line 110):

```typescript
async function handleDashboardToolCall(
  toolName: string,
  input: Record<string, unknown>,
  params: SendAIMessageParams,
): Promise<string> {
  switch (toolName) {
    case "create_dashboard": {
      if (!params.onCreateDashboard) return "Dashboard tools not available";
      const name = typeof input.name === "string" ? input.name : "Untitled Dashboard";
      const result = await params.onCreateDashboard(name);
      if (!result) return "Failed to create dashboard";
      return JSON.stringify({ dashboard_id: result.dashboardId });
    }
    case "add_widget": {
      if (!params.onAddWidget) return "Dashboard tools not available";
      const dashboardId = typeof input.dashboard_id === "string" ? input.dashboard_id : "";
      const widgetType = input.widget_type as "chart" | "kpi" | "text";
      const widget: Omit<DashboardWidget, "id" | "result" | "isLoading" | "error" | "lastRefreshed"> = {
        title: typeof input.title === "string" ? input.title : "Widget",
        x: typeof input.x === "number" ? input.x : 0,
        y: typeof input.y === "number" ? input.y : 0,
        width: typeof input.width === "number" ? input.width : 400,
        height: typeof input.height === "number" ? input.height : 300,
        querySource: "custom",
        query: typeof input.query === "string" ? input.query : "",
        widgetType,
        chartConfig: widgetType === "chart" && input.chart_config
          ? {
              type: (input.chart_config as Record<string, unknown>).type as ChartConfig["type"],
              xAxis: ((input.chart_config as Record<string, unknown>).xAxis as string) ?? null,
              yAxis: ((input.chart_config as Record<string, unknown>).yAxis as string[]) ?? [],
              dataScope: "all" as const,
              colors: (input.chart_config as Record<string, unknown>).colors as Record<string, string> | undefined,
            }
          : undefined,
        kpiConfig: widgetType === "kpi" && input.kpi_config
          ? input.kpi_config as KpiConfig
          : undefined,
        textConfig: widgetType === "text" && input.text_config
          ? input.text_config as TextConfig
          : undefined,
      };
      const result = await params.onAddWidget(dashboardId, widget);
      if (!result) return "Failed to add widget";
      return JSON.stringify({ widget_id: result.widgetId });
    }
    case "get_dashboard": {
      if (!params.onGetDashboard) return "Dashboard tools not available";
      const dashboardId = typeof input.dashboard_id === "string" ? input.dashboard_id : "";
      const result = params.onGetDashboard(dashboardId);
      if (!result) return "Dashboard not found";
      return JSON.stringify(result);
    }
    case "update_widget": {
      if (!params.onUpdateWidget) return "Dashboard tools not available";
      const dashboardId = typeof input.dashboard_id === "string" ? input.dashboard_id : "";
      const widgetId = typeof input.widget_id === "string" ? input.widget_id : "";
      const updates: Partial<DashboardWidget> = {};
      if (typeof input.title === "string") updates.title = input.title;
      if (typeof input.x === "number") updates.x = input.x;
      if (typeof input.y === "number") updates.y = input.y;
      if (typeof input.width === "number") updates.width = input.width;
      if (typeof input.height === "number") updates.height = input.height;
      if (typeof input.query === "string") updates.query = input.query;
      if (typeof input.widget_type === "string") updates.widgetType = input.widget_type as DashboardWidget["widgetType"];
      if (input.chart_config) {
        const cc = input.chart_config as Record<string, unknown>;
        updates.chartConfig = {
          type: cc.type as ChartConfig["type"],
          xAxis: (cc.xAxis as string) ?? null,
          yAxis: (cc.yAxis as string[]) ?? [],
          dataScope: "all" as const,
          colors: cc.colors as Record<string, string> | undefined,
        };
      }
      if (input.kpi_config) updates.kpiConfig = input.kpi_config as KpiConfig;
      if (input.text_config) updates.textConfig = input.text_config as TextConfig;
      await params.onUpdateWidget(dashboardId, widgetId, updates);
      return "Widget updated";
    }
    case "remove_widget": {
      if (!params.onRemoveWidget) return "Dashboard tools not available";
      const dashboardId = typeof input.dashboard_id === "string" ? input.dashboard_id : "";
      const widgetId = typeof input.widget_id === "string" ? input.widget_id : "";
      await params.onRemoveWidget(dashboardId, widgetId);
      return "Widget removed";
    }
    default:
      return "Unknown tool";
  }
}

const DASHBOARD_TOOL_NAMES = new Set([
  "create_dashboard", "add_widget", "get_dashboard", "update_widget", "remove_widget",
]);
```

**Step 2: Update the Anthropic tool-call handling in the while loop**

In the Anthropic loop (around line 202-219), the current code checks `if (toolName !== "run_query")` and returns "Unknown tool". Change this to also handle dashboard tools.

Replace the section that starts with `if (toolName !== "run_query")` (lines ~204-219) and the run_query handling below it with:

```typescript
      // Handle dashboard tools
      if (DASHBOARD_TOOL_NAMES.has(toolName)) {
        const toolResultContent = await handleDashboardToolCall(toolName, input, params);
        const assistantContent: unknown[] = [];
        if (turnResult.assistantText) {
          assistantContent.push({ type: "text", text: turnResult.assistantText });
        }
        assistantContent.push({ type: "tool_use", id: toolUseId, name: toolName, input });
        apiMessages = [
          ...apiMessages,
          { role: "assistant", content: assistantContent },
          {
            role: "user",
            content: [{ type: "tool_result", tool_use_id: toolUseId, content: toolResultContent }],
          },
        ];
        continue;
      }

      if (toolName !== "run_query") {
        // ... existing unknown tool handling (unchanged) ...
      }

      // ... existing run_query handling (unchanged) ...
```

**Step 3: Verify no type errors**

Run: `npm run check`
Expected: No new errors

**Step 4: Commit**

```
feat(ai): handle dashboard tool calls in Anthropic streaming loop
```

---

### Task 5: Handle dashboard tool calls in the OpenAI-compatible streaming loop

**Files:**
- Modify: `src/lib/services/ai.ts:270-358` (OpenAI-compatible `while(true)` loop)

**Step 1: Add dashboard tool handling to the OpenAI loop**

Same pattern as Task 4. In the OpenAI loop, find the `if (toolName !== "run_query")` block (around line 316) and add dashboard handling before it:

```typescript
      // Handle dashboard tools
      if (DASHBOARD_TOOL_NAMES.has(toolName)) {
        const toolResultContent = await handleDashboardToolCall(toolName, input, params);
        apiMessages = [
          ...apiMessages,
          assistantMsg,
          { role: "tool" as const, tool_call_id: toolCallId, content: toolResultContent },
        ];
        continue;
      }

      if (toolName !== "run_query") {
        // ... existing unknown tool handling (unchanged) ...
      }

      // ... existing run_query handling (unchanged) ...
```

**Step 2: Verify no type errors**

Run: `npm run check`
Expected: No new errors

**Step 3: Commit**

```
feat(ai): handle dashboard tool calls in OpenAI-compatible streaming loop
```

---

### Task 6: Bump the tool call limit

**Files:**
- Modify: `src/lib/services/ai.ts` (two places where `toolCallCount > 10`)

**Step 1: Change both limits from 10 to 20**

In the Anthropic loop (around line 197):
```typescript
if (toolCallCount > 20) {
```

In the OpenAI loop (around line 297):
```typescript
if (toolCallCount > 20) {
```

**Step 2: Commit**

```
feat(ai): increase tool call limit to 20 for dashboard creation
```

---

### Task 7: Wire dashboard callbacks in UIStateManager

**Files:**
- Modify: `src/lib/hooks/database/ui-state.svelte.ts`

This is where `sendAIMessageService` is called with all its params. We need to pass the dashboard callbacks.

**Step 1: Add dashboard dependencies to the constructor**

```typescript
import type { DashboardManager } from "./dashboard-manager.svelte.js";
import type { DashboardTabManager } from "./dashboard-tabs.svelte.js";
import type { DashboardWidget } from "$lib/types";

export class UIStateManager {
  aiAllowAllQueries = $state(false);

  constructor(
    private state: DatabaseState,
    private schedulePersistence: (projectId: string | null) => void,
    private executeRawQuery: (query: string) => Promise<Record<string, unknown>[]>,
    private aiChatManager: AIChatManager,
    private persistAIChatMessages: (chatId: string) => Promise<void>,
    private dashboardManager: DashboardManager,
    private dashboardTabs: DashboardTabManager,
  ) {}
```

**Step 2: Add dashboard callbacks to the sendAIMessageService call**

In `_dispatchToAI()`, add these properties to the `sendAIMessageService({...})` call (after `onError`):

```typescript
      onCreateDashboard: async (name: string) => {
        const dashboard = await this.dashboardManager.createDashboard(name);
        if (!dashboard) return null;
        this.dashboardTabs.add(dashboard.id, name);
        return { dashboardId: dashboard.id };
      },
      onAddWidget: async (dashboardId: string, widget: Omit<DashboardWidget, 'id' | 'result' | 'isLoading' | 'error' | 'lastRefreshed'>) => {
        const widgetId = `widget-${crypto.randomUUID()}`;
        const fullWidget = { ...widget, id: widgetId } as DashboardWidget;
        await this.dashboardManager.addWidget(dashboardId, fullWidget);
        await this.dashboardManager.executeWidget(dashboardId, widgetId);
        return { widgetId };
      },
      onGetDashboard: (dashboardId: string) => {
        const dashboard = this.dashboardManager.getDashboard(dashboardId);
        if (!dashboard) return null;
        return {
          id: dashboard.id,
          name: dashboard.name,
          widgets: dashboard.widgets.map(({ result: _, isLoading: __, error: ___, lastRefreshed: ____, ...rest }) => rest),
        };
      },
      onUpdateWidget: async (dashboardId: string, widgetId: string, updates: Partial<DashboardWidget>) => {
        const queryChanged = updates.query !== undefined;
        await this.dashboardManager.updateWidget(dashboardId, widgetId, updates);
        if (queryChanged) {
          await this.dashboardManager.executeWidget(dashboardId, widgetId);
        }
      },
      onRemoveWidget: async (dashboardId: string, widgetId: string) => {
        await this.dashboardManager.removeWidget(dashboardId, widgetId);
      },
```

**Step 3: Verify no type errors**

Run: `npm run check`
Expected: No new errors

**Step 4: Commit**

```
feat(ai): wire dashboard callbacks in UIStateManager
```

---

### Task 8: Update UseDatabase constructor to pass new dependencies

**Files:**
- Modify: `src/lib/hooks/database.svelte.ts:130-136` (UIStateManager construction)

**Step 1: Pass dashboardManager and dashboardTabs to UIStateManager**

The `UIStateManager` is currently constructed on line 130-136 before `dashboardTabs` and `dashboards` are created (lines 191-206). We need to reorder the construction so dashboards are created first.

Move the dashboard manager and tab creation (lines 191-206) to before the UIStateManager construction (before line 130). Then update the UIStateManager constructor:

```typescript
    // Dashboards (must be before UI for AI dashboard tools)
    this.dashboardTabs = new DashboardTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
    );
    this.dashboards = new DashboardManager(
      this.state,
      async (query: string) => {
        return await this.queries.executeRaw(query);
      },
      scheduleProjectPersistence,
    );

    // UI
    this.ui = new UIStateManager(
      this.state,
      scheduleProjectPersistence,
      (query) => this.queries.executeRaw(query),
      this.aiChats,
      (chatId) => this.persistence.persistAIChatMessages(chatId),
      this.dashboards,
      this.dashboardTabs,
    );
```

Remove the old dashboard construction from lines 191-206 (now moved up).

**Step 2: Verify no type errors**

Run: `npm run check`
Expected: No new errors

**Step 3: Commit**

```
feat(ai): wire dashboard managers into UseDatabase constructor
```

---

### Task 9: Include dashboard ID in @-mention context for existing dashboards

**Files:**
- Modify: `src/lib/services/ai-mentions.ts:126-140` (`formatDashboardContext`)

**Step 1: Add dashboard ID to the context block**

When the AI receives a dashboard @-mention, it needs the ID to call `get_dashboard` or `update_widget`. Update `formatDashboardContext`:

```typescript
function formatDashboardContext(dashboard: Dashboard): string {
  const lines: string[] = [`Dashboard: ${dashboard.name} (id: ${dashboard.id})`];

  if (dashboard.widgets.length > 0) {
    lines.push("Widgets:");
    for (const w of dashboard.widgets) {
      lines.push(`  - ${w.title} [id: ${w.id}] (${w.widgetType})`);
      if (w.query) {
        lines.push(`    Query: ${w.query}`);
      }
    }
  }

  return lines.join("\n");
}
```

**Step 2: Verify no type errors**

Run: `npm run check`
Expected: No new errors

**Step 3: Commit**

```
feat(ai): include dashboard and widget IDs in @-mention context
```

---

### Task 10: Manual testing

**Step 1: Start the dev server**

Run: `npm run tauri dev`

**Step 2: Test dashboard creation flow**

1. Connect to a database with some tables
2. Open AI chat
3. Type "Create a sales dashboard" (or similar prompt relevant to the schema)
4. Verify: empty dashboard tab opens immediately
5. Verify: widgets appear one by one on the canvas
6. Verify: widget queries execute and data renders progressively
7. Verify: AI sends a summary message in chat

**Step 3: Test dashboard modification flow**

1. In the same chat, type "Change the first chart to a pie chart" (or reference a dashboard with @-mention)
2. Verify: the AI calls `get_dashboard` then `update_widget`
3. Verify: the widget updates live on the canvas

**Step 4: Test edge cases**

1. Test with no schema sharing enabled (dashboard tools should still be available)
2. Test with OpenAI-compatible provider
3. Test creating a dashboard with >10 widgets (verify the new limit of 20 works)
4. Test error handling: what happens if the AI writes an invalid query for a widget
