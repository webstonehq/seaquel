<script lang="ts">
    import { onMount, onDestroy } from "svelte";
    import { dndzone } from "svelte-dnd-action";
    import { page } from "$app/state";
    import { resolve } from "$app/paths";
    import { isTauri } from "$lib/utils/environment";
    import { Button } from "$lib/components/ui/button";
    import { Input } from "$lib/components/ui/input";
    import { PlusIcon, XIcon, TableIcon, FileCodeIcon, ActivityIcon, NetworkIcon, BarChart3Icon, WorkflowIcon, GitBranchIcon, CableIcon, LayoutDashboardIcon, SettingsIcon, ChevronDownIcon, PlusSquareIcon, DatabaseIcon, PuzzleIcon } from "@lucide/svelte";
    import { RocketIcon } from "@lucide/svelte";
    import { useDatabase } from "$lib/hooks/database.svelte.js";
    import { useShortcuts, findShortcut } from "$lib/shortcuts/index.js";
    import ShortcutKeys from "$lib/components/shortcut-keys.svelte";
    import * as Tooltip from "$lib/components/ui/tooltip/index.js";
    import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
    import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
    import UnsavedChangesDialog from "$lib/components/unsaved-changes-dialog.svelte";
    import BatchUnsavedDialog from "$lib/components/batch-unsaved-dialog.svelte";
    import SaveQueryDialog from "$lib/components/save-query-dialog.svelte";
    import type { QueryTab, SchemaTab, ExplainTab, ErdTab, StatisticsTab, WorkflowTab, VisualizeTab, ConnectionTab, DashboardTab, StarterTab, SettingsTab, CreateTableTab, DataTab, ExtensionsDuckdbTab, ActiveViewType } from "$lib/types";
    import type { Pane } from "$lib/types";
    import type { Component } from "svelte";
    import { usePaneDragState } from "$lib/components/pane-drag-context.svelte.js";

    let { paneId = undefined, pane = undefined }: { paneId?: string; pane?: Pane } = $props();

    const db = useDatabase();
    const shortcuts = useShortcuts();
    const paneDragState = usePaneDragState();


    const isManagePage = $derived(
        page.url.pathname === resolve("/(app)/manage") || page.url.pathname === resolve("/")
    );

    let activeTabType = $derived(db.state.activeView);

    const isTabActive = (id: string, type: string): boolean => {
        if (pane) return pane.activeTabId === id;
        return activeTabType === type && db.state.getActiveTabId(type as ActiveViewType) === id;
    };

    let editingTabId = $state<string | null>(null);
    let editingTabName = $state("");
    let editingInitialWidth = $state(0);
    let editingFont = $state("");

    const autofocus = () => (el: HTMLInputElement) => {
        el.focus();
        el.select();
    };

    const measureTextWidth = (text: string, font: string) => {
        const canvas = document.createElement("canvas");
        const ctx = canvas.getContext("2d")!;
        ctx.font = font;
        return ctx.measureText(text).width;
    };

    const editingInputWidth = $derived(
        Math.max(editingInitialWidth, measureTextWidth(editingTabName, editingFont) + 22)
    );

    const startEditing = (tabId: string, currentName: string, spanEl: HTMLElement) => {
        editingInitialWidth = spanEl.offsetWidth;
        editingFont = getComputedStyle(spanEl).font;
        editingTabId = tabId;
        editingTabName = currentName;
    };

    const finishEditing = () => {
        if (editingTabId && editingTabName.trim()) {
            // Check if it's a dashboard tab or query tab
            const dashboardTab = db.state.dashboardTabs.find(t => t.id === editingTabId);
            if (dashboardTab) {
                db.dashboardTabs.rename(editingTabId, editingTabName.trim());
                const dashboard = db.state.projectDashboards.find((d: { id: string }) => d.id === dashboardTab.dashboardId);
                if (dashboard) {
                    db.dashboards.renameDashboard(dashboard.id, editingTabName.trim());
                }
            } else {
                db.queryTabs.rename(editingTabId, editingTabName.trim());
            }
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

    type TabType = ActiveViewType;

    type TabConfig = {
        icon: Component<{ class?: string }>;
        label: (tab: any) => string;
        renameable?: boolean;
        unsavedIndicator?: (id: string) => string;
        closable?: (tab: any) => boolean;
        switchesConnection?: boolean;
        extraClass?: string;
    };

    const tabConfigs: Record<TabType, TabConfig> = {
        query: {
            icon: FileCodeIcon,
            label: (t: QueryTab) => t.name,
            renameable: true,
            unsavedIndicator: (id) => db.queryTabs.hasUnsavedChanges(id) ? " *" : "",
        },
        schema: {
            icon: TableIcon,
            label: (t: SchemaTab) => `${t.table.schema}.${t.table.name}`,
        },
        explain: { icon: ActivityIcon, label: (t: ExplainTab) => t.name },
        erd: { icon: NetworkIcon, label: (t: ErdTab) => t.name, switchesConnection: true },
        statistics: { icon: BarChart3Icon, label: (t: StatisticsTab) => t.name, switchesConnection: true },
        workflow: { icon: WorkflowIcon, label: (t: WorkflowTab) => t.name, switchesConnection: true },
        visualize: { icon: GitBranchIcon, label: (t: VisualizeTab) => t.name },
        connection: { icon: CableIcon, label: (t: ConnectionTab) => t.name },
        dashboard: {
            icon: LayoutDashboardIcon,
            label: (t: DashboardTab) => t.name,
            renameable: true,
        },
        starter: {
            icon: RocketIcon,
            label: (t: StarterTab) => t.name,
            closable: (t: StarterTab) => t.closable,
            extraClass: "cursor-pointer",
        },
        settings: {
            icon: SettingsIcon,
            label: (t: SettingsTab) => t.name,
            extraClass: "cursor-pointer",
        },
        createTable: {
            icon: PlusSquareIcon,
            label: (t: CreateTableTab) => t.name || "New Table",
        },
        data: {
            icon: DatabaseIcon,
            label: (t: DataTab) => `${t.schemaName}.${t.tableName}`,
        },
        extensionsDuckdb: {
            icon: PuzzleIcon,
            label: (t: ExtensionsDuckdbTab) => t.name,
            switchesConnection: true,
        },
    };

    const tabManagers: Record<TabType, { setActive: (id: string) => void; remove: (id: string) => void }> = {
        query: db.queryTabs,
        schema: db.schemaTabs,
        explain: db.explainTabs,
        erd: db.erdTabs,
        statistics: db.statisticsTabs,
        workflow: db.workflowTabs,
        visualize: db.visualizeTabs,
        connection: db.connectionTabs,
        dashboard: db.dashboardTabs,
        starter: db.starterTabs,
        settings: db.settingsTabs,
        createTable: db.createTableTabs,
        data: db.dataTabs,
        extensionsDuckdb: db.extensionsDuckdbTabs,
    };

    const handleTabClick = (tabId: string, type: TabType, tab: any) => {
        if (tabConfigs[type].switchesConnection && tab.connectionId) {
            db.connections.setActive(tab.connectionId);
        }
        tabManagers[type].setActive(tabId);
        db.ui.setActiveView(type);
    };

    const allTabs = $derived.by(() => {
        if (paneId) {
            const ordered = db.tabs.orderedForPane(paneId);
            return Array.isArray(ordered) ? [...ordered] : [];
        }
        const ordered = db.tabs.ordered;
        return Array.isArray(ordered) ? [...ordered] : [];
    });

    type DndItem = { id: string; type: TabType; tab: QueryTab | SchemaTab | ExplainTab | ErdTab | StatisticsTab | WorkflowTab | VisualizeTab | ConnectionTab | DashboardTab | StarterTab | SettingsTab | CreateTableTab | DataTab | ExtensionsDuckdbTab };

    let draggedItems = $state<DndItem[]>([]);
    let isDragging = $state(false);

    const displayTabs = $derived(isDragging ? draggedItems : allTabs);

    function handleDndConsider(e: CustomEvent<{ items: DndItem[]; info: { id: string } }>) {
        isDragging = true;
        draggedItems = e.detail.items;

        if (paneDragState && paneId && e.detail.info?.id) {
            if (!paneDragState.isDragging) {
                paneDragState.startDrag(e.detail.info.id, paneId);
            }
        }
    }

    function handleDndFinalize(e: CustomEvent<{ items: DndItem[] }>) {
        isDragging = false;
        draggedItems = [];

        const dragResult = paneDragState?.endDrag();
        if (dragResult) {
            const { tabId, sourcePaneId, splitTarget, moveTarget } = dragResult;
            if (splitTarget) {
                if (splitTarget.direction === "left") {
                    db.panes.splitLeft(splitTarget.paneId, tabId, sourcePaneId);
                } else {
                    db.panes.splitRight(splitTarget.paneId, tabId, sourcePaneId);
                }
            } else if (moveTarget) {
                db.panes.moveTab(tabId, sourcePaneId, moveTarget.paneId, moveTarget.index);
            }
            return;
        }

        const newOrder = e.detail.items.map(item => item.id);
        if (paneId) {
            db.panes.reorderPane(paneId, newOrder);
        } else {
            db.tabs.reorder(newOrder);
        }
    }

    const currentTabIndex = $derived(() => {
        const activeId = db.state.getActiveTabId(db.state.activeView);
        if (!activeId) return -1;
        return allTabs.findIndex(t => t.type === db.state.activeView && t.id === activeId);
    });

    const switchToTab = (index: number) => {
        if (index < 0 || index >= allTabs.length) return;
        const { id, type, tab } = allTabs[index];
        handleTabClick(id, type, tab);
    };

    let pendingCloseTabId = $state<string | null>(null);
    let showUnsavedDialog = $state(false);
    let showSaveDialogForClose = $state(false);

    let pendingBatchCloseTabs = $state<{id: string, type: TabType}[]>([]);
    let unsavedTabsInBatch = $state<string[]>([]);
    let showBatchUnsavedDialog = $state(false);

    const closeTabDirect = (id: string, type: TabType) => {
        tabManagers[type].remove(id);
    };

    const tryCloseQueryTab = (tabId: string) => {
        if (db.queryTabs.hasUnsavedChanges(tabId)) {
            pendingCloseTabId = tabId;
            showUnsavedDialog = true;
        } else {
            db.queryTabs.remove(tabId);
        }
    };

    const handleUnsavedDiscard = () => {
        if (pendingCloseTabId) {
            db.queryTabs.remove(pendingCloseTabId);
            pendingCloseTabId = null;
        }
    };

    const handleUnsavedSave = () => {
        showSaveDialogForClose = true;
    };

    const handleUnsavedCancel = () => {
        pendingCloseTabId = null;
    };

    const handleSaveComplete = () => {
        if (pendingCloseTabId) {
            db.queryTabs.remove(pendingCloseTabId);
            pendingCloseTabId = null;
        }
        showSaveDialogForClose = false;
    };

    const tryBatchClose = (tabsToClose: {id: string, type: TabType}[]) => {
        const unsaved = tabsToClose
            .filter(t => t.type === 'query' && db.queryTabs.hasUnsavedChanges(t.id))
            .map(t => t.id);

        if (unsaved.length > 0) {
            pendingBatchCloseTabs = tabsToClose;
            unsavedTabsInBatch = unsaved;
            showBatchUnsavedDialog = true;
        } else {
            tabsToClose.forEach(t => closeTabDirect(t.id, t.type));
        }
    };

    const handleBatchDiscard = () => {
        pendingBatchCloseTabs.forEach(t => closeTabDirect(t.id, t.type));
        pendingBatchCloseTabs = [];
        unsavedTabsInBatch = [];
    };

    const handleBatchCancel = () => {
        pendingBatchCloseTabs = [];
        unsavedTabsInBatch = [];
    };

    const closeCurrentTab = () => {
        const activeId = db.state.getActiveTabId(db.state.activeView);
        if (!activeId) return;
        closeTab(activeId, db.state.activeView);
    };

    const closeTab = (id: string, type: TabType) => {
        if (type === 'query') tryCloseQueryTab(id);
        else closeTabDirect(id, type);
    };

    const closeOtherTabs = (id: string) => {
        const tabsToClose = allTabs.filter(t => t.id !== id);
        tryBatchClose(tabsToClose);
    };

    const closeTabsToRight = (id: string) => {
        const idx = allTabs.findIndex(t => t.id === id);
        tryBatchClose(allTabs.slice(idx + 1));
    };

    const closeTabsToLeft = (id: string) => {
        const idx = allTabs.findIndex(t => t.id === id);
        tryBatchClose(allTabs.slice(0, idx));
    };

    const closeAllTabs = () => {
        tryBatchClose([...allTabs]);
    };

    onMount(() => {
        shortcuts.registerHandler('newTab', () => {
            db.queryTabs.add();
            db.ui.setActiveView("query");
        });
        shortcuts.registerHandler('closeTab', closeCurrentTab);
        shortcuts.registerHandler('nextTab', () => {
            const idx = currentTabIndex();
            switchToTab((idx + 1) % allTabs.length);
        });
        shortcuts.registerHandler('previousTab', () => {
            const idx = currentTabIndex();
            switchToTab((idx - 1 + allTabs.length) % allTabs.length);
        });

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

    // Listen for "Close Tab" menu event from Tauri
    $effect(() => {
        if (!isTauri()) return;

        let cleanup: (() => void) | undefined;

        import("@tauri-apps/api/event").then(({ listen }) => {
            listen("menu-close-tab", () => {
                closeCurrentTab();
            }).then((unlisten) => {
                cleanup = unlisten;
            });
        });

        return () => {
            cleanup?.();
        };
    });
</script>

{#if isManagePage}
    {#if db.state.connectionsLoading || db.state.projectsLoading}
        <!-- Loading state - show nothing -->
    {:else if db.state.activeProjectId}
        <!-- Regular tabs (project has connections) -->
        <div class="flex items-center gap-1 h-full min-w-0">
            <div class="flex-1 overflow-x-auto overflow-y-hidden min-w-0 scrollbar-hide h-full" {@attach (el) => {
                if (paneDragState && paneId) {
                    const id = paneId;
                    paneDragState.registerTabBar(id, el);
                    return () => paneDragState.unregisterTabBar(id);
                }
            }}>
                <div
                    class="flex items-end gap-2 w-max h-full"
                    use:dndzone={{
                        items: displayTabs,
                        type: paneId ? `tabs-${paneId}` : 'tabs',
                        dropTargetStyle: {},
                        dragDisabled: editingTabId !== null
                    }}
                    onconsider={handleDndConsider}
                    onfinalize={handleDndFinalize}
                >
                    {#each displayTabs as { id, type, tab } (id)}
                        {@const config = tabConfigs[type]}
                        {@const TabIcon = config.icon}
                        {@const label = config.label(tab)}
                        {@const isRenameable = config.renameable ?? false}
                        {@const showClose = config.closable ? config.closable(tab) : true}
                        {@const unsaved = config.unsavedIndicator?.(id) ?? ""}
                        <div>
                        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                            <ContextMenu.Root>
                                <ContextMenu.Trigger>
                                    <div
                                        class={[
                                            "relative group shrink-0 flex items-center gap-2 px-3 h-7 text-xs transition-colors",
                                            config.extraClass,
                                            isTabActive(id, type)
                                                ? "bg-muted border-t border-l border-r border-border rounded-t-md -mb-px"
                                                : "hover:bg-muted/50 rounded-t-md -mb-px border-t border-l border-r border-transparent",
                                        ]}
                                        onclick={() => handleTabClick(id, type, tab)}
                                    >
                                        <TabIcon class="size-3 text-muted-foreground" />
                                        {#if isRenameable && editingTabId === id}
                                            <input
                                                bind:value={editingTabName}
                                                class="h-5 px-1 text-xs pe-4 outline-none ring-0"
                                                style="width: {editingInputWidth}px"
                                                onkeydown={handleKeydown}
                                                onblur={finishEditing}
                                                onclick={(e) => e.stopPropagation()}
                                                {@attach autofocus()}
                                            />
                                        {:else if isRenameable}
                                            <span
                                                class="pe-4"
                                                ondblclick={(e) => {
                                                    e.stopPropagation();
                                                    startEditing(id, label, e.currentTarget);
                                                }}
                                            >
                                                {label}{unsaved}
                                            </span>
                                        {:else}
                                            <span class="pe-4">{label}</span>
                                        {/if}
                                        {#if showClose}
                                            <Button
                                                size="icon"
                                                variant="ghost"
                                                class="absolute end-0 top-1/2 -translate-y-1/2 size-5 opacity-0 group-hover:opacity-100 transition-opacity [&_svg:not([class*='size-'])]:size-3"
                                                onclick={(e) => {
                                                    e.stopPropagation();
                                                    closeTab(id, type);
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
                        </div>
                    {/each}
                </div>
            </div>

            <!-- Add new tab button with dropdown -->
            <div class="flex shrink-0 items-center">
                <Tooltip.Root>
                    <Tooltip.Trigger>
                        <Button
                            size="icon"
                            variant="ghost"
                            class="size-7 shrink-0 rounded-e-none [&_svg:not([class*='size-'])]:size-4"
                            onclick={() => {
                                db.queryTabs.add();
                                db.ui.setActiveView("query");
                            }}
                        >
                            <PlusIcon />
                        </Button>
                    </Tooltip.Trigger>
                    <Tooltip.Content side="bottom">
                        <span class="flex items-center gap-2">
                            New Query
                            {#if findShortcut('newTab')}
                                <ShortcutKeys keys={findShortcut('newTab')!.keys} />
                            {/if}
                        </span>
                    </Tooltip.Content>
                </Tooltip.Root>
                <DropdownMenu.Root>
                    <DropdownMenu.Trigger>
                        {#snippet child({ props })}
                            <button
                                {...props}
                                class="flex h-7 w-4 shrink-0 items-center justify-center rounded-e-md text-muted-foreground hover:bg-accent hover:text-accent-foreground"
                            >
                                <ChevronDownIcon class="size-3" />
                            </button>
                        {/snippet}
                    </DropdownMenu.Trigger>
                    <DropdownMenu.Portal>
                        <DropdownMenu.Content align="start" side="bottom" class="min-w-40">
                            <DropdownMenu.Item onclick={() => {
                                db.queryTabs.add();
                                db.ui.setActiveView("query");
                            }}>
                                <FileCodeIcon class="mr-2 size-4" />
                                Query
                            </DropdownMenu.Item>
                            <DropdownMenu.Item onclick={() => {
                                void db.connectionTabs.open();
                            }}>
                                <CableIcon class="mr-2 size-4" />
                                Connection
                            </DropdownMenu.Item>
                            <DropdownMenu.Item onclick={async () => {
                                const dashboard = await db.dashboards.createDashboard("New Dashboard");
                                if (dashboard) {
                                    db.dashboardTabs.add(dashboard.id, dashboard.name);
                                }
                            }}>
                                <LayoutDashboardIcon class="mr-2 size-4" />
                                Dashboard
                            </DropdownMenu.Item>
                        </DropdownMenu.Content>
                    </DropdownMenu.Portal>
                </DropdownMenu.Root>
            </div>
        </div>
    {/if}
{/if}

<UnsavedChangesDialog
    bind:open={showUnsavedDialog}
    onDiscard={handleUnsavedDiscard}
    onSave={handleUnsavedSave}
    onCancel={handleUnsavedCancel}
/>

{#if pendingCloseTabId}
    {@const pendingTab = db.state.queryTabs.find(t => t.id === pendingCloseTabId)}
    {#if pendingTab}
        <SaveQueryDialog
            bind:open={showSaveDialogForClose}
            query={pendingTab.query}
            tabId={pendingCloseTabId}
            onSaveComplete={handleSaveComplete}
        />
    {/if}
{/if}

<BatchUnsavedDialog
    bind:open={showBatchUnsavedDialog}
    unsavedCount={unsavedTabsInBatch.length}
    onDiscardAll={handleBatchDiscard}
    onCancel={handleBatchCancel}
/>
