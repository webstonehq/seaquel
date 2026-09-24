# Connection Drag-and-Drop Reordering — Design

Date: 2026-04-14

## Goal

Allow users to reorder connections via drag & drop in the manage sidebar. The
new order is persisted per project and is reflected everywhere connections are
listed (sidebar, connection selector dropdown, pickers).

## Data model & persistence

Mirror the existing `tab_order` pattern: store the ordering as a per-project
JSON array of connection IDs in the `project_state` table.

### Schema change

In `src/lib/storage/schema.ts`:

```sql
ALTER TABLE project_state ADD COLUMN connection_order TEXT NOT NULL DEFAULT '[]';
```

- Bump `CURRENT_STORAGE_VERSION` from 4 to 5.
- Add a migration that backfills `connection_order` for each existing project
  from the current connection load order (the order returned by
  `connectionsRepo.loadAll` filtered by `project_id`).

### Runtime state

In `src/lib/hooks/database/state.svelte.ts`:

- Add `connectionOrderByProject: Record<string, string[]>` (parallel in spirit
  to how tab ordering is kept per project).
- Rewrite the `projectConnections` derived so that it filters by
  `activeProjectId` and then sorts by the active project's `connectionOrder`
  array. Any connection whose ID is not yet present in the order array (freshly
  added, old data, or edge cases) is appended at the end in its natural order.

### Invariants

- Adding a connection: append its ID to the active project's order array.
- Removing a connection: filter its ID out of the order array.
- Reordering via DnD: replace the array with the new ordering and schedule
  persistence.

### Alternative considered

Adding `display_order INTEGER` to the `connections` table was considered and
rejected: it would require rewriting N connection rows on every reorder, and
doesn't match the tab-order pattern already in use.

## Manager API

New methods on `ConnectionManager`
(`src/lib/hooks/database/connection-manager.svelte.ts`):

```ts
// Replace the entire order for a project (used by DnD).
reorder(projectId: string, orderedIds: string[]): void

// Internal helpers, invoked from existing add/remove paths.
private appendToOrder(projectId: string, connectionId: string): void
private removeFromOrder(projectId: string, connectionId: string): void
```

`reorder` updates `state.connectionOrderByProject[projectId]` and calls
`persistence.scheduleProject(projectId)` so the new ordering is written with
the rest of the project state.

`appendToOrder` and `removeFromOrder` are invoked from the single choke-points
`ConnectionManager.add` and `ConnectionManager.remove`, so every code path that
creates or deletes a connection (sidebar button, DBeaver import, shared-repo
sync, deep-link clone, project import, etc.) keeps the order array in sync
without further plumbing.

## Persistence wiring

- `src/lib/storage/repos/project-state-repo.ts`: serialize/deserialize
  `connection_order` alongside the existing `tab_order`.
- `src/lib/hooks/database/persistence-manager.svelte.ts`: include
  `connectionOrderByProject[projectId]` in the payload written by
  `scheduleProject` / `persistProjectState`, and hydrate it on load.

## Sidebar drag-and-drop

In `src/lib/components/sidebar/manage/connections.svelte`, wrap the
`{#each db.state.projectConnections …}` container with `use:dndzone`, mirroring
`src/lib/components/header-tabs.svelte`:

```svelte
<Sidebar.Menu
  class="px-2"
  use:dndzone={{
    items: displayConnections,
    type: 'connections',
    dropTargetStyle: {},
  }}
  onconsider={handleConsider}
  onfinalize={handleFinalize}
>
```

- `displayConnections` is a local `$state` array that mirrors
  `db.state.projectConnections` during an active drag, so the grabbed item
  renders in its hover position without mutating global state mid-drag.
- On `onfinalize` we call
  `db.connections.reorder(activeProjectId, newOrder)` and clear the local
  drag state.
- The existing context menu and click handlers are untouched —
  `svelte-dnd-action` only hijacks pointer drags on the item itself, not
  clicks. Inner action buttons (labels tooltip, git-branch toggle) already
  call `e.stopPropagation()` on click.

## Edge cases

- **First load with no persisted order.** The migration backfills from the
  current insertion order; new projects start with `[]` and append as
  connections are added.
- **Connections added by non-UI code paths** (shared repo sync, DBeaver
  import, deep-link clone). All paths funnel through `ConnectionManager.add`,
  so ordering stays consistent.
- **Project switching.** The `projectConnections` derived reads from
  `connectionOrderByProject[activeProjectId]`; each project has its own
  ordering automatically.
- **Demo project.** Uses the same mechanism; the demo connection appears in
  whatever order it was added.
- **Drag started on inner action button.** Buttons inside the connection row
  already stop propagation on click. For DnD specifically, the dnd handles
  on the outer item only.
- **Reorder while a connection is connecting.** Purely visual; no effect on
  provider state.

## Testing

- **Unit.** Small test around the derived sort logic: given an order array and
  a connection list, confirm the expected ordered output, including the
  append-unknown-IDs behavior.
- **Manual e2e.**
  - Drag to reorder in the sidebar → reload app → order persists.
  - Switch projects → each has an independent order.
  - Add a new connection → appears at the bottom.
- **Migration.** Open a pre-v5 database, confirm `connection_order` is
  populated from current insertion order after migration.

## Files touched

- `src/lib/storage/schema.ts` (DDL + version bump + migration)
- `src/lib/storage/repos/project-state-repo.ts` (serde)
- `src/lib/hooks/database/state.svelte.ts` (new state, rewrite derived)
- `src/lib/hooks/database/persistence-manager.svelte.ts` (hydrate/save)
- `src/lib/hooks/database/connection-manager.svelte.ts` (reorder + append/remove helpers, invoked from add/remove)
- `src/lib/components/sidebar/manage/connections.svelte` (dndzone wiring)
