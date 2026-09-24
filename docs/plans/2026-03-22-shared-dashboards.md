# Shared Dashboards Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add sharing functionality to dashboards (starred/local/shared sections, share/unshare via git, delete) mirroring the existing query sharing pattern.

**Architecture:** Dashboards are stored as `.json` files in `.seaquel/projects/<name>/dashboards/` (parallel to `queries/`). A `SharedDashboard` type mirrors `SharedQuery`. A `SharedDashboardManager` mirrors `SharedQueryManager`. The sidebar dashboard tab is restructured from a flat list into starred/local/shared collapsible sections matching the queries tab pattern. Dashboards get `starred` field + `starredSharedDashboardIds` in state for shared starred tracking.

**Tech Stack:** SvelteKit 5 + Svelte 5 runes, TypeScript, Tauri FS APIs

---

### Task 1: Add SharedDashboard type and Dashboard starred field

**Files:**
- Modify: `src/lib/types/shared-queries.ts` (add SharedDashboard interface)
- Modify: `src/lib/types/dashboard.ts` (add `starred` to Dashboard)
- Modify: `src/lib/types/index.ts` (export SharedDashboard)

**Step 1: Add SharedDashboard type**

In `src/lib/types/shared-queries.ts`, after the `SharedQuery` interface (line 63), add:

```typescript
/**
 * A shared dashboard loaded from a .json file in the repo.
 */
export interface SharedDashboard {
  /** Unique identifier (derived from repo ID + file path) */
  id: string;
  /** ID of the containing repo */
  repoId: string;
  /** Relative path within the repo */
  filePath: string;
  /** Display name */
  name: string;
  /** Optional description */
  description?: string;
  /** Dashboard widgets (serialized) */
  widgets: import("./dashboard").DashboardWidget[];
  /** Dashboard viewport */
  viewport: { x: number; y: number; zoom: number };
  /** Date filter */
  dateFilter?: { start: string; end: string } | null;
  /** File modification time */
  updatedAt?: Date;
}
```

**Step 2: Add `starred` to Dashboard**

In `src/lib/types/dashboard.ts`, add `starred?: boolean;` to the `Dashboard` interface after `updatedAt`.

**Step 3: Export SharedDashboard**

In `src/lib/types/index.ts`, add `SharedDashboard` to the re-exports from `./shared-queries`.

**Step 4: Run type check**

Run: `npm run check`
Expected: PASS (new types are additive)

---

### Task 2: Add shared dashboard state fields and derived values

**Files:**
- Modify: `src/lib/hooks/database/state.svelte.ts`

**Step 1: Add state fields**

After `starredSharedQueryIds` (line 105), add:

```typescript
/** Set of shared dashboard IDs that the user has starred (persisted locally) */
starredSharedDashboardIds = $state<Set<string>>(new Set());
```

After `sharedQueriesByRepo` (line 110), add:

```typescript
sharedDashboardsByRepo = $state<Record<string, SharedDashboard[]>>({});
```

Add `SharedDashboard` to the imports at the top.

**Step 2: Add derived values**

After `activeRepoQueries` derived (line 388), add:

```typescript
// Derived: shared dashboards for active repo
activeRepoDashboards = $derived(
  this.activeRepoId ? (this.sharedDashboardsByRepo[this.activeRepoId] ?? []) : [],
);
```

After `allSharedQueries` derived (line 397), add:

```typescript
// Derived: all shared dashboards across all repos (for search)
allSharedDashboards = $derived(Object.values(this.sharedDashboardsByRepo).flat());
```

**Step 3: Run type check**

Run: `npm run check`
Expected: PASS

---

### Task 3: Create dashboard file parser

**Files:**
- Create: `src/lib/services/dashboard-file-parser.ts`

**Step 1: Create parser**

