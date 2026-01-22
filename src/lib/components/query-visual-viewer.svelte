<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { SvelteFlow, Background, Controls, MiniMap } from "@xyflow/svelte";
	import "@xyflow/svelte/dist/style.css";
	import { toPng } from "html-to-image";
	import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
	import { save } from "@tauri-apps/plugin-dialog";
	import { writeFile } from "@tauri-apps/plugin-fs";
	import { Button } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
	import * as Popover from "$lib/components/ui/popover";
	import { Label } from "$lib/components/ui/label";
	import { Input } from "$lib/components/ui/input";
	import {
		NetworkIcon,
		DownloadIcon,
		ImageIcon,
		AlertCircleIcon,
		Code2Icon,
		SettingsIcon,
		ArrowDownIcon,
		ArrowUpIcon,
		ArrowRightIcon,
		ArrowLeftIcon
	} from "@lucide/svelte";
	import {
		TableSourceNode,
		JoinNode,
		FilterNode,
		GroupNode,
		ProjectionNode,
		SortNode,
		LimitNode
	} from "./query-visual/nodes";
	import {
		layoutQueryVisualization,
		DEFAULT_LAYOUT_OPTIONS,
		type QueryLayoutOptions,
		type LayoutDirection
	} from "$lib/utils/query-visual-layout";
	import type { Node, Edge, NodeTypes, ColorMode } from "@xyflow/svelte";
	import { mode } from "mode-watcher";

	const db = useDatabase();

	// Map mode-watcher theme to xyflow colorMode
	const colorMode: ColorMode = $derived(mode.current === "dark" ? "dark" : "light");

	// Layout options state
	let layoutOptions = $state<QueryLayoutOptions>({ ...DEFAULT_LAYOUT_OPTIONS });

	// Custom node types
	const nodeTypes: NodeTypes = {
		tableSourceNode: TableSourceNode,
		joinNode: JoinNode,
		filterNode: FilterNode,
		groupNode: GroupNode,
		projectionNode: ProjectionNode,
		sortNode: SortNode,
		limitNode: LimitNode
	};

	// Compute flow data from active visualize tab
	const flowData = $derived.by(() => {
		const tab = db.state.activeVisualizeTab;
		if (!tab?.parsedQuery) {
			return { nodes: [] as Node[], edges: [] as Edge[] };
		}
		return layoutQueryVisualization(tab.parsedQuery, layoutOptions);
	});

	let nodes = $derived(flowData.nodes);
	let edges = $derived(flowData.edges);

	// Export functionality
	let flowContainer: HTMLElement;

	const getFlowElement = () => flowContainer?.querySelector('.svelte-flow') as HTMLElement | null;

	const getImageOptions = () => ({
		backgroundColor: mode.current === 'dark' ? '#0a0a0a' : '#ffffff',
		filter: (node: Element) => {
			return !node.classList?.contains('svelte-flow__minimap') &&
				!node.classList?.contains('svelte-flow__controls');
		}
	});

	const exportToPng = async () => {
		const element = getFlowElement();
		if (!element) return;

		try {
			const filePath = await save({
				defaultPath: `query-visual-${Date.now()}.png`,
				filters: [{ name: 'PNG Image', extensions: ['png'] }]
			});
			if (!filePath) return;

			const dataUrl = await toPng(element, getImageOptions());
			const base64 = dataUrl.split(',')[1];
			const bytes = Uint8Array.from(atob(base64), c => c.charCodeAt(0));
			await writeFile(filePath, bytes);
			toast.success('PNG saved');
		} catch (e) {
			errorToast('Failed to export PNG');
		}
	};

	// Truncate query for display
	const queryPreview = $derived.by(() => {
		const query = db.state.activeVisualizeTab?.sourceQuery || '';
		if (query.length <= 100) return query;
		return query.substring(0, 100) + '...';
	});

	// Direction options
	const directions: { value: LayoutDirection; label: string; icon: typeof ArrowDownIcon }[] = [
		{ value: 'TB', label: 'Top to Bottom', icon: ArrowDownIcon },
		{ value: 'BT', label: 'Bottom to Top', icon: ArrowUpIcon },
		{ value: 'LR', label: 'Left to Right', icon: ArrowRightIcon },
		{ value: 'RL', label: 'Right to Left', icon: ArrowLeftIcon }
	];

	const setDirection = (dir: LayoutDirection) => {
		layoutOptions = { ...layoutOptions, direction: dir };
	};

	const resetLayout = () => {
		layoutOptions = { ...DEFAULT_LAYOUT_OPTIONS };
	};
</script>

