<script lang="ts">
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import { Button } from '$lib/components/ui/button';
	import MonacoEditor from '$lib/components/monaco-editor.svelte';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import { toast } from 'svelte-sonner';
	import { parseSql } from '$lib/tutorial/sql-parser';
	import { getTutorialSchema } from '$lib/tutorial/database';
	import { TUTORIAL_SCHEMA } from '$lib/tutorial/schema';
	import type { SchemaTable } from '$lib/types';

	interface Props {
		/** Optional schema for Monaco autocomplete (for Manage section with real databases) */
		schema?: SchemaTable[];
	}

	let { schema: externalSchema }: Props = $props();

	const qb = useQueryBuilder();

	// Check if we're using tutorial schema or a custom schema (for Manage section)
	const isTutorialSchema = $derived(
		qb.schema.length === TUTORIAL_SCHEMA.length &&
		qb.schema.every((t, i) => t.name === TUTORIAL_SCHEMA[i].name)
	);

	// Get valid table names for parsing - null means accept all tables (for real databases)
	const validTableNames = $derived(
		isTutorialSchema ? undefined : null
	);

	// Get schema for Monaco autocomplete - use external schema if provided, else tutorial schema
	const monacoSchema = $derived(externalSchema ?? getTutorialSchema());

	// Track if we're updating from external source (generated SQL changes)
	let isUpdatingFromExternal = false;

	// Local editor value that syncs with query builder
	let editorValue = $state(qb.customSql ?? qb.generatedSql);

	// Update editor when generated SQL changes (from visual edits on canvas)
	$effect(() => {
		const generated = qb.generatedSql;

		// If user has custom SQL, only update if it was explicitly cleared (customSql became null)
		// This preserves user's typed SQL while they're working
		if (qb.customSql !== null) {
			// User has custom SQL - only update if customSql changed externally (e.g., loaded from storage)
			if (editorValue !== qb.customSql && !isUpdatingFromExternal) {
				isUpdatingFromExternal = true;
				editorValue = qb.customSql;
				isUpdatingFromExternal = false;
			}
			return;
		}

		// No custom SQL - sync with generated SQL from canvas
		// Only update if the generated SQL is different and we're not currently processing user input
		// This prevents cursor jumping when user is typing
		if (editorValue !== generated && !isUpdatingFromExternal) {
			// Check if the current value parses to the same visual state
			// If so, don't update (user's formatting is preserved)
			const currentParsed = parseSql(editorValue, { validTableNames });
			const generatedParsed = parseSql(generated, { validTableNames });

			// If both parse to equivalent states, keep user's formatting
			if (currentParsed && generatedParsed && statesEqual(currentParsed, generatedParsed)) {
				return;
			}

			isUpdatingFromExternal = true;
			editorValue = generated;
			isUpdatingFromExternal = false;
		}
	});

	function handleChange(sql: string) {
		if (isUpdatingFromExternal) return;

		// Save the user's SQL text
		qb.customSql = sql;

		// Try to parse the SQL and update visual state
		const parsed = parseSql(sql, { validTableNames });
		if (parsed) {
			// Valid SQL - update visual state (two-way sync)
			qb.applyFromParsedSql(parsed);
		}
		// If parse fails, keep last valid visual state (option 1)
	}

	// Compare two parsed query states for equality (ignoring formatting differences)
	// Recursive to support nested subqueries
	function statesEqual(
		a: NonNullable<ReturnType<typeof parseSql>>,
		b: NonNullable<ReturnType<typeof parseSql>>
	): boolean {
		return innerQueriesEqual(a, b);
	}

	// Helper to compare inner query structures recursively
	function innerQueriesEqual(
		a: {
			tables: Array<{ tableName: string; selectedColumns: string[] }>;
			joins: Array<{ sourceTable: string; sourceColumn: string; targetTable: string; targetColumn: string; joinType: string }>;
			filters: Array<{ column: string; operator: string; value: string; connector: string; subqueryIndex?: number }>;
			groupBy: Array<{ column: string }>;
			having: Array<{ aggregateFunction: string; column: string; operator: string; value: string; connector: string }>;
			orderBy: Array<{ column: string; direction: string }>;
			limit: number | null;
			selectAggregates: Array<{ function: string; expression: string; alias?: string }>;
			columnAggregates: Array<{ tableName: string; column: string; function: string; alias?: string }>;
			subqueries: Array<{ role: string; innerQuery: unknown }>;
			ctes?: Array<{ name: string; innerQuery: unknown }>;
		},
		b: {
			tables: Array<{ tableName: string; selectedColumns: string[] }>;
			joins: Array<{ sourceTable: string; sourceColumn: string; targetTable: string; targetColumn: string; joinType: string }>;
			filters: Array<{ column: string; operator: string; value: string; connector: string; subqueryIndex?: number }>;
			groupBy: Array<{ column: string }>;
			having: Array<{ aggregateFunction: string; column: string; operator: string; value: string; connector: string }>;
			orderBy: Array<{ column: string; direction: string }>;
			limit: number | null;
			selectAggregates: Array<{ function: string; expression: string; alias?: string }>;
			columnAggregates: Array<{ tableName: string; column: string; function: string; alias?: string }>;
			subqueries: Array<{ role: string; innerQuery: unknown }>;
			ctes?: Array<{ name: string; innerQuery: unknown }>;
		}
	): boolean {
		// Compare tables
		if (a.tables.length !== b.tables.length) return false;
		for (let i = 0; i < a.tables.length; i++) {
			if (a.tables[i].tableName !== b.tables[i].tableName) return false;
			const aColsSorted = [...a.tables[i].selectedColumns].sort();
			const bColsSorted = [...b.tables[i].selectedColumns].sort();
			if (aColsSorted.join(',') !== bColsSorted.join(',')) return false;
		}

		// Compare joins
		if (a.joins.length !== b.joins.length) return false;
		for (let i = 0; i < a.joins.length; i++) {
			if (
				a.joins[i].sourceTable !== b.joins[i].sourceTable ||
				a.joins[i].sourceColumn !== b.joins[i].sourceColumn ||
				a.joins[i].targetTable !== b.joins[i].targetTable ||
				a.joins[i].targetColumn !== b.joins[i].targetColumn ||
				a.joins[i].joinType !== b.joins[i].joinType
			)
				return false;
		}

		// Compare filters
		if (a.filters.length !== b.filters.length) return false;
		for (let i = 0; i < a.filters.length; i++) {
			if (
				a.filters[i].column !== b.filters[i].column ||
				a.filters[i].operator !== b.filters[i].operator ||
				a.filters[i].value !== b.filters[i].value ||
				a.filters[i].connector !== b.filters[i].connector ||
				a.filters[i].subqueryIndex !== b.filters[i].subqueryIndex
			)
				return false;
		}

		// Compare group by
		if (a.groupBy.length !== b.groupBy.length) return false;
		for (let i = 0; i < a.groupBy.length; i++) {
			if (a.groupBy[i].column !== b.groupBy[i].column) return false;
		}

		// Compare having
		if (a.having.length !== b.having.length) return false;
		for (let i = 0; i < a.having.length; i++) {
			if (
				a.having[i].aggregateFunction !== b.having[i].aggregateFunction ||
				a.having[i].column !== b.having[i].column ||
				a.having[i].operator !== b.having[i].operator ||
				a.having[i].value !== b.having[i].value ||
				a.having[i].connector !== b.having[i].connector
			)
				return false;
		}

		// Compare order by
		if (a.orderBy.length !== b.orderBy.length) return false;
		for (let i = 0; i < a.orderBy.length; i++) {
			if (
				a.orderBy[i].column !== b.orderBy[i].column ||
				a.orderBy[i].direction !== b.orderBy[i].direction
			)
				return false;
		}

		// Compare limit
		if (a.limit !== b.limit) return false;

		// Compare select aggregates
		if (a.selectAggregates.length !== b.selectAggregates.length) return false;
		for (let i = 0; i < a.selectAggregates.length; i++) {
			if (
				a.selectAggregates[i].function !== b.selectAggregates[i].function ||
				a.selectAggregates[i].expression !== b.selectAggregates[i].expression ||
				a.selectAggregates[i].alias !== b.selectAggregates[i].alias
			)
				return false;
		}

		// Compare column aggregates
		if (a.columnAggregates.length !== b.columnAggregates.length) return false;
		for (let i = 0; i < a.columnAggregates.length; i++) {
			if (
				a.columnAggregates[i].tableName !== b.columnAggregates[i].tableName ||
				a.columnAggregates[i].column !== b.columnAggregates[i].column ||
				a.columnAggregates[i].function !== b.columnAggregates[i].function ||
				a.columnAggregates[i].alias !== b.columnAggregates[i].alias
			)
				return false;
		}

		// Compare subqueries recursively
		if (a.subqueries.length !== b.subqueries.length) return false;
		for (let i = 0; i < a.subqueries.length; i++) {
			const aSq = a.subqueries[i];
			const bSq = b.subqueries[i];

			if (aSq.role !== bSq.role) return false;

			// Recursively compare inner queries (including nested subqueries)
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			if (!innerQueriesEqual(aSq.innerQuery as any, bSq.innerQuery as any)) {
				return false;
			}
		}

		// Compare CTEs recursively
		const aCtes = a.ctes || [];
		const bCtes = b.ctes || [];
		if (aCtes.length !== bCtes.length) return false;
		for (let i = 0; i < aCtes.length; i++) {
			const aCte = aCtes[i];
			const bCte = bCtes[i];

			if (aCte.name !== bCte.name) return false;

			// Recursively compare inner queries
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			if (!innerQueriesEqual(aCte.innerQuery as any, bCte.innerQuery as any)) {
				return false;
			}
		}

		return true;
	}

	function handleCopy() {
		navigator.clipboard.writeText(qb.generatedSql);
		toast.success('SQL copied to clipboard');
	}

	function handleReset() {
		qb.reset(); // This also clears customSql
		isUpdatingFromExternal = true;
		editorValue = qb.generatedSql;
		isUpdatingFromExternal = false;
	}
</script>

<div class="flex flex-col h-full border-l">
	<!-- Header -->
	<div class="flex items-center justify-between px-3 py-2 border-b bg-muted/50">
		<span class="font-medium text-sm">SQL</span>
		<div class="flex items-center gap-1">
			<Button
				variant="ghost"
				size="sm"
				class="h-7 px-2 gap-1.5 text-xs"
				onclick={handleReset}
				title="Clear and reset"
			>
				<RotateCcwIcon class="size-3.5" />
				Reset
			</Button>
			<Button
				variant="ghost"
				size="sm"
				class="h-7 px-2 gap-1.5 text-xs"
				onclick={handleCopy}
				title="Copy SQL to clipboard"
			>
				<CopyIcon class="size-3.5" />
				Copy
			</Button>
		</div>
	</div>

	<!-- Editor using shared MonacoEditor component -->
	<div class="flex-1 min-h-0">
		<MonacoEditor bind:value={editorValue} schema={monacoSchema} onChange={handleChange} />
	</div>
</div>
