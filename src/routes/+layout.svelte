<script lang="ts">
    import "./layout.css";
    import { ModeWatcher } from "mode-watcher";
    import { Toaster } from "$lib/components/ui/sonner/index.js";
    import AppHeader from "$lib/components/app-header.svelte";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import { setDatabase } from "$lib/hooks/database.svelte.js";
    import { setShortcuts } from "$lib/shortcuts/index.js";
    import KeyboardShortcutsDialog from "$lib/components/keyboard-shortcuts-dialog.svelte";

    setDatabase();
    const shortcuts = setShortcuts();

    let { children } = $props();
</script>

<svelte:window onkeydown={shortcuts.handleKeydown} />

<ModeWatcher />
<Toaster position="bottom-right" theme={"dark"} richColors />
<KeyboardShortcutsDialog />

<Sidebar.Provider class="[--header-height:calc(--spacing(8))] flex-col h-svh overflow-hidden">
    <AppHeader />

    <div class="flex w-full flex-1 min-h-0 overflow-hidden">
        {@render children()}
    </div>
</Sidebar.Provider>
