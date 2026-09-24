# Connection Drag-and-Drop Reordering — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Allow users to reorder connections in the manage sidebar via drag & drop, with ordering persisted per-project and reflected everywhere connections are listed.

**Architecture:** Persist a per-project `connection_order` JSON array of connection IDs in the `project_state` table (mirroring the existing `tab_order` column). Expose it as `state.connectionOrderByProject` and rewrite the `projectConnections` derived to sort by this array, falling back to append-at-end for unknown IDs. Wire `svelte-dnd-action` in `connections.svelte` to call a new `ConnectionManager.reorder()` method that updates state and schedules persistence.

**Tech Stack:** SvelteKit 5, Svelte 5 runes, `svelte-dnd-action` (already used by `header-tabs.svelte`), SQLite via `tauri-plugin-sql`.

**Reference design:** `docs/plans/2026-04-14-connection-drag-drop-reorder-design.md`

**Testing posture:** No unit-test framework in this repo. Validation per task is `npm run check` (typecheck) plus manual verification in `npm run tauri:dev` at the end. CLAUDE.md forbids committing from Claude — no per-task commits.

---

## Task 1: Schema — add `connection_order` column + migration

**Files:**
- Modify: `src/lib/storage/schema.ts`

**Step 1: Add column to CREATE TABLE DDL.**

In `src/lib/storage/schema.ts`, inside the `project_state` `CREATE TABLE` statement (currently lines ~77-96), add a new column right after `tab_order`:

```sql
tab_order TEXT NOT NULL DEFAULT '[]',
connection_order TEXT NOT NULL DEFAULT '[]',
```

**Step 2: Add ALTER TABLE migration entry.**

In the `columnUpgrades` array inside `upgradeSchema()` (starts ~line 310), append:

```ts
{
  table: "project_state",
  column: "connection_order",
  sql: "ALTER TABLE project_state ADD COLUMN connection_order TEXT NOT NULL DEFAULT '[]'",
},
```

No `CURRENT_STORAGE_VERSION` bump is needed — `upgradeSchema` runs DDL column additions on every startup regardless of version, and the DEFAULT `'[]'` handles existing rows. New projects get `'[]'` from the CREATE TABLE DDL. Data-version-based migrations (`CURRENT_STORAGE_VERSION`) are reserved for data transforms, not column additions.

**Step 3: Validate.**

Run: `npm run check`
Expected: no new errors introduced.

---

## Task 2: Repo — load and save `connection_order`

**Files:**
- Modify: `src/lib/storage/repos/project-state-repo.ts`

**Step 1: Extend the load type and return value.**

In the `load()` function, add `connection_order: string;` to the row type passed to the first `db.query<...>`. Then add to the returned object (before `paneLayout`):

```ts
connectionOrder: safeJsonParse(state.connection_order, []),
```

Treat missing column defensively by matching the `pane_layout` pattern — wrap in a separate try/catch reading from `project_state` if you want to be safe across databases where the migration hasn't run yet. Simpler: rely on the migration guarantee and read inline.

**Step 2: Extend save() SQL + params.**

In `save()`, update the INSERT OR REPLACE statement to include `connection_order` in the column list and a new `?` placeholder, and add `JSON.stringify(state.connectionOrder ?? [])` to the params array at the matching position. Place it adjacent to `tab_order` for readability.

**Step 3: Validate.**

Run: `npm run check`
Expected: TypeScript will error because `PersistedProjectState` doesn't have `connectionOrder` yet — that's handled in Task 3.

---

## Task 3: Extend `PersistedProjectState` type

**Files:**
- Modify: `src/lib/types/project.ts`

**Step 1: Add field.**

In the `PersistedProjectState` interface (line 96), after `tabOrder`:

```ts
/** Ordered list of connection IDs for drag-drop ordering (per project) */
connectionOrder?: string[];
```

Optional (`?`) so legacy persisted states without the field still type-check.

**Step 2: Validate.**

Run: `npm run check`
Expected: no new errors; project-state-repo.ts from Task 2 should now type-check.

---

## Task 4: State — add `connectionOrderByProject`, update `projectConnections` derived

**Files:**
- Modify: `src/lib/hooks/database/state.svelte.ts`

**Step 1: Add reactive state.**

After the existing `tabOrderByProject` declaration (line ~126), add:

```ts
// Connection ordering state (stores ordered array of connection IDs per project)
connectionOrderByProject = $state<Record<string, string[]>>({});
```

**Step 2: Rewrite `projectConnections` derived.**

Replace the existing derived (line 219-223):

