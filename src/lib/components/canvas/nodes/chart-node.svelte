<script lang="ts">
	import { Handle, Position, NodeResizer } from "@xyflow/svelte";
	import type { CanvasChartNodeData } from "$lib/types/canvas";
	import type { ChartType } from "$lib/types";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { BarChart, LineChart, PieChart, ScatterChart } from "layerchart";
	import ChartBarIcon from "@lucide/svelte/icons/chart-bar";
	import ChartLineIcon from "@lucide/svelte/icons/chart-line";
	import ChartPieIcon from "@lucide/svelte/icons/chart-pie";
	import ScatterChartIcon from "@lucide/svelte/icons/scatter-chart";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import { Button } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
	import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";

	interface Props {
		id: string;
		data: CanvasChartNodeData;
		isConnectable?: boolean;
		selected?: boolean;
	}

	let { id, data, isConnectable = true, selected = false }: Props = $props();

	const db = useDatabase();

	function handleRemove() {
		db.canvas.removeNode(id);
	}

	function handleChartTypeChange(newType: ChartType) {
		db.canvas.updateNodeData(id, {
			chartConfig: { ...data.chartConfig, type: newType },
		});
	}

	function handleResizeEnd(_event: unknown, params: { width: number; height: number }) {
		db.canvas.updateNodeDimensions(id, params.width, params.height);
	}

	// Transform data for the chart
	const chartData = $derived(
		data.rows.map((row, index) => {
			const item: Record<string, unknown> = { _index: index };

			// Add x-axis value
			if (data.chartConfig.xAxis) {
				item.x = row[data.chartConfig.xAxis];
			} else {
				item.x = `Row ${index + 1}`;
			}

			// Add y-axis values
			data.chartConfig.yAxis.forEach((col) => {
				const val = row[col];
				item[col] = typeof val === "number" ? val : Number(val) || 0;
			});

			return item;
		})
	);

	// Create series for multi-y charts
	const series = $derived(
		data.chartConfig.yAxis.map((col, i) => ({
			key: col,
			label: col,
			value: (d: Record<string, unknown>) => d[col] as number,
			color: `var(--color-chart-${(i % 5) + 1})`,
		}))
	);

	// For pie chart, transform data differently
	const pieData = $derived(
		data.rows.map((row, index) => ({
			name: data.chartConfig.xAxis
				? String(row[data.chartConfig.xAxis] ?? "Unknown")
				: "Unknown",
			value: data.chartConfig.yAxis[0]
				? typeof row[data.chartConfig.yAxis[0]] === "number"
					? (row[data.chartConfig.yAxis[0]] as number)
					: Number(row[data.chartConfig.yAxis[0]]) || 0
				: 0,
			color: `var(--color-chart-${(index % 5) + 1})`,
		}))
	);

	const ChartTypeIcon = $derived.by(() => {
		switch (data.chartConfig.type) {
			case "bar":
				return ChartBarIcon;
			case "line":
				return ChartLineIcon;
			case "pie":
				return ChartPieIcon;
			case "scatter":
				return ScatterChartIcon;
			default:
				return ChartBarIcon;
		}
	});
</script>

<div
	class="bg-card border border-border rounded-lg shadow-lg overflow-hidden h-full flex flex-col"
>
	<NodeResizer
		minWidth={300}
		minHeight={200}
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
		class="!w-3 !h-3 !bg-primary !border-2 !border-background"
	/>

	<!-- Header -->
	<div
		class="bg-muted/50 border-b border-border px-3 py-2 flex items-center justify-between"
	>
		<div class="flex items-center gap-2 min-w-0">
			<ChartTypeIcon class="size-4 text-muted-foreground shrink-0" />
			<span class="font-medium text-sm truncate">Chart</span>
			<span class="text-xs text-muted-foreground">({data.rows.length} rows)</span>
		</div>

		<div class="flex items-center gap-1 shrink-0">
			<DropdownMenu.Root>
				<DropdownMenu.Trigger>
					<Button
						variant="ghost"
						size="icon"
						class="size-6"
					>
						<MoreHorizontalIcon class="size-4" />
					</Button>
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end">
					<DropdownMenu.Sub>
						<DropdownMenu.SubTrigger>
							<ChartBarIcon class="size-4 mr-2" />
							Chart Type
						</DropdownMenu.SubTrigger>
						<DropdownMenu.SubContent>
							<DropdownMenu.Item onclick={() => handleChartTypeChange("bar")}>
								<ChartBarIcon class="size-4 mr-2" />
								Bar Chart
							</DropdownMenu.Item>
							<DropdownMenu.Item onclick={() => handleChartTypeChange("line")}>
								<ChartLineIcon class="size-4 mr-2" />
								Line Chart
							</DropdownMenu.Item>
							<DropdownMenu.Item onclick={() => handleChartTypeChange("pie")}>
								<ChartPieIcon class="size-4 mr-2" />
								Pie Chart
							</DropdownMenu.Item>
							<DropdownMenu.Item onclick={() => handleChartTypeChange("scatter")}>
								<ScatterChartIcon class="size-4 mr-2" />
								Scatter Plot
							</DropdownMenu.Item>
						</DropdownMenu.SubContent>
					</DropdownMenu.Sub>
					<DropdownMenu.Separator />
					<DropdownMenu.Item onclick={handleRemove} class="text-destructive">
						<Trash2Icon class="size-4 mr-2" />
						Remove
					</DropdownMenu.Item>
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		</div>
	</div>

	<!-- Chart content -->
	<div class="flex-1 min-h-0 p-4">
		{#if data.rows.length === 0 || data.chartConfig.yAxis.length === 0}
			<div
				class="flex h-full items-center justify-center text-muted-foreground text-sm"
			>
				No data available for chart visualization
			</div>
		{:else if data.chartConfig.type === "bar"}
			<BarChart
				data={chartData}
				x="x"
				{series}
				axis
				grid
				tooltip
				props={{
					xAxis: { label: data.chartConfig.xAxis ?? "Index" },
					yAxis: { label: data.chartConfig.yAxis[0] ?? "Value" },
				}}
			/>
		{:else if data.chartConfig.type === "line"}
			<LineChart
				data={chartData}
				x="x"
				{series}
				axis
				grid
				tooltip
				props={{
					xAxis: { label: data.chartConfig.xAxis ?? "Index" },
					yAxis: { label: data.chartConfig.yAxis[0] ?? "Value" },
				}}
			/>
		{:else if data.chartConfig.type === "pie"}
			<PieChart data={pieData} key="name" value="value" label="name" c="color" legend tooltip />
		{:else if data.chartConfig.type === "scatter"}
			<ScatterChart
				data={chartData}
				x={(d) => d[data.chartConfig.yAxis[0] ?? "x"] as number}
				y={(d) =>
					d[data.chartConfig.yAxis[1] ?? data.chartConfig.yAxis[0] ?? "x"] as number}
				axis
				grid
				tooltip
				props={{
					xAxis: { label: data.chartConfig.yAxis[0] ?? "X" },
					yAxis: {
						label: data.chartConfig.yAxis[1] ?? data.chartConfig.yAxis[0] ?? "Y",
					},
				}}
			/>
		{/if}
	</div>
</div>
