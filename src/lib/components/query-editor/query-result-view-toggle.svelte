<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Badge } from '$lib/components/ui/badge';
	import { TableIcon, BarChart3Icon, DatabaseIcon, NetworkIcon } from '@lucide/svelte';
	import type { ResultViewMode } from '$lib/types';
	import { m } from '$lib/paraglide/messages.js';

	type Props = {
		mode: ResultViewMode;
		onModeChange: (mode: ResultViewMode) => void;
		hasExplainResult?: boolean;
		hasVisualizeResult?: boolean;
		isExplainStale?: boolean;
		isVisualizeStale?: boolean;
	};

	let {
		mode,
		onModeChange,
		hasExplainResult = false,
		hasVisualizeResult = false,
		isExplainStale = false,
		isVisualizeStale = false
	}: Props = $props();
</script>

<div class="flex items-center gap-0.5 rounded-md border bg-muted/50 p-0.5">
	<Button
		variant={mode === 'table' ? 'default' : 'ghost'}
		size="sm"
		class="h-6 gap-1 px-2"
		onclick={() => onModeChange('table')}
	>
		<TableIcon class="size-3" />
		<span class="text-xs">{m.result_view_table()}</span>
	</Button>
	<Button
		variant={mode === 'chart' ? 'default' : 'ghost'}
		size="sm"
		class="h-6 gap-1 px-2"
		onclick={() => onModeChange('chart')}
	>
		<BarChart3Icon class="size-3" />
		<span class="text-xs">{m.result_view_chart()}</span>
	</Button>
	<Button
		variant={mode === 'explain' ? 'default' : 'ghost'}
		size="sm"
		class="h-6 gap-1 px-2"
		onclick={() => onModeChange('explain')}
	>
		<DatabaseIcon class="size-3" />
		<span class="text-xs">{m.result_view_explain()}</span>
	</Button>
	<Button
		variant={mode === 'visualize' ? 'default' : 'ghost'}
		size="sm"
		class="h-6 gap-1 px-2"
		onclick={() => onModeChange('visualize')}
	>
		<NetworkIcon class="size-3" />
		<span class="text-xs">{m.result_view_visual()}</span>
	</Button>
</div>
