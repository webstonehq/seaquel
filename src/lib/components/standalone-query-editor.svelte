<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { CopyIcon } from '@lucide/svelte';
	import { toast } from 'svelte-sonner';
	import MonacoEditor from '$lib/components/monaco-editor.svelte';
	import VirtualResultsTable from '$lib/components/virtual-results-table.svelte';
	import ParameterInputDialog from '$lib/components/parameter-input-dialog.svelte';
	import * as Resizable from '$lib/components/ui/resizable';
	import {
		hasParameters,
		extractParameters,
		createDefaultParameters,
		substituteParameters
	} from '$lib/db/query-params.js';
	import { parseSql } from '$lib/tutorial/sql-parser';
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import type { QueryExecutor, QueryParameter, ParameterValue, SchemaTable } from '$lib/types';
	import { m } from '$lib/paraglide/messages.js';

	interface Props {
		/** The query executor backend */
		executor: QueryExecutor;
		/** Current SQL value */
		value?: string;
		/** Callback when SQL changes */
		onSqlChange?: (sql: string) => void;
		/** Callback when executing state changes */
		onExecutingChange?: (isExecuting: boolean) => void;
		/** Schema for Monaco autocomplete */
		schema?: SchemaTable[];
		/** Enable two-way sync with visual query builder */
		enableVisualSync?: boolean;
	}

	let { executor, value = '', onSqlChange, onExecutingChange, schema, enableVisualSync = false }: Props = $props();

	// Get query builder for two-way sync (only if enabled at mount time).
	// enableVisualSync is only read at init; getContext() must run during component initialization.
	// svelte-ignore state_referenced_locally
	const qb = enableVisualSync ? useQueryBuilder() : null;

	// Local state
	let editorValue = $state('');
	let isExecuting = $state(false);
	let queryResults = $state<Record<string, unknown>[] | null>(null);
	let queryError = $state<string | null>(null);
	let executionTime = $state<number | null>(null);

	// Parameter dialog state
	let showParamsDialog = $state(false);
	let pendingParams = $state<QueryParameter[]>([]);

	// Sync external value prop changes to local editor state
	$effect(() => {
		editorValue = value;
	});

	// Derived
	const resultColumns = $derived(
		queryResults && queryResults.length > 0 ? Object.keys(queryResults[0]) : []
	);
	const canRunQuery = $derived(editorValue.trim().length > 0 && !isExecuting);

	function handleEditorChange(sql: string) {
		editorValue = sql;
		onSqlChange?.(sql);

		// Two-way sync: parse SQL and update visual query builder
		if (qb) {
			// Use null for validTableNames to accept any table (for real databases too)
			const parsed = parseSql(sql, { validTableNames: null });
			if (parsed) {
				qb.applyFromParsedSql(parsed);
			}
			// If parse fails, keep last valid visual state
		}
	}

	function handleExecute() {
		if (!canRunQuery) return;

		// Check for parameters
		if (hasParameters(editorValue)) {
			const paramNames = extractParameters(editorValue);
			pendingParams = createDefaultParameters(paramNames);
			showParamsDialog = true;
		} else {
			runQuery(editorValue);
		}
	}

	function handleParamExecute(values: ParameterValue[]) {
		showParamsDialog = false;
		// Substitute parameters with inline values (tutorial DB doesn't support bind params)
		const dbType = executor.dbType ?? 'duckdb';
		const { sql: substitutedSql } = substituteParameters(editorValue, values, dbType);
		runQuery(substitutedSql);
	}

	function handleParamCancel() {
		showParamsDialog = false;
	}

	async function runQuery(sql: string) {
		isExecuting = true;
		onExecutingChange?.(true);
		queryError = null;
		const startTime = performance.now();

		try {
			queryResults = await executor.execute(sql);
			executionTime = performance.now() - startTime;
		} catch (err) {
			queryError = err instanceof Error ? err.message : String(err);
			queryResults = null;
			executionTime = null;
		} finally {
			isExecuting = false;
			onExecutingChange?.(false);
		}
	}

	async function handleCopy() {
		try {
			await navigator.clipboard.writeText(editorValue);
			toast.success('SQL copied to clipboard');
		} catch {
			toast.error('Failed to copy');
		}
	}

	async function copyError() {
		if (!queryError) return;
		try {
			await navigator.clipboard.writeText(queryError);
			toast.success(m.workspace_error_copied());
		} catch {
			toast.error(m.workspace_copy_error_failed());
		}
	}

	// Expose methods for parent components
	export function reset() {
		queryResults = null;
		queryError = null;
		executionTime = null;
	}

	export function executeQuery() {
		handleExecute();
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
</script>

<div class="flex h-full flex-col">
	<Resizable.PaneGroup direction="vertical" class="flex-1">
		<!-- SQL Editor Pane -->
		<Resizable.Pane defaultSize={50} minSize={20}>
			<div class="flex h-full flex-col border-b">
				<!-- Toolbar -->
				<div class="flex items-center justify-between border-b bg-muted/50 px-3 py-2">
					<span class="text-sm font-medium">SQL</span>
					<div class="flex items-center gap-1">
						<Button
							variant="ghost"
							size="sm"
							class="h-7 gap-1.5 px-2 text-xs"
							onclick={handleCopy}
						>
							<CopyIcon class="size-3.5" />
							Copy
						</Button>
					</div>
				</div>

				<!-- Editor -->
				<div class="min-h-0 flex-1">
					<MonacoEditor
						bind:value={editorValue}
						{schema}
						onChange={handleEditorChange}
						onExecute={handleExecute}
					/>
				</div>
			</div>
		</Resizable.Pane>

		<Resizable.Handle withHandle />

		<!-- Results Pane -->
		<Resizable.Pane defaultSize={50} minSize={15}>
			<div class="flex h-full flex-col">
				<div class="flex items-center justify-between border-b bg-muted/50 px-3 py-2">
					<span class="text-sm font-medium">{m.workspace_results()}</span>
					{#if queryResults !== null}
						<span class="text-xs text-muted-foreground">
							{queryResults.length === 1
								? m.workspace_row_count({ count: 1 })
								: m.workspace_rows_count({ count: queryResults.length })}
							{#if executionTime !== null}
								· {executionTime.toFixed(0)}ms
							{/if}
						</span>
					{/if}
				</div>
				<div class="min-h-0 flex-1 overflow-auto">
					{#if queryError}
						<div class="bg-destructive/10 p-4 text-sm text-destructive">
							<div class="flex items-start justify-between gap-2">
								<div>
									<p class="font-medium">{m.workspace_error()}</p>
									<p class="mt-1 font-mono text-xs">{queryError}</p>
								</div>
								<Button
									variant="ghost"
									size="icon"
									class="size-8 shrink-0 text-destructive/70 hover:bg-destructive/10 hover:text-destructive"
									onclick={copyError}
								>
									<CopyIcon class="size-4" />
								</Button>
							</div>
						</div>
					{:else if queryResults !== null && queryResults.length > 0}
						<VirtualResultsTable columns={resultColumns} rows={queryResults} compact />
					{:else if queryResults !== null}
						<div class="p-4 text-center text-sm text-muted-foreground">
							{m.workspace_no_rows()}
						</div>
					{:else}
						<div class="p-4 text-center text-sm text-muted-foreground">
							{m.workspace_run_query_hint()}
						</div>
					{/if}
				</div>
			</div>
		</Resizable.Pane>
	</Resizable.PaneGroup>
</div>

<ParameterInputDialog
	bind:open={showParamsDialog}
	parameters={pendingParams}
	onExecute={handleParamExecute}
	onCancel={handleParamCancel}
/>
