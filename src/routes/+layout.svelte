<script lang="ts">
    import { page } from "$app/state";
    import { resolve } from "$app/paths";
    import { locales, localizeHref } from "$lib/paraglide/runtime";
    import "./layout.css";
    import { ModeWatcher } from "mode-watcher";
    import { Toaster } from "$lib/components/ui/sonner/index.js";
    import AppHeader from "$lib/components/app-header.svelte";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import { setDatabase, useDatabase } from "$lib/hooks/database.svelte.js";
    import { setShortcuts } from "$lib/shortcuts/index.js";
    import KeyboardShortcutsDialog from "$lib/components/keyboard-shortcuts-dialog.svelte";
    import CommandPalette from "$lib/components/command-palette.svelte";
    import SettingsDialog from "$lib/components/settings-dialog.svelte";
    import { settingsDialogStore } from "$lib/stores/settings-dialog.svelte.js";
    import { themeStore } from "$lib/stores/theme.svelte.js";
    import { applyThemeColors } from "$lib/themes/apply";
    import DbeaverImportDialog from "$lib/components/dbeaver-import-dialog.svelte";
    import type { ThemeColors } from "$lib/types/theme";
    import { toast } from "svelte-sonner";
    import { errorToast } from "$lib/utils/toast";
    import { m } from "$lib/paraglide/messages.js";
    import { onMount } from "svelte";
    import { onboardingStore } from "$lib/stores/onboarding.svelte.js";
    import { dbeaverImportStore } from "$lib/stores/dbeaver-import.svelte.js";
    import { tutorialProgressStore } from "$lib/stores/tutorial-progress.svelte.js";
    import { isTauri } from "$lib/utils/environment";
    import { initializeDemo } from "$lib/demo/init";

    setDatabase();

    const db = useDatabase();
    const shortcuts = setShortcuts();
    let { children } = $props();

    // Check if we're in the theme editor window (no app shell needed)
    const isThemeEditor = $derived(
        page.url.pathname.startsWith("/windows/theme-editor"),
    );

    // Initialize stores on mount
    onMount(async () => {
        await themeStore.initialize();
        await tutorialProgressStore.initialize();

        if (isTauri()) {
            // Desktop app: initialize onboarding and dbeaver import
            await onboardingStore.initialize();
            await dbeaverImportStore.initialize();
        } else {
            // Browser demo: initialize DuckDB with sample data
            try {
                const providerConnectionId = await initializeDemo();
                if (providerConnectionId) {
                    await db.connections.addDemoConnection(
                        providerConnectionId,
                    );
                    toast.success("Demo database loaded with sample data");
                }
            } catch (error) {
                console.error("[Demo] Failed to initialize:", error);
                errorToast("Failed to initialize demo database");
            }
        }
    });

    // Apply active theme whenever it changes
    $effect(() => {
        if (themeStore.isLoaded) {
            themeStore.applyActiveTheme();
        }
    });

    function handleBeforeUnload() {
        db.persistence.flush();
        themeStore.flush();
    }

    // Tauri-only event listeners
    $effect(() => {
        if (!isTauri()) return;

        // Dynamically import Tauri APIs only in desktop mode
        let cleanupFns: (() => void)[] = [];

        (async () => {
            const { listen } = await import("@tauri-apps/api/event");
            const { installUpdate } = await import("$lib/api/tauri");

            // Listen for app updates
            const unlistenUpdate = await listen<string>(
                "update-downloaded",
                (event) => {
                    toast.success(`Update v${event.payload} downloaded`, {
                        action: {
                            label: "Install & Restart",
                            onClick: () => installUpdate(),
                        },
                        duration: Infinity,
                    });
                },
            );
            cleanupFns.push(unlistenUpdate);

            // Listen for Settings menu event
            const unlistenSettings = await listen("menu-settings", () => {
                settingsDialogStore.open();
            });
            cleanupFns.push(unlistenSettings);

            // Listen for theme editor color updates (real-time preview)
            const unlistenColorUpdate = await listen<{ colors: ThemeColors }>(
                "theme-editor:color-update",
                (event) => {
                    applyThemeColors(event.payload.colors);
                },
            );
            cleanupFns.push(unlistenColorUpdate);

            // Listen for theme save from editor
            const unlistenThemeSave = await listen<{
                themeId: string | null;
                name: string;
                isDark: boolean;
                colors: ThemeColors;
            }>("theme-editor:save", (event) => {
                const { themeId, name, isDark, colors } = event.payload;
                if (themeId) {
                    themeStore.updateTheme(themeId, { name, isDark, colors });
                } else {
                    themeStore.addTheme({ name, isDark, colors });
                }
                toast.success(m.theme_save_success());
            });
            cleanupFns.push(unlistenThemeSave);

            // Listen for theme editor cancel (restore original theme)
            const unlistenThemeCancel = await listen(
                "theme-editor:cancel",
                () => {
                    themeStore.applyActiveTheme();
                },
            );
            cleanupFns.push(unlistenThemeCancel);
        })();

        return () => {
            cleanupFns.forEach((fn) => fn());
        };
    });
    // When Learn is disabled, always treat as "manage" for sidebar width
    const activeNavItem = $derived(
        onboardingStore.learnEnabled && page.url.pathname.startsWith(resolve("/learn")) ? "learn" : "manage",
    );

    // Redirect to /manage if Learn is disabled and on a /learn route
    $effect(() => {
        if (!onboardingStore.learnEnabled && page.url.pathname.startsWith(resolve("/learn"))) {
            import("$app/navigation").then(({ goto }) => {
                goto(resolve("/manage"));
            });
        }
    });
</script>

<svelte:window
    onkeydown={shortcuts.handleKeydown}
    onbeforeunload={handleBeforeUnload}
/>
<ModeWatcher />
<Toaster position="bottom-right" richColors expand />

{#if isThemeEditor}
    <!-- Theme editor window: minimal layout, no app shell -->
    {@render children()}
{:else}
    <!-- Main app window: full app shell with header and sidebars -->
    <KeyboardShortcutsDialog />
    <CommandPalette />
    <SettingsDialog />
    <DbeaverImportDialog />

    <Sidebar.Provider
        class="[--header-height:calc(--spacing(8))] flex-col h-svh overflow-hidden"
        style={onboardingStore.learnEnabled ? "--sidebar-width: 20rem" : ""}
    >
        <AppHeader />
        <div class="flex w-full flex-1 min-h-0 overflow-hidden">
            {@render children()}
        </div>
    </Sidebar.Provider>
    <div style="display:none">
        {#each locales as locale}
            <a href={localizeHref(page.url.pathname, { locale })}>
                {locale}
            </a>
        {/each}
    </div>
{/if}
