# Drag-to-Draw Widget on Dashboard Grid

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let users draw a rectangle on the dashboard grid to create a new widget at the drawn position/size, and change the grid from 12 to 24 columns.

**Architecture:** Add pointer event handling on empty grid space to track a drag rectangle snapped to grid cells. On release, open the widget editor panel pre-filled with the drawn x/y/w/h. Remove the unused `isEditing` field from `DashboardTab`.

**Tech Stack:** Svelte 5 runes, CSS Grid, pointer events

---

### Task 1: Change grid from 12 to 24 columns

**Files:**
- Modify: `src/lib/hooks/database/dashboard-manager.svelte.ts:30` — default `gridColumns: 12` → `24`
- Modify: `src/lib/components/dashboard/dashboard-grid.svelte:135` — resize max constraint `12` → `24`
- Modify: `src/lib/components/dashboard/dashboard-widget-editor.svelte:128` — default widget `w: 4` → `6`
- Modify: `src/lib/types/dashboard.ts:27` — update comment from `12-column` to `24-column`

**Step 1: Update default gridColumns in dashboard-manager.svelte.ts**

In `createDashboard`, change:
```typescript
gridColumns: 12,
```
to:
```typescript
gridColumns: 24,
```

**Step 2: Update resize max-width constraint in dashboard-grid.svelte**

Line 135, change:
```typescript
const newW = Math.max(2, Math.min(12, Math.round(resizeStartW + dx / cellW)));
```
to:
```typescript
const newW = Math.max(2, Math.min(24, Math.round(resizeStartW + dx / cellW)));
```

**Step 3: Update default widget width in dashboard-widget-editor.svelte**

Line 128, change:
```typescript
w: widget?.w ?? 4,
```
to:
```typescript
w: widget?.w ?? 6,
```

**Step 4: Update type comment in dashboard.ts**

Line 27, change:
```typescript
// Grid position (12-column CSS grid)
```
to:
```typescript
// Grid position (24-column CSS grid)
```

---

### Task 2: Remove `isEditing` from DashboardTab

**Files:**
- Modify: `src/lib/types/dashboard.ts:70` — remove `isEditing` field
- Modify: `src/lib/hooks/database/dashboard-tabs.svelte.ts:82,115-117` — remove `isEditing` from tab creation and remove `setEditing` method
- Modify: `src/lib/hooks/database/project-manager.svelte.ts:389` — remove `isEditing` from restoration

**Step 1: Remove `isEditing` from DashboardTab type**

In `src/lib/types/dashboard.ts`, remove line 70:
```typescript
  isEditing: boolean;
```

**Step 2: Remove `isEditing` from tab creation in dashboard-tabs.svelte.ts**

Line 82, remove `isEditing: !dashboardId,` from the `newTab` object.

**Step 3: Remove `setEditing` method from dashboard-tabs.svelte.ts**

Remove lines 112-117 (the `setEditing` method and its doc comment).

**Step 4: Remove `isEditing` from state restoration in project-manager.svelte.ts**

Line 389, remove `isEditing: false,` from the tab mapping.

---

### Task 3: Add drag-to-draw interaction on the dashboard grid

**Files:**
- Modify: `src/lib/components/dashboard/dashboard-grid.svelte` — add draw state, pointer handlers, and selection ghost
- Modify: `src/lib/components/dashboard/dashboard-view.svelte` — pass new callback and position to widget editor

**Step 1: Add Props and draw state to dashboard-grid.svelte**

Add a new callback prop and draw state variables. Update the `Props` interface:

```typescript
interface Props {
    dashboard: Dashboard;
    onEditWidget: (widget: DashboardWidget) => void;
    onDrawWidget: (rect: { x: number; y: number; w: number; h: number }) => void;
}

let { dashboard, onEditWidget, onDrawWidget }: Props = $props();
```

Add draw state variables after the resize state block (~line 39):

