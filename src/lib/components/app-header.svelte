<script lang="ts">
    import { Badge } from "$lib/components/ui/badge";
    import SidebarIcon from "@lucide/svelte/icons/sidebar";
    import { Button } from "$lib/components/ui/button/index.js";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import { useDatabase } from "$lib/hooks/database.svelte.js";
    import { ScrollArea } from "$lib/components/ui/scroll-area";
    import ConnectionDialog from "$lib/components/connection-dialog.svelte";
    import XIcon from "@lucide/svelte/icons/x";
    import PlusIcon from "@lucide/svelte/icons/plus";
    import BotIcon from "@lucide/svelte/icons/bot";
    import NetworkIcon from "@lucide/svelte/icons/network";
    import ThemeToggle from "./theme-toggle.svelte";
    import type { DatabaseType } from "$lib/types";

    const db = useDatabase();
    const sidebar = Sidebar.useSidebar();

    const handleCloseConnection = (e: MouseEvent, connectionId: string) => {
        e.stopPropagation();
        db.toggleConnection(connectionId);
    };
    let showConnectionDialog = $state(false);
    let dialogPrefillData = $state<{
        id?: string;
        name?: string;
        type?: DatabaseType;
        host?: string;
        port?: number;
        databaseName?: string;
        username?: string;
        sslMode?: string;
        connectionString?: string;
    } | undefined>(undefined);

    const handleConnectionClick = (connection: typeof db.connections[0]) => {
        // If connection has a database instance, just activate it
        if (connection.database) {
            db.setActiveConnection(connection.id);
        } else {
            // If no database (persisted connection), open dialog with prefilled values
            dialogPrefillData = {
                id: connection.id,
                name: connection.name,
                type: connection.type,
                host: connection.host,
                port: connection.port,
                databaseName: connection.databaseName,
                username: connection.username,
                sslMode: connection.sslMode,
                connectionString: connection.connectionString,
            };
            showConnectionDialog = true;
        }
    };
</script>

<header
    class="bg-background sticky top-0 z-50 flex w-lvw items-center border-b"
>
    <div
        data-tauri-drag-region
        class="pl-18 h-(--header-height) flex w-full items-center gap-2 pr-2 justify-between"
    >
        <div class="flex items-center gap-1">
            <Button
                class="size-8"
                variant="ghost"
                size="icon"
                onclick={sidebar.toggle}
            >
                <SidebarIcon />
            </Button>
            {#if db.connections.length > 0}
                <ScrollArea orientation="horizontal" class="flex-1">
                    <div class="flex items-center gap-1">
                        {#each db.connections as connection (connection.id)}
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
                            </div>
                        {/each}
                        <Button
                            size="icon"
                            variant="ghost"
                            class="size-8"
                            onclick={() => {
                                dialogPrefillData = undefined;
                                showConnectionDialog = true;
                            }}
                        >
                            <PlusIcon class="size-4" />
                        </Button>
                    </div>
                </ScrollArea>
            {:else}
                <Badge
                    variant="outline"
                    class="cursor-pointer hover:bg-muted transition-colors"
                    onclick={() => {
                        dialogPrefillData = undefined;
                        showConnectionDialog = true;
                    }}
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

<ConnectionDialog bind:open={showConnectionDialog} prefill={dialogPrefillData} />
