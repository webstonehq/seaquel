<script lang="ts">
	import { untrack } from 'svelte';
	import type { Dashboard, ResolvedDashboardVersion, DashboardWidget } from '$lib/types';
	import DashboardCanvas from './dashboard-canvas.svelte';
	import { useDatabase } from '$lib/hooks/database.svelte.js';
	import { Button } from '$lib/components/ui/button';
	import { XIcon, RotateCcwIcon } from '@lucide/svelte';

	interface Props {
		left: ResolvedDashboardVersion;
		right: ResolvedDashboardVersion;
		onClose: () => void;
		onRestore: (version: ResolvedDashboardVersion) => void;
	}

	let { left, right, onClose, onRestore }: Props = $props();

	const db = useDatabase();

	function formatRelativeTime(date: Date): string {
		const now = Date.now();
		const diff = now - date.getTime();
		const seconds = Math.floor(diff / 1000);
		if (seconds < 60) return `${seconds}s ago`;
		const minutes = Math.floor(seconds / 60);
		if (minutes < 60) return `${minutes}m ago`;
		const hours = Math.floor(minutes / 60);
		if (hours < 24) return `${hours}h ago`;
		const days = Math.floor(hours / 24);
		return `${days}d ago`;
	}

	/**
	 * Build a synthetic Dashboard object from a version snapshot for the read-only canvas.
	 * Widgets are cloned so we can mutate runtime state without affecting the snapshot.
	 */
	function snapshotToDashboard(version: ResolvedDashboardVersion): Dashboard {
		return {
			id: `diff-${version.id}`,
			name: version.dashboard.name,
			description: version.dashboard.description,
			projectId: '',
			widgets: (version.dashboard.widgets as DashboardWidget[]).map(w => ({ ...w })),
			viewport: version.dashboard.viewport,
			dateFilter: version.dashboard.dateFilter,
			createdAt: version.createdAt,
			updatedAt: version.createdAt,
			shared: false,
		};
	}

	const isConnected = $derived(!!db.state.activeConnection?.providerConnectionId);

	async function executeWidgetsForDashboard(dashboard: Dashboard) {
		if (!isConnected) return;

		const dateFilter = dashboard.dateFilter;
		const projectId = db.state.activeProjectId;
		const savedQueries = projectId ? (db.state.queriesByProject[projectId] ?? []) : [];

		await Promise.all(
			dashboard.widgets.map(async (widget) => {
				if (widget.widgetType === 'text') return;

				let query = widget.query;
				if (widget.querySource === 'saved' && widget.savedQueryId) {
					const savedQuery = savedQueries.find(sq => sq.id === widget.savedQueryId);
					if (savedQuery) query = savedQuery.query;
				}

				if (!query) return;

				if (dateFilter) {
					const isValidDate = (val: string) => /^[\d\-T:.Z ]+$/.test(val);
					const escapeDate = (val: string) => `'${val.replace(/'/g, "''")}'`;
					if (isValidDate(dateFilter.start) && isValidDate(dateFilter.end)) {
						query = query
							.replace(/\{\{start_date\}\}/g, escapeDate(dateFilter.start))
							.replace(/\{\{end_date\}\}/g, escapeDate(dateFilter.end));
					}
				}

				widget.isLoading = true;
				try {
					widget.result = await db.dashboards.runWidgetQuery(query);
					widget.isLoading = false;
					widget.lastRefreshed = new Date();
				} catch (error) {
					widget.isLoading = false;
					widget.error = error instanceof Error ? error.message : 'Query execution failed';
				}
			})
		);
	}

	const isCurrent = (version: ResolvedDashboardVersion) => version.id === 'current';
	const versionLabel = (version: ResolvedDashboardVersion) =>
		isCurrent(version) ? 'Current' : `Version ${version.version}`;

	let leftDashboard: Dashboard | null = $state(null);
	let rightDashboard: Dashboard | null = $state(null);

	export function refreshAll() {
		if (leftDashboard) executeWidgetsForDashboard(leftDashboard);
		if (rightDashboard) executeWidgetsForDashboard(rightDashboard);
	}

	$effect(() => {
		// Track props and connection state as dependencies
		const _connected = isConnected;
		const l = snapshotToDashboard(left);
		const r = snapshotToDashboard(right);
		leftDashboard = l;
		rightDashboard = r;
		// Read the proxies inside untrack to avoid circular dependency
		untrack(() => {
			executeWidgetsForDashboard(leftDashboard!);
			executeWidgetsForDashboard(rightDashboard!);
		});
	});
</script>

<div class="flex h-full flex-col overflow-hidden">
	<!-- Header -->
	<div class="flex items-center justify-between border-b bg-muted/30 px-3 py-2">
		<span class="text-sm font-medium">Comparing Versions</span>
		<Button variant="ghost" size="icon" class="size-7" onclick={onClose}>
			<XIcon class="size-3.5" />
		</Button>
	</div>

	<!-- Side-by-side canvases -->
	{#if leftDashboard && rightDashboard}
		<div class="flex flex-1 min-h-0">
			<!-- Left version -->
			<div class="flex flex-1 flex-col border-r min-w-0">
				<div class="flex items-center justify-between border-b bg-muted/20 px-3 py-1.5">
					<div class="flex items-center gap-2">
						<span class="text-xs font-medium">{versionLabel(left)}</span>
						<span class="text-xs text-muted-foreground">{formatRelativeTime(left.createdAt)}</span>
						<span class="text-xs text-muted-foreground">
							({left.dashboard.widgets.length} widget{left.dashboard.widgets.length !== 1 ? 's' : ''})
						</span>
					</div>
					<Button variant="ghost" size="sm" class="h-6 text-xs gap-1 {isCurrent(left) ? 'invisible' : ''}" onclick={() => onRestore(left)}>
							<RotateCcwIcon class="size-3" />
							Restore
						</Button>
				</div>
				<DashboardCanvas dashboard={leftDashboard} readonly />
			</div>

			<!-- Right version -->
			<div class="flex flex-1 flex-col min-w-0">
				<div class="flex items-center justify-between border-b bg-muted/20 px-3 py-1.5">
					<div class="flex items-center gap-2">
						<span class="text-xs font-medium">{versionLabel(right)}</span>
						<span class="text-xs text-muted-foreground">{formatRelativeTime(right.createdAt)}</span>
						<span class="text-xs text-muted-foreground">
							({right.dashboard.widgets.length} widget{right.dashboard.widgets.length !== 1 ? 's' : ''})
						</span>
					</div>
					<Button variant="ghost" size="sm" class="h-6 text-xs gap-1 {isCurrent(right) ? 'invisible' : ''}" onclick={() => onRestore(right)}>
							<RotateCcwIcon class="size-3" />
							Restore
						</Button>
				</div>
				<DashboardCanvas dashboard={rightDashboard} readonly />
			</div>
		</div>
	{/if}
</div>
