<script lang="ts">
    import { page } from "$app/state";
    import { resolve } from "$app/paths";
    import { locales, localizeHref } from "$lib/paraglide/runtime";
    // `layout.css` stays at `src/routes/layout.css` (one level up) so the root
    // layout can also apply it to public auth pages. ModeWatcher and Toaster
    // are rendered by the root layout now — no need to repeat them here.
    import AppHeader from "$lib/components/app-header.svelte";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import { setDatabase, useDatabase } from "$lib/hooks/database.svelte.js";
    import { setShortcuts } from "$lib/shortcuts/index.js";
    import { themeStore } from "$lib/stores/theme.svelte.js";
    import { applyThemeColors } from "$lib/themes/apply";
    import type { ThemeColors } from "$lib/types/theme";
    import { toast } from "svelte-sonner";
    import { errorToast } from "$lib/utils/toast";
    import { showErrorUnlessShown } from "$lib/errors";
    import { m } from "$lib/paraglide/messages.js";
    import { onMount } from "svelte";
    import { onboardingStore } from "$lib/stores/onboarding.svelte.js";
    import { licenseStore } from "$lib/stores/license.svelte.js";
    import { licenseNudgeStore } from "$lib/stores/license-nudge.svelte.js";
    import LicenseNudgeCard from "$lib/components/license-nudge-card.svelte";
    import { dbeaverImportStore } from "$lib/stores/dbeaver-import.svelte.js";
    import { tablePlusImportStore } from "$lib/stores/tableplus-import.svelte.js";
    import { tutorialProgressStore } from "$lib/stores/tutorial-progress.svelte.js";
    import { isDemo, isTauri, isWeb } from "$lib/utils/environment";
    import { initLogger } from "$lib/utils/logger";
    import { updateStore } from "$lib/stores/update.svelte.js";
    import type { UpdateInfo } from "$lib/api/tauri";
    import { deepLinkDialogStore } from "$lib/stores/deep-link-dialog.svelte.js";
    import { deepLinkProjectPickerStore } from "$lib/stores/deep-link-project-picker.svelte.js";
    import { sshHostKeyPromptStore } from "$lib/stores/ssh-host-key-prompt.svelte.js";
    import VaultGate from "$lib/components/vault/vault-gate.svelte";
    import { handleDeepLink } from "$lib/services/deep-link";
    import { setupFileDropListener } from "$lib/services/file-drop.svelte.js";
    import FileDropOverlay from "$lib/components/file-drop-overlay.svelte";
    import ConnectionSecretsNotice from "$lib/components/connection-secrets-notice.svelte";
    import { connectionSecretsNotice } from "$lib/stores/connection-secrets-notice.svelte.js";
    import StorageErrorScreen from "$lib/components/storage-error-screen.svelte";
    import { storageGate } from "$lib/storage/storage-gate.svelte";

    setDatabase();

    const db = useDatabase();
    const shortcuts = setShortcuts();
    let { children } = $props();
    let commandPaletteOpen = $state(false);

    $effect(() => {
        shortcuts.registerHandler("commandPalette", () => {
            commandPaletteOpen = !commandPaletteOpen;
        });
        return () => shortcuts.unregisterHandler("commandPalette");
    });

    // Check if we're in a standalone window (no app shell needed)
    const isStandaloneWindow = $derived(
        page.url.pathname.startsWith("/windows/"),
    );

    // Public auth pages render their own self-contained card layout — no
    // app shell, no header, no sidebar. The underlying database context is
    // still set up so signing in (which does a full-page reload) starts from
    // a clean slate.
    const isAuthPage = $derived(
        page.url.pathname === "/login" || page.url.pathname === "/signup",
    );

    // Initialize stores on mount
    onMount(async () => {
        // Web-mode auth gate: every non-auth page requires a session. If
        // missing, bounce to /login with the original path as `redirect=`.
        // Desktop (Tauri) and demo builds skip this entirely — they have no
        // /api/auth endpoint to call.
        if (isWeb() && !isAuthPage) {
            const { getAuthClient } = await import("$lib/auth-client");
            const session = await getAuthClient().getSession();
            if (!session.data?.user) {
                const here = window.location.pathname + window.location.search;
                window.location.href = `/login?redirect=${encodeURIComponent(here)}`;
                return;
            }
        }

        // The first storage call (shared with `UseDatabase`'s init). Legacy
        // or corrupt storage stops here: the template shows the storage
        // error screen and none of the stores load.
        if (!(await storageGate.check())) return;

        const commonInit = [
            initLogger(),
            themeStore.initialize(),
            tutorialProgressStore.initialize(),
        ];

        if (isTauri()) {
            // Desktop app: initialize all independent stores in parallel
            await Promise.all([
                ...commonInit,
                onboardingStore.initialize(),
                licenseStore.initialize(),
                licenseNudgeStore.initialize(),
                dbeaverImportStore.initialize(),
                tablePlusImportStore.initialize(),
                updateStore.initialize(),
            ]);
        } else {
            await Promise.all(commonInit);
            if (!isDemo()) return;
            // Browser demo: initialize DuckDB with sample data
            try {
                const { initializeDemo } = await import("$lib/demo/init");
                const { createDemoDashboard } = await import("$lib/demo/sample-dashboard");
                const providerConnectionId = await initializeDemo();
                if (providerConnectionId) {
                    // Persisted connections must be loaded first, so a saved
                    // demo connection (and its labels) is updated, not duplicated.
                    await db.whenReady();
                    await db.connections.addDemoConnection(
                        providerConnectionId,
                    );
                    await createDemoDashboard(db);
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
        // Browsers don't await async unload work, so this is best-effort. On
        // desktop the onCloseRequested handler below does the reliable flush.
        void db.persistence.flush();
        themeStore.flush();
    }

    // Tauri-only event listeners
    $effect(() => {
        if (!isTauri()) return;

        // Dynamically import Tauri APIs only in desktop mode
        let cleanupFns: (() => void)[] = [];

        (async () => {
            // Nothing to listen for behind the storage error screen.
            if (!(await storageGate.check())) return;

            const { listen } = await import("@tauri-apps/api/event");

            // Flush pending debounced writes before the window actually closes;
            // `onbeforeunload` can't await, so work scheduled there is lost.
            // Standalone windows (e.g. the theme editor) own their own close
            // handling, so only the main app window registers this.
            if (!isStandaloneWindow) {
                const { getCurrentWebviewWindow } = await import(
                    "@tauri-apps/api/webviewWindow"
                );
                const appWindow = getCurrentWebviewWindow();
                const unlistenClose = await appWindow.onCloseRequested(
                    async (event) => {
                        event.preventDefault();
                        try {
                            await db.persistence.flush();
                            themeStore.flush();
                        } catch (error) {
                            console.error(
                                "[seaquel] flush on close failed:",
                                error,
                            );
                        }
                        await appWindow.destroy();
                    },
                );
                cleanupFns.push(() => {
                    void unlistenClose();
                });
            }

            // Listen for app updates
            const unlistenUpdate = await listen<UpdateInfo>(
                "update-downloaded",
                (event) => {
                    updateStore.setUpdateDownloaded(event.payload);
                },
            );
            cleanupFns.push(unlistenUpdate);

            // Listen for Settings menu event
            const unlistenSettings = await listen("menu-settings", () => {
                db.settingsTabs.open("app");
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

            // File drag-and-drop handling
            const unlistenFileDrop = await setupFileDropListener(db);
            cleanupFns.push(unlistenFileDrop);

            // Deep link handling
            const { onOpenUrl, getCurrent } = await import("@tauri-apps/plugin-deep-link");

            // Handle deep links while running
            const unlistenDeepLink = await onOpenUrl((urls) => {
                for (const url of urls) void handleDeepLink(url, db).catch(showErrorUnlessShown);
            });
            cleanupFns.push(unlistenDeepLink);

            // Handle deep link that launched the app (after db is ready)
            const launchUrls = await getCurrent();
            if (launchUrls?.length) {
                for (const url of launchUrls) void handleDeepLink(url, db).catch(showErrorUnlessShown);
            }
        })();

        return () => {
            cleanupFns.forEach((fn) => fn());
        };
    });
    // When Learn is disabled, always treat as "manage" for sidebar width
    const activeNavItem = $derived(
        onboardingStore.learnEnabled && page.url.pathname.startsWith(resolve("/(app)/learn")) ? "learn" : "manage",
    );

    // Redirect to /manage if Learn is disabled and on a /learn route
    $effect(() => {
        if (!onboardingStore.learnEnabled && page.url.pathname.startsWith(resolve("/(app)/learn"))) {
            import("$app/navigation").then(({ goto }) => {
                goto(resolve("/(app)/manage"));
            });
        }
    });
</script>

<svelte:window
    onkeydown={shortcuts.handleKeydown}
    onbeforeunload={handleBeforeUnload}
/>
<!-- ModeWatcher and Toaster are rendered by the root layout so /login and
     /signup get them too. -->

{#if !storageGate.blocked}
    <FileDropOverlay />
{/if}

{#if storageGate.blocked && !isAuthPage}
    <!-- Storage can't be opened (legacy or corrupt): no app, no retry. -->
    <StorageErrorScreen error={storageGate.blocked} />
{:else if isStandaloneWindow || isAuthPage}
    <!-- Standalone window or public auth page: no app shell -->
    {@render children()}
{:else}
    <!-- Main app window: full app shell with header and sidebars -->
    {#if shortcuts.showHelp}
        {#await import("$lib/components/keyboard-shortcuts-dialog.svelte") then module}
            <module.default />
        {/await}
    {/if}
    {#if commandPaletteOpen}
        {#await import("$lib/components/command-palette.svelte") then module}
            <module.default bind:open={commandPaletteOpen} />
        {/await}
    {/if}
    {#if dbeaverImportStore.isOpen}
        {#await import("$lib/components/dbeaver-import-dialog.svelte") then module}
            <module.default />
        {/await}
    {/if}
    {#if tablePlusImportStore.isOpen}
        {#await import("$lib/components/tableplus-import-dialog.svelte") then module}
            <module.default />
        {/await}
    {/if}
    {#if deepLinkDialogStore.open}
        {#await import("$lib/components/deep-link-clone-dialog.svelte") then module}
            <module.default />
        {/await}
    {/if}
    {#if deepLinkProjectPickerStore.open}
        {#await import("$lib/components/deep-link-project-picker-dialog.svelte") then module}
            <module.default />
        {/await}
    {/if}
    {#if sshHostKeyPromptStore.open}
        {#await import("$lib/components/ssh-host-key-dialog.svelte") then module}
            <module.default />
        {/await}
    {/if}
    {#if isTauri()}
        <LicenseNudgeCard />
    {/if}
    {#if isWeb()}
        <VaultGate />
    {/if}
    {#if connectionSecretsNotice.open}
        <ConnectionSecretsNotice />
    {/if}

    <Sidebar.Provider
        class="[--header-height:calc(--spacing(8))] flex-col h-svh overflow-hidden"
        style={onboardingStore.learnEnabled ? "--sidebar-width: 20rem" : ""}
    >
        {#if !db.state.isDashboardFullscreen}
            <AppHeader />
        {/if}
        <div class="flex w-full flex-1 min-h-0 overflow-hidden">
            {@render children()}
        </div>
    </Sidebar.Provider>
    <div style="display:none">
        {#each locales as locale (locale)}
            <a href={localizeHref(page.url.pathname, { locale })}>
                {locale}
            </a>
        {/each}
    </div>
{/if}