```typescript
/**
 * Parser for dashboard .json files in shared repos.
 */
import type { SharedDashboard } from "$lib/types";
import type { DashboardWidget } from "$lib/types/dashboard";

interface DashboardFileContent {
  name: string;
  description?: string;
  widgets: DashboardWidget[];
  viewport?: { x: number; y: number; zoom: number };
  dateFilter?: { start: string; end: string } | null;
}

/**
 * Parse a dashboard .json file into a SharedDashboard object.
 */
export function parseDashboardFile(
  content: string,
  repoId: string,
  filePath: string,
): SharedDashboard | null {
  try {
    const data: DashboardFileContent = JSON.parse(content);

    if (!data.name) {
      const fileName = filePath.split("/").pop() || "untitled";
      data.name = fileName.replace(/\.json$/i, "").replace(/[-_]/g, " ");
    }

    // Strip runtime state from widgets
    const widgets = (data.widgets ?? []).map(
      ({ result: _, isLoading: __, error: ___, lastRefreshed: ____, ...rest }) => rest,
    ) as DashboardWidget[];

    return {
      id: `${repoId}:${filePath}`,
      repoId,
      filePath,
      name: data.name,
      description: data.description,
      widgets,
      viewport: data.viewport ?? { x: 0, y: 0, zoom: 1 },
      dateFilter: data.dateFilter ?? null,
    };
  } catch {
    console.warn(`Failed to parse dashboard file: ${filePath}`);
    return null;
  }
}

/**
 * Serialize a SharedDashboard to JSON file content.
 */
export function serializeDashboardFile(dashboard: SharedDashboard): string {
  // Strip runtime state from widgets
  const widgets = dashboard.widgets.map(
    ({ result: _, isLoading: __, error: ___, lastRefreshed: ____, ...rest }) => rest,
  );

  const content: DashboardFileContent = {
    name: dashboard.name,
    ...(dashboard.description && { description: dashboard.description }),
    widgets,
    viewport: dashboard.viewport,
    ...(dashboard.dateFilter && { dateFilter: dashboard.dateFilter }),
  };

  return JSON.stringify(content, null, 2) + "\n";
}

/**
 * Generate a valid filename from a dashboard name.
 */
export function dashboardNameToFilename(name: string): string {
  const base = name
    .toLowerCase()
    .replace(/[^a-z0-9\s-]/g, "")
    .replace(/\s+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");
  return `${base || "untitled"}.json`;
}
```

**Step 2: Run type check**

Run: `npm run check`
Expected: PASS

---

### Task 4: Create SharedDashboardManager

**Files:**
- Create: `src/lib/hooks/database/shared-dashboard-manager.svelte.ts`

**Step 1: Create manager**

Model this directly after `SharedQueryManager` but for dashboards. Key methods: `createDashboard`, `deleteDashboard`, `shareDashboard`, `unshareDashboard`, `getDashboard`.

