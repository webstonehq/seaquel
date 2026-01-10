<script lang="ts">
    import SidebarIcon from "@lucide/svelte/icons/sidebar";
    import { Button } from "$lib/components/ui/button/index.js";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
    import * as Dialog from "$lib/components/ui/dialog/index.js";
    import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
    import DatabaseIcon from "@lucide/svelte/icons/database";
    import { useDatabase } from "$lib/hooks/database.svelte.js";
    import ConnectionDialog from "$lib/components/connection-dialog.svelte";
    import ConnectionWizard from "$lib/components/connection-wizard/connection-wizard.svelte";
    import PlusIcon from "@lucide/svelte/icons/plus";
    import BotIcon from "@lucide/svelte/icons/bot";
    import NetworkIcon from "@lucide/svelte/icons/network";
    import ThemeToggle from "./theme-toggle.svelte";
    import LanguageToggle from "./language-toggle.svelte";
    import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";
    import { m } from "$lib/paraglide/messages.js";
    import { getFeatures } from "$lib/features";

    const db = useDatabase();
    const features = getFeatures();
    const sidebar = Sidebar.useSidebar();

    // Sort connections by last connected (most recent first)
    const sortedConnections = $derived(
        [...db.state.connections].sort((a, b) => {
            const aTime = a.lastConnected?.getTime() ?? 0;
            const bTime = b.lastConnected?.getTime() ?? 0;
            return bTime - aTime;
        })
    );

    // Remove connection confirmation dialog state
    let showRemoveDialog = $state(false);
    let connectionToRemove = $state<string | null>(null);
    let connectionToRemoveName = $state("");

    const handleConnectionClick = (connection: typeof db.state.connections[0]) => {
        // If connection has a database instance, just activate it
        if (connection.database || connection.mssqlConnectionId || connection.providerConnectionId) {
            db.connections.setActive(connection.id);
        } else {
            // If no database (persisted connection), open dialog with prefilled values
            connectionDialogStore.open({
                id: connection.id,
                name: connection.name,
                type: connection.type,
                host: connection.host,
                port: connection.port,
                databaseName: connection.databaseName,
                username: connection.username,
                sslMode: connection.sslMode,
                connectionString: connection.connectionString,
            });
        }
    };

    const confirmRemoveConnection = (connectionId: string, name: string) => {
        connectionToRemove = connectionId;
        connectionToRemoveName = name;
        showRemoveDialog = true;
    };

    const handleRemoveConnection = () => {
        if (connectionToRemove) {
            db.connections.remove(connectionToRemove);
            connectionToRemove = null;
            connectionToRemoveName = "";
        }
        showRemoveDialog = false;
    };
</script>

<header
    class="bg-background sticky top-0 z-50 flex w-lvw items-center border-b"
