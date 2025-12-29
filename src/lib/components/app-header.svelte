<script lang="ts">
    import { Badge } from "$lib/components/ui/badge";
    import SidebarIcon from "@lucide/svelte/icons/sidebar";
    import { Button } from "$lib/components/ui/button/index.js";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
    import * as Dialog from "$lib/components/ui/dialog/index.js";
    import { useDatabase } from "$lib/hooks/database.svelte.js";
    import ConnectionDialog from "$lib/components/connection-dialog.svelte";
    import XIcon from "@lucide/svelte/icons/x";
    import PlusIcon from "@lucide/svelte/icons/plus";
    import BotIcon from "@lucide/svelte/icons/bot";
    import NetworkIcon from "@lucide/svelte/icons/network";
    import ThemeToggle from "./theme-toggle.svelte";
    import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";

    const db = useDatabase();
    const sidebar = Sidebar.useSidebar();

    // Remove connection confirmation dialog state
    let showRemoveDialog = $state(false);
    let connectionToRemove = $state<string | null>(null);
    let connectionToRemoveName = $state("");

    const handleCloseConnection = (e: MouseEvent, connectionId: string) => {
        e.stopPropagation();
        db.toggleConnection(connectionId);
    };

    const handleConnectionClick = (connection: typeof db.connections[0]) => {
        // If connection has a database instance, just activate it
        if (connection.database) {
            db.setActiveConnection(connection.id);
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
            db.removeConnection(connectionToRemove);
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
        <div class="flex items-center gap-1 flex-1 min-w-0">
            <Button
                class="size-8 shrink-0"
                variant="ghost"
                size="icon"
                onclick={sidebar.toggle}
            >
                <SidebarIcon />
            </Button>
            {#if db.connections.length > 0}
                <div class="flex-1 overflow-x-auto overflow-y-hidden min-w-0 scrollbar-hide">
                    <div class="flex items-center gap-1 w-max">
                        {#each db.connections as connection (connection.id)}
                            <ContextMenu.Root>
                                <ContextMenu.Trigger>
                                    <div
                                        role="button"
                                        tabindex="0"
                                        class={[
                                            "relative group shrink-0 flex items-center gap-2 px-3 h-5 text-xs rounded-md",
                                            db.activeConnectionId === connection.id
                                                ? "bg-primary text-primary-foreground border-primary"
                                                : "bg-background hover:bg-muted border-border",
                                        ]}
                                        onclick={() =>
                                            handleConnectionClick(connection)}
                                        onkeyup={(e) => {
                                            if (e.key !== "Escape") {
                                                handleConnectionClick(connection);
                                            }
                                        }}
                                    >
                                        <span
                                            class={[
                                                "size-2 rounded-full shrink-0",
                                                connection.database
                                                    ? "bg-green-500"
                                                    : "bg-gray-400"
                                            ]}
                                            title={connection.database ? "Connected" : "Disconnected"}
                                        ></span>
                                        <span class="pr-4">{connection.name}</span>
                                        {#if connection.database}
                                            <Button
                                                size="icon"
                                                variant="ghost"
                                                class={[
                                                    "absolute right-0 top-1/2 -translate-y-1/2 size-5 opacity-0 group-hover:opacity-100 transition-opacity",
                                                ]}
                                                onclick={(e) =>
                                                    handleCloseConnection(e, connection.id)}
                                            >
                                                <XIcon class="size-3" />
                                            </Button>
                                        {/if}
                                    </div>
                                </ContextMenu.Trigger>
                                <ContextMenu.Content class="w-40">
                                    {#if connection.database}
                                        <ContextMenu.Item onclick={() => db.toggleConnection(connection.id)}>
                                            Disconnect
                                        </ContextMenu.Item>
                                        <ContextMenu.Separator />
                                    {:else}
                                        <ContextMenu.Item onclick={() => handleConnectionClick(connection)}>
                                            Connect
                                        </ContextMenu.Item>
                                        <ContextMenu.Separator />
                                    {/if}
                                    <ContextMenu.Item
                                        class="text-destructive focus:text-destructive"
                                        onclick={() => confirmRemoveConnection(connection.id, connection.name)}
                                    >
                                        Remove Connection
                                    </ContextMenu.Item>
                                </ContextMenu.Content>
                            </ContextMenu.Root>
                        {/each}
                        <Button
                            size="icon"
                            variant="ghost"
                            class="size-8"
                            onclick={() => connectionDialogStore.open()}
                        >
                            <PlusIcon class="size-4" />
                        </Button>
                    </div>
                </div>
            {:else}
                <Badge
                    variant="outline"
                    class="cursor-pointer hover:bg-muted transition-colors"
                    onclick={() => connectionDialogStore.open()}
                >
                    <PlusIcon class="size-3 mr-1" />
                    Add new connection
                </Badge>
            {/if}
        </div>
        <div class="flex items-center gap-1">
            {#if db.activeConnection?.database}
                <Button
                    size="icon"
                    variant="ghost"
                    class="size-8"
                    title="View ERD"
                    onclick={() => db.addErdTab()}
                >
                    <NetworkIcon class="size-5" />
                </Button>
            {/if}
            <Button
                size="icon"
                variant="ghost"
                class="size-8"
                onclick={() => db.toggleAI()}
            >
                <BotIcon class="size-5" />
            </Button>
            <ThemeToggle />
        </div>
    </div>
</header>

<ConnectionDialog bind:open={connectionDialogStore.isOpen} prefill={connectionDialogStore.prefill} />

<Dialog.Root bind:open={showRemoveDialog}>
    <Dialog.Content class="max-w-md">
        <Dialog.Header>
            <Dialog.Title>Remove Connection</Dialog.Title>
            <Dialog.Description>
                Are you sure you want to remove "{connectionToRemoveName}"? This will delete the saved connection and all associated data.
            </Dialog.Description>
        </Dialog.Header>
        <Dialog.Footer class="gap-2">
            <Button variant="outline" onclick={() => showRemoveDialog = false}>
                Cancel
            </Button>
            <Button variant="destructive" onclick={handleRemoveConnection}>
                Remove
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
