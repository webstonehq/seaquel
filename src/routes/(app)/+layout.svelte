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
    import KeyboardShortcutsDialog from "$lib/components/keyboard-shortcuts-dialog.svelte";
    import CommandPalette from "$lib/components/command-palette.svelte";
    import { themeStore } from "$lib/stores/theme.svelte.js";
    import { applyThemeColors } from "$lib/themes/apply";
    import DbeaverImportDialog from "$lib/components/dbeaver-import-dialog.svelte";
    import TablePlusImportDialog from "$lib/components/tableplus-import-dialog.svelte";
    import type { ThemeColors } from "$lib/types/theme";
    import { toast } from "svelte-sonner";
    import { errorToast } from "$lib/utils/toast";
    import { m } from "$lib/paraglide/messages.js";
    import { onMount } from "svelte";
    import { onboardingStore } from "$lib/stores/onboarding.svelte.js";
    import { licenseStore } from "$lib/stores/license.svelte.js";
    import { licenseNudgeStore } from "$lib/stores/license-nudge.svelte.js";
    import LicenseNudgeCard from "$lib/components/license-nudge-card.svelte";
    import { dbeaverImportStore } from "$lib/stores/dbeaver-import.svelte.js";
    import { tablePlusImportStore } from "$lib/stores/tableplus-import.svelte.js";
    import { tutorialProgressStore } from "$lib/stores/tutorial-progress.svelte.js";
    import { isTauri, isWeb } from "$lib/utils/environment";
    import { getAuthClient } from "$lib/auth-client";
    import { initLogger } from "$lib/utils/logger";
    import { initializeDemo } from "$lib/demo/init";
    import { createDemoDashboard } from "$lib/demo/sample-dashboard";
    import { updateStore } from "$lib/stores/update.svelte.js";
    import type { UpdateInfo } from "$lib/api/tauri";
    import DeepLinkCloneDialog from "$lib/components/deep-link-clone-dialog.svelte";
    import DeepLinkProjectPickerDialog from "$lib/components/deep-link-project-picker-dialog.svelte";
    import SshHostKeyDialog from "$lib/components/ssh-host-key-dialog.svelte";
    import VaultGate from "$lib/components/vault/vault-gate.svelte";
    import { handleDeepLink } from "$lib/services/deep-link";
    import { setupFileDropListener } from "$lib/services/file-drop.svelte.js";
    import FileDropOverlay from "$lib/components/file-drop-overlay.svelte";

    setDatabase();

    const db = useDatabase();
    const shortcuts = setShortcuts();
    let { children } = $props();

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
            const session = await getAuthClient().getSession();
            if (!session.data?.user) {
                const here = window.location.pathname + window.location.search;
                window.location.href = `/login?redirect=${encodeURIComponent(here)}`;
                return;
            }
        }

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
            // Browser demo: initialize DuckDB with sample data
            try {
                const providerConnectionId = await initializeDemo();
                if (providerConnectionId) {
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
                for (const url of urls) handleDeepLink(url, db);
            });
            cleanupFns.push(unlistenDeepLink);

            // Handle deep link that launched the app (after db is ready)
            const launchUrls = await getCurrent();
            if (launchUrls?.length) {
                for (const url of launchUrls) handleDeepLink(url, db);
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

<FileDropOverlay />

{#if isStandaloneWindow || isAuthPage}
    <!-- Standalone window or public auth page: no app shell -->
    {@render children()}
{:else}
    <!-- Main app window: full app shell with header and sidebars -->
    <KeyboardShortcutsDialog />
    <CommandPalette />
    <DbeaverImportDialog />
    <TablePlusImportDialog />
    <DeepLinkCloneDialog />
    <DeepLinkProjectPickerDialog />
    <SshHostKeyDialog />
    {#if isTauri()}
        <LicenseNudgeCard />
    {/if}
    {#if isWeb()}
        <VaultGate />
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
        {#each locales as locale}
            <a href={localizeHref(page.url.pathname, { locale })}>
                {locale}
            </a>
        {/each}
    </div>
{/if}
