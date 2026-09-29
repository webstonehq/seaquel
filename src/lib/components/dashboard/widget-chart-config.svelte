<script lang="ts">
	import type { ChartConfig, ChartType } from '$lib/types';
	import { Label } from '$lib/components/ui/label';
	import { Button } from '$lib/components/ui/button';
	import BarChartIcon from '@lucide/svelte/icons/bar-chart';
	import LineChartIcon from '@lucide/svelte/icons/line-chart';
	import PieChartIcon from '@lucide/svelte/icons/pie-chart';
	import ScatterChartIcon from '@lucide/svelte/icons/scatter-chart';
	import AreaChartIcon from '@lucide/svelte/icons/area-chart';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';

	interface Props {
		config: ChartConfig;
		columns: string[];
		rows?: Record<string, unknown>[];
		onConfigChange: (config: ChartConfig) => void;
	}

	let { config, columns, rows = [], onConfigChange }: Props = $props();

	const chartTypes: { value: ChartType; label: string; icon: typeof BarChartIcon }[] = [
		{ value: 'bar', label: 'Bar', icon: BarChartIcon },
		{ value: 'line', label: 'Line', icon: LineChartIcon },
		{ value: 'area', label: 'Area', icon: AreaChartIcon },
		{ value: 'pie', label: 'Pie', icon: PieChartIcon },
		{ value: 'scatter', label: 'Scatter', icon: ScatterChartIcon },
	];

	const presetColors = [
		'#f97316', '#3b82f6', '#10b981', '#8b5cf6', '#ef4444',
		'#ec4899', '#14b8a6', '#f59e0b', '#6366f1', '#06b6d4',
	];

	function setType(type: ChartType) {
		onConfigChange({ ...config, type });
	}

	function setXAxis(xAxis: string | null) {
		onConfigChange({ ...config, xAxis });
	}

	function toggleYAxis(column: string) {
		const current = config.yAxis;
		if (current.includes(column)) {
			if (current.length > 1) {
				onConfigChange({ ...config, yAxis: current.filter((c) => c !== column) });
			}
		} else {
			onConfigChange({ ...config, yAxis: [...current, column] });
		}
	}

	function setColor(column: string, color: string) {
		onConfigChange({ ...config, colors: { ...config.colors, [column]: color } });
	}

	function resetColor(column: string) {
		if (!config.colors) return;
		const { [column]: _, ...rest } = config.colors;
		const hasColors = Object.keys(rest).length > 0;
		onConfigChange({ ...config, colors: hasColors ? rest : undefined });
	}

	function getDefaultColor(index: number): string {
		return `var(--color-chart-${(index % 5) + 1})`;
	}

	// Identify numeric columns
	const numericColumns = $derived(columns);

	// Unique slice labels for pie chart colors
	const pieSliceLabels = $derived(
		config.type === 'pie' && config.xAxis && rows.length > 0
			? [...new Set(rows.map((r) => String(r[config.xAxis!] ?? 'Unknown')))]
			: []
	);
</script>