```typescript
import type { SharedDashboard, Dashboard } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import { SEAQUEL_DIR, type SharedRepoManager } from "./shared-repo-manager.svelte.js";
import {
  parseDashboardFile,
  serializeDashboardFile,
  dashboardNameToFilename,
} from "$lib/services/dashboard-file-parser";
import { nameToFilename } from "$lib/services/config-file-parser";
import { readTextFile, writeTextFile, remove, mkdir, exists } from "@tauri-apps/plugin-fs";
import { join, dirname } from "@tauri-apps/api/path";

function parseDashboardId(dashboardId: string): { repoId: string; filePath: string } {
  const [repoId, ...pathParts] = dashboardId.split(":");
  return { repoId, filePath: pathParts.join(":") };
}

export class SharedDashboardManager {
  constructor(
    private state: DatabaseState,
    private repoManager: SharedRepoManager,
  ) {}

  private getDashboardsBasePath(): string | null {
    const project = this.state.projects.find((p) => p.id === this.state.activeProjectId);
    if (!project) return null;
    const dirName = nameToFilename(project.name);
    return `${SEAQUEL_DIR}/projects/${dirName}/dashboards`;
  }

  async createDashboard(
    name: string,
    widgets: Dashboard["widgets"],
    viewport: Dashboard["viewport"],
    options?: {
      description?: string;
      dateFilter?: Dashboard["dateFilter"];
    },
  ): Promise<string | null> {
    const repoId = this.state.activeRepoId;
    if (!repoId) return null;

    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return null;

    const dashboardsBase = this.getDashboardsBasePath();
    if (!dashboardsBase) return null;

    const filename = dashboardNameToFilename(name);
    const filePath = `${dashboardsBase}/${filename}`;

    const sharedDashboard: SharedDashboard = {
      id: `${repoId}:${filePath}`,
      repoId,
      filePath,
      name,
      description: options?.description,
      widgets,
      viewport,
      dateFilter: options?.dateFilter ?? null,
      updatedAt: new Date(),
    };

    const content = serializeDashboardFile(sharedDashboard);
    const fullPath = await join(repo.path, filePath);
    const folderPath = await dirname(fullPath);

    if (!(await exists(folderPath))) {
      await mkdir(folderPath, { recursive: true });
    }

    await writeTextFile(fullPath, content);

    const dashboards = this.state.sharedDashboardsByRepo[repoId] ?? [];
    this.state.sharedDashboardsByRepo = {
      ...this.state.sharedDashboardsByRepo,
      [repoId]: [...dashboards, sharedDashboard],
    };

    await this.repoManager.refreshRepoStatus(repoId);
    return sharedDashboard.id;
  }

  async deleteDashboard(dashboardId: string): Promise<boolean> {
    const { repoId, filePath } = parseDashboardId(dashboardId);

    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return false;

    const dashboards = this.state.sharedDashboardsByRepo[repoId] ?? [];
    const dashboard = dashboards.find((d) => d.id === dashboardId);
    if (!dashboard) return false;

    const fullPath = await join(repo.path, filePath);
    await remove(fullPath);

    this.state.sharedDashboardsByRepo = {
      ...this.state.sharedDashboardsByRepo,
      [repoId]: dashboards.filter((d) => d.id !== dashboardId),
    };

    await this.repoManager.refreshRepoStatus(repoId);
    return true;
  }

  getDashboard(dashboardId: string): SharedDashboard | null {
    const { repoId } = parseDashboardId(dashboardId);
    const dashboards = this.state.sharedDashboardsByRepo[repoId] ?? [];
    return dashboards.find((d) => d.id === dashboardId) ?? null;
  }

  async shareDashboard(dashboard: Dashboard): Promise<string | null> {
    // Strip runtime state from widgets
    const widgets = dashboard.widgets.map(
      ({ result: _, isLoading: __, error: ___, lastRefreshed: ____, ...rest }) => rest,
    );
    return this.createDashboard(dashboard.name, widgets, dashboard.viewport, {
      dateFilter: dashboard.dateFilter,
    });
  }

  async unshareDashboard(dashboardId: string): Promise<boolean> {
    return this.deleteDashboard(dashboardId);
  }
}
```

**Step 2: Run type check**

Run: `npm run check`
Expected: PASS

---

### Task 5: Load shared dashboards from repo + cleanup on removeRepo

**Files:**
- Modify: `src/lib/hooks/database/shared-repo-manager.svelte.ts`

**Step 1: Import dashboard parser**

Add to imports at top:

```typescript
import { parseDashboardFile } from "$lib/services/dashboard-file-parser";
```

**Step 2: Add dashboard scanning to `loadQueriesFromRepo`**

In the `loadQueriesFromRepo` method (around line 407), after scanning for queries, also scan for dashboards. Add a `dashboards: SharedDashboard[]` array, scan `.seaquel/projects/<name>/dashboards/` for `.json` files, and set `this.state.sharedDashboardsByRepo[repoId] = dashboards`.

Inside the `for (const entry of entries)` loop (after the queries scan block), add:

```typescript
const dashboardsDir = await join(projectsDir, entry.name, "dashboards");
if (await exists(dashboardsDir)) {
  const dashboardsRelBase = `${SEAQUEL_DIR}/projects/${entry.name}/dashboards`;
  await this.scanDashboardDirectory(repo.path, dashboardsRelBase, repoId, dashboards);
}
```

And after setting `sharedQueriesByRepo`, add:

```typescript
this.state.sharedDashboardsByRepo = {
  ...this.state.sharedDashboardsByRepo,
  [repoId]: dashboards,
};
```

**Step 3: Add `scanDashboardDirectory` method**

Add a private method to scan for .json dashboard files (similar to `scanDirectory` but for .json):

