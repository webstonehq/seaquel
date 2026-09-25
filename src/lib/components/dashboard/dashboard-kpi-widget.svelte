<script lang="ts">
	import type { DashboardWidget } from '$lib/types';
	import { Loader2Icon, AlertCircleIcon } from '@lucide/svelte';
	import { cellText, toNumber } from '$lib/values';

	interface Props {
		widget: DashboardWidget;
	}

	let { widget }: Props = $props();

	const formattedValue = $derived.by(() => {
		if (!widget.result || widget.result.length === 0 || !widget.kpiConfig) return null;

		const row = widget.result[0];
		const raw = row[widget.kpiConfig.valueColumn];
		if (raw == null) return 'N/A';

		const num = typeof raw === 'number' ? raw : toNumber(raw);
		if (isNaN(num)) return cellText(raw);

		const format = widget.kpiConfig.format ?? 'number';

		let formatted: string;
		if (format === 'percentage') {
			formatted = new Intl.NumberFormat(undefined, { style: 'percent', minimumFractionDigits: 1 }).format(num / 100);
		} else {
			formatted = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 }).format(num);
		}

		const prefix = widget.kpiConfig.prefix ?? '';
		const suffix = widget.kpiConfig.suffix ?? '';
		return `${prefix}${formatted}${suffix}`;
	});

	// Trend: if there are 2+ rows, compare last two
	const trend = $derived.by(() => {
		if (!widget.result || widget.result.length < 2 || !widget.kpiConfig) return null;
		const col = widget.kpiConfig.valueColumn;
		const current = toNumber(widget.result[0][col]);
		const previous = toNumber(widget.result[1][col]);
		if (isNaN(current) || isNaN(previous) || previous === 0) return null;
		const delta = ((current - previous) / Math.abs(previous)) * 100;
		return { delta, direction: delta >= 0 ? 'up' as const : 'down' as const };
	});
</script>

<div class="flex h-full flex-col items-center justify-center p-2">
	{#if widget.isLoading}
		<Loader2Icon class="size-6 animate-spin text-muted-foreground" />
	{:else if widget.error}
		<div class="flex flex-col items-center gap-2 text-destructive">
			<AlertCircleIcon class="size-5" />
			<span class="text-xs">{widget.error}</span>
		</div>
	{:else if formattedValue !== null}
		<p class="text-2xl font-medium text-muted-foreground uppercase tracking-wider">
			{widget.kpiConfig?.label ?? widget.title}
		</p>
		<p class="font-bold tabular-nums" style="font-size: clamp(1rem, 20cqh, 2rem); line-height: 1.1;">
			{formattedValue}
		</p>
		{#if trend}
			<p class="text-sm {trend.direction === 'up' ? 'text-green-600 dark:text-green-400' : 'text-red-600 dark:text-red-400'}">
				{trend.direction === 'up' ? '+' : ''}{trend.delta.toFixed(1)}%
			</p>
		{/if}
	{:else}
		<p class="text-sm text-muted-foreground">No data</p>
	{/if}
</div>
