<script lang="ts">
    import { Toaster } from "$lib/components/ui/sonner";
    import { SidebarInset } from "$lib/components/ui/sidebar";
    import SidebarLeft from "$lib/components/sidebar-left.svelte";
    import QueryEditor from "$lib/components/query-editor.svelte";
    import TableViewer from "$lib/components/table-viewer.svelte";
    import AIAssistant from "$lib/components/ai-assistant.svelte";
    import { ScrollArea } from "$lib/components/ui/scroll-area";
    import { Button } from "$lib/components/ui/button";
    import { Input } from "$lib/components/ui/input";
    import { PlusIcon, XIcon, TableIcon, FileCodeIcon } from "@lucide/svelte";
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
</script>

<Toaster position="bottom-right" richColors />

<SidebarLeft />
<SidebarInset class="flex flex-col">
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
            {/if}
        </div>
    {/if}
</SidebarInset>

<AIAssistant />