```typescript
private async scanDashboardDirectory(
  basePath: string,
  relativePath: string,
  repoId: string,
  dashboards: SharedDashboard[],
): Promise<void> {
  const fullPath = relativePath ? await join(basePath, relativePath) : basePath;

  try {
    const entries = await readDir(fullPath);

    for (const entry of entries) {
      if (entry.name.startsWith(".")) continue;

      const entryRelativePath = relativePath ? `${relativePath}/${entry.name}` : entry.name;

      if (entry.isDirectory) {
        await this.scanDashboardDirectory(basePath, entryRelativePath, repoId, dashboards);
      } else if (entry.name.toLowerCase().endsWith(".json")) {
        const filePath = await join(basePath, entryRelativePath);
        const content = await readTextFile(filePath);
        const dashboard = parseDashboardFile(content, repoId, entryRelativePath);

        if (dashboard) {
          try {
            const meta = await stat(filePath);
            if (meta.mtime) {
              dashboard.updatedAt = new Date(meta.mtime);
            }
          } catch {
            // Ignore stat errors
          }
          dashboards.push(dashboard);
        }
      }
    }
  } catch (error) {
    console.warn(`Failed to scan dashboard directory ${fullPath}:`, error);
  }
}
```

Add `SharedDashboard` to the imports from `$lib/types`.

**Step 4: Clean up dashboards in `removeRepo`**

In the `removeRepo` method (around line 219), after cleaning up `sharedQueriesByRepo`, add:

```typescript
const { [repoId]: _dashboards2, ...remainingDashboards } = this.state.sharedDashboardsByRepo;
this.state.sharedDashboardsByRepo = remainingDashboards;
```

**Step 5: Add dashboards dir to `exportProject`**

In `exportProject` (around line 689-696), after `const queriesDir`, add:

```typescript
const dashboardsDir = await join(projectDir, "dashboards");
```

And after `await mkdir(queriesDir, { recursive: true });`, add:

```typescript
await mkdir(dashboardsDir, { recursive: true });
```

**Step 6: Add dashboards dir to `setGitRepoPath`**

In `project-manager.svelte.ts` `setGitRepoPath` (around line 219-220), after creating `queries` dir, add:

```typescript
await mkdir(await join(projectDir, "dashboards"), { recursive: true });
```

**Step 7: Run type check**

Run: `npm run check`
Expected: PASS

---

### Task 6: Add starred dashboard toggle + persistence

**Files:**
- Modify: `src/lib/hooks/database/dashboard-manager.svelte.ts` (add starred toggle methods)
- Modify: `src/lib/hooks/database/persistence-manager.svelte.ts` (persist starredSharedDashboardIds)
- Modify: `src/lib/hooks/database/project-manager.svelte.ts` (restore starredSharedDashboardIds)
- Modify: `src/lib/types/project.ts` (add to PersistedProjectState)
- Modify: `src/lib/storage/repository.ts` (load/save starred dashboard IDs)
- Modify: `src/lib/storage/schema.ts` (add column to project_state table)

**Step 1: Add `starred` toggle methods to DashboardManager**

Add to `DashboardManager` class:

```typescript
toggleDashboardStarred(id: string): void {
  const projectId = this.state.activeProjectId;
  if (!projectId) return;

  const dashboards = this.state.dashboardsByProject[projectId] ?? [];
  this.state.dashboardsByProject = {
    ...this.state.dashboardsByProject,
    [projectId]: dashboards.map((d) =>
      d.id === id ? { ...d, starred: !d.starred } : d,
    ),
  };
  this.scheduleProjectPersistence(projectId);
}

toggleSharedDashboardStarred(id: string): void {
  const newSet = new Set(this.state.starredSharedDashboardIds);
  if (newSet.has(id)) {
    newSet.delete(id);
  } else {
    newSet.add(id);
  }
  this.state.starredSharedDashboardIds = newSet;
  this.scheduleProjectPersistence(this.state.activeProjectId);
}
```

**Step 2: Add `starred` to dashboard persistence**

In `persistence-manager.svelte.ts`, in `persistProjectState` method, after `starredSharedQueryIds`, add:

```typescript
starredSharedDashboardIds: Array.from(this.state.starredSharedDashboardIds),
```

**Step 3: Add to PersistedProjectState**

In `src/lib/types/project.ts`, add after `starredSharedQueryIds`:

```typescript
/** IDs of shared dashboards that are starred */
starredSharedDashboardIds?: string[];
```

**Step 4: Restore on load**

