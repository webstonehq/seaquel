<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Label } from '$lib/components/ui/label';
	import {
		Popover,
		PopoverContent,
		PopoverTrigger
	} from '$lib/components/ui/popover';
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger
	} from '$lib/components/ui/select';
	import { SettingsIcon, BarChartIcon, LineChartIcon, PieChartIcon, ScatterChartIcon } from '@lucide/svelte';
	import type { ChartConfig, ChartType } from '$lib/types';

	type Props = {
		config: ChartConfig;
		columns: string[];
		onConfigChange: (config: ChartConfig) => void;
	};

	let { config, columns, onConfigChange }: Props = $props();

	let open = $state(false);

	const chartTypes: { value: ChartType; label: string; icon: typeof BarChartIcon }[] = [
		{ value: 'bar', label: 'Bar Chart', icon: BarChartIcon },
		{ value: 'line', label: 'Line Chart', icon: LineChartIcon },
		{ value: 'pie', label: 'Pie Chart', icon: PieChartIcon },
		{ value: 'scatter', label: 'Scatter Plot', icon: ScatterChartIcon }
	];

	const handleTypeChange = (type: ChartType) => {
		onConfigChange({ ...config, type });
	};

	const handleXAxisChange = (xAxis: string | null) => {
		onConfigChange({ ...config, xAxis });
	};

	const handleYAxisChange = (yAxis: string[]) => {
		onConfigChange({ ...config, yAxis });
	};

	const toggleYAxis = (column: string) => {
		const current = config.yAxis;
		if (current.includes(column)) {
			// Remove column (but keep at least one)
			if (current.length > 1) {
				handleYAxisChange(current.filter((c) => c !== column));
			}
		} else {
			// Add column
			handleYAxisChange([...current, column]);
		}
	};
</script>

<Popover bind:open>
	<PopoverTrigger>
		<Button variant="ghost" size="sm" class="h-7 gap-1.5">
			<SettingsIcon class="size-3.5" />
			<span class="text-xs">Configure</span>
		</Button>
	</PopoverTrigger>
	<PopoverContent class="w-72" align="end">
		<div class="grid gap-4">
			<div class="space-y-2">
				<h4 class="font-medium text-sm">Chart Configuration</h4>
				<p class="text-xs text-muted-foreground">
					Customize how your data is visualized.
				</p>
			</div>

			<div class="grid gap-3">
				<!-- Chart Type -->
				<div class="grid gap-1.5">
					<Label class="text-xs">Chart Type</Label>
					<div class="flex gap-1">
						{#each chartTypes as chartType}
							<Button
								variant={config.type === chartType.value ? 'default' : 'outline'}
								size="sm"
								class="h-8 flex-1 px-2"
								onclick={() => handleTypeChange(chartType.value)}
								title={chartType.label}
							>
								<chartType.icon class="size-4" />
							</Button>
						{/each}
					</div>
				</div>

				<!-- X Axis -->
				<div class="grid gap-1.5">
					<Label class="text-xs">X Axis (Categories)</Label>
					<Select
						type="single"
						value={config.xAxis ?? undefined}
						onValueChange={(v) => handleXAxisChange(v ?? null)}
					>
						<SelectTrigger class="h-8 text-xs">
							{config.xAxis ?? 'Row Index'}
						</SelectTrigger>
						<SelectContent>
							<SelectItem value="">Row Index</SelectItem>
							{#each columns as column}
								<SelectItem value={column}>{column}</SelectItem>
							{/each}
						</SelectContent>
					</Select>
				</div>

				<!-- Y Axis -->
				<div class="grid gap-1.5">
					<Label class="text-xs">Y Axis (Values)</Label>
					<div class="flex flex-wrap gap-1">
						{#each columns as column}
							<Button
								variant={config.yAxis.includes(column) ? 'default' : 'outline'}
								size="sm"
								class="h-6 px-2 text-xs"
								onclick={() => toggleYAxis(column)}
							>
								{column}
							</Button>
						{/each}
					</div>
					{#if config.yAxis.length === 0}
						<p class="text-xs text-destructive">Select at least one column</p>
					{/if}
				</div>
			</div>
		</div>
	</PopoverContent>
</Popover>
