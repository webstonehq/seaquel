<script lang="ts">
	import { SvelteFlowProvider } from '@xyflow/svelte';
	import { onMount } from 'svelte';
	import * as Resizable from '$lib/components/ui/resizable';
	import { setQueryBuilder, QueryBuilderState } from '$lib/hooks/query-builder.svelte';
	import type { QueryBuilderTable, SchemaTable } from '$lib/types';
	import QueryBuilderCanvas from '$lib/components/query-builder/canvas.svelte';
	import FilterPanel from '$lib/components/query-builder/filter-panel.svelte';
	import TablePalette from '$lib/components/query-builder/table-palette.svelte';
	import SqlEditor from '$lib/components/query-builder/sql-editor.svelte';
	import DndProvider from '$lib/components/query-builder/dnd-provider.svelte';
	import { parseSql } from '$lib/tutorial/sql-parser';

	interface Props {
		/** Schema tables to show in the palette (QueryBuilderTable format) */
		schema: QueryBuilderTable[];
		/** Original schema for Monaco autocomplete (SchemaTable format) */
		monacoSchema?: SchemaTable[];
		/** Initial SQL value to load into the builder */
		initialSql: string;
		/** Callback to get the current SQL (called by parent to read current value) */
		getSql?: () => string;
	}

	let { schema, monacoSchema, initialSql, getSql = $bindable() }: Props = $props();

	// Create a new QueryBuilderState instance for this panel
	const qb = new QueryBuilderState();
	setQueryBuilder(qb);

	// Set schema (reactive - updates when schema changes)
	$effect(() => {
		qb.setSchema(schema);
	});

	// Initialize from the provided SQL once on mount
	onMount(() => {
		if (initialSql.trim()) {
			try {
				const parsed = parseSql(initialSql, { validTableNames: null });
				if (parsed) {
					qb.applyFromParsedSql(parsed);
					// Set customSql so the editor shows the original formatting
					qb.customSql = initialSql;
				}
			} catch {
				// If parsing fails, just set as custom SQL
				qb.customSql = initialSql;
			}
		}
	});

	// Expose method to get current SQL
	getSql = () => qb.customSql ?? qb.generatedSql;
</script>

<SvelteFlowProvider>
	<DndProvider>
		<Resizable.PaneGroup direction="horizontal" class="h-full">
			<!-- Table Palette -->
			<Resizable.Pane defaultSize={15} minSize={12} maxSize={25}>
				<TablePalette {schema} />
			</Resizable.Pane>

			<Resizable.Handle />

			<!-- Canvas + Filter Panel -->
			<Resizable.Pane defaultSize={55} minSize={30}>
				<Resizable.PaneGroup direction="vertical">
					<Resizable.Pane defaultSize={60} minSize={20}>
						<!-- Canvas -->
						<div class="h-full">
							<QueryBuilderCanvas />
						</div>
					</Resizable.Pane>

					<Resizable.Handle withHandle />

					<Resizable.Pane defaultSize={40} minSize={15}>
						<!-- Filter Panel -->
						<FilterPanel />
					</Resizable.Pane>
				</Resizable.PaneGroup>
			</Resizable.Pane>

			<Resizable.Handle />

			<!-- SQL Editor (uses the same component as Learn sandbox) -->
			<Resizable.Pane defaultSize={30} minSize={20}>
				<SqlEditor schema={monacoSchema} />
			</Resizable.Pane>
		</Resizable.PaneGroup>
	</DndProvider>
</SvelteFlowProvider>
