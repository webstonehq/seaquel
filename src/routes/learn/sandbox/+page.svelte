<script lang="ts">
	import { SidebarInset } from '$lib/components/ui/sidebar';
	import SidebarLeft from '$lib/components/sidebar-left.svelte';
	import { QueryBuilderWorkspace, TablePalette } from '$lib/components/query-builder';
	import { QueryBuilderState, setQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import { Button } from '$lib/components/ui/button';
	import { RotateCcwIcon, PlayIcon, Loader2Icon } from '@lucide/svelte';

	// Initialize query builder state
	const qb = setQueryBuilder(new QueryBuilderState());

	let workspace = $state<ReturnType<typeof QueryBuilderWorkspace> | undefined>();
	const workspaceState = $derived(workspace?.getState?.() ?? { canRunQuery: false, isExecuting: false });

	function handleReset() {
		qb.reset();
		workspace?.reset?.();
	}

	function handleRunQuery() {
		workspace?.runQuery?.();
	}
</script>

<SidebarLeft />

<SidebarInset class="flex flex-col h-full overflow-hidden">
	<!-- Header -->
	<div class="flex items-center justify-between px-4 py-2 border-b">
		<div>
			<h1 class="font-semibold">SQL Sandbox</h1>
			<p class="text-sm text-muted-foreground">Build queries visually - drag tables to start</p>
		</div>
		<div class="flex items-center gap-2">
			<Button variant="outline" size="sm" onclick={handleReset}>
				<RotateCcwIcon class="size-4 mr-2" />
				Reset
			</Button>
			<Button size="sm" disabled={!workspaceState.canRunQuery} onclick={handleRunQuery}>
				{#if workspaceState.isExecuting}
					<Loader2Icon class="size-4 mr-2 animate-spin" />
					Running...
				{:else}
					<PlayIcon class="size-4 mr-2" />
					Run Query
				{/if}
			</Button>
		</div>
	</div>

	<!-- Main content -->
	<QueryBuilderWorkspace bind:this={workspace}>
		{#snippet leftPanel()}
			<div class="w-56 shrink-0 border-r flex flex-col">
				<TablePalette />
			</div>
		{/snippet}
	</QueryBuilderWorkspace>
</SidebarInset>