In `project-manager.svelte.ts` `loadProjectState`, after restoring `starredSharedQueryIds` (line 717), add:

```typescript
this.state.starredSharedDashboardIds = new Set(persistedState.starredSharedDashboardIds ?? []);
```

**Step 5: Add `starred` to dashboard SQLite persistence**

In `src/lib/storage/schema.ts`, add migration to add `starred` column to dashboards table and `starred_shared_dashboard_ids` column to project_state table. Or if migrations aren't versioned, add the column to the schema definition.

In `src/lib/storage/repository.ts`, in the `projectStateRepo`:
- In `save()`: serialize `starredSharedDashboardIds` just like `starredSharedQueryIds`
- In `load()`: deserialize it

In `dashboardsRepo.save()`, persist the `starred` field. In `dashboardsRepo.loadByProject()`, load it.

**Step 6: Run type check**

Run: `npm run check`
Expected: PASS

---

### Task 7: Wire SharedDashboardManager into UseDatabase

**Files:**
- Modify: `src/lib/hooks/database.svelte.ts`

**Step 1: Import and instantiate**

Add import:
```typescript
import { SharedDashboardManager } from "./database/shared-dashboard-manager.svelte.js";
```

Add to class properties:
```typescript
readonly sharedDashboards: SharedDashboardManager;
```

After `this.sharedQueries = new SharedQueryManager(...)` (line 213), add:
```typescript
this.sharedDashboards = new SharedDashboardManager(this.state, this.sharedRepos);
```

**Step 2: Run type check**

Run: `npm run check`
Expected: PASS

---

### Task 8: Restructure dashboard sidebar with starred/local/shared sections

**Files:**
- Modify: `src/lib/components/sidebar-manage.svelte`

This is the main UI change. Replace the current flat dashboard list (lines 1045-1109) with the starred/local/shared collapsible pattern matching queries.

**Step 1: Add dashboard state variables and handlers**

In the script section, add new state/derived values:

```typescript
// Dashboard section state
let dashboardStarredExpanded = $state(true);
let dashboardLocalExpanded = $state(true);
let dashboardSharedExpanded = $state(true);
let dashboardSearchQuery = $state("");

// Filtered shared dashboards
const filteredSharedDashboards = $derived(
  db.state.activeRepoDashboards.filter(
    (item) => item.name.toLowerCase().includes(dashboardSearchQuery.toLowerCase()),
  ),
);

// Shared dashboard names to exclude from local list
const sharedDashboardNames = $derived(
  new Set(db.state.activeRepoDashboards.map((d) => d.name.toLowerCase())),
);

// Local dashboards (exclude those that exist as shared, filter by search)
const filteredLocalDashboards = $derived(
  db.state.projectDashboards.filter(
    (item) =>
      !sharedDashboardNames.has(item.name.toLowerCase()) &&
      item.name.toLowerCase().includes(dashboardSearchQuery.toLowerCase()),
  ),
);

// Starred dashboards
const starredLocalDashboards = $derived(
  filteredLocalDashboards.filter((item) => item.starred),
);
const starredSharedDashboards = $derived(
  filteredSharedDashboards.filter((item) => db.state.starredSharedDashboardIds.has(item.id)),
);
const totalStarredDashboardCount = $derived(starredLocalDashboards.length + starredSharedDashboards.length);

// Non-starred local dashboards
const localDashboards = $derived(
  filteredLocalDashboards.filter((item) => !item.starred),
);

// Non-starred shared dashboards
const nonStarredSharedDashboards = $derived(
  filteredSharedDashboards.filter((item) => !db.state.starredSharedDashboardIds.has(item.id)),
);
```

**Step 2: Add share/unshare dashboard handlers**

