# Convert Starter Tabs to Regular Tabs Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Convert starter tabs (Getting Started, Migration Tips) from a separate system into regular tabs managed by `BaseTabManager`, integrated with tab ordering and pane system — eliminating all special-case code.

**Architecture:** Add `"starter"` as a new `ViewType`. Rewrite `StarterTabManager` to extend `BaseTabManager<StarterTab>`. Wire it into `TabOrderingManager.ordered`, `PaneManager.getTabViewType`/`syncGlobalActiveState`, persistence, and rendering. Remove all special-case starter tab rendering and the old `initializeStarterTabs` callback wiring.

**Tech Stack:** SvelteKit 5, Svelte 5 runes, TypeScript

---

### Task 1: Add `"starter"` to `ViewType` and state

Add `"starter"` everywhere the view type union appears.

**Files:**
- Modify: `src/lib/hooks/database/pane-manager.svelte.ts` (ViewType, getTabViewType, syncGlobalActiveState)
- Modify: `src/lib/hooks/database/state.svelte.ts` (activeView type)
- Modify: `src/lib/hooks/database.svelte.ts` (setActiveView type)

**Step 1: Add `"starter"` to ViewType in pane-manager.svelte.ts**

At line 4, change:
```ts
type ViewType =
  | "query"
  | "schema"
  | "explain"
  | "erd"
  | "statistics"
  | "workflow"
  | "visualize"
  | "connection"
  | "dashboard";
```
To:
```ts
type ViewType =
  | "query"
  | "schema"
  | "explain"
  | "erd"
  | "statistics"
  | "workflow"
  | "visualize"
  | "connection"
  | "dashboard"
  | "starter";
```

**Step 2: Add starter to `getTabViewType` in pane-manager.svelte.ts**

After the dashboard check (line ~307), add:
```ts
    if ((this.state.starterTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "starter";
```

**Step 3: Add starter to `syncGlobalActiveState` in pane-manager.svelte.ts**

In the `setters` record (after the `dashboard` entry, around line 484), add:
```ts
      starter: (id) => {
        this.state.activeStarterTabIdByProject = {
          ...this.state.activeStarterTabIdByProject,
          [projectId]: id,
        };
      },
```

**Step 4: Add `"starter"` to `activeView` type in state.svelte.ts**

At line 145, change the union to include `| "starter"`.

**Step 5: Add `"starter"` to `setActiveView` parameter type in database.svelte.ts**

At line 98, add `| "starter"` to the `view` parameter union.

**Step 6: Add `"starter"` to `setActiveView` parameter in connection-tabs.svelte.ts**

The `ConnectionTabManager` constructor takes a `setActiveView` callback with the same view type union. Add `| "starter"` to both the `setActiveView` field type (line ~63) and constructor parameter (line ~84). Do the same for `ConnectionTabManager`'s `previousView` type.

**Step 7: Verify**

Run: `npm run check`

---

### Task 2: Rewrite `StarterTabManager` to extend `BaseTabManager`

Replace the old standalone manager with one that uses `BaseTabManager`.

**Files:**
- Rewrite: `src/lib/hooks/database/starter-tabs.svelte.ts`

**Step 1: Rewrite the file**

```ts
import type { StarterTab, StarterTabType } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";

/**
 * Manages starter tabs using the standard BaseTabManager pattern.
 * Starter tabs are shown when no database connection is active,
 * providing quick actions and migration guidance.
 */
export class StarterTabManager extends BaseTabManager<StarterTab> {
  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
  ) {
    super(state, tabOrdering, schedulePersistence);
  }

  protected get accessors(): TabStateAccessors<StarterTab> {
    return {
      getTabs: () => this.state.starterTabsByProject,
      setTabs: (r) => (this.state.starterTabsByProject = r),
      getActiveId: () => this.state.activeStarterTabIdByProject,
      setActiveId: (r) => (this.state.activeStarterTabIdByProject = r),
    };
  }

  /**
   * Add a starter tab by type.
   */
  add(tabType?: StarterTabType): string | null {
    if (!this.state.activeProjectId) return null;

    const config: Record<StarterTabType, { id: string; name: string }> = {
      "getting-started": { id: "getting-started", name: "Getting Started" },
      "migration-tips": { id: "migration-tips", name: "Migration Tips" },
    };

    const type = tabType ?? "getting-started";
    const { id, name } = config[type];

    // Don't add duplicate
    const existing = this.getProjectTabs();
    if (existing.some((t) => t.id === id)) {
      this.setActive(id);
      return id;
    }

    const newTab: StarterTab = {
      id,
      type,
      name,
      closable: true,
    };

    this.appendTab(newTab);
    return newTab.id;
  }

  /**
   * Initialize default starter tabs for a new project.
   * Adds both Getting Started and Migration Tips tabs.
   */
  initializeDefaults(projectId: string): void {
    const existing = this.state.starterTabsByProject[projectId];
    if (existing && existing.length > 0) return;

    // Temporarily set activeProjectId context if needed
    const prevProjectId = this.state.activeProjectId;
    this.state.activeProjectId = projectId;

    this.add("getting-started");
    this.add("migration-tips");

    // Set the first tab as active
    const tabs = this.state.starterTabsByProject[projectId] ?? [];
    if (tabs.length > 0) {
      this.setActive(tabs[0].id);
    }

    this.state.activeProjectId = prevProjectId;
  }

  /**
   * Reset to default starter tabs.
   * Removes all existing starter tabs and re-adds defaults.
   */
  reset(): void {
    if (!this.state.activeProjectId) return;
    const projectId = this.state.activeProjectId;

    // Remove existing starter tabs
    const existing = this.getProjectTabs();
    for (const tab of existing) {
      this.remove(tab.id);
    }

    // Re-add defaults
    this.add("getting-started");
    this.add("migration-tips");

    // Set the first tab as active
    const tabs = this.state.starterTabsByProject[projectId] ?? [];
    if (tabs.length > 0) {
      this.setActive(tabs[0].id);
    }
  }
}
```

