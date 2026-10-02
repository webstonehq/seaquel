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
    import { extractErrorMessage, showErrorUnlessShown } from "$lib/errors";
    import { m } from "$lib/paraglide/messages.js";
    import { onMount } from "svelte";
    import { onboardingStore } from "$lib/stores/onboarding.svelte.js";
    import { licenseStore } from "$lib/stores/license.svelte.js";
    import { licenseNudgeStore } from "$lib/stores/license-nudge.svelte.js";
    import LicenseNudgeCard from "$lib/components/license-nudge-card.svelte";
    import { dbeaverImportStore, tablePlusImportStore } from "$lib/stores/connection-import.svelte.js";
    import { linkProjectDialogStore } from "$lib/stores/link-project-dialog.svelte.js";
    import { unlinkProjectDialogStore } from "$lib/stores/unlink-project-dialog.svelte.js";
    import { sharedProjectImportStore } from "$lib/stores/shared-project-import.svelte.js";
    import { tutorialProgressStore } from "$lib/stores/tutorial-progress.svelte.js";
    import { isDemo, isTauri, isWeb } from "$lib/utils/environment";
    import { initLogger } from "$lib/utils/logger";
    import { updateStore } from "$lib/stores/update.svelte.js";
    import type { UpdateInfo } from "$lib/api/tauri";
    import { deepLinkDialogStore } from "$lib/stores/deep-link-dialog.svelte.js";
    import { sshHostKeyPromptStore } from "$lib/stores/ssh-host-key-prompt.svelte.js";
    import VaultGate from "$lib/components/vault/vault-gate.svelte";
    import { handleDeepLink } from "$lib/services/deep-link";
    import { setupFileDropListener } from "$lib/services/file-drop.svelte.js";
    import FileDropOverlay from "$lib/components/file-drop-overlay.svelte";
    import ConnectionSecretsNotice from "$lib/components/connection-secrets-notice.svelte";
    import { connectionSecretsNotice } from "$lib/stores/connection-secrets-notice.svelte.js";
    import StorageErrorScreen from "$lib/components/storage-error-screen.svelte";
    import { storageGate } from "$lib/storage/storage-gate.svelte";
    import { windowIdReady } from "$lib/core/window-id";

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

    /**
     * The demo's start (phase 8, Decision 19): its Core opened before the
     * app rendered (`src/routes/+layout.ts`); this shows Core's notices
     * once and connects and seeds the demo database. The branch is on the
     * build-time constant, so desktop and web bundles don't contain it.
     */
    const startDemoPage =
        import.meta.env.VITE_BUILD_TARGET === "demo"
            ? async () => {
                  const { demoCore } = await import("$lib/demo/core");
                  const opened = demoCore();
                  // The root layout shows why Core didn't open.
                  if (!opened) return;
                  for (const notice of opened.notices) {
                      toast.warning(
                          notice.code === "STORAGE_CORRUPT"
                              ? m.demo_notice_storage_corrupt()
                              : m.demo_notice_storage_unavailable(),
                      );
                  }
                  try {
                      const { startDemo } = await import("$lib/demo/init");
                      const { getProvider } = await import("$lib/providers");
                      await startDemo(db, opened.core, await getProvider());
                      toast.success("Demo database loaded with sample data");
                  } catch (error) {
                      console.error("[Demo] Failed to initialize:", error);
                      errorToast(
                          `Failed to initialize demo database: ${extractErrorMessage(error)}`,
                      );
                  }
              }
            : null;

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

        // This tab's window id first: it is the origin of every Core call
        // (web: a duplicated tab makes a new one here, before any call).
        await windowIdReady();
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
            await startDemoPage?.();
        }
    });

    // Apply active theme whenever it changes
    $effect(() => {
        if (themeStore.isLoaded) {
            themeStore.applyActiveTheme();
        }
    });

    /** Web and the demo save on `pagehide` instead of `beforeunload`. */
    function savesOnPageHide() {
        return isWeb() || isDemo();
    }

    function handleBeforeUnload() {
        // Web and the demo save on `pagehide` instead (below), where the
        // active project's pending save leaves outside the write queue: as a
        // `keepalive` request on web, into the view-state journal in the demo
        // (Chromium runs no task between `beforeunload` and `pagehide`, so a
        // save queued here would miss the snapshot; Task 7 probe, item 1).
        // Flushing here first would put it on the queue instead.
        if (savesOnPageHide()) return;
        // Browsers don't await async unload work, so this is best-effort. On
        // desktop the onCloseRequested handler below does the reliable flush.
        void db.flush();
        void themeStore.flush();
    }

    function handlePageHide() {
        if (!savesOnPageHide()) return;
        db.saveOnPageHide();
        void themeStore.flush();
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
                            await db.flush();
                            await themeStore.flush();
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
            }>("theme-editor:save", async (event) => {
                const { themeId, name, isDark, colors } = event.payload;
                // Said once Core stored it; a refusal is shown by the store.
                const saved = themeId
                    ? await themeStore.updateTheme(themeId, { name, isDark, colors })
                    : (await themeStore.addTheme({ name, isDark, colors })) !== null;
                if (saved) toast.success(m.theme_save_success());
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
    onpagehide={handlePageHide}
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
        {#await import("$lib/components/connection-import-dialog.svelte") then module}
            <module.default store={dbeaverImportStore} />
        {/await}
    {/if}
    {#if tablePlusImportStore.isOpen}
        {#await import("$lib/components/connection-import-dialog.svelte") then module}
            <module.default store={tablePlusImportStore} />
        {/await}
    {/if}
    {#if linkProjectDialogStore.isOpen}
        {#await import("$lib/components/link-project-dialog.svelte") then module}
            <module.default />
        {/await}
    {/if}
    {#if sharedProjectImportStore.isOpen}
        <!-- One mount for the header, the getting-started tab and deep links. -->
        {#await import("$lib/components/import-shared-project-dialog.svelte") then module}
            <module.default />
        {/await}
    {/if}
    {#if unlinkProjectDialogStore.isOpen}
        {#await import("$lib/components/unlink-project-dialog.svelte") then module}
            <module.default />
        {/await}
    {/if}
    {#if db.state.sharedConflict}
        {@const conflict = db.state.sharedConflict}
        {#await import("$lib/components/shared-queries/sync-conflict-dialog.svelte") then module}
            <module.default
                open={true}
                onOpenChange={(open) => { if (!open) db.sharedRepos.closeConflict(); }}
                repoId={conflict.repoId}
                conflictFiles={conflict.files}
            />
        {/await}
    {/if}
    {#if deepLinkDialogStore.open}
        {#await import("$lib/components/deep-link-clone-dialog.svelte") then module}
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