```typescript
const handleShareDashboard = async (dashboard: Dashboard) => {
  try {
    const wasStarred = dashboard.starred;
    // Delete the local copy first
    db.dashboards.deleteDashboard(dashboard.id);
    const sharedId = await db.sharedDashboards.shareDashboard(dashboard);
    if (sharedId && wasStarred) {
      db.dashboards.toggleSharedDashboardStarred(sharedId);
    }
  } catch (error) {
    console.error("Failed to share dashboard:", error);
  }
};

const handleUnshareDashboard = async (dashboardId: string) => {
  try {
    const wasStarred = db.state.starredSharedDashboardIds.has(dashboardId);
    const sharedDashboard = db.sharedDashboards.getDashboard(dashboardId);
    if (sharedDashboard) {
      // Create local copy
      const local = await db.dashboards.createDashboard(sharedDashboard.name);
      if (local) {
        // Copy widgets and viewport to the local dashboard
        for (const widget of sharedDashboard.widgets) {
          await db.dashboards.addWidget(local.id, { ...widget, id: `widget-${crypto.randomUUID()}` });
        }
        await db.dashboards.updateViewport(local.id, sharedDashboard.viewport);
        if (wasStarred) {
          db.dashboards.toggleDashboardStarred(local.id);
        }
      }
    }
    await db.sharedDashboards.unshareDashboard(dashboardId);
    if (wasStarred) {
      db.dashboards.toggleSharedDashboardStarred(dashboardId);
    }
  } catch (error) {
    console.error("Failed to unshare dashboard:", error);
  }
};

const handleSharedDashboardClick = (dashboard: SharedDashboard) => {
  // Open shared dashboard in a tab — create a temporary local dashboard from the shared data
  // For now, just open a new dashboard tab
  db.dashboardTabs.add(undefined, dashboard.name);
};
```

Update `dashboardToDelete` type to include a `type` field:
```typescript
let dashboardToDelete = $state<{ id: string; name: string; type: "local" | "shared" } | null>(null);
```

Update `confirmDeleteDashboard`:
```typescript
const confirmDeleteDashboard = () => {
  if (!dashboardToDelete) return;
  if (dashboardToDelete.type === "shared") {
    db.sharedDashboards.deleteDashboard(dashboardToDelete.id);
  } else {
    db.dashboards.deleteDashboard(dashboardToDelete.id);
    const tabsToClose = db.state.dashboardTabs.filter(
      (t) => t.dashboardId === dashboardToDelete!.id
    );
    for (const t of tabsToClose) {
      db.dashboardTabs.remove(t.id);
    }
  }
  showDeleteDashboardDialog = false;
  dashboardToDelete = null;
};
```

**Step 3: Replace dashboards tab panel HTML**

Replace the dashboards tab panel content (lines 1045-1109) with the new starred/local/shared structure. Follow the exact same pattern as the queries tab (starred collapsible with starred local + starred shared items, local collapsible, shared collapsible).

Each dashboard item should show:
- `LayoutDashboardIcon` for local, same icon for shared
- Dashboard name
- Widget count badge
- Hover actions: trash icon (delete), star icon, git branch icon (share/unshare)
- Context menu on shared items: "Mark as local only", "Delete"

**Step 4: Update footer count**

Update the dashboards footer to include shared count:

```typescript
{:else if sidebarTab === "dashboards"}
  {db.state.projectDashboards.length + db.state.activeRepoDashboards.length} dashboard{(db.state.projectDashboards.length + db.state.activeRepoDashboards.length) !== 1 ? 's' : ''}
```

**Step 5: Add search input**

Add a search input at the top of the dashboards panel (matching the queries panel pattern) that filters by `dashboardSearchQuery`.

**Step 6: Run type check**

Run: `npm run check`
Expected: PASS

---

### Task 9: Handle dashboard cleanup on project unshare

**Files:**
- Modify: `src/lib/hooks/database/project-manager.svelte.ts`

In `setGitRepoPath`, the existing unshare cleanup code already calls `this.sharedRepos.removeRepo(repoId)` which cleans up `sharedDashboardsByRepo` (added in Task 5 Step 4). No additional work needed here — verify this is correct.

**Step 1: Run type check**

Run: `npm run check`
Expected: PASS

---

### Task 10: Final verification

**Step 1: Run full type check**

Run: `npm run check`
Expected: 0 errors

**Step 2: Manual testing checklist**

1. Create a dashboard, verify it shows in "Local" section
2. Star a dashboard, verify it moves to "Starred" section
3. Share a project (link git repo), then share a dashboard — verify it moves to "Shared" section
4. Unshare a dashboard — verify it moves back to "Local"
5. Delete a dashboard from each section
6. Unshare the project — verify shared dashboards disappear
7. Re-share the project — verify shared dashboards reappear from disk