```ts
projectConnections = $derived.by(() => {
  if (!this.activeProjectId) return [];
  const filtered = this.connections.filter((c) => c.projectId === this.activeProjectId);
  const order = this.connectionOrderByProject[this.activeProjectId] ?? [];
  if (order.length === 0) return filtered;
  const indexById = new Map(order.map((id, i) => [id, i]));
  const known: typeof filtered = [];
  const unknown: typeof filtered = [];
  for (const c of filtered) {
    if (indexById.has(c.id)) known.push(c);
    else unknown.push(c);
  }
  known.sort((a, b) => indexById.get(a.id)! - indexById.get(b.id)!);
  return [...known, ...unknown];
});
```

Rationale: unknown IDs (freshly-added connections, or legacy data before the order was initialized) appear at the end in their natural order; known IDs sort by the persisted order.

**Step 3: Validate.**

Run: `npm run check`
Expected: no new errors.

---

## Task 5: ConnectionManager — add reorder + order-maintenance helpers

**Files:**
- Modify: `src/lib/hooks/database/connection-manager.svelte.ts`

**Step 1: Add public `reorder` and private helpers.**

At the end of the `ConnectionManager` class (before the closing brace), add:

```ts
/**
 * Replace the entire connection order for a project (used by DnD).
 */
reorder(projectId: string, orderedIds: string[]): void {
  this.state.connectionOrderByProject = {
    ...this.state.connectionOrderByProject,
    [projectId]: [...orderedIds],
  };
  this.persistence.scheduleProject(projectId);
}

/**
 * Append a connection ID to its project's order (if not already present).
 */
private appendToOrder(projectId: string, connectionId: string): void {
  const current = this.state.connectionOrderByProject[projectId] ?? [];
  if (current.includes(connectionId)) return;
  this.state.connectionOrderByProject = {
    ...this.state.connectionOrderByProject,
    [projectId]: [...current, connectionId],
  };
}

/**
 * Remove a connection ID from its project's order.
 */
private removeFromOrder(projectId: string, connectionId: string): void {
  const current = this.state.connectionOrderByProject[projectId] ?? [];
  if (!current.includes(connectionId)) return;
  this.state.connectionOrderByProject = {
    ...this.state.connectionOrderByProject,
    [projectId]: current.filter((id) => id !== connectionId),
  };
}
```

**Step 2: Call `appendToOrder` from `add()`.**

In `add()` (line ~247), after the `this.state.connections.push(newConnection)` block:

```ts
if (!this.state.connections.find((c) => c.id === newConnection.id)) {
  this.state.connections.push(newConnection);
}
this.appendToOrder(projectId, newConnection.id);   // <— add this line
```

**Step 3: Call `appendToOrder` from `initializePersistedConnections()`.**

After Phase 2 pushes connections to state (line ~131), append each to its project's order *only* if not already present (handles legacy rows loaded before order was persisted):

```ts
// Phase 2: Register all connections in state (must complete before loading data)
for (const connection of connectionEntries) {
  this.state.connections.push(connection);
  this.stateRestoration.initializeConnectionMaps(connection.id);
  this.appendToOrder(connection.projectId, connection.id);  // <— add
}
```

This ensures legacy databases where `connection_order` is `'[]'` get backfilled in memory by connection-load order. The first persist will write the full order back to disk.

**Step 4: Call `removeFromOrder` from `remove()`.**

In `remove()` around line 599, right after the filter:

```ts
this.state.connections = this.state.connections.filter((c) => c.id !== id);
this.stateRestoration.cleanupConnectionMaps(id);
if (connection) {
  this.removeFromOrder(connection.projectId, id);
}
```

**Step 5: Call `appendToOrder` from `addDemoConnection()`.**

In `addDemoConnection()` (line ~667), after the `existingIndex` branch:

```ts
this.stateRestoration.initializeConnectionMaps(connectionId);
this.appendToOrder(projectId, connectionId);   // <— add
```

**Step 6: Validate.**

Run: `npm run check`
Expected: no new errors.

---

## Task 6: Persistence manager — serialize/hydrate `connectionOrder`

**Files:**
- Modify: `src/lib/hooks/database/persistence-manager.svelte.ts`

**Step 1: Serialize on save.**

In `persistProjectState()` (line ~382), add to the `state: PersistedProjectState = { ... }` object (next to `tabOrder`):

```ts
tabOrder: this.state.tabOrderByProject[projectId] ?? [],
connectionOrder: this.state.connectionOrderByProject[projectId] ?? [],
```

**Step 2: Validate.**

Run: `npm run check`
Expected: no new errors.

---

## Task 7: Project manager — hydrate `connectionOrderByProject` on project load

**Files:**
- Modify: `src/lib/hooks/database/project-manager.svelte.ts`

**Step 1: Initialize empty array for new projects.**

