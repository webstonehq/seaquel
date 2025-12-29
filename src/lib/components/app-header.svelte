<script lang="ts">
    import { Badge } from "$lib/components/ui/badge";
    import SidebarIcon from "@lucide/svelte/icons/sidebar";
    import { Button } from "$lib/components/ui/button/index.js";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
    import * as Dialog from "$lib/components/ui/dialog/index.js";
    import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
    import DatabaseIcon from "@lucide/svelte/icons/database";
    import { useDatabase } from "$lib/hooks/database.svelte.js";
    import ConnectionDialog from "$lib/components/connection-dialog.svelte";
    import PlusIcon from "@lucide/svelte/icons/plus";
    import BotIcon from "@lucide/svelte/icons/bot";
    import NetworkIcon from "@lucide/svelte/icons/network";
    import ThemeToggle from "./theme-toggle.svelte";
    import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";

    const db = useDatabase();
    const sidebar = Sidebar.useSidebar();

    // Sort connections by last connected (most recent first)
    const sortedConnections = $derived(
        [...db.connections].sort((a, b) => {
            const aTime = a.lastConnected?.getTime() ?? 0;
            const bTime = b.lastConnected?.getTime() ?? 0;
            return bTime - aTime;
        })
    );

    // Remove connection confirmation dialog state
    let showRemoveDialog = $state(false);
    let connectionToRemove = $state<string | null>(null);
    let connectionToRemoveName = $state("");

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
                <DropdownMenu.Root>
                    <DropdownMenu.Trigger class="flex items-center gap-2 px-3 h-8 text-sm rounded-md bg-background hover:bg-muted transition-colors">
                        <DatabaseIcon class="size-4 text-muted-foreground" />
                        {#if db.activeConnection}
                            <span
                                class={[
                                    "size-2 rounded-full shrink-0",
                                    db.activeConnection.database ? "bg-green-500" : "bg-gray-400"
                                ]}
                            ></span>
                            <span class="max-w-32 truncate" title={db.activeConnection.name}>{db.activeConnection.name}</span>
                        {:else}
                            <span class="text-muted-foreground">Select Connection</span>
                        {/if}
                    </DropdownMenu.Trigger>
                    <DropdownMenu.Content class="w-56" align="start">
                        {#each sortedConnections as connection (connection.id)}
                            <ContextMenu.Root>
                                <ContextMenu.Trigger class="w-full">
                                    <DropdownMenu.Item
                                        class={[
                                            "flex items-center gap-2 cursor-pointer",
                                            db.activeConnectionId === connection.id && "bg-accent"
                                        ]}
                                        onclick={() => handleConnectionClick(connection)}
                                    >
                                        <span
                                            class={[
                                                "size-2 rounded-full shrink-0",
                                                connection.database ? "bg-green-500" : "bg-gray-400"
                                            ]}
                                            title={connection.database ? "Connected" : "Disconnected"}
                                        ></span>
                                        <span class="flex-1 truncate">{connection.name}</span>
                                        {#if db.activeConnectionId === connection.id}
                                            <span class="text-xs text-muted-foreground">Active</span>
                                        {/if}
                                    </DropdownMenu.Item>
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
                        <DropdownMenu.Separator />
                        <DropdownMenu.Item
                            class="flex items-center gap-2 cursor-pointer"
                            onclick={() => connectionDialogStore.open()}
                        >
                            <PlusIcon class="size-4" />
                            Add Connection
                        </DropdownMenu.Item>
                    </DropdownMenu.Content>
                </DropdownMenu.Root>
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
