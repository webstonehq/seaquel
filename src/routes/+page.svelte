<script lang="ts">
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

    const db = useDatabase();

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

    // Get all tabs in order for keyboard navigation
    const allTabs = $derived([
        ...db.queryTabs.map(t => ({ id: t.id, type: 'query' as const })),
        ...db.schemaTabs.map(t => ({ id: t.id, type: 'schema' as const })),
        ...db.explainTabs.map(t => ({ id: t.id, type: 'explain' as const }))
    ]);

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

    const handleGlobalKeydown = (e: KeyboardEvent) => {
        // Ignore if typing in an input
        if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) {
            return;
        }

        const isMod = e.metaKey || e.ctrlKey;

        // Cmd+T: New query tab
        if (isMod && e.key === 't') {
            e.preventDefault();
            db.addQueryTab();
            return;
        }

        // Cmd+W: Close current tab
        if (isMod && e.key === 'w') {
            e.preventDefault();
            closeCurrentTab();
            return;
        }

        // Cmd+1-9: Switch to tab by index
        if (isMod && e.key >= '1' && e.key <= '9') {
            e.preventDefault();
            const index = parseInt(e.key) - 1;
            switchToTab(index);
            return;
        }

        // Cmd+Shift+] or Cmd+Shift+[: Next/Previous tab
        if (isMod && e.shiftKey) {
            const idx = currentTabIndex();
            if (e.key === ']' || e.key === '}') {
                e.preventDefault();
                switchToTab((idx + 1) % allTabs.length);
                return;
            }
            if (e.key === '[' || e.key === '{') {
                e.preventDefault();
                switchToTab((idx - 1 + allTabs.length) % allTabs.length);
                return;
            }
        }
    };
</script>

<svelte:window onkeydown={handleGlobalKeydown} />

<Toaster position="bottom-right" richColors />

<SidebarLeft />
<SidebarInset class="flex flex-col h-full overflow-hidden">
    {#if db.activeConnectionId}
        <!-- Unified Tab Bar -->
        <div class="flex items-center gap-2 p-2 border-b bg-muted/30 overflow-hidden">
            <ScrollArea orientation="horizontal" class="flex-1">
                <div class="flex items-center gap-1 pb-1">
                    <!-- Query Tabs -->
                    {#each db.queryTabs as tab (tab.id)}
                        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                        <div
                            class={[
                                "relative group shrink-0 flex items-center gap-2 px-3 h-7 text-xs rounded-md transition-colors",
                                activeTabType === "query" && db.activeQueryTabId === tab.id
                                    ? "bg-background shadow-sm"
                                    : "hover:bg-muted",
                            ]}
                            onclick={() => handleQueryTabClick(tab.id)}
                        >
                            <FileCodeIcon class="size-3 text-muted-foreground" />
                            {#if editingTabId === tab.id}
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
                                        startEditing(tab.id, tab.name);
                                    }}
                                >
                                    {tab.name}
                                </span>
                            {/if}
                            <Button
                                size="icon"
                                variant="ghost"
                                class="absolute right-0 top-1/2 -translate-y-1/2 size-5 opacity-0 group-hover:opacity-100 transition-opacity [&_svg:not([class*='size-'])]:size-3"
                                onclick={(e) => {
                                    e.stopPropagation();
                                    db.removeQueryTab(tab.id);
                                }}
                            >
                                <XIcon />
                            </Button>
                        </div>
                    {/each}

                    <!-- Schema Tabs -->
                    {#each db.schemaTabs as tab (tab.id)}
                        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                        <div
                            class={[
                                "relative group shrink-0 flex items-center gap-2 px-3 h-7 text-xs rounded-md transition-colors",
                                activeTabType === "schema" && db.activeSchemaTabId === tab.id
                                    ? "bg-background shadow-sm"
                                    : "hover:bg-muted",
                            ]}
                            onclick={() => handleSchemaTabClick(tab.id)}
                        >
                            <TableIcon class="size-3 text-muted-foreground" />
                            <span class="pr-4">{tab.table.schema}.{tab.table.name}</span>
                            <Button
                                size="icon"
                                variant="ghost"
                                class="absolute right-0 top-1/2 -translate-y-1/2 size-5 opacity-0 group-hover:opacity-100 transition-opacity [&_svg:not([class*='size-'])]:size-3"
                                onclick={(e) => {
                                    e.stopPropagation();
                                    db.removeSchemaTab(tab.id);
                                }}
                            >
                                <XIcon />
                            </Button>
                        </div>
                    {/each}

                    <!-- Explain Tabs -->
                    {#each db.explainTabs as tab (tab.id)}
                        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                        <div
                            class={[
                                "relative group shrink-0 flex items-center gap-2 px-3 h-7 text-xs rounded-md transition-colors",
                                activeTabType === "explain" && db.activeExplainTabId === tab.id
                                    ? "bg-background shadow-sm"
                                    : "hover:bg-muted",
                            ]}
                            onclick={() => handleExplainTabClick(tab.id)}
                        >
                            <ActivityIcon class="size-3 text-muted-foreground" />
                            <span class="pr-4">{tab.name}</span>
                            <Button
                                size="icon"
                                variant="ghost"
                                class="absolute right-0 top-1/2 -translate-y-1/2 size-5 opacity-0 group-hover:opacity-100 transition-opacity [&_svg:not([class*='size-'])]:size-3"
                                onclick={(e) => {
                                    e.stopPropagation();
                                    db.removeExplainTab(tab.id);
                                }}
                            >
                                <XIcon />
                            </Button>
                        </div>
                    {/each}
                </div>
            </ScrollArea>

            <!-- Add new query tab button -->
            <Button
                size="icon"
                variant="ghost"
                class="size-7 shrink-0 [&_svg:not([class*='size-'])]:size-4"
                onclick={() => db.addQueryTab()}
            >
                <PlusIcon />
            </Button>
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
