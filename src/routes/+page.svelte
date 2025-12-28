<script lang="ts">
    import { onMount, onDestroy } from "svelte";
    import { Toaster } from "$lib/components/ui/sonner";
    import { SidebarInset } from "$lib/components/ui/sidebar";
    import SidebarLeft from "$lib/components/sidebar-left.svelte";
    import QueryEditor from "$lib/components/query-editor.svelte";
    import TableViewer from "$lib/components/table-viewer.svelte";
    import ExplainViewer from "$lib/components/explain-viewer.svelte";
    import AIAssistant from "$lib/components/ai-assistant.svelte";
    import { ScrollArea } from "$lib/components/ui/scroll-area";
    import { Button } from "$lib/components/ui/button";
    import { Input } from "$lib/components/ui/input";
    import { PlusIcon, XIcon, TableIcon, FileCodeIcon, ActivityIcon } from "@lucide/svelte";
    import { useDatabase } from "$lib/hooks/database.svelte.js";
    import { useShortcuts, findShortcut } from "$lib/shortcuts/index.js";
    import ShortcutKeys from "$lib/components/shortcut-keys.svelte";
    import * as Tooltip from "$lib/components/ui/tooltip/index.js";

    const db = useDatabase();
    const shortcuts = useShortcuts();

    // Track which type of tab is active: 'query' or 'schema'
    let activeTabType = $derived(db.activeView);

    // For editing query tab names
    let editingTabId = $state<string | null>(null);
    let editingTabName = $state("");

    const startEditing = (tabId: string, currentName: string) => {
        editingTabId = tabId;
        editingTabName = currentName;
    };

    const finishEditing = () => {
        if (editingTabId && editingTabName.trim()) {
            db.renameQueryTab(editingTabId, editingTabName.trim());
        }
        editingTabId = null;
        editingTabName = "";
    };

    const handleKeydown = (e: KeyboardEvent) => {
        if (e.key === "Enter") {
            finishEditing();
        } else if (e.key === "Escape") {
            editingTabId = null;
            editingTabName = "";
        }
    };

    const handleQueryTabClick = (tabId: string) => {
        db.setActiveQueryTab(tabId);
        db.setActiveView("query");
    };

    const handleSchemaTabClick = (tabId: string) => {
        db.setActiveSchemaTab(tabId);
        db.setActiveView("schema");
    };

    const handleExplainTabClick = (tabId: string) => {
        db.setActiveExplainTab(tabId);
        db.setActiveView("explain");
    };

    // Extract timestamp from tab ID for ordering
    const getTabTimestamp = (id: string): number => {
        const match = id.match(/\d+$/);
        return match ? parseInt(match[0], 10) : 0;
    };

    // Get all tabs in creation order for unified tab bar
    const allTabs = $derived([
        ...db.queryTabs.map(t => ({ id: t.id, type: 'query' as const, tab: t })),
        ...db.schemaTabs.map(t => ({ id: t.id, type: 'schema' as const, tab: t })),
        ...db.explainTabs.map(t => ({ id: t.id, type: 'explain' as const, tab: t }))
    ].sort((a, b) => getTabTimestamp(a.id) - getTabTimestamp(b.id)));

    const currentTabIndex = $derived(() => {
        if (db.activeView === 'query' && db.activeQueryTabId) {
            return allTabs.findIndex(t => t.type === 'query' && t.id === db.activeQueryTabId);
        }
        if (db.activeView === 'schema' && db.activeSchemaTabId) {
            return allTabs.findIndex(t => t.type === 'schema' && t.id === db.activeSchemaTabId);
        }
        if (db.activeView === 'explain' && db.activeExplainTabId) {
            return allTabs.findIndex(t => t.type === 'explain' && t.id === db.activeExplainTabId);
        }
        return -1;
    });

    const switchToTab = (index: number) => {
        if (index < 0 || index >= allTabs.length) return;
        const tab = allTabs[index];
        if (tab.type === 'query') {
            handleQueryTabClick(tab.id);
        } else if (tab.type === 'schema') {
            handleSchemaTabClick(tab.id);
        } else if (tab.type === 'explain') {
            handleExplainTabClick(tab.id);
        }
    };

    const closeCurrentTab = () => {
        if (db.activeView === 'query' && db.activeQueryTabId) {
            db.removeQueryTab(db.activeQueryTabId);
        } else if (db.activeView === 'schema' && db.activeSchemaTabId) {
            db.removeSchemaTab(db.activeSchemaTabId);
        } else if (db.activeView === 'explain' && db.activeExplainTabId) {
            db.removeExplainTab(db.activeExplainTabId);
        }
    };

    // Register keyboard shortcuts
    onMount(() => {
        shortcuts.registerHandler('newTab', () => db.addQueryTab());
        shortcuts.registerHandler('closeTab', closeCurrentTab);
        shortcuts.registerHandler('nextTab', () => {
            const idx = currentTabIndex();
            switchToTab((idx + 1) % allTabs.length);
        });
        shortcuts.registerHandler('previousTab', () => {
            const idx = currentTabIndex();
            switchToTab((idx - 1 + allTabs.length) % allTabs.length);
        });

        // Register tab 1-9 handlers
        for (let i = 1; i <= 9; i++) {
            shortcuts.registerHandler(`goToTab${i}`, () => switchToTab(i - 1));
        }
    });

    onDestroy(() => {
        shortcuts.unregisterHandler('newTab');
        shortcuts.unregisterHandler('closeTab');
        shortcuts.unregisterHandler('nextTab');
        shortcuts.unregisterHandler('previousTab');
        for (let i = 1; i <= 9; i++) {
            shortcuts.unregisterHandler(`goToTab${i}`);
        }
    });
