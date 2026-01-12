<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { TableIcon, BarChart3Icon } from '@lucide/svelte';
	import type { ResultViewMode } from '$lib/types';

	type Props = {
		mode: ResultViewMode;
		onModeChange: (mode: ResultViewMode) => void;
	};

	let { mode, onModeChange }: Props = $props();
</script>

<div class="flex items-center gap-0.5 rounded-md border bg-muted/50 p-0.5">
	<Button
		variant={mode === 'table' ? 'default' : 'ghost'}
		size="sm"
		class="h-6 gap-1 px-2"
		onclick={() => onModeChange('table')}
	>
		<TableIcon class="size-3" />
		<span class="text-xs">Table</span>
	</Button>
	<Button
		variant={mode === 'chart' ? 'default' : 'ghost'}
		size="sm"
		class="h-6 gap-1 px-2"
		onclick={() => onModeChange('chart')}
	>
		<BarChart3Icon class="size-3" />
		<span class="text-xs">Chart</span>
	</Button>
</div>
