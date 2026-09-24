# Unify Pane Rendering Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate the duplicate no-connections rendering path so all tab states use `PaneContainer`, fixing the bug where connection tabs don't render in the no-connections state and enabling split panes everywhere.

**Architecture:** Remove the `projectConnections.length === 0` branch from `+page.svelte` so it always renders `PaneContainer`. Update `PaneContent` to show starter tab content when the pane has no regular tabs and no connections. Integrate starter tabs into the main `HeaderTabs` tab bar so they render in pane mode. Remove the separate simplified starter-tabs branch from `HeaderTabs`.

**Tech Stack:** SvelteKit 5, Svelte 5 runes, TypeScript

---

### Task 1: Update `PaneContent` to render starter tabs

When a pane has no regular tabs (`pane.tabIds.length === 0`) and no connections, `PaneContent` currently shows `GettingStartedContent`. It should instead check for an active starter tab and render `StarterTabContent`, falling back to `NoTabsEmptyState` when all starter tabs are closed.

**Files:**
- Modify: `src/lib/components/pane-content.svelte:9,76-79`

**Step 1: Add starter tab imports**

Add these imports to the script block:

```svelte
import StarterTabContent from "$lib/components/starter-tabs/starter-tab-content.svelte";
import NoTabsEmptyState from "$lib/components/starter-tabs/no-tabs-empty-state.svelte";
```

**Step 2: Replace the empty-pane content block**

Replace the block at lines 76-79:

```svelte
{#if pane.tabIds.length === 0}
    <div class="flex-1 min-h-0 flex flex-col">
        <GettingStartedContent />
    </div>
```

With:

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
```

**Step 3: Verify**

Run: `npm run check`

**Step 4: Commit**

```
git add src/lib/components/pane-content.svelte
git commit -m "Show starter tabs in PaneContent when no connections"
```

---

### Task 2: Integrate starter tabs into the main `HeaderTabs` tab bar

The main tab bar branch (line 481, `projectConnections.length > 0`) needs to also render when there are no connections. Starter tabs should appear before regular tabs when `projectConnections.length === 0`. The separate simplified starter-tabs branch (lines 966-1031) should be removed.

**Files:**
- Modify: `src/lib/components/header-tabs.svelte:481,966-1031`

**Step 1: Widen the main branch condition**

Change line 481 from:

```svelte
{:else if db.state.activeProjectId && db.state.projectConnections.length > 0}
```

To:

```svelte
{:else if db.state.activeProjectId}
```

**Step 2: Add starter tabs before `displayTabs` in the tab bar**

Inside the DnD container (`{#each displayTabs ...}`), add a block **before** the `{#each displayTabs}` loop (after the opening `<div>` with `use:dndzone` at line 495) that renders starter tabs when there are no connections:

```svelte
{#if db.state.projectConnections.length === 0}
    {#each db.state.starterTabs as starterTab (starterTab.id)}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div
            class={[
                "relative group shrink-0 flex items-center gap-2 px-3 h-7 text-xs transition-colors cursor-pointer",
                db.state.activeView !== "connection" && db.state.activeStarterTabId === starterTab.id
                    ? "bg-muted border-t border-l border-r border-border rounded-t-md -mb-px"
                    : "hover:bg-muted/50 rounded-t-md -mb-px border-t border-l border-r border-transparent",
            ]}
            onclick={() => {
                db.starterTabs.setActive(starterTab.id);
                db.ui.setActiveView("query");
            }}
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
                        db.starterTabs.remove(starterTab.id);
                    }}
                >
                    <XIcon />
                </Button>
            {/if}
        </div>
    {/each}
{/if}
```

Note: These starter tabs are placed **outside** the DnD zone — they should be inserted before the DnD `<div>`, inside the scrollable container but not inside the `use:dndzone` div.

**Step 3: Remove the old starter-tabs else branch**

Delete the entire `{:else}` block at lines 966-1031 (from `{:else}` through the closing `</div>` of the no-connections branch, up to `{/if}` that closes the `isManagePage` check).

**Step 4: Verify**

Run: `npm run check`

**Step 5: Commit**

```
git add src/lib/components/header-tabs.svelte
git commit -m "Integrate starter tabs into main header tab bar"
```

---

### Task 3: Simplify `+page.svelte` to always use `PaneContainer`

Remove the `projectConnections.length === 0` branch and always render `PaneContainer`.

**Files:**
- Modify: `src/routes/manage/+page.svelte:7-9,56-82`

**Step 1: Remove unused imports**

Remove these imports (lines 7-9) that are no longer needed:

```typescript
import StarterTabContent from "$lib/components/starter-tabs/starter-tab-content.svelte";
import GettingStartedContent from "$lib/components/starter-tabs/getting-started-content.svelte";
import ConnectionTabView from "$lib/components/connection-tab-view.svelte";
```

Also remove the `HeaderTabs` import (line 14) since it's now rendered inside `PaneContent`.

**Step 2: Replace the template branching**

Replace lines 56-83 (the entire `SidebarInset` content):

```svelte
<SidebarInset class="flex flex-col h-full overflow-hidden min-w-0">
    {#if db.state.connectionsLoading || db.state.projectsLoading}
        <!-- Loading state - show nothing to prevent flash -->
    {:else}
        <div class="flex-1 min-h-0 flex flex-col">
            <PaneContainer />
        </div>
    {/if}
</SidebarInset>
```

**Step 3: Verify**

Run: `npm run check`

**Step 4: Commit**

```
git add src/routes/manage/+page.svelte
git commit -m "Always use PaneContainer, remove duplicate rendering path"
```

---

### Task 4: Manual verification

**Step 1: Test the no-connections state**

1. Start dev server: `npm run tauri dev`
2. Create a new project
3. Verify "Getting Started" tab renders with starter content
4. Click "Add Connection" — verify connection tab appears and renders correctly
5. Close the connection tab — verify it returns to starter content
6. Close the "Getting Started" tab — verify the empty state shows
7. Click "Show Getting Started" — verify tabs reappear

**Step 2: Test the has-connections state**

1. Connect to a database
2. Verify all tab types still work (query, schema, explain, etc.)
3. Verify split panes still work
4. Verify DnD tab reordering still works

**Step 3: Commit final state**

If any fixes were needed, commit them.
