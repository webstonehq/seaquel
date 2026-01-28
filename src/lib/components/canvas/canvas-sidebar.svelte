<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { formatRelativeTime } from "$lib/utils.js";
	import { Button } from "$lib/components/ui/button";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import { Input } from "$lib/components/ui/input";
	import {
		ChevronRightIcon,
		PlusIcon,
		ClockIcon,
		SaveIcon,
		FileIcon,
		Trash2Icon,
		CodeIcon,
		PencilIcon,
	} from "@lucide/svelte";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import { m } from "$lib/paraglide/messages.js";

	const db = useDatabase();

	let savedExpanded = $state(true);
	let timelineExpanded = $state(true);
	let editingCanvasId = $state<string | null>(null);
	let editingName = $state("");

	const handleAddQueryNode = () => {
		db.canvas.addQueryNode();
	};

	const savedCanvases = $derived(db.state.savedCanvases);

	const handleLoadCanvas = (canvasId: string) => {
		db.canvas.loadCanvas(canvasId);
	};

	const handleDeleteCanvas = (canvasId: string) => {
		db.canvas.deleteCanvas(canvasId);
	};

	const handleNewCanvas = () => {
		db.canvas.clearCanvas();
	};

	const handleSaveCanvas = () => {
		// Use existing canvas name if updating, otherwise generate default name
		const activeCanvas = savedCanvases.find(c => c.id === db.canvasState.activeCanvasId);
		const name = activeCanvas?.name ?? `Canvas ${new Date().toLocaleString()}`;
		db.canvas.saveCanvas(name);
	};

	const startRename = (canvasId: string, currentName: string) => {
		editingCanvasId = canvasId;
		editingName = currentName;
	};

	const confirmRename = () => {
		if (editingCanvasId && editingName.trim()) {
			db.canvas.renameCanvas(editingCanvasId, editingName.trim());
		}
		editingCanvasId = null;
		editingName = "";
	};

	const cancelRename = () => {
		editingCanvasId = null;
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
		<span class="font-semibold text-sm">{m.canvas_title()}</span>
		<div class="flex items-center gap-1">
			<Button variant="ghost" size="icon" class="size-7" onclick={handleAddQueryNode} title={m.canvas_add_query_node()}>
				<CodeIcon class="size-4" />
			</Button>
			<Button variant="ghost" size="icon" class="size-7" onclick={handleSaveCanvas} title={m.canvas_save()}>
				<SaveIcon class="size-4" />
			</Button>
		</div>
	</div>

	<!-- Content -->
	<ScrollArea class="flex-1">
		<div class="p-2 space-y-2">
			<!-- Saved Canvases Section -->
			<Collapsible bind:open={savedExpanded}>
				<CollapsibleTrigger class="flex items-center gap-2 w-full p-2 hover:bg-muted/50 rounded-md">
					<ChevronRightIcon class="size-4 transition-transform {savedExpanded ? 'rotate-90' : ''}" />
					<SaveIcon class="size-4 text-muted-foreground" />
					<span class="text-sm font-medium flex-1 text-left">{m.canvas_saved()}</span>
					<span class="text-xs text-muted-foreground">{savedCanvases.length}</span>
				</CollapsibleTrigger>
				<CollapsibleContent>
					<div class="pl-6 space-y-0.5">
						<button
							class="flex items-center gap-2 w-full p-1.5 hover:bg-muted/50 rounded-md text-sm text-left text-muted-foreground"
							onclick={handleNewCanvas}
						>
							<PlusIcon class="size-3.5 shrink-0" />
							<span>{m.canvas_new()}</span>
						</button>

						{#each savedCanvases as canvas}
							{#if editingCanvasId === canvas.id}
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
											class="flex items-center gap-2 w-full p-1.5 hover:bg-muted/50 rounded-md text-sm text-left {db.canvasState.activeCanvasId === canvas.id ? 'bg-muted' : ''}"
											onclick={() => handleLoadCanvas(canvas.id)}
										>
											<FileIcon class="size-3.5 text-muted-foreground shrink-0" />
											<span class="truncate flex-1">{canvas.name}</span>
										</button>
									</ContextMenu.Trigger>
									<ContextMenu.Content>
										<ContextMenu.Item onclick={() => startRename(canvas.id, canvas.name)}>
											<PencilIcon class="size-4 mr-2" />
											{m.canvas_rename()}
										</ContextMenu.Item>
										<ContextMenu.Item onclick={() => handleDeleteCanvas(canvas.id)} class="text-destructive">
											<Trash2Icon class="size-4 mr-2" />
											{m.canvas_delete()}
										</ContextMenu.Item>
									</ContextMenu.Content>
								</ContextMenu.Root>
							{/if}
						{/each}

						{#if savedCanvases.length === 0}
							<div class="p-2 text-xs text-muted-foreground text-center">
								{m.canvas_no_saved()}
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
					<span class="text-sm font-medium flex-1 text-left">{m.canvas_timeline()}</span>
				</CollapsibleTrigger>
				<CollapsibleContent>
					<div class="pl-6 space-y-0.5 max-h-48 overflow-auto">
						{#each db.canvasState.timeline.slice(0, 20) as entry}
							<div class="flex items-start gap-2 p-1.5 text-xs">
								<span class="text-muted-foreground shrink-0">
									{formatRelativeTime(new Date(entry.timestamp))}
								</span>
								<span class="truncate">{entry.description}</span>
							</div>
						{/each}

						{#if db.canvasState.timeline.length === 0}
							<div class="p-2 text-xs text-muted-foreground text-center">
								{m.canvas_no_activity()}
							</div>
						{/if}
					</div>
				</CollapsibleContent>
			</Collapsible>
		</div>
	</ScrollArea>
</div>
