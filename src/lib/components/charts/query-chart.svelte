<script lang="ts">
	import {
		BarChart, LineChart, PieChart, ScatterChart, AreaChart,
		LinearGradient, Area, Bars, Spline
	} from 'layerchart';
	import type { ChartConfig } from '$lib/types';
	import { createDefaultChartConfig } from './chart-utils';

	type Props = {
		columns: string[];
		rows: unknown[][];
		config?: ChartConfig;
		onConfigChange?: (config: ChartConfig) => void;
	};

	let { columns, rows, config, onConfigChange }: Props = $props();

	// Use provided config or create default
	let chartConfig = $derived(config ?? createDefaultChartConfig(columns, rows));

	// Resolve column-name → position once per config change.
	const xIdx = $derived(chartConfig.xAxis ? columns.indexOf(chartConfig.xAxis) : -1);
	const yIdx = $derived(chartConfig.yAxis.map((col) => columns.indexOf(col)));

	// Transform data for the chart
	let chartData = $derived(
		rows.map((row, index) => {
			const item: Record<string, unknown> = { _index: index };

			// Add x-axis value
			item.x = xIdx !== -1 ? (row[xIdx] ?? `Row ${index + 1}`) : `Row ${index + 1}`;

			// Add y-axis values
			chartConfig.yAxis.forEach((col, i) => {
				const val = yIdx[i] === -1 ? undefined : row[yIdx[i]];
				item[col] = typeof val === 'number' ? val : Number(val) || 0;
			});

			return item;
		})
	);

	// Create series for multi-y charts
	let series = $derived(
		chartConfig.yAxis.map((col, i) => ({
			key: col,
			label: col,
			value: (d: Record<string, unknown>) => d[col] as number,
			color: chartConfig.colors?.[col] ?? `var(--color-chart-${(i % 5) + 1})`
		}))
	);

	// For pie chart, transform data differently
	let pieData = $derived(
		rows.map((row) => {
			const name = xIdx !== -1 ? String(row[xIdx] ?? 'Unknown') : 'Unknown';
			const yValRaw = yIdx[0] !== undefined && yIdx[0] !== -1 ? row[yIdx[0]] : undefined;
			const value = yValRaw === undefined
				? 0
				: typeof yValRaw === 'number'
					? yValRaw
					: Number(yValRaw) || 0;
			return { name, value };
		})
	);

	// X-axis label props: rotate labels to avoid overlap
	const xAxisProps = {
		tickLabelProps: { rotate: -45, textAnchor: 'end' as const, dy: 4 },
	};

	// Colors for pie slices — always provide a range so layerchart never falls back to black
	let pieColors = $derived.by(() => {
		if (chartConfig.type !== 'pie') return undefined;
		const uniqueNames = [...new Set(pieData.map((d) => d.name))];
		return uniqueNames.map(
			(name, i) => chartConfig.colors?.[name] ?? `var(--color-chart-${(i % 5) + 1})`
		);
	});
</script>

<div class="h-full w-full p-4">
	{#if rows.length === 0 || chartConfig.yAxis.length === 0}
		<div class="flex h-full items-center justify-center text-muted-foreground">
			No data available for chart visualization
		</div>
	{:else if chartConfig.type === 'bar'}
		<BarChart
			data={chartData}
			x="x"
			{series}
			seriesLayout="overlap"
			axis
			grid
			padding={{ left: 48, bottom: 48 }}
			props={{ xAxis: xAxisProps }}
		>
			{#snippet marks({ context })}
				{#each context.series.visibleSeries as s (s.key)}
					<LinearGradient stops={[`color-mix(in oklch, ${s.color}, white 30%)`, s.color] as string[]} vertical>
						{#snippet children({ gradient })}
							<Bars
								seriesKey={s.key}
								rounded="edge"
								radius={4}
								strokeWidth={1}
								opacity={(d: unknown) => (context.series.isHighlighted(context.cKey(d) ?? s.key, true) ? 1 : 0.1)}
								fill={gradient}
							/>
						{/snippet}
					</LinearGradient>
				{/each}
			{/snippet}
		</BarChart>
	{:else if chartConfig.type === 'line'}
		<LineChart
			data={chartData}
			x="x"
			{series}
			axis
			grid
			padding={{ left: 48, bottom: 48 }}
			props={{ xAxis: xAxisProps }}
		>
			{#snippet belowMarks({ context })}
				{#each context.series.visibleSeries as s (s.key)}
					<LinearGradient stops={[s.color, 'transparent'] as string[]} vertical>
						{#snippet children({ gradient })}
							<Area y1={s.value ?? s.key} fill={gradient} fillOpacity={0.3} line={false} />
						{/snippet}
					</LinearGradient>
				{/each}
			{/snippet}
		</LineChart>
	{:else if chartConfig.type === 'pie'}
		<PieChart
			data={pieData}
			key="name"
			value="value"
			label="name"
			cRange={pieColors}
			legend
		/>
	{:else if chartConfig.type === 'area'}
		<AreaChart
			data={chartData}
			x="x"
			{series}
			seriesLayout="overlap"
			axis
			grid
			padding={{ left: 48, bottom: 48 }}
			props={{ xAxis: xAxisProps }}
		>
			{#snippet marks({ context })}
				{#each context.series.visibleSeries as s (s.key)}
					<LinearGradient stops={[s.color, 'transparent'] as string[]} vertical>
						{#snippet children({ gradient })}
							<Area seriesKey={s.key} fillOpacity={0.3} line fill={gradient} />
						{/snippet}
					</LinearGradient>
				{/each}
			{/snippet}
		</AreaChart>
	{:else if chartConfig.type === 'scatter'}
		<ScatterChart
			data={chartData}
			x={(d) => Number(d.x) || 0}
			y={(d) => d[chartConfig.yAxis[0] ?? 'x'] as number}
			axis
			grid
			padding={{ left: 48, bottom: 48 }}
			props={{ xAxis: xAxisProps }}
		/>
	{/if}
</div>