>
    <div
        data-tauri-drag-region
        class="pl-18 h-(--header-height) flex w-full items-center gap-2 pr-2 justify-between"
    >
        <div data-tauri-drag-region class="flex items-center gap-1 flex-1 min-w-0">
            <Button
                class="size-8 shrink-0"
                variant="ghost"
                size="icon"
                onclick={sidebar.toggle}
            >
                <SidebarIcon />
            </Button>
            {#if db.state.activeConnection}
                <DropdownMenu.Root>
                    <DropdownMenu.Trigger class="flex items-center gap-2 px-3 h-8 text-sm rounded-md bg-background hover:bg-muted transition-colors">
                        <DatabaseIcon class="size-4 text-muted-foreground" />
                        <span
                            class={[
                                "size-2 rounded-full shrink-0",
                                (db.state.activeConnection.database || db.state.activeConnection.mssqlConnectionId || db.state.activeConnection.providerConnectionId) ? "bg-green-500" : "bg-gray-400"
                            ]}
                        ></span>
                        <span class="max-w-32 truncate" title={db.state.activeConnection.name}>{db.state.activeConnection.name}</span>
                    </DropdownMenu.Trigger>
                    <DropdownMenu.Content class="w-56" align="start">
                        {#each sortedConnections as connection (connection.id)}
                            <ContextMenu.Root>
                                <ContextMenu.Trigger class="w-full">
                                    <DropdownMenu.Item
                                        class={[
                                            "flex items-center gap-2 cursor-pointer",
                                            db.state.activeConnectionId === connection.id && "bg-accent"
                                        ]}
                                        onclick={() => handleConnectionClick(connection)}
                                    >
                                        <span
                                            class={[
                                                "size-2 rounded-full shrink-0",
                                                (connection.database || connection.mssqlConnectionId || connection.providerConnectionId) ? "bg-green-500" : "bg-gray-400"
                                            ]}
                                            title={(connection.database || connection.mssqlConnectionId || connection.providerConnectionId) ? m.header_connected() : m.header_disconnected()}
                                        ></span>
                                        <span class="flex-1 truncate">{connection.name}</span>
                                        {#if db.state.activeConnectionId === connection.id}
                                            <span class="text-xs text-muted-foreground">{m.header_active()}</span>
                                        {/if}
                                    </DropdownMenu.Item>
                                </ContextMenu.Trigger>
                                <ContextMenu.Content class="w-40">
                                    {#if connection.database || connection.mssqlConnectionId || connection.providerConnectionId}
                                        <ContextMenu.Item onclick={() => db.connections.toggle(connection.id)}>
                                            {m.header_disconnect()}
                                        </ContextMenu.Item>
                                        <ContextMenu.Separator />
                                    {:else}
                                        <ContextMenu.Item onclick={() => handleConnectionClick(connection)}>
                                            {m.header_connect()}
                                        </ContextMenu.Item>
                                        <ContextMenu.Separator />
                                    {/if}
                                    <ContextMenu.Item
                                        class="text-destructive focus:text-destructive"
                                        onclick={() => confirmRemoveConnection(connection.id, connection.name)}
                                    >
                                        {m.header_remove_connection()}
                                    </ContextMenu.Item>
                                </ContextMenu.Content>
                            </ContextMenu.Root>
                        {/each}
                        {#if features.newConnections}
                            <DropdownMenu.Separator />
                            <DropdownMenu.Item
                                class="flex items-center gap-2 cursor-pointer"
                                onclick={() => connectionDialogStore.open()}
                            >
                                <PlusIcon class="size-4" />
                                {m.header_add_connection()}
                            </DropdownMenu.Item>
                        {/if}
                    </DropdownMenu.Content>
                </DropdownMenu.Root>
            {/if}
        </div>
        <div class="flex items-center gap-1">
            {#if db.state.activeConnection?.database || db.state.activeConnection?.mssqlConnectionId || db.state.activeConnection?.providerConnectionId}
                <Button
                    size="icon"
                    variant="ghost"
                    class="size-8"
                    title={m.header_view_erd()}
                    aria-label={m.header_view_erd()}
                    onclick={() => db.erdTabs.add()}
                >
                    <NetworkIcon class="size-5" />
                </Button>
                <Button
                    size="icon"
                    variant="ghost"
                    class="size-8"
                    aria-label={m.header_toggle_ai()}
                    onclick={() => db.ui.toggleAI()}
                >
                    <BotIcon class="size-5" />
                </Button>
            {/if}
            <LanguageToggle />
            <ThemeToggle />
        </div>
    </div>
</header>

<ConnectionDialog bind:open={connectionDialogStore.isOpen} prefill={connectionDialogStore.prefill} />
<ConnectionWizard />

<Dialog.Root bind:open={showRemoveDialog}>
    <Dialog.Content class="max-w-md">
        <Dialog.Header>
            <Dialog.Title>{m.header_remove_dialog_title()}</Dialog.Title>
            <Dialog.Description>
                {m.header_remove_dialog_description({ name: connectionToRemoveName })}
            </Dialog.Description>
        </Dialog.Header>
        <Dialog.Footer class="gap-2">
            <Button variant="outline" onclick={() => showRemoveDialog = false}>
                {m.header_button_cancel()}
            </Button>
            <Button variant="destructive" onclick={handleRemoveConnection}>
                {m.header_button_remove()}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
