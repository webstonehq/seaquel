<script lang="ts">
	import { SvelteFlowProvider } from '@xyflow/svelte';
	import * as Resizable from '$lib/components/ui/resizable';
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import { executeQuery, getTutorialSchema } from '$lib/tutorial/database';
	import QueryBuilderCanvas from './canvas.svelte';
	import FilterPanel from './filter-panel.svelte';
	import DndProvider from './dnd-provider.svelte';
	import StandaloneQueryEditor from '$lib/components/standalone-query-editor.svelte';
	import type { QueryExecutor } from '$lib/types';

	interface Props {
		/** Content for the left sidebar (e.g., table palette, challenge card) */
		leftPanel?: import('svelte').Snippet;
		/** Callback when executing state changes */
		onExecutingChange?: (isExecuting: boolean) => void;
	}

	let { leftPanel, onExecutingChange }: Props = $props();

	const qb = useQueryBuilder();

	// Create tutorial executor
	const tutorialExecutor: QueryExecutor = {
		execute: executeQuery,
		dbType: 'duckdb'
	};

	// Get schema for Monaco autocomplete
	const tutorialSchema = getTutorialSchema();

	// The SQL value - use custom SQL if user has typed something, otherwise use generated SQL
	const sqlValue = $derived(qb.customSql ?? qb.generatedSql);

	// Handle SQL changes from the editor
	function handleSqlChange(sql: string) {
		qb.customSql = sql;
	}

	// Reference to the editor for external control
	let editorRef: { reset: () => void; executeQuery: () => void; getState: () => { isExecuting: boolean; queryResults: Record<string, unknown>[] | null; queryError: string | null; executionTime: number | null; canRunQuery: boolean } } | undefined;

	// Reactive state for external consumers
	let isExecutingState = $state(false);
	const canRunQueryState = $derived(sqlValue.trim().length > 0 && !isExecutingState);

	export function reset() {
		editorRef?.reset();
	}

	export async function runQuery() {
		editorRef?.executeQuery();
	}

	function setExecuting(value: boolean) {
		isExecutingState = value;
		onExecutingChange?.(value);
	}

	export function getState() {
		return {
			canRunQuery: canRunQueryState,
			isExecuting: isExecutingState
		};
	}
</script>

<SvelteFlowProvider>
	<DndProvider>
		<div class="flex-1 flex min-h-0">
			<!-- Left panel -->
			{#if leftPanel}
				{@render leftPanel()}
			{/if}

			<!-- Canvas + Filter panel | SQL editor + Results -->
			<Resizable.PaneGroup direction="horizontal" class="flex-1">
				<Resizable.Pane defaultSize={55} minSize={30}>
					<Resizable.PaneGroup direction="vertical">
						<Resizable.Pane defaultSize={55} minSize={15}>
							<!-- Canvas -->
							<div class="h-full">
								<QueryBuilderCanvas />
							</div>
						</Resizable.Pane>

						<Resizable.Handle withHandle />

						<Resizable.Pane defaultSize={45} minSize={10}>
							<!-- Filter panel -->
							<div class="h-full">
								<FilterPanel />
							</div>
						</Resizable.Pane>
					</Resizable.PaneGroup>
				</Resizable.Pane>

				<Resizable.Handle withHandle />

				<Resizable.Pane defaultSize={45} minSize={20}>
					<!-- SQL Editor + Results (unified component) -->
					<div class="h-full border-l">
						<StandaloneQueryEditor
							bind:this={editorRef}
							executor={tutorialExecutor}
							value={sqlValue}
							onSqlChange={handleSqlChange}
							onExecutingChange={setExecuting}
							schema={tutorialSchema}
							enableVisualSync
						/>
					</div>
				</Resizable.Pane>
			</Resizable.PaneGroup>
		</div>
	</DndProvider>
</SvelteFlowProvider>
