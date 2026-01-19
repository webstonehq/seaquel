<script lang="ts">
	import { Handle, Position, NodeResizer } from "@xyflow/svelte";
	import type { CanvasQueryNodeData } from "$lib/types/canvas";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import SuggestiveHandle from "../suggestive-handle.svelte";
	import CodeIcon from "@lucide/svelte/icons/code";
	import PlayIcon from "@lucide/svelte/icons/play";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import LoaderIcon from "@lucide/svelte/icons/loader";
	import AlertCircleIcon from "@lucide/svelte/icons/alert-circle";
	import PencilIcon from "@lucide/svelte/icons/pencil";
	import { Button } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
	import * as Popover from "$lib/components/ui/popover";
	import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";
	import MonacoEditor from "$lib/components/monaco-editor.svelte";

	interface Props {
		id: string;
		data: CanvasQueryNodeData;
		isConnectable?: boolean;
		selected?: boolean;
	}

	let { id, data, isConnectable = true, selected = false }: Props = $props();

	const db = useDatabase();

	let localQuery = $state(data.query);
	let editorOpen = $state(false);

	// Sync local query back to node data when it changes
	$effect(() => {
		if (localQuery !== data.query) {
			db.canvas.updateNodeData(id, { query: localQuery });
		}
	});

	// Update local when data changes externally
	$effect(() => {
		if (data.query !== localQuery) {
			localQuery = data.query;
		}
	});

	async function handleExecute() {
		await db.canvas.executeQueryNode(id);
	}

	async function handleExecuteAndClose() {
		editorOpen = false;
		await db.canvas.executeQueryNode(id);
	}

	function handleRemove() {
		db.canvas.removeNode(id);
	}

	function handleResizeEnd(_event: unknown, params: { width: number; height: number }) {
		db.canvas.updateNodeDimensions(id, params.width, params.height);
	}

	// Get preview lines for display
	const queryPreview = $derived.by(() => {
		if (!localQuery.trim()) return null;
		const lines = localQuery.split("\n").slice(0, 3);
		const preview = lines.join("\n");
		const hasMore = localQuery.split("\n").length > 3;
		return { text: preview, hasMore };
	});

	// Suggestions for the output handle (run query if we have one)
	const outputSuggestions = $derived(
		localQuery.trim() && !data.isExecuting
			? [
					{
						label: "Run Query",
						icon: PlayIcon,
						action: handleExecute,
					},
				]
			: []
	);
</script>

<div
	class="bg-card border border-border rounded-lg shadow-lg overflow-hidden h-full flex flex-col"
>
	<NodeResizer
		minWidth={250}
		minHeight={100}
		isVisible={selected}
		lineClass="!border-primary"
		handleClass="!bg-primary !border-primary"
		onResizeEnd={handleResizeEnd}
	/>

	<!-- Input handle -->
	<Handle
		type="target"
		position={Position.Left}
		{isConnectable}
		id="input"
		class="!w-3 !h-3 !bg-blue-500 !border-2 !border-background"
	/>

	<!-- Output handle -->
	<SuggestiveHandle
		nodeId={id}
		type="source"
		position={Position.Right}
		{isConnectable}
		id="output"
		suggestions={outputSuggestions}
		class="!w-3 !h-3 !bg-primary !border-2 !border-background"
	/>

	<!-- Header -->
	<div class="bg-muted/50 border-b border-border px-3 py-2 flex items-center justify-between">
		<div class="flex items-center gap-2 min-w-0 nodrag">
			<CodeIcon class="size-4 text-muted-foreground shrink-0" />
			<input
				type="text"
				value={data.name}
				onchange={(e) => db.canvas.updateNodeData(id, { name: e.currentTarget.value })}
				class="font-medium text-sm bg-transparent border-none outline-none focus:ring-1 focus:ring-primary rounded px-1 min-w-0"
			/>
		</div>

		<div class="flex items-center gap-1 shrink-0">
			{#if data.executionTime !== undefined}
				<span class="text-xs text-muted-foreground">
					{data.executionTime.toFixed(0)}ms
				</span>
			{/if}

			<Button
				variant="ghost"
				size="icon"
				class="size-7"
				onclick={handleExecute}
				disabled={data.isExecuting || !localQuery.trim()}
				title="Run query (Cmd+Enter)"
			>
				{#if data.isExecuting}
					<LoaderIcon class="size-4 animate-spin" />
				{:else}
					<PlayIcon class="size-4" />
				{/if}
			</Button>

			<DropdownMenu.Root>
				<DropdownMenu.Trigger>
					<Button variant="ghost" size="icon" class="size-7">
						<MoreHorizontalIcon class="size-4" />
					</Button>
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end">
					<DropdownMenu.Item onclick={handleRemove} class="text-destructive">
						<Trash2Icon class="size-4 mr-2" />
						Remove
					</DropdownMenu.Item>
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		</div>
	</div>

	<!-- Query Preview / Editor Trigger -->
	<Popover.Root bind:open={editorOpen}>
		<Popover.Trigger class="w-full nodrag nopan">
			<div
				class="p-3 text-left cursor-pointer hover:bg-muted/30 transition-colors min-h-[60px]"
			>
				{#if queryPreview}
					<pre class="text-xs font-mono text-foreground whitespace-pre-wrap break-all line-clamp-3">{queryPreview.text}{#if queryPreview.hasMore}<span class="text-muted-foreground">...</span>{/if}</pre>
				{:else}
					<div class="flex items-center gap-2 text-muted-foreground">
						<PencilIcon class="size-3.5" />
						<span class="text-xs">Click to write SQL...</span>
					</div>
				{/if}
			</div>
		</Popover.Trigger>
		<Popover.Content class="w-[550px] p-0" side="bottom" align="start" sideOffset={8}>
			<div class="flex flex-col">
				<!-- Popover Header -->
				<div class="flex items-center justify-between px-3 py-2 border-b border-border bg-muted/30">
					<span class="text-sm font-medium">Edit Query</span>
					<Button
						size="sm"
						class="h-7"
						onclick={handleExecuteAndClose}
						disabled={data.isExecuting || !localQuery.trim()}
					>
						{#if data.isExecuting}
							<LoaderIcon class="size-3.5 mr-1.5 animate-spin" />
						{:else}
							<PlayIcon class="size-3.5 mr-1.5" />
						{/if}
						Run
					</Button>
				</div>
				<!-- Monaco Editor -->
				<div class="h-[300px]">
					<MonacoEditor
						bind:value={localQuery}
						schema={db.state.activeSchema ?? []}
						onExecute={handleExecuteAndClose}
					/>
				</div>
			</div>
		</Popover.Content>
	</Popover.Root>

	<!-- Error display -->
	{#if data.error}
		<div class="px-3 py-2 bg-destructive/10 border-t border-destructive/20 flex items-start gap-2">
			<AlertCircleIcon class="size-4 text-destructive shrink-0 mt-0.5" />
			<span class="text-xs text-destructive line-clamp-2">{data.error}</span>
		</div>
	{/if}
</div>
