<script lang="ts">
	import { untrack } from 'svelte';
	import type { DashboardWidget, ResolvedDashboardVersion } from '$lib/types';
	import { useDatabase } from '$lib/hooks/database.svelte.js';
	import { createDashboardSnapshot } from '$lib/utils/dashboard-versions';
	import DashboardToolbar from './dashboard-toolbar.svelte';
	import DashboardCanvas from './dashboard-canvas.svelte';
	import DashboardWidgetEditor from './dashboard-widget-editor.svelte';
	import DashboardDiffView from './dashboard-diff-view.svelte';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import Loader2Icon from '@lucide/svelte/icons/loader-2';
	import ChartBarIcon from '@lucide/svelte/icons/chart-bar';
	import GaugeIcon from '@lucide/svelte/icons/gauge';
	import TypeIcon from '@lucide/svelte/icons/type';
	import { isTauri } from '$lib/utils/environment';

	let { tabId: propTabId = undefined }: { tabId?: string } = $props();

	const db = useDatabase();

	const tab = $derived(
		propTabId
			? db.state.dashboardTabs.find(t => t.id === propTabId) ?? null
			: db.state.activeDashboardTab
	);

	const dashboard = $derived(
		tab ? db.dashboards.getDashboard(tab.dashboardId) : undefined
	);

	let widgetEditorOpen = $state(false);
	let editingWidget = $state<DashboardWidget | null>(null);
	let pendingWidget = $state<DashboardWidget | null>(null);
	const isFullscreen = $derived(db.state.isDashboardFullscreen);

	// Version history / diff mode
	const versions = $derived(
		dashboard ? db.dashboards.getResolvedVersionsForDashboard(dashboard.id) : []
	);
	let diffLeft = $state<ResolvedDashboardVersion | null>(null);
	let diffRight = $state<ResolvedDashboardVersion | null>(null);
	const diffMode = $derived(diffLeft !== null && diffRight !== null);

	let dashboardToolbar = $state<ReturnType<typeof DashboardToolbar> | undefined>();
	let diffView = $state<ReturnType<typeof DashboardDiffView> | undefined>();

	function handleDiffVersions(selected: ResolvedDashboardVersion[]) {
		if (selected.length === 0) {
			diffLeft = null;
			diffRight = null;
		} else if (selected.length === 1 && dashboard) {
			// Diff selected version against current dashboard state
			diffLeft = selected[0];
			diffRight = {
				id: 'current',
				dashboardId: dashboard.id,
				version: 0,
				dashboard: createDashboardSnapshot(dashboard),
				createdAt: dashboard.updatedAt,
			};
		} else if (selected.length === 2) {
			diffLeft = selected[0];
			diffRight = selected[1];
		}
	}

	function handleCloseDiff() {
		diffLeft = null;
		diffRight = null;
		dashboardToolbar?.clearVersionSelection();
	}

	function handleRestoreVersion(version: ResolvedDashboardVersion) {
		if (!dashboard) return;
		db.dashboards.restoreVersion(dashboard.id, version);
		handleCloseDiff();
	}

	async function getAppWindow() {
		if (!isTauri()) return null;
		const { getCurrentWebviewWindow } = await import('@tauri-apps/api/webviewWindow');
		return getCurrentWebviewWindow();
	}

	// Delete confirmation state
	let showDeleteWidgetDialog = $state(false);
	let widgetToDeleteId = $state<string | null>(null);

	// Context menu state
	let contextMenuOpen = $state(false);
	let contextMenuPosition = $state({ x: 0, y: 0 });
	let contextMenuCanvasPosition = $state({ x: 0, y: 0 });

	// Auto-create dashboard for new tabs without a dashboardId
	let creatingDashboard = false;
	$effect(() => {
		if (tab && !tab.dashboardId && !creatingDashboard) {
			creatingDashboard = true;
			// untrack to avoid re-triggering the effect when dashboardsByProject updates
			untrack(() => {
				void createDashboardForTab().finally(() => { creatingDashboard = false; });
			});
		}
	});

	// Refresh all widgets when the active connection changes
	let prevConnectionId = $state(db.state.activeConnectionId);
	$effect(() => {
		const connectionId = db.state.activeConnectionId;
		if (connectionId !== prevConnectionId) {
			prevConnectionId = connectionId;
			handleRefreshAll();
		}
	});

	async function createDashboardForTab() {
		if (!tab) return;
		const newDashboard = await db.dashboards.createDashboard(tab.name);
		if (newDashboard) {
			db.dashboardTabs.setDashboardId(tab.id, newDashboard.id);
		}
	}

	function createWidgetAtPosition(
		position: { x: number; y: number },
		widgetType: 'chart' | 'kpi' | 'text' = 'chart',
	): DashboardWidget {
		const defaultWidth = 400;
		const defaultHeight = 300;
		return {
			id: `widget-${crypto.randomUUID()}`,
			title: 'New Widget',
			x: position.x - defaultWidth / 2,
			y: position.y - defaultHeight / 2,
			width: defaultWidth,
			height: defaultHeight,
			querySource: 'custom',
			query: '',
			widgetType,
		};
	}

	function openEditorForNewWidget(widget: DashboardWidget) {
		editingWidget = null;
		pendingWidget = widget;
		widgetEditorOpen = true;
	}

	function handleAddWidget() {
		if (!dashboard) return;

		// Place widget at viewport center
		const vp = dashboard.viewport;
		const centerX = (-vp.x + 400) / vp.zoom;
		const centerY = (-vp.y + 300) / vp.zoom;

		openEditorForNewWidget(createWidgetAtPosition({ x: centerX, y: centerY }));
	}

	function handleAddWidgetAt(position: { x: number; y: number }) {
		openEditorForNewWidget(createWidgetAtPosition(position));
	}

	function handleCanvasContextMenu(event: MouseEvent, canvasPosition: { x: number; y: number }) {
		contextMenuPosition = { x: event.clientX, y: event.clientY };
		contextMenuCanvasPosition = canvasPosition;
		contextMenuOpen = true;
	}

	function handleContextMenuAdd(widgetType: 'chart' | 'kpi' | 'text') {
		openEditorForNewWidget(createWidgetAtPosition(contextMenuCanvasPosition, widgetType));
		contextMenuOpen = false;
	}

	function handleWindowClick() {
		if (contextMenuOpen) contextMenuOpen = false;
	}

	function handleEditWidget(widget: DashboardWidget) {
		editingWidget = widget;
		pendingWidget = null;
		widgetEditorOpen = true;
	}

	function handleDuplicateWidget(widget: DashboardWidget) {
		if (!dashboard) return;
		const duplicate: DashboardWidget = {
			...widget,
			id: `widget-${crypto.randomUUID()}`,
			title: `${widget.title} (copy)`,
			x: widget.x + 40,
			y: widget.y + 40,
		};
		db.dashboards.addWidget(dashboard.id, duplicate);
		db.dashboards.executeWidget(dashboard.id, duplicate.id);
	}

	function handleDeleteWidget(widgetId: string) {
		widgetToDeleteId = widgetId;
		showDeleteWidgetDialog = true;
	}

	function confirmDeleteWidget() {
		if (!dashboard || !widgetToDeleteId) return;
		if (editingWidget?.id === widgetToDeleteId) {
			handleWidgetEditorClose();
		}
		db.dashboards.removeWidget(dashboard.id, widgetToDeleteId);
		widgetToDeleteId = null;
		showDeleteWidgetDialog = false;
	}

	function handleRefreshAll() {
		if (!dashboard) return;
		db.dashboards.executeAllWidgets(dashboard.id);
		diffView?.refreshAll();
	}

	function handleWidgetSave(widget: DashboardWidget) {
		if (!dashboard) return;

		if (editingWidget) {
			db.dashboards.updateWidget(dashboard.id, editingWidget.id, widget);
			db.dashboards.executeWidget(dashboard.id, editingWidget.id);
		} else {
			db.dashboards.addWidget(dashboard.id, widget);
			db.dashboards.executeWidget(dashboard.id, widget.id);
		}

		if (widget.autoRefreshSeconds && widget.autoRefreshSeconds > 0) {
			db.dashboards.startAutoRefresh(dashboard.id, widget.id);
		}

		widgetEditorOpen = false;
		editingWidget = null;
		pendingWidget = null;
	}

	function handleWidgetEditorClose() {
		widgetEditorOpen = false;
		editingWidget = null;
		pendingWidget = null;
	}

	async function handleToggleFullscreen() {
		const appWindow = await getAppWindow();
		if (!appWindow) return;
		const newState = !isFullscreen;
		await appWindow.setFullscreen(newState);
		db.state.isDashboardFullscreen = newState;
	}

	async function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && isFullscreen) {
			const appWindow = await getAppWindow();
			if (!appWindow) return;
			await appWindow.setFullscreen(false);
			db.state.isDashboardFullscreen = false;
		}
	}