**Step 2: Verify**

Run: `npm run check` (will have errors until wiring is updated in next tasks)

---

### Task 3: Add starter tabs to `TabOrderingManager.ordered`

**Files:**
- Modify: `src/lib/hooks/database/tab-ordering.svelte.ts`

**Step 1: Add StarterTab import**

Add `StarterTab` to the imports from `$lib/types` (line 1).

**Step 2: Add `"starter"` to the type union in `orderedForPane` return type (line ~111)**

Add `| "starter"` to the type literal union.

**Step 3: Add `"starter"` to the type union in `ordered` getter return type (line ~163)**

Add `| "starter"` to the type literal union.

**Step 4: Add `StarterTab` to the tab union in both return types**

Add `| StarterTab` after `DashboardTab` in both the `orderedForPane` and `ordered` return type unions.

**Step 5: Add `"starter"` to the type union in `allTabsUnordered` (line ~201)**

Add `| "starter"` to the type literal union and `| StarterTab` to the tab union.

**Step 6: Add starter tabs to the ordered getter loop**

After the dashboard tabs loop (after line ~250), add:
```ts
    const starterTabs = this.state.starterTabs || [];

    for (const t of starterTabs) {
      allTabsUnordered.push({ id: t.id, type: "starter", tab: t });
    }
```

**Step 7: Verify**

Run: `npm run check`

---

### Task 4: Update wiring in `database.svelte.ts` and `ProjectManager`

**Files:**
- Modify: `src/lib/hooks/database.svelte.ts`
- Modify: `src/lib/hooks/database/project-manager.svelte.ts`

**Step 1: Update StarterTabManager construction in database.svelte.ts**

Change line ~211 from:
```ts
this.starterTabs = new StarterTabManager(this.state, scheduleProjectPersistence);
```
To:
```ts
this.starterTabs = new StarterTabManager(this.state, this.tabs, scheduleProjectPersistence);
```

**Step 2: Remove the `setInitializeStarterTabsCallback` call in database.svelte.ts**

Delete lines ~277-279:
```ts
    this.projects.setInitializeStarterTabsCallback((projectId: string) => {
      this.starterTabs.initializeDefaults(projectId);
    });
```

**Step 3: Give ProjectManager direct access to StarterTabManager**

In `database.svelte.ts`, after `this.projects.setSharedRepoManager(this.sharedRepos);` (line ~281), add:
```ts
    this.projects.setStarterTabManager(this.starterTabs);
```

**Step 4: Update ProjectManager to use StarterTabManager directly**

In `src/lib/hooks/database/project-manager.svelte.ts`:

a) Add import at top:
```ts
import type { StarterTabManager } from "./starter-tabs.svelte.js";
```

b) Replace the `initializeStarterTabs` field and methods:
- Change field (line ~27) from:
```ts
  private initializeStarterTabs: ((projectId: string) => void) | null = null;
```
To:
```ts
  private starterTabManager: StarterTabManager | null = null;
```

- Replace `setInitializeStarterTabsCallback` method (lines ~57-60) with:
```ts
  setStarterTabManager(manager: StarterTabManager): void {
    this.starterTabManager = manager;
  }
```

c) In the new project creation section (line ~602), change:
```ts
      this.initializeStarterTabs?.(projectId);
```
To:
```ts
      this.starterTabManager?.initializeDefaults(projectId);
```