<div class="flex flex-col h-full">
	{#if db.state.activeVisualizeTab}
		{#if db.state.activeVisualizeTab.parsedQuery}
			<!-- Summary Header -->
			<div class="p-4 border-b bg-muted/30 shrink-0">
				<div class="flex items-start justify-between mb-3">
					<div>
						<h2 class="text-lg font-semibold flex items-center gap-2">
							<NetworkIcon class="size-5" />
							Query Visualization
						</h2>
						<p class="text-sm text-muted-foreground mt-1 max-w-2xl">
							Visual breakdown of the SQL query structure
						</p>
					</div>
				</div>

				<!-- Query Preview and Controls -->
				<div class="flex items-center gap-2 flex-wrap">
					<div class="flex items-center gap-2 text-xs bg-muted px-3 py-1.5 rounded-md max-w-xl">
						<Code2Icon class="size-3 shrink-0 text-muted-foreground" />
						<code class="font-mono truncate" title={db.state.activeVisualizeTab.sourceQuery}>
							{queryPreview}
						</code>
					</div>

					<!-- Layout Settings Popover -->
					<Popover.Root>
						<Popover.Trigger>
							<Button variant="outline" size="sm" class="h-8">
								<SettingsIcon class="size-4 me-2" />
								Layout
							</Button>
						</Popover.Trigger>
						<Popover.Content class="w-72" align="start">
							<div class="space-y-4">
								<div class="flex items-center justify-between">
									<h4 class="font-medium text-sm">Layout Settings</h4>
									<Button variant="ghost" size="sm" class="h-6 text-xs" onclick={resetLayout}>
										Reset
									</Button>
								</div>

								<!-- Direction -->
								<div class="space-y-2">
									<Label class="text-xs text-muted-foreground">Direction</Label>
									<div class="grid grid-cols-4 gap-1">
										{#each directions as dir}
											<Button
												variant={layoutOptions.direction === dir.value ? "default" : "outline"}
												size="sm"
												class="h-8 px-2"
												onclick={() => setDirection(dir.value)}
												title={dir.label}
											>
												<dir.icon class="size-4" />
											</Button>
										{/each}
									</div>
								</div>

								<!-- Node Spacing -->
								<div class="space-y-2">
									<Label class="text-xs text-muted-foreground">Node Spacing (px)</Label>
									<Input
										type="number"
										min={20}
										max={200}
										step={10}
										value={layoutOptions.nodeSpacing}
										oninput={(e) => {
											const val = parseInt(e.currentTarget.value, 10);
											if (!isNaN(val) && val >= 20 && val <= 200) {
												layoutOptions = { ...layoutOptions, nodeSpacing: val };
											}
										}}
										class="h-8"
									/>
								</div>

								<!-- Rank Spacing -->
								<div class="space-y-2">
									<Label class="text-xs text-muted-foreground">Level Spacing (px)</Label>
									<Input
										type="number"
										min={40}
										max={300}
										step={10}
										value={layoutOptions.rankSpacing}
										oninput={(e) => {
											const val = parseInt(e.currentTarget.value, 10);
											if (!isNaN(val) && val >= 40 && val <= 300) {
												layoutOptions = { ...layoutOptions, rankSpacing: val };
											}
										}}
										class="h-8"
									/>
								</div>
							</div>
						</Popover.Content>
					</Popover.Root>

					<!-- Export Dropdown -->
					<DropdownMenu.Root>
						<DropdownMenu.Trigger>
							<Button variant="outline" size="sm" class="h-8">
								<DownloadIcon class="size-4 me-2" />
								Export
							</Button>
						</DropdownMenu.Trigger>
						<DropdownMenu.Content>
							<DropdownMenu.Item onclick={exportToPng}>
								<ImageIcon class="size-4 me-2" />
								Download PNG
							</DropdownMenu.Item>
						</DropdownMenu.Content>
					</DropdownMenu.Root>
				</div>
			</div>

			<!-- Flow Diagram -->
			<div class="flex-1 min-h-0" bind:this={flowContainer}>
				{#if nodes.length > 0}
					<SvelteFlow
						{nodes}
						{edges}
						{nodeTypes}
						{colorMode}
						fitView
						minZoom={0.1}
						maxZoom={2}
						nodesDraggable={true}
						nodesConnectable={false}
						elementsSelectable={true}
						deleteKey={null}
						proOptions={{ hideAttribution: true }}
					>
						<Background />
						<Controls />
						<MiniMap />
					</SvelteFlow>
				{:else}
					<div class="h-full flex items-center justify-center text-muted-foreground">
						<div class="text-center">
							<NetworkIcon class="size-12 mx-auto mb-2 opacity-20" />
							<p class="text-sm">Unable to visualize query structure</p>
						</div>
					</div>
				{/if}
			</div>
		{:else if db.state.activeVisualizeTab.parseError}
			<!-- Parse error state -->
			<div class="flex-1 flex items-center justify-center text-muted-foreground">
				<div class="text-center max-w-md">
					<AlertCircleIcon class="size-12 mx-auto mb-4 text-destructive opacity-50" />
					<h3 class="text-lg font-semibold mb-2">Could not parse query</h3>
					<p class="text-sm mb-4">{db.state.activeVisualizeTab.parseError}</p>
					<div class="bg-muted p-3 rounded-md">
						<code class="text-xs font-mono break-all text-left block">
							{db.state.activeVisualizeTab.sourceQuery}
						</code>
					</div>
				</div>
			</div>
		{:else}
			<!-- Loading/empty state -->
			<div class="flex-1 flex items-center justify-center text-muted-foreground">
				<div class="text-center">
					<NetworkIcon class="size-12 mx-auto mb-2 opacity-20" />
					<p class="text-sm">Processing query...</p>
				</div>
			</div>
		{/if}
	{:else}
		<!-- No tab selected state -->
		<div class="flex-1 flex items-center justify-center text-muted-foreground">
			<div class="text-center">
				<NetworkIcon class="size-12 mx-auto mb-2 opacity-20" />
				<p class="text-sm">No visualization selected</p>
			</div>
		</div>
	{/if}
</div>
