# AI Dashboard Creation

## Summary

Allow users to create and modify dashboards through the AI chat. A prompt like "Create a sales dashboard" causes the AI to analyze the database schema, generate appropriate widgets with SQL queries, and build the dashboard progressively in real-time.

## Tools

Five new AI tools added alongside the existing `run_query` tool:

### `create_dashboard(name)`

- Creates an empty dashboard and opens it as a new tab immediately.
- Returns `{ dashboard_id }` so subsequent tool calls can reference it.
- The user sees the empty canvas right away.

### `add_widget(dashboard_id, widget)`

- Adds a single widget to the dashboard.
- The widget query executes automatically on add, so the user sees data populate in real-time.
- Widget definition follows the existing `DashboardWidget` shape (minus runtime fields):
  ```json
  {
    "title": "Total Revenue",
    "x": 20, "y": 20, "width": 220, "height": 140,
    "querySource": "custom",
    "query": "SELECT SUM(amount) as total_revenue FROM orders",
    "widgetType": "kpi",
    "kpiConfig": { "label": "Total Revenue", "valueColumn": "total_revenue", "prefix": "$" }
  }
  ```
- Widget `id` is generated server-side (not by the AI).

### `get_dashboard(dashboard_id)`

- Returns the full dashboard definition including all widgets.
- Used when the AI needs to read an existing dashboard before modifying it.
- Returns the same shape as the persisted dashboard (widgets without runtime state).

### `update_widget(dashboard_id, widget_id, updates)`

- Applies a partial update to a single widget.
- The widget re-executes its query automatically if the query changed.
- The user sees the change live (e.g., chart type switches from pie to bar).

### `remove_widget(dashboard_id, widget_id)`

- Removes a widget from the dashboard.
- The user sees it disappear immediately.

## System Prompt Additions

The AI system prompt is extended with:

1. **Dashboard capability description** -- tells the AI it can create and modify dashboards.
2. **Layout conventions** -- pixel-based positioning guidance so the AI produces clean layouts:
   - Canvas starts at (0, 0), widgets positioned with ~20px gaps.
   - KPI widgets: ~220x140px. Fit 4 across in a row starting at y=20.
   - Chart widgets: ~460x340px. Fit 2 across per row.
   - Text widgets: flexible sizing.
   - Standard row heights: KPI row ~160px, chart row ~360px.
3. **Widget type guidance** -- when to use each type:
   - `kpi`: single aggregate number (counts, sums, averages).
   - `chart` (bar/line/area/pie/scatter): comparisons, trends, distributions.
   - `text`: section headers or notes.
4. **ChartConfig rules** -- `xAxis` is the category/label column, `yAxis` is an array of numeric columns, `type` matches the visualization intent.

## UX Flow

1. User opens AI chat, types "Create a sales dashboard".
2. AI receives the message along with schema context (tables, columns, indexes).
3. AI calls `create_dashboard("Sales Dashboard")`.
4. An empty dashboard tab opens immediately. Chat shows a tool-use indicator.
5. AI calls `add_widget(...)` for each widget sequentially. Each widget appears on the canvas and its query runs, populating data in real-time.
6. After all widgets are added, AI sends a summary message in chat: "I've created your Sales Dashboard with 8 widgets: 4 KPIs, 3 charts, and a text header."
7. User can follow up: "Change the revenue chart to an area chart" or "Add a widget showing top customers".
8. AI calls `get_dashboard(id)` to read current state, then `update_widget(...)` or `add_widget(...)` as needed.

## Implementation

### 1. Define tool schemas (`src/lib/services/ai.ts`)

Add five tool definitions (matching the format of `RUN_QUERY_TOOL`) with JSON Schema input definitions. Each tool schema describes its parameters and constraints.

### 2. Handle tool calls in the AI message loop (`src/lib/services/ai.ts`)

Extend the Anthropic and OpenAI-compatible tool-call handling loops. When the AI calls a dashboard tool:

- `create_dashboard`: call `DashboardManager.createDashboard()`, open a dashboard tab, return the ID.
- `add_widget`: generate a widget ID, call `DashboardManager.addWidget()`, then `DashboardManager.executeWidget()`, return success.
- `get_dashboard`: call `DashboardManager.getDashboard()`, strip runtime fields, return as JSON.
- `update_widget`: call `DashboardManager.updateWidget()`, re-execute if query changed, return success.
- `remove_widget`: call `DashboardManager.removeWidget()`, return success.

### 3. Wire callbacks through `SendAIMessageParams`

Add new callback parameters so the AI service can interact with the dashboard system:

```typescript
interface SendAIMessageParams {
  // ... existing params ...
  dashboardManager: {
    createDashboard: (name: string) => Promise<{ id: string } | null>;
    addWidget: (dashboardId: string, widget: Omit<DashboardWidget, 'id'>) => Promise<string>;
    getDashboard: (dashboardId: string) => Dashboard | undefined;
    updateWidget: (dashboardId: string, widgetId: string, updates: Partial<DashboardWidget>) => Promise<void>;
    removeWidget: (dashboardId: string, widgetId: string) => Promise<void>;
    executeWidget: (dashboardId: string, widgetId: string) => Promise<void>;
  };
  openDashboardTab: (dashboardId: string, name: string) => void;
}
```

### 4. Extend system prompt (`src/lib/services/ai.ts`)

Update `buildSystemPrompt()` to include dashboard capabilities, layout conventions, and widget type guidance when dashboard tools are available.

### 5. Tool call limit

The current limit is 10 tool calls per message. A dashboard with 4 KPIs + 4 charts + 1 create = 9 calls, which fits. Consider bumping the limit to 15 to allow slightly larger dashboards.

### 6. @-mention support

The existing `buildMentionItems()` already includes dashboards. When a user @-mentions a dashboard, its ID should be included in context so the AI can call `get_dashboard` on it.

## Out of Scope

- AI-generated dashboard templates/presets.
- Drag-and-drop rearrangement of AI-created widgets (already works via existing dashboard canvas).
- Auto-refresh configuration by AI (users can set this manually).
- Dashboard sharing through AI.