```typescript
// --- Draw-to-create state ---
let isDrawing = $state(false);
let drawStartCol = $state(0);
let drawStartRow = $state(0);
let drawCurrentCol = $state(0);
let drawCurrentRow = $state(0);

const drawRect = $derived.by(() => {
    if (!isDrawing) return null;
    const x = Math.min(drawStartCol, drawCurrentCol);
    const y = Math.min(drawStartRow, drawCurrentRow);
    const w = Math.max(2, Math.abs(drawCurrentCol - drawStartCol) + 1);
    const h = Math.max(1, Math.abs(drawCurrentRow - drawStartRow) + 1);
    return { x, y, w, h };
});
```

**Step 2: Add helper to convert pointer position to grid coordinates**

Add after `getGridMetrics()`:

```typescript
function pointerToGrid(e: PointerEvent): { col: number; row: number } | null {
    const { colWidth, gridRect } = getGridMetrics();
    if (colWidth === 0) return null;
    const cellW = colWidth + GAP;
    const cellH = ROW_HEIGHT + GAP;
    const relX = e.clientX - gridRect.left - 16; // 16 = half of p-4 padding
    const relY = e.clientY - gridRect.top - 16;
    const col = Math.max(0, Math.min(dashboard.gridColumns - 1, Math.floor(relX / cellW)));
    const row = Math.max(0, Math.floor(relY / cellH));
    return { col, row };
}
```

**Step 3: Add draw start handler on the grid element**

Add a `handleDrawStart` function:

```typescript
function handleDrawStart(e: PointerEvent) {
    // Only start drawing on direct grid clicks (not on widgets or resize handles)
    if (e.target !== gridEl) return;
    if (e.button !== 0) return;

    const pos = pointerToGrid(e);
    if (!pos) return;

    e.preventDefault();
    isDrawing = true;
    drawStartCol = pos.col;
    drawStartRow = pos.row;
    drawCurrentCol = pos.col;
    drawCurrentRow = pos.row;

    const handleMove = (moveEvent: PointerEvent) => {
        const movePos = pointerToGrid(moveEvent);
        if (movePos) {
            drawCurrentCol = movePos.col;
            drawCurrentRow = movePos.row;
        }
    };

    const handleUp = () => {
        if (isDrawing && drawRect) {
            onDrawWidget(drawRect);
        }
        isDrawing = false;
        window.removeEventListener('pointermove', handleMove);
        window.removeEventListener('pointerup', handleUp);
    };

    window.addEventListener('pointermove', handleMove);
    window.addEventListener('pointerup', handleUp);
}
```

**Step 4: Attach the draw handler to the grid div**

On the grid `<div>`, add:
```svelte
onpointerdown={handleDrawStart}
```

**Step 5: Render the draw selection ghost**

Add inside the grid div, after the widget `{#each}` block:

```svelte
{#if isDrawing && drawRect}
    <div
        class="rounded-lg border-2 border-dashed border-primary/40 bg-primary/10 pointer-events-none"
        style="grid-column: {drawRect.x + 1} / span {drawRect.w}; grid-row: {drawRect.y + 1} / span {drawRect.h};"
    ></div>
{/if}
```

**Step 6: Extend the grid min-height to allow drawing below existing widgets**

Update the `maxRow` derived to add extra rows for drawing space:

```typescript
const maxRow = $derived.by(() => {
    if (dashboard.widgets.length === 0) return 6;
    return Math.max(6, ...dashboard.widgets.map((w) => w.y + w.h)) + 4;
});
```

---

### Task 4: Wire up dashboard-view to pass drawn rect to widget editor

**Files:**
- Modify: `src/lib/components/dashboard/dashboard-view.svelte` — add `handleDrawWidget`, pass position to editor
- Modify: `src/lib/components/dashboard/dashboard-widget-editor.svelte` — accept optional initial position

**Step 1: Update dashboard-widget-editor.svelte to accept initial position**

Add a new prop:

```typescript
interface Props {
    open: boolean;
    widget: DashboardWidget | null;
    dashboardId: string;
    initialRect?: { x: number; y: number; w: number; h: number } | null;
    onClose: () => void;
    onSave: (widget: DashboardWidget) => void;
}

let { open, widget, dashboardId, initialRect = null, onClose, onSave }: Props = $props();
```