In `loadProjectState()`, in the `if (!persistedState)` branch (line ~709), after `this.state.tabOrderByProject[projectId] = [];` add:

```ts
this.state.connectionOrderByProject[projectId] = [];
```

**Step 2: Restore from persisted state.**

Further down in the same function (around line 811, near `this.state.tabOrderByProject[projectId] = persistedState.tabOrder;`), add:

```ts
this.state.connectionOrderByProject[projectId] = persistedState.connectionOrder ?? [];
```

Use `?? []` because `connectionOrder` is optional on the type.

**Step 3: Validate.**

Run: `npm run check`
Expected: no new errors.

---

## Task 8: Sidebar — wire `svelte-dnd-action`

**Files:**
- Modify: `src/lib/components/sidebar/manage/connections.svelte`

**Step 1: Add import and local drag state.**

At the top of the `<script lang="ts">` block, add:

```ts
import { dndzone } from "svelte-dnd-action";
```

In the existing state block, add:

```ts
let isDragging = $state(false);
let draggedConnections = $state<typeof db.state.projectConnections>([]);
const displayConnections = $derived(isDragging ? draggedConnections : db.state.projectConnections);
```

Note: `svelte-dnd-action` requires each list item to carry an `id` property at the top level — `DatabaseConnection` already has `id`, so we can pass connections directly without wrapping.

**Step 2: Add DnD handlers.**

Still in the `<script>` block:

```ts
function handleDndConsider(e: CustomEvent<{ items: typeof db.state.projectConnections }>) {
  isDragging = true;
  draggedConnections = e.detail.items;
}

function handleDndFinalize(e: CustomEvent<{ items: typeof db.state.projectConnections }>) {
  isDragging = false;
  draggedConnections = [];
  const projectId = db.state.activeProjectId;
  if (!projectId) return;
  db.connections.reorder(projectId, e.detail.items.map((c) => c.id));
}
```

**Step 3: Apply `use:dndzone` to the connections list.**

Replace the `<Sidebar.Menu class="px-2">` opening tag (line ~110) and its `{#each}` iteration:

```svelte
<Sidebar.Menu
  class="px-2"
  use:dndzone={{
    items: displayConnections,
    type: 'connections',
    dropTargetStyle: {},
    dragDisabled: false,
    flipDurationMs: 150,
  }}
  onconsider={handleDndConsider}
  onfinalize={handleDndFinalize}
>
  {#each displayConnections as connection (connection.id)}
```

Close out the `{/each}` as before.

**Step 4: Wrap each item's root in a div with the connection id.**

`svelte-dnd-action` sets its item identifier from the DOM `id` of each root child. The current `{#each}` renders a `<ContextMenu.Root>` as the first child — that is a Bits UI component that may not accept `id` as a DOM attribute. Wrap each iteration with an explicit keyed `<li>` or `<div>`:

```svelte
{#each displayConnections as connection (connection.id)}
  <div id={connection.id}>
    <ContextMenu.Root>
      <!-- ...existing content unchanged... -->
    </ContextMenu.Root>
  </div>
{/each}
```

Keep all existing behavior inside the wrapper untouched.

**Step 5: Validate.**

Run: `npm run check`
Expected: no new errors.

---

## Task 9: Manual verification in dev

**Step 1: Start the app.**

Run: `npm run tauri:dev`

**Step 2: Golden-path checks.**

Do all of these:

1. **Reorder.** Grab a connection in the sidebar "Connections" section and drop it to a new position. Confirm it visually moves and stays there.
2. **Persist across reload.** Close and reopen the app (or refresh dev). Confirm the new order is preserved.
3. **Add connection.** Add a new connection via the `+` button. Confirm it appears at the bottom of the list.
4. **Remove connection.** Delete a connection. Confirm the remaining connections keep their relative order.
5. **Reflected in other surfaces.** If the app has a connection selector or any connection picker, open it and confirm the new order is respected (this is why we changed the `projectConnections` derived rather than sorting in the sidebar alone).
6. **Multiple projects.** Switch to another project and reorder its connections independently. Switch back — each project retains its own ordering.
7. **Context menu unaffected.** Right-click a connection — the existing context menu still works. Click on a connection — it still activates (drag is separate from click).

**Step 3: Typecheck and lint once more.**

Run: `npm run check`
Run: `npm run lint`
Expected: both pass with no new errors or warnings introduced.

---

## Rollout notes

- No breaking changes: `connectionOrder` is optional in the persisted type, and the derived handles unknown IDs gracefully.
- No data migration required — the DEFAULT `'[]'` on the new column backfills existing rows, and `initializePersistedConnections` fills `connectionOrderByProject` in memory on startup based on load order. The first persist writes the populated order back to disk.
- No i18n changes (no user-facing strings added).
