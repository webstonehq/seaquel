<script lang="ts">
	import { SidebarInset } from '$lib/components/ui/sidebar';
	import SidebarLeft from '$lib/components/sidebar-left.svelte';
	import {
		QueryBuilderCanvas,
		TablePalette,
		FilterPanel,
		SqlEditor
	} from '$lib/components/query-builder';
	import { QueryBuilderState, setQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import * as Resizable from '$lib/components/ui/resizable';
	import { Button } from '$lib/components/ui/button';
	import { RotateCcwIcon, PlayIcon } from '@lucide/svelte';

	// Initialize query builder state
	const qb = setQueryBuilder(new QueryBuilderState());

	function handleReset() {
		qb.reset();
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
			<Button size="sm" disabled>
				<PlayIcon class="size-4 mr-2" />
				Run Query
			</Button>
		</div>
	</div>

	<!-- Main content -->
	<div class="flex-1 flex min-h-0">
		<!-- Table palette -->
		<TablePalette />

		<!-- Canvas + SQL editor -->
		<Resizable.PaneGroup direction="horizontal" class="flex-1">
			<Resizable.Pane defaultSize={60} minSize={30}>
				<div class="flex flex-col h-full">
					<!-- Canvas -->
					<div class="flex-1 min-h-0">
						<QueryBuilderCanvas />
					</div>
					<!-- Filter panel -->
					<FilterPanel />
				</div>
			</Resizable.Pane>

			<Resizable.Handle withHandle />

			<Resizable.Pane defaultSize={40} minSize={20}>
				<SqlEditor />
			</Resizable.Pane>
		</Resizable.PaneGroup>
	</div>
</SidebarInset>
