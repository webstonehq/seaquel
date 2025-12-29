<script lang="ts">
    import "./layout.css";
    import { ModeWatcher } from "mode-watcher";
    import { Toaster } from "$lib/components/ui/sonner/index.js";
    import AppHeader from "$lib/components/app-header.svelte";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import { setDatabase, useDatabase } from "$lib/hooks/database.svelte.js";
    import { setShortcuts } from "$lib/shortcuts/index.js";
    import KeyboardShortcutsDialog from "$lib/components/keyboard-shortcuts-dialog.svelte";
    import CommandPalette from "$lib/components/command-palette.svelte";
    import { listen } from "@tauri-apps/api/event";
    import { invoke } from "@tauri-apps/api/core";
    import { toast } from "svelte-sonner";

    setDatabase();
    const db = useDatabase();
    const shortcuts = setShortcuts();

    let { children } = $props();

    function handleBeforeUnload() {
        db.flushPersistence();
    }

    $effect(() => {
        const unlisten = listen<string>("update-downloaded", (event) => {
            toast.success(`Update v${event.payload} downloaded`, {
                action: {
                    label: "Install & Restart",
                    onClick: () => invoke("install_update"),
                },
                duration: Infinity,
            });
        });
        return () => {
            unlisten.then((fn) => fn());
        };
    });
</script>

<svelte:window onkeydown={shortcuts.handleKeydown} onbeforeunload={handleBeforeUnload} />

<ModeWatcher />
<Toaster position="bottom-right" theme={"dark"} richColors />
<KeyboardShortcutsDialog />
<CommandPalette />

<Sidebar.Provider class="[--header-height:calc(--spacing(8))] flex-col h-svh overflow-hidden">
    <AppHeader />

    <div class="flex w-full flex-1 min-h-0 overflow-hidden">
        {@render children()}
    </div>
</Sidebar.Provider>
