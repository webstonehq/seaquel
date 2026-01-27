<script lang="ts">
	import { SvelteFlowProvider } from '@xyflow/svelte';
	import * as Resizable from '$lib/components/ui/resizable';
	import { Button } from '$lib/components/ui/button';
	import { CopyIcon } from '@lucide/svelte';
	import { toast } from 'svelte-sonner';
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import { executeQuery } from '$lib/tutorial/database';
	import VirtualResultsTable from '$lib/components/virtual-results-table.svelte';
	import QueryBuilderCanvas from './canvas.svelte';
	import FilterPanel from './filter-panel.svelte';
	import SqlEditor from './sql-editor.svelte';
	import DndProvider from './dnd-provider.svelte';

	interface Props {
		/** Content for the left sidebar (e.g., table palette, challenge card) */
		leftPanel?: import('svelte').Snippet;
	}

	let { leftPanel }: Props = $props();

	const qb = useQueryBuilder();

	// Query execution state
	let isExecuting = $state(false);
	let queryResults = $state<Record<string, unknown>[] | null>(null);
	let queryError = $state<string | null>(null);
	let executionTime = $state<number | null>(null);

	// Derived columns from results
	const resultColumns = $derived(
		queryResults && queryResults.length > 0 ? Object.keys(queryResults[0]) : []
	);

	// The SQL to execute - use custom SQL if user has typed something, otherwise use generated SQL
	const activeSql = $derived(qb.customSql ?? qb.generatedSql);

	// Can run query if there's SQL and not currently executing
	const canRunQuery = $derived(activeSql.trim().length > 0 && !isExecuting);

	export function reset() {
		queryResults = null;
		queryError = null;
		executionTime = null;
	}

	export async function runQuery() {
		if (!canRunQuery) return;

		isExecuting = true;
		queryError = null;
		const startTime = performance.now();

		try {
			queryResults = await executeQuery(activeSql);
			executionTime = performance.now() - startTime;
		} catch (err) {
			queryError = err instanceof Error ? err.message : String(err);
			queryResults = null;
			executionTime = null;
		} finally {
			isExecuting = false;
		}
	}

	export function getState() {
		return {
			isExecuting,
			queryResults,
			queryError,
			executionTime,
			canRunQuery
		};
	}

	async function copyError() {
		if (!queryError) return;
		try {
			await navigator.clipboard.writeText(queryError);
			toast.success('Error copied to clipboard');
		} catch {
			toast.error('Failed to copy error');
		}
	}
</script>

<SvelteFlowProvider>
	<DndProvider>
		<div class="flex-1 flex min-h-0">
			<!-- Left panel -->
			{#if leftPanel}
				{@render leftPanel()}
			{/if}

			<!-- Canvas + SQL editor + Results -->
			<Resizable.PaneGroup direction="horizontal" class="flex-1">
				<Resizable.Pane defaultSize={60} minSize={30}>
					<Resizable.PaneGroup direction="vertical">
						<Resizable.Pane defaultSize={65} minSize={20}>
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

						<Resizable.Pane defaultSize={35} minSize={15}>
							<!-- Results panel -->
							<div class="flex flex-col h-full border-t">
								<div class="flex items-center justify-between px-3 py-2 border-b bg-muted/50">
									<span class="font-medium text-sm">Results</span>
									{#if queryResults !== null}
										<span class="text-xs text-muted-foreground">
											{queryResults.length} row{queryResults.length !== 1 ? 's' : ''}
											{#if executionTime !== null}
												· {executionTime.toFixed(0)}ms
											{/if}
										</span>
									{/if}
								</div>
								<div class="flex-1 min-h-0 overflow-auto">
									{#if queryError}
										<div class="p-4 text-sm text-destructive bg-destructive/10">
											<div class="flex items-start justify-between gap-2">
												<div>
													<p class="font-medium">Error</p>
													<p class="font-mono text-xs mt-1">{queryError}</p>
												</div>
												<Button
													variant="ghost"
													size="icon"
													class="shrink-0 size-8 text-destructive/70 hover:text-destructive hover:bg-destructive/10"
													aria-label="Copy error"
													onclick={copyError}
												>
													<CopyIcon class="size-4" />
												</Button>
											</div>
										</div>
									{:else if queryResults !== null && queryResults.length > 0}
										<VirtualResultsTable columns={resultColumns} rows={queryResults} compact />
									{:else if queryResults !== null}
										<div class="p-4 text-sm text-muted-foreground text-center">
											Query executed successfully. No rows returned.
										</div>
									{:else}
										<div class="p-4 text-sm text-muted-foreground text-center">
											Build a query and click "Run Query" to see results
										</div>
									{/if}
								</div>
							</div>
						</Resizable.Pane>
					</Resizable.PaneGroup>
				</Resizable.Pane>

				<Resizable.Handle withHandle />

				<Resizable.Pane defaultSize={40} minSize={20}>
					<SqlEditor />
				</Resizable.Pane>
			</Resizable.PaneGroup>
		</div>
	</DndProvider>
</SvelteFlowProvider>