</script>

<svelte:window onkeydown={handleKeydown} onclick={handleWindowClick} />

<div class="flex h-full flex-col overflow-hidden">
	{#if !tab}
		<div class="flex h-full items-center justify-center text-muted-foreground">
			No dashboard selected
		</div>
	{:else if !dashboard}
		<div class="flex h-full items-center justify-center">
			<Loader2Icon class="size-6 animate-spin text-muted-foreground" />
		</div>
	{:else}
		<DashboardToolbar
			bind:this={dashboardToolbar}
			{dashboard}
			{isFullscreen}
			{versions}
			onAddWidget={handleAddWidget}
			onRefreshAll={handleRefreshAll}
			onToggleFullscreen={handleToggleFullscreen}
			onDiffVersions={handleDiffVersions}
		/>

		{#if diffMode && diffLeft && diffRight}
			<DashboardDiffView
				bind:this={diffView}
				left={diffLeft}
				right={diffRight}
				onClose={handleCloseDiff}
				onRestore={handleRestoreVersion}
			/>
		{:else}
			<div class="flex flex-1 min-h-0 overflow-hidden">
				<DashboardCanvas
					{dashboard}
					editingWidgetId={editingWidget?.id}
					onEditWidget={handleEditWidget}
					onDuplicateWidget={handleDuplicateWidget}
					onDeleteWidget={handleDeleteWidget}
					onAddWidgetAt={handleAddWidgetAt}
					onContextMenu={handleCanvasContextMenu}
				/>

				<!-- Widget editor sidebar -->
				<DashboardWidgetEditor
					open={widgetEditorOpen}
					widget={editingWidget}
					dashboardId={dashboard.id}
					initialWidget={pendingWidget}
					onClose={handleWidgetEditorClose}
					onSave={handleWidgetSave}
					onDelete={handleDeleteWidget}
				/>
			</div>
		{/if}
	{/if}
</div>

<AlertDialog.Root bind:open={showDeleteWidgetDialog}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Delete Widget</AlertDialog.Title>
			<AlertDialog.Description>
				Are you sure you want to delete this widget? This action cannot be undone.
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action onclick={confirmDeleteWidget} class="bg-destructive text-white hover:bg-destructive/90">
				Delete
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>

{#if contextMenuOpen}
	<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
	<div
		class="fixed z-50 min-w-[160px] rounded-md border bg-popover p-1 text-popover-foreground shadow-md"
		style="left: {contextMenuPosition.x}px; top: {contextMenuPosition.y}px;"
		onclick={(e: MouseEvent) => e.stopPropagation()}
	>
		<button
			class="flex w-full items-center rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
			onclick={() => handleContextMenuAdd('chart')}
		>
			<ChartBarIcon class="mr-2 size-4" />
			Add Chart
		</button>
		<button
			class="flex w-full items-center rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
			onclick={() => handleContextMenuAdd('kpi')}
		>
			<GaugeIcon class="mr-2 size-4" />
			Add KPI
		</button>
		<button
			class="flex w-full items-center rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
			onclick={() => handleContextMenuAdd('text')}
		>
			<TypeIcon class="mr-2 size-4" />
			Add Text
		</button>
	</div>
{/if}
