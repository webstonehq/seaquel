<script lang="ts">
    import SidebarIcon from "@lucide/svelte/icons/sidebar";
    import { isMac } from "$lib/shortcuts/platform";
    import { Button } from "$lib/components/ui/button/index.js";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import * as Dialog from "$lib/components/ui/dialog/index.js";
    import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
    import DeleteConfirmDialog from "$lib/components/delete-confirm-dialog.svelte";
    import CheckIcon from "@lucide/svelte/icons/check";
    import { useDatabase } from "$lib/hooks/database.svelte.js";

    import PlusIcon from "@lucide/svelte/icons/plus";
    import SparklesIcon from "@lucide/svelte/icons/sparkles";
    import NetworkIcon from "@lucide/svelte/icons/network";
    import SettingsIcon from "@lucide/svelte/icons/settings";
    import FolderGit2Icon from "@lucide/svelte/icons/folder-git-2";
    import ImportSharedProjectDialog from "./import-shared-project-dialog.svelte";
    import { sharedProjectImportStore } from "$lib/stores/shared-project-import.svelte.js";
    import { toast } from "svelte-sonner";
    import { errorToast } from "$lib/utils/toast";
    import ExternalLinkIcon from "@lucide/svelte/icons/external-link";
    import CircleDollarSignIcon from "@lucide/svelte/icons/circle-dollar-sign";
    import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
    import ListChecksIcon from "@lucide/svelte/icons/list-checks";
    import MessageSquareTextIcon from "@lucide/svelte/icons/message-square-text";
    import BookOpenIcon from "@lucide/svelte/icons/book-open";
    import LanguageToggle from "./language-toggle.svelte";
    import { m } from "$lib/paraglide/messages.js";
    import { DEFAULT_PROJECT_ID } from "$lib/types";
    import { Badge } from "$lib/components/ui/badge/index.js";
    import { licenseStore } from "$lib/stores/license.svelte.js";
    import { updateStore } from "$lib/stores/update.svelte.js";

    import { isTauri, isWeb } from "$lib/utils/environment";
    import LogOutIcon from "@lucide/svelte/icons/log-out";
    import UsersIcon from "@lucide/svelte/icons/users";
    import { authClient } from "$lib/auth-client";
    import { page } from "$app/state";
    import { resolve } from "$app/paths";
    import UpdateBadge from "./update-badge.svelte";
    import { aiSettingsStore } from "$lib/stores/ai-settings.svelte.js";

    let appVersion = $state("");

    $effect(() => {
        if (isTauri()) {
            import("@tauri-apps/api/app").then(({ getVersion }) => {
                getVersion().then((v) => {
                    appVersion = v;
                });
            });
        }
    });

    let checkingForUpdates = $state(false);

    const checkForUpdates = async () => {
        checkingForUpdates = true;
        try {
            const { checkForUpdate } = await import("$lib/api/tauri");
            const info = await checkForUpdate();
            if (info) {
                updateStore.setUpdateAvailable(info);
            } else {
                updateStore.showUpToDate();
            }
        } catch {
            errorToast("Failed to check for updates");
        } finally {
            checkingForUpdates = false;
        }
    };

    const openExternal = (url: string) => {
        if (isTauri()) {
            import("$lib/api/tauri").then(({ openPath }) => {
                openPath(url);
            });
        } else {
            window.open(url, "_blank");
        }
    };

    const isLearnPage = $derived(page.url.pathname.startsWith(resolve("/(app)/learn")));

    const db = useDatabase();
    const sidebar = Sidebar.useSidebar();

    // Project management state
    let showNewProjectDialog = $state(false);
    let showRemoveProjectDialog = $state(false);
    let projectDropdownOpen = $state(false);
    let newProjectName = $state("");
    let projectToRemove = $state<string | null>(null);
    let projectToRemoveName = $state("");
    const handleCreateProject = async () => {
        if (!newProjectName.trim()) return;
        await db.projects.add(newProjectName.trim());
        newProjectName = "";
        showNewProjectDialog = false;
    };

    const openProjectSettings = (projectId: string) => {
        // Switch to the project first if needed, then open settings tab
        if (db.state.activeProjectId !== projectId) {
            db.projects.setActive(projectId);
        }
        db.settingsTabs.open("project");
    };

    const confirmRemoveProject = (projectId: string, name: string) => {
        projectToRemove = projectId;
        projectToRemoveName = name;
        showRemoveProjectDialog = true;
    };

    const handleRemoveProject = async () => {
        if (projectToRemove) {
            await db.projects.remove(projectToRemove);
            projectToRemove = null;
            projectToRemoveName = "";
        }
        showRemoveProjectDialog = false;
    };

    const handleImportFromRepo = async () => {
        if (!isTauri()) return;
        try {
            const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
            const selected = await openDialog({
                directory: true,
                multiple: false,
                title: "Select Git repository folder",
            });
            if (!selected) return;

            const projects = await db.sharedRepos.scanForSharedProjects(selected as string);
            if (projects.length === 0) {
                toast.info(m.shared_import_none_found());
                return;
            }
            if (projects.length === 1) {
                await db.projects.importFromGitRepo(selected as string, projects);
                toast.success(m.shared_import_success({ count: 1 }));
                return;
            }
            sharedProjectImportStore.openWithResults(selected as string, projects);
        } catch (error) {
            errorToast(error instanceof Error ? error.message : String(error));
        }
    };