</script>

<Toaster position="bottom-right" richColors />

<SidebarLeft />
<SidebarInset class="flex flex-col h-full overflow-hidden">
    {#if db.activeConnectionId}
        <!-- Unified Tab Bar -->
        <div class="flex items-center gap-2 p-2 border-b bg-muted/30 overflow-hidden">
            <ScrollArea orientation="horizontal" class="flex-1">
                <div class="flex items-center gap-1 pb-1">
                    <!-- All tabs in creation order -->
                    {#each allTabs as { id, type, tab } (id)}
                        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                        {#if type === 'query'}
                            {@const queryTab = tab as import('$lib/types').QueryTab}
                            <div
                                class={[
                                    "relative group shrink-0 flex items-center gap-2 px-3 h-7 text-xs rounded-md transition-colors",
                                    activeTabType === "query" && db.activeQueryTabId === id
                                        ? "bg-background shadow-sm"
                                        : "hover:bg-muted",
                                ]}
                                onclick={() => handleQueryTabClick(id)}
                            >
                                <FileCodeIcon class="size-3 text-muted-foreground" />
                                {#if editingTabId === id}
                                    <Input
                                        bind:value={editingTabName}
                                        class="h-5 px-1 text-xs w-24"
                                        onblur={finishEditing}
                                        onkeydown={handleKeydown}
                                        onclick={(e) => e.stopPropagation()}
                                    />
                                {:else}
                                    <span
                                        class="pr-4"
                                        ondblclick={(e) => {
                                            e.stopPropagation();
                                            startEditing(id, queryTab.name);
                                        }}
                                    >
                                        {queryTab.name}
                                    </span>
                                {/if}
                                <Button
                                    size="icon"
                                    variant="ghost"
                                    class="absolute right-0 top-1/2 -translate-y-1/2 size-5 opacity-0 group-hover:opacity-100 transition-opacity [&_svg:not([class*='size-'])]:size-3"
                                    onclick={(e) => {
                                        e.stopPropagation();
                                        db.removeQueryTab(id);
                                    }}
                                >
                                    <XIcon />
                                </Button>
                            </div>
                        {:else if type === 'schema'}
                            {@const schemaTab = tab as import('$lib/types').SchemaTab}
                            <div
                                class={[
                                    "relative group shrink-0 flex items-center gap-2 px-3 h-7 text-xs rounded-md transition-colors",
                                    activeTabType === "schema" && db.activeSchemaTabId === id
                                        ? "bg-background shadow-sm"
                                        : "hover:bg-muted",
                                ]}
                                onclick={() => handleSchemaTabClick(id)}
                            >
                                <TableIcon class="size-3 text-muted-foreground" />
                                <span class="pr-4">{schemaTab.table.schema}.{schemaTab.table.name}</span>
                                <Button
                                    size="icon"
                                    variant="ghost"
                                    class="absolute right-0 top-1/2 -translate-y-1/2 size-5 opacity-0 group-hover:opacity-100 transition-opacity [&_svg:not([class*='size-'])]:size-3"
                                    onclick={(e) => {
                                        e.stopPropagation();
                                        db.removeSchemaTab(id);
                                    }}
                                >
                                    <XIcon />
                                </Button>
                            </div>
                        {:else if type === 'explain'}
                            {@const explainTab = tab as import('$lib/types').ExplainTab}
                            <div
                                class={[
                                    "relative group shrink-0 flex items-center gap-2 px-3 h-7 text-xs rounded-md transition-colors",
                                    activeTabType === "explain" && db.activeExplainTabId === id
                                        ? "bg-background shadow-sm"
                                        : "hover:bg-muted",
                                ]}
                                onclick={() => handleExplainTabClick(id)}
                            >
                                <ActivityIcon class="size-3 text-muted-foreground" />
                                <span class="pr-4">{explainTab.name}</span>
                                <Button
                                    size="icon"
                                    variant="ghost"
                                    class="absolute right-0 top-1/2 -translate-y-1/2 size-5 opacity-0 group-hover:opacity-100 transition-opacity [&_svg:not([class*='size-'])]:size-3"
                                    onclick={(e) => {
                                        e.stopPropagation();
                                        db.removeExplainTab(id);
                                    }}
                                >
                                    <XIcon />
                                </Button>
                            </div>
                        {/if}
                    {/each}
                </div>
            </ScrollArea>

            <!-- Add new query tab button -->
            <Tooltip.Root>
                <Tooltip.Trigger>
                    <Button
                        size="icon"
                        variant="ghost"
                        class="size-7 shrink-0 [&_svg:not([class*='size-'])]:size-4"
                        onclick={() => db.addQueryTab()}
                    >
                        <PlusIcon />
                    </Button>
                </Tooltip.Trigger>
                <Tooltip.Content side="bottom">
                    <span class="flex items-center gap-2">
                        New Tab
                        {#if findShortcut('newTab')}
                            <ShortcutKeys keys={findShortcut('newTab')!.keys} />
                        {/if}
                    </span>
                </Tooltip.Content>
            </Tooltip.Root>
        </div>

        <!-- Content Area -->
        <div class="flex-1 min-h-0 flex flex-col">
            {#if activeTabType === "query"}
                <QueryEditor />
            {:else if activeTabType === "schema"}
                <ScrollArea orientation="both" class="h-full">
                    <TableViewer />
                </ScrollArea>
            {:else if activeTabType === "explain"}
                <ExplainViewer />
            {/if}
        </div>
    {/if}
</SidebarInset>

<AIAssistant />
