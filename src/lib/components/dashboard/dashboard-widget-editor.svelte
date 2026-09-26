<script lang="ts">
	import type { DashboardWidget, ChartConfig } from '$lib/types';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
	import { XIcon } from '@lucide/svelte';
	import { useDatabase } from '$lib/hooks/database.svelte.js';
	import WidgetQueryEditor from './widget-query-editor.svelte';
	import WidgetChartConfig from './widget-chart-config.svelte';
	import QueryChart from '$lib/components/charts/query-chart.svelte';
	import DashboardKpiWidget from './dashboard-kpi-widget.svelte';
	import DashboardTextWidget from './dashboard-text-widget.svelte';
	import { createDefaultChartConfig } from '$lib/components/charts/chart-utils';

	interface Props {
		open: boolean;
		widget: DashboardWidget | null;
		dashboardId: string;
		initialWidget?: DashboardWidget | null;
		onClose: () => void;
		onSave: (widget: DashboardWidget) => void;
		onDelete?: (widgetId: string) => void;
	}

	let { open, widget, dashboardId, initialWidget = null, onClose, onSave, onDelete }: Props = $props();

	const db = useDatabase();

	// Editable state
	let title = $state('');
	let description = $state('');
	let widgetType = $state<'chart' | 'kpi' | 'text'>('chart');
	let querySource = $state<'custom' | 'saved'>('custom');
	let query = $state('');
	let savedQueryId = $state<string | undefined>(undefined);
	let chartConfig = $state<ChartConfig>({ type: 'bar', xAxis: null, yAxis: [], dataScope: 'all' });
	let kpiLabel = $state('');
	let kpiValueColumn = $state('');
	let kpiFormat = $state<'number' | 'percentage'>('number');
	let kpiPrefix = $state('');
	let kpiSuffix = $state('');
	let textContent = $state('');
	let autoRefreshSeconds = $state(0);

	// Execution state
	let previewResult = $state<Record<string, unknown>[] | undefined>(undefined);
	let isExecuting = $state(false);
	let executeError = $state<string | undefined>(undefined);

	// Computed
	const resultColumns = $derived(
		previewResult && previewResult.length > 0 ? Object.keys(previewResult[0]) : []
	);
	// QueryChart takes columnar rows; `previewResult` comes back from
	// `runWidgetQuery` (read-only) as row objects (shared with non-streaming consumers).
	// Convert once reactively so we don't rebuild the array on every render
	// of the markup below.
	const columnarPreview = $derived(
		previewResult ? previewResult.map((r) => resultColumns.map((c) => r[c])) : []
	);

	const savedQueries = $derived(db.state.projectQueries);

	const autoRefreshOptions = [
		{ value: 0, label: 'Off' },
		{ value: 30, label: '30s' },
		{ value: 60, label: '1m' },
		{ value: 300, label: '5m' },
		{ value: 900, label: '15m' },
	];

	// Initialize from widget when opening
	$effect(() => {
		if (open && widget) {
			title = widget.title;
			description = widget.description ?? '';
			widgetType = widget.widgetType;
			querySource = widget.querySource;
			query = widget.query;
			savedQueryId = widget.savedQueryId;
			chartConfig = widget.chartConfig ?? { type: 'bar', xAxis: null, yAxis: [], dataScope: 'all' };
			kpiLabel = widget.kpiConfig?.label ?? '';
			kpiValueColumn = widget.kpiConfig?.valueColumn ?? '';
			kpiFormat = widget.kpiConfig?.format ?? 'number';
			kpiPrefix = widget.kpiConfig?.prefix ?? '';
			kpiSuffix = widget.kpiConfig?.suffix ?? '';
			textContent = widget.textConfig?.content ?? '';
			autoRefreshSeconds = widget.autoRefreshSeconds ?? 0;
			previewResult = widget.result;
			executeError = widget.error;
		} else if (open && !widget) {
			// New widget defaults
			title = 'New Widget';
			description = '';
			widgetType = initialWidget?.widgetType ?? 'chart';
			querySource = 'custom';
			query = '';
			savedQueryId = undefined;
			chartConfig = { type: 'bar', xAxis: null, yAxis: [], dataScope: 'all' };
			kpiLabel = '';
			kpiValueColumn = '';
			kpiFormat = 'number';
			kpiPrefix = '';
			kpiSuffix = '';
			textContent = '';
			autoRefreshSeconds = 0;
			previewResult = undefined;
			executeError = undefined;
		}
	});

	async function runQuery() {
		const q = querySource === 'saved' && savedQueryId
			? savedQueries.find((sq) => sq.id === savedQueryId)?.query ?? ''
			: query;

		if (!q.trim()) return;

		isExecuting = true;
		executeError = undefined;
		try {
			previewResult = await db.dashboards.runWidgetQuery(q);
			// Auto-configure chart if no config set. Chart helpers take
			// columnar rows, so we reuse the `columnarPreview` $derived.
			if (previewResult.length > 0 && chartConfig.yAxis.length === 0) {
				chartConfig = createDefaultChartConfig(resultColumns, columnarPreview);
			} else if (previewResult.length > 0) {
				// Filter out stale columns that no longer exist in the results
				// (e.g., after the user modifies the query to add/remove aliases)
				const currentColumns = new Set(Object.keys(previewResult[0]));
				const validYAxis = chartConfig.yAxis.filter((col) => currentColumns.has(col));
				const validXAxis = chartConfig.xAxis && !currentColumns.has(chartConfig.xAxis) ? null : chartConfig.xAxis;
				if (validYAxis.length !== chartConfig.yAxis.length || validXAxis !== chartConfig.xAxis) {
					chartConfig = { ...chartConfig, yAxis: validYAxis, xAxis: validXAxis };
				}
			}
		} catch (error) {
			executeError = error instanceof Error ? error.message : 'Query failed';
			previewResult = undefined;
		}
		isExecuting = false;
	}

	function handleSave() {
		// When editing, look up the widget's live position/size from the dashboard
		// to avoid overwriting canvas changes (drag/resize) with stale values.
		const liveWidget = widget
			? db.dashboards.getDashboard(dashboardId)?.widgets.find((w) => w.id === widget.id)
			: undefined;

		const saved: DashboardWidget = {
			id: widget?.id ?? initialWidget?.id ?? `widget-${crypto.randomUUID()}`,
			title,
			description: description || undefined,
			x: liveWidget?.x ?? initialWidget?.x ?? 0,
			y: liveWidget?.y ?? initialWidget?.y ?? 0,
			width: liveWidget?.width ?? initialWidget?.width ?? 400,
			height: liveWidget?.height ?? initialWidget?.height ?? 300,
			querySource: widgetType === 'text' ? 'custom' : querySource,
			query: widgetType === 'text' ? '' : (querySource === 'custom' ? query : ''),
			savedQueryId: widgetType === 'text' ? undefined : (querySource === 'saved' ? savedQueryId : undefined),
			widgetType,
			chartConfig: widgetType === 'chart' ? chartConfig : undefined,
			kpiConfig: widgetType === 'kpi'
				? {
						label: kpiLabel || title,
						valueColumn: kpiValueColumn,
						format: kpiFormat,
						prefix: kpiPrefix || undefined,
						suffix: kpiSuffix || undefined,
					}
				: undefined,
			textConfig: widgetType === 'text'
				? { content: textContent }
				: undefined,
			autoRefreshSeconds: widgetType === 'text' ? undefined : (autoRefreshSeconds || undefined),
			result: previewResult,
			isLoading: false,
			error: executeError,
		};
		onSave(saved);
	}

	// Preview widget for live preview
	const previewWidget = $derived<DashboardWidget>({
		id: 'preview',
		title,
		x: 0,
		y: 0,
		width: 400,
		height: 300,
		querySource,
		query,
		widgetType,
		chartConfig: widgetType === 'chart' ? chartConfig : undefined,
		kpiConfig: widgetType === 'kpi'
			? { label: kpiLabel || title, valueColumn: kpiValueColumn, format: kpiFormat, prefix: kpiPrefix, suffix: kpiSuffix }
			: undefined,
		textConfig: widgetType === 'text'
			? { content: textContent }
			: undefined,
		result: previewResult,
		isLoading: isExecuting,
		error: executeError,
	});
