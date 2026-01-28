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
	import { m } from '$lib/paraglide/messages.js';

	type Props = {
		config: ChartConfig;
		columns: string[];
		onConfigChange: (config: ChartConfig) => void;
	};

	let { config, columns, onConfigChange }: Props = $props();

	let open = $state(false);

	const chartTypes: { value: ChartType; label: () => string; icon: typeof BarChartIcon }[] = [
		{ value: 'bar', label: () => m.chart_bar(), icon: BarChartIcon },
		{ value: 'line', label: () => m.chart_line(), icon: LineChartIcon },
		{ value: 'pie', label: () => m.chart_pie(), icon: PieChartIcon },
		{ value: 'scatter', label: () => m.chart_scatter(), icon: ScatterChartIcon }
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
		<Button variant="ghost" size="sm" class="h-7 gap-1.5 px-2">
			<SettingsIcon class="size-3.5" />
			{m.chart_configure()}
		</Button>
	</PopoverTrigger>
	<PopoverContent class="w-72" align="end">
		<div class="grid gap-4">
			<div class="space-y-2">
				<h4 class="font-medium text-sm">{m.chart_configuration()}</h4>
				<p class="text-xs text-muted-foreground">
					{m.chart_customize_hint()}
				</p>
			</div>

			<div class="grid gap-3">
				<!-- Chart Type -->
				<div class="grid gap-1.5">
					<Label class="text-xs">{m.chart_type()}</Label>
					<div class="flex gap-1">
						{#each chartTypes as chartType}
							<Button
								variant={config.type === chartType.value ? 'default' : 'outline'}
								size="sm"
								class="h-8 flex-1 px-2"
								onclick={() => handleTypeChange(chartType.value)}
								title={chartType.label()}
							>
								<chartType.icon class="size-4" />
							</Button>
						{/each}
					</div>
				</div>

				<!-- X Axis -->
				<div class="grid gap-1.5">
					<Label class="text-xs">{m.chart_x_axis()}</Label>
					<Select
						type="single"
						value={config.xAxis ?? undefined}
						onValueChange={(v) => handleXAxisChange(v ?? null)}
					>
						<SelectTrigger class="h-8 text-xs">
							{config.xAxis ?? m.chart_row_index()}
						</SelectTrigger>
						<SelectContent>
							<SelectItem value="">{m.chart_row_index()}</SelectItem>
							{#each columns as column}
								<SelectItem value={column}>{column}</SelectItem>
							{/each}
						</SelectContent>
					</Select>
				</div>

				<!-- Y Axis -->
				<div class="grid gap-1.5">
					<Label class="text-xs">{m.chart_y_axis()}</Label>
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
						<p class="text-xs text-destructive">{m.chart_select_column()}</p>
					{/if}
				</div>
			</div>
		</div>
	</PopoverContent>
</Popover>
