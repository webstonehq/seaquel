<script lang="ts">
    import { onMount, onDestroy } from "svelte";
    import { SidebarInset } from "$lib/components/ui/sidebar";
    import SidebarLeft from "$lib/components/sidebar-left.svelte";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import { useDatabase } from "$lib/hooks/database.svelte.js";
    import { useShortcuts } from "$lib/shortcuts/index.js";
    import { useSidebar } from "$lib/components/ui/sidebar/context.svelte.js";
    import PaneContainer from "$lib/components/pane-container.svelte";
    import { aiSettingsStore } from "$lib/stores/ai-settings.svelte.js";
    import { m } from "$lib/paraglide/messages.js";

    const db = useDatabase();
    const shortcuts = useShortcuts();
    const sidebar = useSidebar();

    const rightPanelOpen = $derived(
        db.state.activeRightPanel === "pendingChanges" ||
        (db.state.activeRightPanel === "ai" && !db.state.isDashboardFullscreen && aiSettingsStore.available)
    );

    // Register keyboard shortcuts (settings + sidebar only; tab shortcuts are in header-tabs)
    onMount(() => {
        shortcuts.registerHandler('openSettings', () => {
            db.settingsTabs.open("app");
        });

        shortcuts.registerHandler('openProjectSettings', () => {
            if (db.state.activeProjectId) {
                db.settingsTabs.open("project");
            }
        });

        shortcuts.registerHandler('toggleSidebar', () => {
            sidebar.toggle();
        });
    });

    onDestroy(() => {
        shortcuts.unregisterHandler('openSettings');
        shortcuts.unregisterHandler('openProjectSettings');
        shortcuts.unregisterHandler('toggleSidebar');
    });
</script>

{#if !db.state.isDashboardFullscreen}
    <SidebarLeft />
{/if}

<SidebarInset class="flex flex-col h-full overflow-hidden min-w-0">
    {#if db.state.connectionsLoading || db.state.projectsLoading}
        <div class="flex h-full items-center justify-center text-sm text-muted-foreground" role="status">
            {m.ext_loading()}
        </div>
    {:else}
        <div class="flex-1 min-h-0 flex flex-col">
            <PaneContainer />
        </div>
    {/if}
</SidebarInset>

<Sidebar.Provider
    class="contents"
    open={rightPanelOpen}
    onOpenChange={(open) => {
        if (!open) db.state.activeRightPanel = null;
    }}
    style="--sidebar-width: 24rem"
>
    <Sidebar.Root side="right" collapsible="offcanvas" class="top-(--header-height) h-[calc(100svh-var(--header-height))]">
        {#if db.state.activeRightPanel === "pendingChanges"}
            {#await import("$lib/components/pending-changes-sheet.svelte") then module}
                <module.default />
            {/await}
        {:else if db.state.activeRightPanel === "ai"}
            {#await import("$lib/components/ai-assistant.svelte") then module}
                <module.default />
            {/await}
        {/if}
    </Sidebar.Root>
</Sidebar.Provider>
