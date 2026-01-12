<script lang="ts">
	import { BarChart, LineChart, PieChart, ScatterChart } from 'layerchart';
	import type { ChartConfig } from '$lib/types';
	import { createDefaultChartConfig } from './chart-utils';

	type Props = {
		columns: string[];
		rows: Record<string, unknown>[];
		config?: ChartConfig;
		onConfigChange?: (config: ChartConfig) => void;
	};

	let { columns, rows, config, onConfigChange }: Props = $props();

	// Use provided config or create default
	let chartConfig = $derived(config ?? createDefaultChartConfig(columns, rows));

	// Transform data for the chart
	let chartData = $derived(
		rows.map((row, index) => {
			const item: Record<string, unknown> = { _index: index };

			// Add x-axis value
			if (chartConfig.xAxis) {
				item.x = row[chartConfig.xAxis];
			} else {
				item.x = `Row ${index + 1}`;
			}

			// Add y-axis values
			chartConfig.yAxis.forEach((col) => {
				const val = row[col];
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
			color: `var(--color-chart-${(i % 5) + 1})`
		}))
	);

	// For pie chart, transform data differently
	let pieData = $derived(
		rows.map((row) => ({
			name: chartConfig.xAxis ? String(row[chartConfig.xAxis] ?? 'Unknown') : 'Unknown',
			value: chartConfig.yAxis[0]
				? typeof row[chartConfig.yAxis[0]] === 'number'
					? row[chartConfig.yAxis[0]] as number
					: Number(row[chartConfig.yAxis[0]]) || 0
				: 0
		}))
	);
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
			axis
			grid
			tooltip
			props={{
				xAxis: { label: chartConfig.xAxis ?? 'Index' },
				yAxis: { label: chartConfig.yAxis[0] ?? 'Value' }
			}}
		/>
	{:else if chartConfig.type === 'line'}
		<LineChart
			data={chartData}
			x="x"
			{series}
			axis
			grid
			tooltip
			props={{
				xAxis: { label: chartConfig.xAxis ?? 'Index' },
				yAxis: { label: chartConfig.yAxis[0] ?? 'Value' }
			}}
		/>
	{:else if chartConfig.type === 'pie'}
		<PieChart
			data={pieData}
			value="value"
			label="name"
			legend
			tooltip
		/>
	{:else if chartConfig.type === 'scatter'}
		<ScatterChart
			data={chartData}
			x={(d) => d[chartConfig.yAxis[0] ?? 'x'] as number}
			y={(d) => d[chartConfig.yAxis[1] ?? chartConfig.yAxis[0] ?? 'x'] as number}
			axis
			grid
			tooltip
			props={{
				xAxis: { label: chartConfig.yAxis[0] ?? 'X' },
				yAxis: { label: chartConfig.yAxis[1] ?? chartConfig.yAxis[0] ?? 'Y' }
			}}
		/>
	{/if}
</div>
