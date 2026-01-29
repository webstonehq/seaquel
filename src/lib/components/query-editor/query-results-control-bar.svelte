<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Badge } from '$lib/components/ui/badge';
	import { RefreshCwIcon, XIcon } from '@lucide/svelte';
	import { QueryResultViewToggle, QueryExportMenu } from './index.js';
	import VisualizeLayoutPopover from './visualize-layout-popover.svelte';
	import { ChartConfigPopover } from '$lib/components/charts/index.js';
	import type { ResultViewMode, ChartConfig, QueryResult } from '$lib/types';
	import type { QueryLayoutOptions } from '$lib/utils/query-visual-layout';

	type Props = {
		currentViewMode: ResultViewMode;
		onViewModeChange: (mode: ResultViewMode) => void;
		explainResult?: { result?: unknown; isAnalyze: boolean; isExecuting?: boolean } | null;
		visualizeResult?: { parsedQuery?: unknown; parseError?: string } | null;
		isExplainStale: boolean;
		isVisualizeStale: boolean;
		// Chart props (only shown when there's an active non-error result)
		currentChartConfig?: ChartConfig;
		activeResult?: QueryResult | null;
		onChartConfigChange?: (config: ChartConfig) => void;
		// Export (only shown for table/chart modes with results)
		onExport?: (format: 'csv' | 'json' | 'sql' | 'markdown') => void;
		onCopy?: (format: 'csv' | 'json' | 'sql' | 'markdown') => void;
		// Explain actions
		onRefreshExplain?: (analyze: boolean) => void;
		onCloseExplain?: () => void;
		// Visualize actions
		visualizeLayoutOptions?: QueryLayoutOptions;
		onVisualizeLayoutChange?: (options: QueryLayoutOptions) => void;
		onRefreshVisualize?: () => void;
		onCloseVisualize?: () => void;
	};

	let {
		currentViewMode,
		onViewModeChange,
		explainResult = null,
		visualizeResult = null,
		isExplainStale,
		isVisualizeStale,
		currentChartConfig,
		activeResult = null,
		onChartConfigChange,
		onExport,
		onCopy,
		onRefreshExplain,
		onCloseExplain,
		visualizeLayoutOptions,
		onVisualizeLayoutChange,
		onRefreshVisualize,
		onCloseVisualize,
	}: Props = $props();

	const hasExplainResult = $derived(!!explainResult?.result || !!explainResult?.isExecuting);
	const hasVisualizeResult = $derived(!!visualizeResult?.parsedQuery || !!visualizeResult?.parseError);
</script>

<div class="flex items-center justify-between px-2 py-1.5 border-b bg-muted/20">
	<div class="flex items-center gap-2">
		<QueryResultViewToggle
			mode={currentViewMode}
			onModeChange={onViewModeChange}
			{hasExplainResult}
			{hasVisualizeResult}
			{isExplainStale}
			{isVisualizeStale}
		/>
		{#if currentViewMode === 'explain' && explainResult?.result}
			<Badge variant={explainResult.isAnalyze ? 'default' : 'secondary'} class="h-5">
				{explainResult.isAnalyze ? 'ANALYZE' : 'EXPLAIN'}
			</Badge>
		{/if}
	</div>
	<div class="flex items-center gap-1">
		{#if currentViewMode === 'chart' && currentChartConfig && activeResult && onChartConfigChange}
			<ChartConfigPopover
				config={currentChartConfig}
				columns={activeResult.columns}
				onConfigChange={onChartConfigChange}
			/>
		{/if}
		{#if (currentViewMode === 'table' || currentViewMode === 'chart') && activeResult && onExport && onCopy}
			<QueryExportMenu onExport={onExport} onCopy={onCopy} />
		{/if}
		{#if currentViewMode === 'explain' && explainResult && onRefreshExplain && onCloseExplain}
			<Button
				size="sm"
				variant="ghost"
				class="h-7 gap-1.5 px-2"
				onclick={() => onRefreshExplain(explainResult.isAnalyze)}
				title={isExplainStale ? 'Refresh with updated query' : 'Re-run explain'}
			>
				<RefreshCwIcon class="size-3.5" />
				{isExplainStale ? 'Refresh' : 'Re-run'}
			</Button>
			<Button
				size="sm"
				variant="ghost"
				class="h-7 px-2"
				onclick={onCloseExplain}
				title="Close"
			>
				<XIcon class="size-3.5" />
			</Button>
		{/if}
		{#if currentViewMode === 'visualize' && visualizeResult}
			{#if visualizeLayoutOptions && onVisualizeLayoutChange}
				<VisualizeLayoutPopover
					layoutOptions={visualizeLayoutOptions}
					onLayoutChange={onVisualizeLayoutChange}
				/>
			{/if}
			{#if onRefreshVisualize}
				<Button
					size="sm"
					variant="ghost"
					class="h-7 gap-1.5 px-2"
					onclick={onRefreshVisualize}
					title={isVisualizeStale ? 'Refresh with updated query' : 'Re-run visualization'}
				>
					<RefreshCwIcon class="size-3.5" />
					{isVisualizeStale ? 'Refresh' : 'Re-run'}
				</Button>
			{/if}
			{#if onCloseVisualize}
				<Button
					size="sm"
					variant="ghost"
					class="h-7 px-2"
					onclick={onCloseVisualize}
					title="Close"
				>
					<XIcon class="size-3.5" />
				</Button>
			{/if}
		{/if}
	</div>
</div>
