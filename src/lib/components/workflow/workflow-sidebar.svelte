<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { formatRelativeTime } from "$lib/utils.js";
	import { Button } from "$lib/components/ui/button";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { Input } from "$lib/components/ui/input";
	import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import ClockIcon from "@lucide/svelte/icons/clock";
	import SaveIcon from "@lucide/svelte/icons/save";
	import FileIcon from "@lucide/svelte/icons/file";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import CodeIcon from "@lucide/svelte/icons/code";
	import PencilIcon from "@lucide/svelte/icons/pencil";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import { m } from "$lib/paraglide/messages.js";

	const db = useDatabase();

	let savedExpanded = $state(true);
	let timelineExpanded = $state(true);
	let editingWorkflowId = $state<string | null>(null);
	let editingName = $state("");

	const handleAddQueryNode = () => {
		db.workflow.addQueryNode();
	};

	const savedWorkflows = $derived(db.state.savedWorkflows);

	const handleLoadWorkflow = (workflowId: string) => {
		// Reads the workflow; one that can't be read is said by the manager.
		void db.workflow.loadWorkflow(workflowId);
	};

	const handleDeleteWorkflow = (workflowId: string) => {
		// A failure is shown by the manager.
		void db.workflow.deleteWorkflow(workflowId);
	};

	const handleNewWorkflow = () => {
		db.workflow.clearWorkflow();
	};

	const handleSaveWorkflow = () => {
		// The workflow keeps its own name (a new one gets a default); Core
		// stores it, and a refusal (too large on this server, say) is shown
		// while the canvas stays.
		void db.workflow.saveWorkflow();
	};

	const startRename = (workflowId: string, currentName: string) => {
		editingWorkflowId = workflowId;
		editingName = currentName;
	};

	const confirmRename = () => {
		if (editingWorkflowId && editingName.trim()) {
			void db.workflow.renameWorkflow(editingWorkflowId, editingName.trim());
		}
		editingWorkflowId = null;
		editingName = "";
	};

	const cancelRename = () => {
		editingWorkflowId = null;
		editingName = "";
	};

	const handleRenameKeydown = (e: KeyboardEvent) => {
		if (e.key === "Enter") {
			confirmRename();
		} else if (e.key === "Escape") {
			cancelRename();
		}
	};
</script>

<div class="w-64 border-r border-border bg-sidebar flex flex-col h-full">
	<!-- Header -->
	<div class="p-3 border-b border-border flex items-center justify-between">
		<span class="font-semibold text-sm">{m.workflow_title()}</span>
		<div class="flex items-center gap-1">
			<Button variant="ghost" size="icon" class="size-7" onclick={handleAddQueryNode} title={m.workflow_add_query_node()}>
				<CodeIcon class="size-4" />
			</Button>
			<Button variant="ghost" size="icon" class="size-7" onclick={handleSaveWorkflow} title={m.workflow_save()}>
				<SaveIcon class="size-4" />
			</Button>
		</div>
	</div>

	<!-- Content -->
	<ScrollArea class="flex-1">
		<div class="p-2 space-y-2">
			<!-- Saved Workflows Section -->
			<Collapsible bind:open={savedExpanded}>
				<CollapsibleTrigger class="flex items-center gap-2 w-full p-2 hover:bg-muted/50 rounded-md">
					<ChevronRightIcon class="size-4 transition-transform {savedExpanded ? 'rotate-90' : ''}" />
					<SaveIcon class="size-4 text-muted-foreground" />
					<span class="text-sm font-medium flex-1 text-left">{m.workflow_saved()}</span>
					<span class="text-xs text-muted-foreground">{savedWorkflows.length}</span>
				</CollapsibleTrigger>
				<CollapsibleContent>
					<div class="pl-6 space-y-0.5">
						<button
							class="flex items-center gap-2 w-full p-1.5 hover:bg-muted/50 rounded-md text-sm text-left text-muted-foreground"
							onclick={handleNewWorkflow}
						>
							<PlusIcon class="size-3.5 shrink-0" />
							<span>{m.workflow_new()}</span>
						</button>

						{#each savedWorkflows as workflow (workflow.id)}
							{#if editingWorkflowId === workflow.id}
								<div class="flex items-center gap-2 w-full p-1.5">
									<FileIcon class="size-3.5 text-muted-foreground shrink-0" />
									<Input
										class="h-6 text-sm flex-1"
										bind:value={editingName}
										onkeydown={handleRenameKeydown}
										onblur={confirmRename}
										autofocus
									/>
								</div>
							{:else}
								<ContextMenu.Root>
									<ContextMenu.Trigger>
										<button
											class="flex items-center gap-2 w-full p-1.5 hover:bg-muted/50 rounded-md text-sm text-left {db.workflowState.activeWorkflowId === workflow.id ? 'bg-muted' : ''}"
											onclick={() => handleLoadWorkflow(workflow.id)}
										>
											<FileIcon class="size-3.5 text-muted-foreground shrink-0" />
											<span class="truncate flex-1">{workflow.name}</span>
										</button>
									</ContextMenu.Trigger>
									<ContextMenu.Content>
										<ContextMenu.Item onclick={() => startRename(workflow.id, workflow.name)}>
											<PencilIcon class="size-4 mr-2" />
											{m.workflow_rename()}
										</ContextMenu.Item>
										<ContextMenu.Item onclick={() => handleDeleteWorkflow(workflow.id)} class="text-destructive">
											<Trash2Icon class="size-4 mr-2" />
											{m.workflow_delete()}
										</ContextMenu.Item>
									</ContextMenu.Content>
								</ContextMenu.Root>
							{/if}
						{/each}

						{#if savedWorkflows.length === 0}
							<div class="p-2 text-xs text-muted-foreground text-center">
								{m.workflow_no_saved()}
							</div>
						{/if}
					</div>
				</CollapsibleContent>
			</Collapsible>

			<!-- Timeline Section -->
			<Collapsible bind:open={timelineExpanded}>
				<CollapsibleTrigger class="flex items-center gap-2 w-full p-2 hover:bg-muted/50 rounded-md">
					<ChevronRightIcon class="size-4 transition-transform {timelineExpanded ? 'rotate-90' : ''}" />
					<ClockIcon class="size-4 text-muted-foreground" />
					<span class="text-sm font-medium flex-1 text-left">{m.workflow_timeline()}</span>
				</CollapsibleTrigger>
				<CollapsibleContent>
					<div class="pl-6 space-y-0.5 max-h-48 overflow-auto">
						{#each db.workflowState.timeline.slice(0, 20) as entry (entry.id)}
							<div class="flex items-start gap-2 p-1.5 text-xs">
								<span class="text-muted-foreground shrink-0">
									{formatRelativeTime(new Date(entry.timestamp))}
								</span>
								<span class="truncate">{entry.description}</span>
							</div>
						{/each}

						{#if db.workflowState.timeline.length === 0}
							<div class="p-2 text-xs text-muted-foreground text-center">
								{m.workflow_no_activity()}
							</div>
						{/if}
					</div>
				</CollapsibleContent>
			</Collapsible>
		</div>
	</ScrollArea>
</div>