</script>

{#if open}
	<div class="h-full w-[380px] shrink-0 border-l bg-background flex flex-col">
		<!-- Header -->
		<div class="flex items-center justify-between border-b px-4 py-2">
			<h3 class="text-sm font-medium">{widget ? 'Edit Widget' : 'Add Widget'}</h3>
			<Button variant="ghost" size="icon" class="size-6" onclick={onClose}>
				<XIcon class="size-3.5" />
			</Button>
		</div>

		<!-- Scrollable content -->
		<div class="flex-1 min-h-0 overflow-y-auto">
			<div class="space-y-5 p-4">
				<!-- Title & Description -->
				<div class="space-y-3">
					<div class="space-y-1.5">
						<Label class="text-xs">Title</Label>
						<Input class="h-8 text-sm" bind:value={title} placeholder="Widget title" />
					</div>
					<div class="space-y-1.5">
						<Label class="text-xs">Description</Label>
						<Input class="h-8 text-sm" bind:value={description} placeholder="Optional description" />
					</div>
				</div>

				<!-- Widget Type -->
				<div class="space-y-1.5">
					<Label class="text-xs">Widget Type</Label>
					<div class="flex gap-1">
						<Button
							variant={widgetType === 'chart' ? 'default' : 'outline'}
							size="sm"
							class="h-7 text-xs"
							onclick={() => (widgetType = 'chart')}
						>
							Chart
						</Button>
						<Button
							variant={widgetType === 'kpi' ? 'default' : 'outline'}
							size="sm"
							class="h-7 text-xs"
							onclick={() => (widgetType = 'kpi')}
						>
							KPI
						</Button>
						<Button
							variant={widgetType === 'text' ? 'default' : 'outline'}
							size="sm"
							class="h-7 text-xs"
							onclick={() => (widgetType = 'text')}
						>
							Text
						</Button>
					</div>
				</div>

				{#if widgetType === 'text'}
					<!-- Text Content -->
					<div class="space-y-3">
						<div class="space-y-1.5">
							<Label class="text-xs">Content</Label>
							<textarea
								class="w-full min-h-[120px] rounded-md border bg-background px-3 py-2 text-sm resize-y"
								bind:value={textContent}
								placeholder="Enter your text here..."
							></textarea>
						</div>
					</div>

					<!-- Text Preview -->
					{#if textContent}
						<div class="space-y-1.5">
							<Label class="text-xs">Preview</Label>
							<div class="h-40 rounded-md border overflow-hidden">
								<DashboardTextWidget widget={previewWidget} />
							</div>
						</div>
					{/if}
				{:else}
					<!-- Data Source -->
					<div class="space-y-3">
						<div class="space-y-1.5">
							<Label class="text-xs">Data Source</Label>
							<div class="flex gap-1">
								<Button
									variant={querySource === 'custom' ? 'default' : 'outline'}
									size="sm"
									class="h-7 text-xs"
									onclick={() => (querySource = 'custom')}
								>
									Custom Query
								</Button>
								<Button
									variant={querySource === 'saved' ? 'default' : 'outline'}
									size="sm"
									class="h-7 text-xs"
									onclick={() => (querySource = 'saved')}
								>
									Saved Query
								</Button>
							</div>
						</div>

						{#if querySource === 'custom'}
							<WidgetQueryEditor
								{query}
								{isExecuting}
								onQueryChange={(v) => (query = v)}
								onRun={runQuery}
							/>
						{:else}
							<div class="space-y-2">
								<Label class="text-xs">Select Saved Query</Label>
								<select
									class="w-full h-8 rounded-md border bg-background px-3 text-sm"
									value={savedQueryId ?? ''}
									onchange={(e) => {
										savedQueryId = (e.target as HTMLSelectElement).value || undefined;
									}}
								>
									<option value="">Choose a saved query...</option>
									{#each savedQueries as sq}
										<option value={sq.id}>{sq.name}</option>
									{/each}
								</select>
								<Button
									variant="outline"
									size="sm"
									class="h-6 text-xs"
									onclick={runQuery}
									disabled={isExecuting || !savedQueryId}
								>
									Run Query
								</Button>
							</div>
						{/if}
					</div>

					<!-- Results Preview -->
					{#if previewResult && previewResult.length > 0}
						<div class="space-y-1.5">
							<Label class="text-xs">Results ({previewResult.length} rows)</Label>
							<div class="max-h-28 overflow-auto rounded-md border text-xs">
								<table class="w-full">
									<thead class="sticky top-0 bg-muted">
										<tr>
											{#each resultColumns as col}
												<th class="px-2 py-1 text-left font-medium">{col}</th>
											{/each}
										</tr>
									</thead>
									<tbody>
										{#each previewResult.slice(0, 5) as row}
											<tr class="border-t">
												{#each resultColumns as col}
													<td class="px-2 py-1 truncate max-w-[100px]">{String(row[col] ?? '')}</td>
												{/each}
											</tr>
										{/each}
									</tbody>
								</table>
							</div>
						</div>
					{/if}

					{#if executeError}
						<div class="rounded-md border border-destructive/50 p-2 text-xs text-destructive">
							{executeError}
						</div>
					{/if}

					<!-- Chart / KPI Config -->
					{#if widgetType === 'chart' && resultColumns.length > 0}
						<WidgetChartConfig
							config={chartConfig}
							columns={resultColumns}
							rows={previewResult}
							onConfigChange={(c) => (chartConfig = c)}
						/>
					{:else if widgetType === 'kpi' && resultColumns.length > 0}
						<div class="space-y-3">
							<div class="space-y-1.5">
								<Label class="text-xs">KPI Label</Label>
								<Input class="h-8 text-sm" bind:value={kpiLabel} placeholder="e.g., Total Users" />
							</div>
							<div class="space-y-1.5">
								<Label class="text-xs">Value Column</Label>
								<select
									class="w-full h-8 rounded-md border bg-background px-3 text-sm"
									bind:value={kpiValueColumn}
								>
									<option value="">Select column...</option>
									{#each resultColumns as col}
										<option value={col}>{col}</option>
									{/each}
								</select>
							</div>
							<div class="space-y-1.5">
								<Label class="text-xs">Format</Label>
								<div class="flex gap-1">
									{#each ['number', 'percentage'] as fmt}
										<Button
											variant={kpiFormat === fmt ? 'default' : 'outline'}
											size="sm"
											class="h-6 text-xs capitalize"
											onclick={() => (kpiFormat = fmt as typeof kpiFormat)}
										>
											{fmt}
										</Button>
									{/each}
								</div>
							</div>
							<div class="grid grid-cols-2 gap-2">
								<div class="space-y-1.5">
									<Label class="text-xs">Prefix</Label>
									<Input class="h-8 text-sm" bind:value={kpiPrefix} placeholder="e.g., $" />
								</div>
								<div class="space-y-1.5">
									<Label class="text-xs">Suffix</Label>
									<Input class="h-8 text-sm" bind:value={kpiSuffix} placeholder="e.g., users" />
								</div>
							</div>
						</div>
					{/if}

					<!-- Live Preview -->
					{#if previewResult && previewResult.length > 0}
						<div class="space-y-1.5">
							<Label class="text-xs">Preview</Label>
							<div class="h-40 rounded-md border overflow-hidden">
								{#if widgetType === 'kpi'}
									<DashboardKpiWidget widget={previewWidget} />
								{:else if resultColumns.length > 0}
									<QueryChart
										columns={resultColumns}
										rows={columnarPreview}
										config={chartConfig}
									/>
								{/if}
							</div>
						</div>
					{/if}

					<!-- Auto-refresh -->
					<div class="space-y-1.5">
						<Label class="text-xs">Auto-refresh</Label>
						<div class="flex gap-1 flex-wrap">
							{#each autoRefreshOptions as opt}
								<Button
									variant={autoRefreshSeconds === opt.value ? 'default' : 'outline'}
									size="sm"
									class="h-6 text-xs"
									onclick={() => (autoRefreshSeconds = opt.value)}
								>
									{opt.label}
								</Button>
							{/each}
						</div>
					</div>
				{/if}
			</div>
		</div>

		<!-- Footer -->
		<div class="flex items-center border-t px-4 py-3">
			{#if widget && onDelete}
				<Button variant="ghost" size="sm" class="text-destructive hover:text-destructive" onclick={() => onDelete(widget.id)}>
					Delete
				</Button>
			{/if}
			<div class="flex-1"></div>
			<div class="flex items-center gap-2">
				<Button variant="outline" size="sm" onclick={onClose}>Cancel</Button>
				<Button size="sm" onclick={handleSave}>
					{widget ? 'Save Changes' : 'Add Widget'}
				</Button>
			</div>
		</div>
	</div>
{/if}