In `handleSave()`, use `initialRect` for new widget positioning:

```typescript
x: widget?.x ?? initialRect?.x ?? 0,
y: widget?.y ?? initialRect?.y ?? 0,
w: widget?.w ?? initialRect?.w ?? 6,
h: widget?.h ?? initialRect?.h ?? 2,
```

**Step 2: Add handleDrawWidget in dashboard-view.svelte**

Add state and handler:

```typescript
let drawnRect = $state<{ x: number; y: number; w: number; h: number } | null>(null);

function handleDrawWidget(rect: { x: number; y: number; w: number; h: number }) {
    editingWidget = null;
    drawnRect = rect;
    widgetEditorOpen = true;
}
```

Update `handleAddWidget` to clear `drawnRect`:

```typescript
function handleAddWidget() {
    editingWidget = null;
    drawnRect = null;
    widgetEditorOpen = true;
}
```

Update `handleEditWidget` to clear `drawnRect`:

```typescript
function handleEditWidget(widget: DashboardWidget) {
    editingWidget = widget;
    drawnRect = null;
    widgetEditorOpen = true;
}
```

Update `handleWidgetEditorClose` to clear `drawnRect`:

```typescript
function handleWidgetEditorClose() {
    widgetEditorOpen = false;
    editingWidget = null;
    drawnRect = null;
}
```

**Step 3: Pass props to child components**

Pass `onDrawWidget` to `DashboardGrid`:

```svelte
<DashboardGrid
    {dashboard}
    onEditWidget={handleEditWidget}
    onDrawWidget={handleDrawWidget}
/>
```

Pass `initialRect` to `DashboardWidgetEditor`:

```svelte
<DashboardWidgetEditor
    open={widgetEditorOpen}
    widget={editingWidget}
    dashboardId={dashboard.id}
    initialRect={drawnRect}
    onClose={handleWidgetEditorClose}
    onSave={handleWidgetSave}
/>
```

**Step 4: Also show the grid when editor is open (even if no widgets)**

Update the empty state condition:

```svelte
{#if dashboard.widgets.length === 0 && !widgetEditorOpen}
    <DashboardEmptyState onAddWidget={handleAddWidget} />
{:else}
    <DashboardGrid
        {dashboard}
        onEditWidget={handleEditWidget}
        onDrawWidget={handleDrawWidget}
    />
{/if}
```

This is already the current behavior — no change needed here. The grid shows when `widgetEditorOpen` is true since the condition already handles it.

---

### Task 5: Clear drawnRect after widget save

**Files:**
- Modify: `src/lib/components/dashboard/dashboard-view.svelte`

**Step 1: Update handleWidgetSave**

In `handleWidgetSave`, use `drawnRect` for the new widget position instead of auto-calculating `maxY`:

```typescript
function handleWidgetSave(widget: DashboardWidget) {
    if (!dashboard) return;

    if (editingWidget) {
        db.dashboards.updateWidget(dashboard.id, editingWidget.id, widget);
        db.dashboards.executeWidget(dashboard.id, editingWidget.id);
    } else {
        if (!drawnRect) {
            const maxY = dashboard.widgets.length > 0
                ? Math.max(...dashboard.widgets.map((w) => w.y + w.h))
                : 0;
            widget.y = maxY;
        }
        db.dashboards.addWidget(dashboard.id, widget);
        db.dashboards.executeWidget(dashboard.id, widget.id);
    }

    if (widget.autoRefreshSeconds && widget.autoRefreshSeconds > 0) {
        db.dashboards.startAutoRefresh(dashboard.id, widget.id);
    }

    widgetEditorOpen = false;
    editingWidget = null;
    drawnRect = null;
}
```

The `drawnRect` position is already baked into the widget via `initialRect` in the editor's `handleSave`, so we just skip the `maxY` auto-positioning when `drawnRect` is set.
