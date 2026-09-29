<script lang="ts">
	import type { Dashboard, DashboardWidget } from '$lib/types';
	import { useDatabase } from '$lib/hooks/database.svelte.js';
	import { SvelteFlow, Background, Controls, MiniMap, type ColorMode, type Node, type Viewport } from '@xyflow/svelte';
	import LayoutDashboardIcon from '@lucide/svelte/icons/layout-dashboard';
	import '@xyflow/svelte/dist/style.css';
	import { mode } from 'mode-watcher';
	import { dashboardNodeTypes } from './nodes';

	interface Props {
		dashboard: Dashboard;
		editingWidgetId?: string | null;
		readonly?: boolean;
		onEditWidget?: (widget: DashboardWidget) => void;
		onDuplicateWidget?: (widget: DashboardWidget) => void;
		onDeleteWidget?: (widgetId: string) => void;
		onAddWidgetAt?: (position: { x: number; y: number }) => void;
		onContextMenu?: (event: MouseEvent, position: { x: number; y: number }) => void;
	}

	let { dashboard, editingWidgetId = null, readonly = false, onEditWidget, onDuplicateWidget, onDeleteWidget, onAddWidgetAt, onContextMenu }: Props = $props();

	const db = useDatabase();

	const colorMode: ColorMode = $derived(mode.current === 'dark' ? 'dark' : 'light');

	const noop = () => {};

	function handleRefreshWidget(widgetId: string) {
		if (readonly) return;
		db.dashboards.executeWidget(dashboard.id, widgetId);
	}

	function handleResizeEnd(widgetId: string, size: { width: number; height: number }) {
		if (readonly) return;
		db.dashboards.resizeWidget(dashboard.id, widgetId, size);
	}

	const nodes: Node[] = $derived(
		dashboard.widgets.map((widget) => ({
			id: widget.id,
			type: 'dashboardWidget',
			position: { x: widget.x, y: widget.y },
			width: widget.width,
			height: widget.height,
			data: {
				widget,
				onEditWidget: readonly ? noop : onEditWidget,
				onRefreshWidget: readonly ? noop : handleRefreshWidget,
				onDuplicateWidget: readonly ? noop : onDuplicateWidget,
				onDeleteWidget: readonly ? noop : onDeleteWidget,
				onResizeEnd: readonly ? noop : handleResizeEnd,
			},
			selected: !readonly && editingWidgetId === widget.id,
		}))
	);

	function handleNodeDragStop({ targetNode }: { targetNode: Node | null; nodes: Node[]; event: MouseEvent | TouchEvent }) {
		if (readonly || !targetNode) return;
		const dashboardId = dashboard.id;
		db.dashboards.moveWidget(dashboardId, targetNode.id, {
			x: targetNode.position.x,
			y: targetNode.position.y,
		});
	}

	const getInitialViewport = () => dashboard.viewport;
	const initialViewport = getInitialViewport();
	let viewport: Viewport = $state(initialViewport);

	let viewportDebounceTimer: ReturnType<typeof setTimeout> | undefined;

	const dashboardId = $derived(dashboard.id);

	$effect(() => {
		if (readonly) return;
		// Track viewport changes for debounced persistence
		const vp = viewport;
		const id = dashboardId;
		clearTimeout(viewportDebounceTimer);
		viewportDebounceTimer = setTimeout(() => {
			db.dashboards.updateViewport(id, {
				x: vp.x,
				y: vp.y,
				zoom: vp.zoom,
			});
		}, 500);
	});

	function screenToCanvas(event: MouseEvent): { x: number; y: number } {
		return {
			x: (event.clientX - viewport.x) / viewport.zoom,
			y: (event.clientY - viewport.y) / viewport.zoom,
		};
	}

	// Double-click detection on pane
	let lastPaneClickTime = 0;

	function handlePaneClick({ event }: { event: MouseEvent }) {
		if (readonly) return;
		const now = Date.now();
		if (now - lastPaneClickTime < 400) {
			onAddWidgetAt?.(screenToCanvas(event));
			lastPaneClickTime = 0;
		} else {
			lastPaneClickTime = now;
		}
	}

	function handlePaneContextMenu({ event }: { event: MouseEvent }) {
		if (readonly) return;
		event.preventDefault();
		onContextMenu?.(event, screenToCanvas(event));
	}
</script>

<div class="flex-1 min-h-0 w-full relative">
	<SvelteFlow
		{nodes}
		edges={[]}
		nodeTypes={dashboardNodeTypes}
		{colorMode}
		{initialViewport}
		bind:viewport
		minZoom={0.1}
		maxZoom={2}
		nodesDraggable={!readonly}
		nodesConnectable={false}
		elementsSelectable={!readonly}
		deleteKey={null}
		onnodedragstop={handleNodeDragStop}
		onpaneclick={handlePaneClick}
		onpanecontextmenu={handlePaneContextMenu}
		proOptions={{ hideAttribution: true }}
		panOnScroll={true}
		snapGrid={undefined}
		zoomOnScroll={false}
	>
		<Background />
		<Controls />
		<MiniMap />
	</SvelteFlow>

	{#if !readonly && dashboard.widgets.length === 0}
		<div class="absolute inset-0 flex items-center justify-center pointer-events-none z-[1]">
			<div class="text-center text-muted-foreground">
				<LayoutDashboardIcon class="size-12 mx-auto mb-3 opacity-20" />
				<p class="text-sm">Double-click to add a widget</p>
				<p class="text-xs mt-1 opacity-70">Or right-click for more options</p>
			</div>
		</div>
	{/if}
</div>