d) In the restore section (lines ~697-709), replace the starter tab restoration block:
```ts
    // Restore starter tabs
    if (persistedState.starterTabs && persistedState.starterTabs.length > 0) {
      this.state.starterTabsByProject[projectId] = persistedState.starterTabs.map((t) => ({
        id: t.id,
        type: t.type,
        name: t.name,
        closable: t.closable,
      }));
      this.state.activeStarterTabIdByProject[projectId] = persistedState.activeStarterTabId ?? null;
    } else {
      // Initialize default starter tabs if none persisted
      this.initializeStarterTabs?.(projectId);
    }
```
With:
```ts
    // Restore starter tabs
    if (persistedState.starterTabs && persistedState.starterTabs.length > 0) {
      this.state.starterTabsByProject[projectId] = persistedState.starterTabs.map((t) => ({
        id: t.id,
        type: t.type,
        name: t.name,
        closable: t.closable,
      }));
      this.state.activeStarterTabIdByProject[projectId] = persistedState.activeStarterTabId ?? null;
      // Register restored starter tabs with tab ordering (add to tabOrder if not already present)
      const tabOrder = this.state.tabOrderByProject[projectId] ?? [];
      const starterIds = persistedState.starterTabs.map((t) => t.id);
      const newIds = starterIds.filter((id) => !tabOrder.includes(id));
      if (newIds.length > 0) {
        this.state.tabOrderByProject[projectId] = [...newIds, ...tabOrder];
      }
    } else {
      this.starterTabManager?.initializeDefaults(projectId);
    }
```

**Step 5: Verify**

Run: `npm run check`

---

### Task 5: Update `HeaderTabs` rendering

Remove special-case starter tab rendering, add `"starter"` to the displayTabs loop, and update all type-dispatch functions.

**Files:**
- Modify: `src/lib/components/header-tabs.svelte`

**Step 1: Add `"starter"` to `TabType` and `DndItem` (line ~189)**

Change:
```ts
type TabType = 'query' | 'schema' | 'explain' | 'erd' | 'statistics' | 'workflow' | 'visualize' | 'connection' | 'dashboard';
type DndItem = { id: string; type: TabType; tab: QueryTab | SchemaTab | ExplainTab | ErdTab | StatisticsTab | WorkflowTab | VisualizeTab | import('$lib/types').ConnectionTab | DashboardTab };
```
To:
```ts
type TabType = 'query' | 'schema' | 'explain' | 'erd' | 'statistics' | 'workflow' | 'visualize' | 'connection' | 'dashboard' | 'starter';
type DndItem = { id: string; type: TabType; tab: QueryTab | SchemaTab | ExplainTab | ErdTab | StatisticsTab | WorkflowTab | VisualizeTab | import('$lib/types').ConnectionTab | DashboardTab | import('$lib/types').StarterTab };
```

**Step 2: Add `handleStarterTabClick` function**

After `handleDashboardTabClick` (around line 177), add:
```ts
    const handleStarterTabClick = (tabId: string) => {
        db.starterTabs.setActive(tabId);
        db.ui.setActiveView("starter");
    };
```

**Step 3: Add starter to `isTabActive` (around line 62)**

After the dashboard line, add:
```ts
        if (type === 'starter') return activeTabType === 'starter' && db.state.activeStarterTabId === id;
```

**Step 4: Add starter to `switchToTab` (around line 291)**

After the dashboard case, add:
```ts
        } else if (tab.type === 'starter') {
            handleStarterTabClick(tab.id);
```

**Step 5: Add starter to `closeTabDirect` (around line 314)**

After the dashboard line, add:
```ts
        else if (type === 'starter') db.starterTabs.remove(id);
```

**Step 6: Add starter to `closeTab` (around line 405)**

After the dashboard line, add:
```ts
        else if (type === 'starter') db.starterTabs.remove(id);
```

**Step 7: Remove the special-case starter tabs block**

Remove the entire block that renders starter tabs before the DnD zone (the `{#if db.state.projectConnections.length === 0}...{/if}` block with starter tabs, around lines 486-518). Also remove the outer wrapper div (line 485 `<div class="flex items-end gap-1 w-max h-full">`) and its closing `</div>` (line 973), restoring the DnD div as direct child of the scrollable container.

**Step 8: Add starter tab rendering in the displayTabs loop**

