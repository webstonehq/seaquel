<script lang="ts">
    import SidebarIcon from "@lucide/svelte/icons/sidebar";
    import { Button } from "$lib/components/ui/button/index.js";
    import * as Sidebar from "$lib/components/ui/sidebar/index.js";
    import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
    import * as Dialog from "$lib/components/ui/dialog/index.js";
    import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
    import CheckIcon from "@lucide/svelte/icons/check";
    import { useDatabase } from "$lib/hooks/database.svelte.js";
    import ConnectionWizard from "$lib/components/connection-wizard/connection-wizard.svelte";
    import PlusIcon from "@lucide/svelte/icons/plus";
    import BotIcon from "@lucide/svelte/icons/bot";
    import NetworkIcon from "@lucide/svelte/icons/network";
    import ThemeToggle from "./theme-toggle.svelte";
    import LanguageToggle from "./language-toggle.svelte";
    import { m } from "$lib/paraglide/messages.js";
    import { DEFAULT_PROJECT_ID } from "$lib/types";

    const db = useDatabase();
    const sidebar = Sidebar.useSidebar();

    // Project management state
    let showNewProjectDialog = $state(false);
    let showEditProjectDialog = $state(false);
    let showRemoveProjectDialog = $state(false);
    let newProjectName = $state("");
    let editProjectName = $state("");
    let projectToEdit = $state<string | null>(null);
    let projectToRemove = $state<string | null>(null);
    let projectToRemoveName = $state("");

    const handleCreateProject = async () => {
        if (!newProjectName.trim()) return;
        await db.projects.add(newProjectName.trim());
        newProjectName = "";
        showNewProjectDialog = false;
    };

    const handleEditProject = async () => {
        if (!projectToEdit || !editProjectName.trim()) return;
        db.projects.update(projectToEdit, { name: editProjectName.trim() });
        projectToEdit = null;
        editProjectName = "";
        showEditProjectDialog = false;
    };

    const openEditDialog = (projectId: string, name: string) => {
        projectToEdit = projectId;
        editProjectName = name;
        showEditProjectDialog = true;
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
</script>

<header
    class="bg-background sticky top-0 z-50 flex w-lvw items-center border-b"
>
    <div
        data-tauri-drag-region
        class="pl-18 h-(--header-height) flex w-full items-center gap-2 pr-2 justify-between"
    >
        <div data-tauri-drag-region class="flex items-center gap-1 flex-1 min-w-0">
            <Button
                class="size-8 shrink-0"
                variant="ghost"
                size="icon"
                onclick={sidebar.toggle}
            >
                <SidebarIcon />
            </Button>
            <!-- Project Dropdown -->
            <DropdownMenu.Root>
                <DropdownMenu.Trigger class="flex items-center gap-2 px-3 h-8 text-sm rounded-md bg-background hover:bg-muted transition-colors">
                    <span class="max-w-40 truncate" title={db.state.activeProject?.name || m.project_default_name()}>
                        {db.state.activeProject?.name || m.project_default_name()}
                    </span>
                </DropdownMenu.Trigger>
                <DropdownMenu.Content class="w-56" align="start">
                    {#each db.state.projects as project (project.id)}
                        <ContextMenu.Root>
                            <ContextMenu.Trigger class="w-full">
                                <DropdownMenu.Item
                                    class="flex items-center gap-2 cursor-pointer"
                                    onclick={() => db.projects.setActive(project.id)}
                                >
                                    <span class="w-4">
                                        {#if db.state.activeProjectId === project.id}
                                            <CheckIcon class="size-4" />
                                        {/if}
                                    </span>
                                    <span class="flex-1 truncate">{project.name}</span>
                                </DropdownMenu.Item>
                            </ContextMenu.Trigger>
                            <ContextMenu.Content class="w-40">
                                <ContextMenu.Item onclick={() => openEditDialog(project.id, project.name)}>
                                    {m.project_edit()}
                                </ContextMenu.Item>
                                {#if project.id !== DEFAULT_PROJECT_ID && db.state.projects.length > 1}
                                    <ContextMenu.Separator />
                                    <ContextMenu.Item
                                        class="text-destructive focus:text-destructive"
                                        onclick={() => confirmRemoveProject(project.id, project.name)}
                                    >
                                        {m.project_delete()}
                                    </ContextMenu.Item>
                                {/if}
                            </ContextMenu.Content>
                        </ContextMenu.Root>
                    {/each}
                    <DropdownMenu.Separator />
                    <DropdownMenu.Item
                        class="flex items-center gap-2 cursor-pointer"
                        onclick={() => showNewProjectDialog = true}
                    >
                        <PlusIcon class="size-4" />
                        {m.project_new()}
                    </DropdownMenu.Item>
                </DropdownMenu.Content>
            </DropdownMenu.Root>
        </div>
        <div class="flex items-center gap-1">
            {#if db.state.activeConnection?.database || db.state.activeConnection?.mssqlConnectionId || db.state.activeConnection?.providerConnectionId}
                <Button
                    size="icon"
                    variant="ghost"
                    class="size-8"
                    title={m.header_view_erd()}
                    aria-label={m.header_view_erd()}
                    onclick={() => db.erdTabs.add()}
                >
                    <NetworkIcon class="size-5" />
                </Button>
                <Button
                    size="icon"
                    variant="ghost"
                    class="size-8"
                    aria-label={m.header_toggle_ai()}
                    onclick={() => db.ui.toggleAI()}
                >
                    <BotIcon class="size-5" />
                </Button>
            {/if}
            <LanguageToggle />
            <ThemeToggle />
        </div>
    </div>
</header>

<ConnectionWizard />

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

<!-- Edit Project Dialog -->
<Dialog.Root bind:open={showEditProjectDialog}>
    <Dialog.Content class="max-w-md">
        <Dialog.Header>
            <Dialog.Title>{m.project_edit_dialog_title()}</Dialog.Title>
        </Dialog.Header>
        <div class="py-4">
            <input
                type="text"
                class="w-full px-3 py-2 border rounded-md bg-background"
                placeholder={m.project_name_placeholder()}
                bind:value={editProjectName}
                onkeydown={(e) => e.key === "Enter" && handleEditProject()}
            />
        </div>
        <Dialog.Footer class="gap-2">
            <Button variant="outline" onclick={() => { showEditProjectDialog = false; editProjectName = ""; projectToEdit = null; }}>
                {m.header_button_cancel()}
            </Button>
            <Button onclick={handleEditProject} disabled={!editProjectName.trim()}>
                {m.project_save()}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>

<!-- Remove Project Dialog -->
<Dialog.Root bind:open={showRemoveProjectDialog}>
    <Dialog.Content class="max-w-md">
        <Dialog.Header>
            <Dialog.Title>{m.project_delete_dialog_title()}</Dialog.Title>
            <Dialog.Description>
                {m.project_delete_dialog_description({ name: projectToRemoveName })}
            </Dialog.Description>
        </Dialog.Header>
        <Dialog.Footer class="gap-2">
            <Button variant="outline" onclick={() => showRemoveProjectDialog = false}>
                {m.header_button_cancel()}
            </Button>
            <Button variant="destructive" onclick={handleRemoveProject}>
                {m.project_delete_confirm()}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