</script>

<header
    class="bg-background sticky top-0 z-50 flex w-lvw items-center border-b"
>
    <div
        data-tauri-drag-region
        class="h-(--header-height) flex w-full items-center pr-2 {isMac() ? 'pl-18' : ''}"
    >
        <!-- Left section: sidebar toggle + project dropdown (fixed sidebar width) -->
        <div data-tauri-drag-region class="flex items-center gap-1 shrink-0 w-(--sidebar-width) {isMac() ? '' : 'ps-2'}">
            <Button
                class="size-6 shrink-0"
                variant="ghost"
                size="icon"
                onclick={sidebar.toggle}
            >
                <SidebarIcon />
            </Button>
            <!-- Project Dropdown -->
            <DropdownMenu.Root bind:open={projectDropdownOpen}>
                <DropdownMenu.Trigger>
                    {#snippet child({ props })}
                        <Button
                            {...props}
                            variant="ghost"
                            size="sm"
                            class="h-6 px-3"
                        >
                            <span class="max-w-40 truncate" title={db.state.activeProject?.name || m.project_default_name()}>
                                {db.state.activeProject?.name || m.project_default_name()}
                            </span>
                        </Button>
                    {/snippet}
                </DropdownMenu.Trigger>
                <DropdownMenu.Content class="w-56" align="start">
                    {#each db.state.projects as project (project.id)}
                        <div class="flex items-center group">
                            <DropdownMenu.Item
                                class="flex-1 min-w-0 flex items-center gap-2 cursor-pointer"
                                onclick={() => db.projects.setActive(project.id)}
                            >
                                <span class="w-4 shrink-0">
                                    {#if db.state.activeProjectId === project.id}
                                        <CheckIcon class="size-4" />
                                    {/if}
                                </span>
                                <span class="flex-1 truncate">{project.name}</span>
                            </DropdownMenu.Item>
                            <button
                                class="size-6 flex items-center justify-center rounded-sm text-muted-foreground hover:text-foreground hover:bg-accent shrink-0 opacity-0 group-hover:opacity-100 me-1"
                                onclick={() => { projectDropdownOpen = false; openProjectSettings(project.id); }}
                                title={m.project_settings()}
                            >
                                <SettingsIcon class="size-3" />
                            </button>
                        </div>
                    {/each}
                    <DropdownMenu.Separator />
                    <DropdownMenu.Item
                        class="flex items-center gap-2 cursor-pointer"
                        onclick={() => showNewProjectDialog = true}
                    >
                        <PlusIcon class="size-4" />
                        {m.project_new()}
                    </DropdownMenu.Item>
                    {#if isTauri()}
                        <DropdownMenu.Item
                            class="flex items-center gap-2 cursor-pointer"
                            onclick={handleImportFromRepo}
                        >
                            <FolderGit2Icon class="size-4" />
                            {m.shared_import_from_repo()}
                        </DropdownMenu.Item>
                    {/if}
                </DropdownMenu.Content>
            </DropdownMenu.Root>
        </div>

        <!-- Middle section: empty drag space -->
        <div data-tauri-drag-region class="flex-1 min-w-0 h-full"></div>

        <!-- Right section: action buttons -->
        <div class="flex items-center gap-1 shrink-0">
            {#if isTauri()}
                <button
                    class="cursor-pointer"
                    onclick={() => db.settingsTabs.open("app", "license")}
                >
                    <Badge variant={licenseStore.status === "active" ? "default" : licenseStore.status === "expired" || licenseStore.status === "invalid" ? "destructive" : "secondary"}>
                        {licenseStore.badgeLabel}
                    </Badge>
                </button>
            {/if}
            {#if !isLearnPage && (db.state.activeConnection?.providerConnectionId)}
                {#if db.state.activePendingChangesCount > 0}
                    <Button
                        variant="ghost"
                        size="sm"
                        class="h-6 px-2 gap-1.5 text-amber-600 dark:text-amber-400"
                        title={`${m.header_pending_changes()} (${db.state.activePendingChangesCount})`}
                        aria-label={m.header_pending_changes()}
                        onclick={() => db.pendingChanges.toggleSheet()}
                    >
                        <ListChecksIcon class="size-3.5" />
                        <span class="text-xs">{m.header_pending_changes()}&nbsp;({db.state.activePendingChangesCount})</span>
                    </Button>
                {:else}
                    <Button
                        size="icon"
                        variant="ghost"
                        class="size-6"
                        title={m.header_pending_changes()}
                        aria-label={m.header_pending_changes()}
                        onclick={() => db.pendingChanges.toggleSheet()}
                    >
                        <ListChecksIcon class="size-3.5" />
                    </Button>
                {/if}
                <Button
                    size="icon"
                    variant="ghost"
                    class="size-6"
                    title={m.header_view_erd()}
                    aria-label={m.header_view_erd()}
                    onclick={() => db.erdTabs.add()}
                >
                    <NetworkIcon class="size-3.5" />
                </Button>
                {#if aiSettingsStore.settings.enabled}
                <Button
                    size="icon"
                    variant="ghost"
                    class="size-6"
                    title={m.header_toggle_ai()}
                    aria-label={m.header_toggle_ai()}
                    onclick={() => db.ui.toggleAI()}
                >
                    <SparklesIcon class="size-3.5" />
                </Button>
                {/if}
            {/if}
            {#if isTauri()}
                <UpdateBadge />
            {/if}
            <LanguageToggle />
            <DropdownMenu.Root>
                <DropdownMenu.Trigger>
                    {#snippet child({ props })}
                        <Button
                            {...props}
                            size="icon"
                            variant="ghost"
                            class="size-6"
                            title={m.header_settings()}
                            aria-label={m.header_settings()}
                        >
                            <SettingsIcon class="size-3.5" />
                        </Button>
                    {/snippet}
                </DropdownMenu.Trigger>
                <DropdownMenu.Content class="w-52" align="end">
                    <DropdownMenu.Item onclick={() => db.settingsTabs.open("app")}>
                        Settings
                        <DropdownMenu.Shortcut>⌘,</DropdownMenu.Shortcut>
                    </DropdownMenu.Item>
                    <div class="flex items-center gap-2 px-2 py-1.5">
                        <div class="h-px flex-1 bg-border"></div>
                        <span class="text-muted-foreground text-xs shrink-0">Seaquel{appVersion ? ` v${appVersion}` : ""}</span>
                        <div class="h-px flex-1 bg-border"></div>
                    </div>
                    {#if isTauri() && licenseStore.status !== "active"}
                        <DropdownMenu.Item
                            class="text-emerald-600 dark:text-emerald-400 focus:text-emerald-600 dark:focus:text-emerald-400"
                            onclick={() => db.settingsTabs.open("app", "license")}
                        >
                            <CircleDollarSignIcon class="size-4" />
                            Purchase License
                        </DropdownMenu.Item>
                    {/if}
                    {#if isTauri()}
                        <DropdownMenu.Item onclick={checkForUpdates} disabled={checkingForUpdates}>
                            <RefreshCwIcon class="size-4 {checkingForUpdates ? 'animate-spin' : ''}" />
                            {checkingForUpdates ? "Checking..." : "Check for Updates"}
                        </DropdownMenu.Item>
                    {/if}
                    <DropdownMenu.Item onclick={() => openExternal("https://github.com/webstonehq/seaquel/issues")}>
                        <MessageSquareTextIcon class="size-4" />
                        Feedback
                        <DropdownMenu.Shortcut><ExternalLinkIcon class="size-3" /></DropdownMenu.Shortcut>
                    </DropdownMenu.Item>
                    <DropdownMenu.Item onclick={() => openExternal("https://seaquel.app/changelog")}>
                        <BookOpenIcon class="size-4" />
                        Changelog
                        <DropdownMenu.Shortcut><ExternalLinkIcon class="size-3" /></DropdownMenu.Shortcut>
                    </DropdownMenu.Item>
                    {#if isWeb()}
                        <DropdownMenu.Separator />
                        <DropdownMenu.Item onclick={() => db.settingsTabs.open("app", "team")}>
                            <UsersIcon class="size-4" />
                            Team
                        </DropdownMenu.Item>
                        <DropdownMenu.Item
                            onclick={async () => {
                                await authClient.signOut();
                                window.location.href = "/login";
                            }}
                        >
                            <LogOutIcon class="size-4" />
                            Sign out
                        </DropdownMenu.Item>
                    {/if}
                </DropdownMenu.Content>
            </DropdownMenu.Root>
        </div>
    </div>
</header>

<!-- New Project Dialog -->
<Dialog.Root bind:open={showNewProjectDialog}>
    <Dialog.Content class="max-w-md">
        <Dialog.Header>
            <Dialog.Title>{m.project_new_dialog_title()}</Dialog.Title>
            <Dialog.Description>
                {m.project_new_dialog_description()}
            </Dialog.Description>
        </Dialog.Header>
        <div class="py-4">
            <input
                type="text"
                class="w-full px-3 py-2 border rounded-md bg-background"
                placeholder={m.project_name_placeholder()}
                bind:value={newProjectName}
                onkeydown={(e) => e.key === "Enter" && handleCreateProject()}
            />
        </div>
        <Dialog.Footer class="gap-2">
            <Button variant="outline" onclick={() => { showNewProjectDialog = false; newProjectName = ""; }}>
                {m.header_button_cancel()}
            </Button>
            <Button onclick={handleCreateProject} disabled={!newProjectName.trim()}>
                {m.project_create()}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>

<!-- Remove Project Dialog -->
<DeleteConfirmDialog
    bind:open={showRemoveProjectDialog}
    title={m.project_delete_dialog_title()}
    description={m.project_delete_dialog_description({ name: projectToRemoveName })}
    cancelText={m.header_button_cancel()}
    confirmText={m.project_delete_confirm()}
    onconfirm={handleRemoveProject}
/>

<!-- Import Shared Project Dialog -->
<ImportSharedProjectDialog />