After the dashboard `{:else if type === 'dashboard'}` block (ending around line 968) and before the closing `{/if}`, add:
```svelte
                        {:else if type === 'starter'}
                            {@const starterTab = tab as import('$lib/types').StarterTab}
                            <ContextMenu.Root>
                                <ContextMenu.Trigger>
                                    <div
                                        class={[
                                            "relative group shrink-0 flex items-center gap-2 px-3 h-7 text-xs transition-colors cursor-pointer",
                                            isTabActive(id, 'starter')
                                                ? "bg-muted border-t border-l border-r border-border rounded-t-md -mb-px"
                                                : "hover:bg-muted/50 rounded-t-md -mb-px border-t border-l border-r border-transparent",
                                        ]}
                                        onclick={() => handleStarterTabClick(id)}
                                    >
                                        <RocketIcon class="size-3 text-muted-foreground" />
                                        <span class="pr-4">{starterTab.name}</span>
                                        {#if starterTab.closable}
                                            <Button
                                                size="icon"
                                                variant="ghost"
                                                class="absolute right-0 top-1/2 -translate-y-1/2 size-5 opacity-0 group-hover:opacity-100 transition-opacity [&_svg:not([class*='size-'])]:size-3"
                                                onclick={(e) => {
                                                    e.stopPropagation();
                                                    db.starterTabs.remove(id);
                                                }}
                                            >
                                                <XIcon />
                                            </Button>
                                        {/if}
                                    </div>
                                </ContextMenu.Trigger>
                                <ContextMenu.Portal>
                                    <ContextMenu.Content class="w-40">
                                        <ContextMenu.Item onclick={() => closeTab(id, type)}>Close</ContextMenu.Item>
                                        <ContextMenu.Item onclick={() => closeOtherTabs(id)}>Close Others</ContextMenu.Item>
                                        <ContextMenu.Item onclick={() => closeTabsToRight(id)}>Close Right</ContextMenu.Item>
                                        <ContextMenu.Item onclick={() => closeTabsToLeft(id)}>Close Left</ContextMenu.Item>
                                        <ContextMenu.Separator />
                                        <ContextMenu.Item onclick={closeAllTabs}>Close All</ContextMenu.Item>
                                        {#if paneId}
                                            <ContextMenu.Separator />
                                            <ContextMenu.Item onclick={() => db.panes.splitLeft(paneId, id)}>Split Left</ContextMenu.Item>
                                            <ContextMenu.Item onclick={() => db.panes.splitRight(paneId, id)}>Split Right</ContextMenu.Item>
                                        {/if}
                                    </ContextMenu.Content>
                                </ContextMenu.Portal>
                            </ContextMenu.Root>
```

**Step 9: Verify**

Run: `npm run check`

---

### Task 6: Update `PaneContent` rendering

Remove special-case starter tab rendering, add standard `"starter"` branch.

**Files:**
- Modify: `src/lib/components/pane-content.svelte`

**Step 1: Remove `NoTabsEmptyState` import**

Remove the line:
```ts
import NoTabsEmptyState from "$lib/components/starter-tabs/no-tabs-empty-state.svelte";
```

**Step 2: Add derived for active starter tab**

After the `activeStatisticsTab` derived (around line 48), add:
```ts
    const activeStarterTab = $derived(
        paneViewType === 'starter' && pane.activeTabId
            ? db.state.starterTabs.find(t => t.id === pane.activeTabId) ?? null
            : null
    );
```

**Step 3: Replace the special-case empty-pane-no-connections block**

Replace lines ~78-89:
```svelte
    {#if pane.tabIds.length === 0 && db.state.projectConnections.length === 0}
        <div class="flex-1 min-h-0 flex flex-col">
            {#if db.state.activeStarterTab}
                <StarterTabContent tab={db.state.activeStarterTab} />
            {:else}
                <NoTabsEmptyState />
            {/if}
        </div>
    {:else if pane.tabIds.length === 0}
        <div class="flex-1 min-h-0 flex flex-col">
            <GettingStartedContent />
        </div>
    {:else if paneViewType === "connection" && activeConnectionTab}
```

With:
```svelte
    {#if pane.tabIds.length === 0}
        <div class="flex-1 min-h-0 flex flex-col">
            <GettingStartedContent />
        </div>
    {:else if paneViewType === "starter" && activeStarterTab}
        <div class="flex-1 min-h-0 flex flex-col">
            <StarterTabContent tab={activeStarterTab} />
        </div>
    {:else if paneViewType === "connection" && activeConnectionTab}
```

**Step 4: Verify**

Run: `npm run check`

---

### Task 7: Delete unused files and clean up

**Files:**
- Delete: `src/lib/components/starter-tabs/no-tabs-empty-state.svelte`

**Step 1: Delete the file**

```bash
rm src/lib/components/starter-tabs/no-tabs-empty-state.svelte
```

**Step 2: Verify**

Run: `npm run check`

Ensure zero errors. The pre-existing warning about `state_referenced_locally` in `query-editor.svelte` is expected and unrelated.
