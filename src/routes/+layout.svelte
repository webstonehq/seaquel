<script lang="ts">
    import "./layout.css";
    import { ModeWatcher } from "mode-watcher";
    import { Toaster } from "$lib/components/ui/sonner/index.js";
    import AppHeader from "$lib/components/app-header.svelte";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import { setDatabase } from "$lib/hooks/database.svelte.js";
    setDatabase();

    let { children } = $props();
</script>

<ModeWatcher />
<Toaster position="bottom-right" theme={"dark"} richColors />

<Sidebar.Provider class="[--header-height:calc(--spacing(8))] flex-col">
    <AppHeader />

    <div class="flex w-full min-h-svh overflow-hidden">
        {@render children()}
    </div>
</Sidebar.Provider>