<div class="space-y-4">
	<!-- Chart Type -->
	<div class="space-y-1.5">
		<Label class="text-xs">Chart Type</Label>
		<div class="flex gap-1">
			{#each chartTypes as ct}
				<Button
					variant={config.type === ct.value ? 'default' : 'outline'}
					size="sm"
					class="h-7 px-2 text-xs"
					onclick={() => setType(ct.value)}
				>
					<ct.icon class="mr-1 size-3" />
					{ct.label}
				</Button>
			{/each}
		</div>
	</div>

	<!-- X Axis -->
	<div class="space-y-1.5">
		<Label class="text-xs">X Axis (Category)</Label>
		<div class="flex flex-wrap gap-1">
			{#each columns as col}
				<Button
					variant={config.xAxis === col ? 'default' : 'outline'}
					size="sm"
					class="h-6 px-2 text-xs"
					onclick={() => setXAxis(col)}
				>
					{col}
				</Button>
			{/each}
		</div>
	</div>

	<!-- Y Axis -->
	<div class="space-y-1.5">
		<Label class="text-xs">Y Axis (Values)</Label>
		<div class="flex flex-wrap gap-1">
			{#each numericColumns as col}
				<Button
					variant={config.yAxis.includes(col) ? 'default' : 'outline'}
					size="sm"
					class="h-6 px-2 text-xs"
					onclick={() => toggleYAxis(col)}
				>
					{col}
				</Button>
			{/each}
		</div>
	</div>

	<!-- Series Colors -->
	{#if config.type === 'pie' && pieSliceLabels.length > 0}
		<div class="space-y-1.5">
			<Label class="text-xs">Slice Colors</Label>
			<div class="space-y-2">
				{#each pieSliceLabels as sliceLabel, i}
					{@const customColor = config.colors?.[sliceLabel]}
					<div class="flex items-center gap-2">
						<span
							class="size-5 shrink-0 rounded border border-border"
							style:background={customColor ?? getDefaultColor(i)}
						></span>
						<span class="text-xs text-muted-foreground truncate flex-1 min-w-0">{sliceLabel}</span>
						<div class="flex items-center gap-0.5 shrink-0">
							{#each presetColors as preset}
								<button
									type="button"
									class="size-3.5 rounded-full border transition-transform hover:scale-125"
									class:border-foreground={customColor === preset}
									class:border-transparent={customColor !== preset}
									style:background={preset}
									title={preset}
									onclick={() => setColor(sliceLabel, preset)}
								></button>
							{/each}
							<label
								class="relative size-3.5 rounded-full cursor-pointer shrink-0 border border-border transition-transform hover:scale-125"
								style:background="conic-gradient(red, yellow, lime, aqua, blue, magenta, red)"
								title="Custom color"
							>
								<input
									type="color"
									class="absolute inset-0 size-full opacity-0 cursor-pointer"
									value={customColor ?? '#3b82f6'}
									oninput={(e) => setColor(sliceLabel, (e.target as HTMLInputElement).value)}
								/>
							</label>
						</div>
						<button
							type="button"
							class="transition-colors w-3 shrink-0"
							class:text-muted-foreground={customColor}
							class:hover:text-foreground={customColor}
							class:invisible={!customColor}
							title="Reset to default"
							onclick={() => resetColor(sliceLabel)}
						>
							<RotateCcwIcon class="size-3" />
						</button>
					</div>
				{/each}
			</div>
		</div>
	{:else if config.yAxis.length > 0}
		<div class="space-y-1.5">
			<Label class="text-xs">Series Colors</Label>
			<div class="space-y-2">
				{#each config.yAxis as col, i}
					{@const customColor = config.colors?.[col]}
					<div class="flex items-center gap-2">
						<span
							class="size-5 shrink-0 rounded border border-border"
							style:background={customColor ?? getDefaultColor(i)}
						></span>
						<span class="text-xs text-muted-foreground truncate flex-1 min-w-0">{col}</span>
						<div class="flex items-center gap-0.5 shrink-0">
							{#each presetColors as preset}
								<button
									type="button"
									class="size-3.5 rounded-full border transition-transform hover:scale-125"
									class:border-foreground={customColor === preset}
									class:border-transparent={customColor !== preset}
									style:background={preset}
									title={preset}
									onclick={() => setColor(col, preset)}
								></button>
							{/each}
							<label
								class="relative size-3.5 rounded-full cursor-pointer shrink-0 border border-border transition-transform hover:scale-125"
								style:background="conic-gradient(red, yellow, lime, aqua, blue, magenta, red)"
								title="Custom color"
							>
								<input
									type="color"
									class="absolute inset-0 size-full opacity-0 cursor-pointer"
									value={customColor ?? '#3b82f6'}
									oninput={(e) => setColor(col, (e.target as HTMLInputElement).value)}
								/>
							</label>
						</div>
						<button
							type="button"
							class="transition-colors w-3 shrink-0"
							class:text-muted-foreground={customColor}
							class:hover:text-foreground={customColor}
							class:invisible={!customColor}
							title="Reset to default"
							onclick={() => resetColor(col)}
						>
							<RotateCcwIcon class="size-3" />
						</button>
					</div>
				{/each}
			</div>
		</div>
	{/if}
</div>
