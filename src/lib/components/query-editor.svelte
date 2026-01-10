<script lang="ts">
	import { onMount, onDestroy } from "svelte";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { useShortcuts } from "$lib/shortcuts/index.js";
	import { useSidebar } from "$lib/components/ui/sidebar/context.svelte.js";
	import { Button } from "$lib/components/ui/button";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { PlayIcon } from "@lucide/svelte";
	import { toast } from "svelte-sonner";
	import SaveQueryDialog from "$lib/components/save-query-dialog.svelte";
	import ParameterInputDialog from "$lib/components/parameter-input-dialog.svelte";
	import MonacoEditor, { type MonacoEditorRef } from "$lib/components/monaco-editor.svelte";
	import * as Resizable from "$lib/components/ui/resizable";
	import { save } from "@tauri-apps/plugin-dialog";
	import { writeTextFile } from "@tauri-apps/plugin-fs";
	import { format as formatSQL } from "sql-formatter";
	import VirtualResultsTable from "$lib/components/virtual-results-table.svelte";
	import { formatConfig, getExportContent, type ExportFormat } from "$lib/utils/export-formats.js";
	import { m } from "$lib/paraglide/messages.js";
	import { splitSqlStatements } from "$lib/db/sql-parser.js";
	import { hasParameters, extractParameters, createDefaultParameters } from "$lib/db/query-params.js";
	import type { QueryParameter, ParameterValue } from "$lib/types";
	import QueryExampleCard from "$lib/components/empty-states/query-example-card.svelte";
	import { sampleQueries } from "$lib/config/sample-queries.js";
	import PlusIcon from "@lucide/svelte/icons/plus";

	// Import subcomponents
	import {
		QueryToolbar,
		QueryResultTabs,
		QueryExportMenu,
		QueryPagination,
		QueryErrorDisplay
	} from "$lib/components/query-editor/index.js";

	const db = useDatabase();
	const shortcuts = useShortcuts();
	const sidebar = useSidebar();
	let showSaveDialog = $state(false);
	let showParamsDialog = $state(false);
	let pendingParams = $state<QueryParameter[]>([]);
	// Track pending action type: 'query' for regular execute, or explain details
	let pendingAction = $state<'query' | { type: 'explain'; analyze: boolean; cursorOffset: number } | null>(null);
	let deletingRowIndex = $state<number | null>(null);
	let pendingDeleteRow = $state<{ index: number; row: Record<string, unknown> } | null>(null);
	let showDeleteConfirm = $state(false);
	let monacoRef = $state<MonacoEditorRef | null>(null);

	// Get the active result (for multi-statement support)
	const activeResult = $derived(db.state.activeQueryResult);
	const activeResultIndex = $derived(db.state.activeQueryTab?.activeResultIndex ?? 0);
	const allResults = $derived(db.state.activeQueryTab?.results ?? []);

	// Track query content for live statement count
	let currentQuery = $state(db.state.activeQueryTab?.query ?? '');

	// Update currentQuery when active tab changes
	$effect(() => {
		currentQuery = db.state.activeQueryTab?.query ?? '';
	});

	// Live statement count from current query text
	const liveStatementCount = $derived.by(() => {
		if (!currentQuery?.trim()) return 0;
		const dbType = db.state.activeConnection?.type ?? "postgres";
		return splitSqlStatements(currentQuery, dbType).length;
	});

	// Check if results are editable (have source table with primary keys)
	const isEditable = $derived(
		activeResult?.sourceTable &&
		activeResult?.sourceTable.primaryKeys.length > 0 &&
		!activeResult?.isError
	);

	// Get sample queries for the active connection type
	const activeSampleQueries = $derived(
		sampleQueries[db.state.activeConnection?.type ?? "postgres"]?.slice(0, 2) ?? []
	);

	// Handle trying a sample query
	const handleTrySampleQuery = (query: string) => {
		if (db.state.activeQueryTabId) {
			db.queryTabs.updateContent(db.state.activeQueryTabId, query);
		} else {
			db.queryTabs.add(undefined, query);
		}
	};

	async function handleCellSave(rowIndex: number, column: string, newValue: string) {
		if (!db.state.activeQueryTabId || !activeResult?.sourceTable) return;

		const result = await db.queries.updateCell(
			db.state.activeQueryTabId,
			activeResultIndex,
			rowIndex,
			column,
			newValue,
			activeResult.sourceTable
		);

		if (result.success) {
			toast.success(m.query_cell_updated());
		} else {
			toast.error(m.query_cell_update_failed({ error: result.error || '' }));
		}
	}

	function confirmDeleteRow(rowIndex: number, row: Record<string, unknown>) {
		pendingDeleteRow = { index: rowIndex, row };
		showDeleteConfirm = true;
	}

	async function handleDeleteRow() {
		if (!pendingDeleteRow || !db.state.activeQueryTabId || !activeResult?.sourceTable) return;

		deletingRowIndex = pendingDeleteRow.index;
		showDeleteConfirm = false;

		const result = await db.queries.deleteRow(
			activeResult.sourceTable,
			pendingDeleteRow.row
		);

		if (result.success) {
			toast.success(m.query_row_deleted());
			await db.queries.execute(db.state.activeQueryTabId);
		} else {
			toast.error(m.query_row_delete_failed({ error: result.error || '' }));
		}

		deletingRowIndex = null;
		pendingDeleteRow = null;
	}

	/**
	 * Get parameter definitions for the active query.
	 * Uses linked saved query parameters if available, otherwise creates defaults.
	 */
	const getParameterDefinitions = (query: string): QueryParameter[] => {
		const savedQueryId = db.state.activeQueryTab?.savedQueryId;
		const savedQuery = savedQueryId
			? db.state.activeConnectionSavedQueries.find((q) => q.id === savedQueryId)
			: null;

		if (savedQuery?.parameters && savedQuery.parameters.length > 0) {
			return savedQuery.parameters;
		}

		// Create default parameters from extracted names
		const paramNames = extractParameters(query);
		return createDefaultParameters(paramNames);
	};

	const handleExecute = () => {
		if (!db.state.activeQueryTabId || !db.state.activeQueryTab) return;

		const query = db.state.activeQueryTab.query;

		// Check if query has parameters
		if (hasParameters(query)) {
			pendingParams = getParameterDefinitions(query);
			pendingAction = 'query';
			showParamsDialog = true;
		} else {
			// No parameters, execute directly
			db.queries.execute(db.state.activeQueryTabId);
		}
	};

	const handleParamExecute = (values: ParameterValue[]) => {
		if (!db.state.activeQueryTabId) return;

		if (pendingAction === 'query') {
			db.queries.executeWithParams(db.state.activeQueryTabId, values);
		} else if (pendingAction && pendingAction.type === 'explain') {
			db.explainTabs.executeWithParams(
				db.state.activeQueryTabId,
				values,
				pendingAction.analyze,
				pendingAction.cursorOffset
			);
		}
		pendingAction = null;
	};

	const handleParamCancel = () => {
		pendingAction = null;
	};

	const handleExplain = (analyze: boolean) => {
		if (!db.state.activeQueryTabId || !db.state.activeQueryTab) return;

		const query = db.state.activeQueryTab.query;
		const cursorOffset = monacoRef?.getCursorOffset() ?? 0;

		// Check if query has parameters
		if (hasParameters(query)) {
			pendingParams = getParameterDefinitions(query);
			pendingAction = { type: 'explain', analyze, cursorOffset };
			showParamsDialog = true;
		} else {
			// No parameters, execute directly
			db.explainTabs.execute(db.state.activeQueryTabId, analyze, cursorOffset);
		}
	};

	const handleSave = () => {
		if (!db.state.activeQueryTab?.query.trim()) return;
		showSaveDialog = true;
	};

	const handleFormat = () => {
		if (!db.state.activeQueryTab?.query.trim()) return;
		try {
			const formatted = formatSQL(db.state.activeQueryTab.query, {
				language: "postgresql",
				tabWidth: 2,
				keywordCase: "upper"
			});
			db.queryTabs.updateContent(db.state.activeQueryTabId!, formatted);
		} catch {
			toast.error(m.query_format_failed());
		}
	};

	const getContent = (format: ExportFormat): string => {
		if (!activeResult) return format === "json" ? "[]" : "";
		return getExportContent(format, activeResult.columns, activeResult.rows);
	};

	const handleExport = async (format: ExportFormat) => {
		if (!activeResult) return;

		const config = formatConfig[format];
		const timestamp = new Date().toISOString().slice(0, 19).replace(/[:-]/g, "");
		const defaultName = `query_results_${timestamp}.${config.extension}`;
		const filters = [{ name: config.name, extensions: [config.extension] }];

		const filePath = await save({
			defaultPath: defaultName,
			filters
		});

		if (!filePath) return;

		const content = getContent(format);
		await writeTextFile(filePath, content);
	};

	const handleCopy = async (format: ExportFormat) => {
		if (!activeResult) return;

		const content = getContent(format);
		const formatNames: Record<ExportFormat, string> = {
			csv: "CSV",
			json: "JSON",
			sql: "SQL INSERT",
			markdown: "Markdown"
		};

		try {
			await navigator.clipboard.writeText(content);
			toast.success(m.query_copied_to_clipboard({ format: formatNames[format] }));
		} catch {
			toast.error(m.query_copy_failed());
		}
	};

	// Context menu for copying cells
	let contextCell = $state<{ value: unknown; column: string; row: Record<string, unknown> } | null>(null);

	const handleCellRightClick = (value: unknown, column: string, row: Record<string, unknown>) => {
		contextCell = { value, column, row };
	};

	const copyCell = async () => {
		if (!contextCell) return;
		const value = contextCell.value === null || contextCell.value === undefined ? "" : String(contextCell.value);
		await navigator.clipboard.writeText(value);
		toast.success(m.query_cell_copied());
	};

	const copyRowAsJSON = async () => {
		if (!contextCell) return;
		await navigator.clipboard.writeText(JSON.stringify(contextCell.row, null, 2));
		toast.success(m.query_row_copied());
	};

	const copyColumn = async () => {
		if (!contextCell || !activeResult) return;
		const col = contextCell.column;
		const values = activeResult.rows
			.map(row => row[col])
			.map(v => v === null || v === undefined ? "" : String(v))
			.join("\n");
		await navigator.clipboard.writeText(values);
		toast.success(m.query_column_copied());
	};

	// Register keyboard shortcuts
	onMount(() => {
		shortcuts.registerHandler('saveQuery', handleSave);
		shortcuts.registerHandler('formatSql', handleFormat);
	});

	onDestroy(() => {
		shortcuts.unregisterHandler('saveQuery');
		shortcuts.unregisterHandler('formatSql');
	});
</script>

<div class="flex flex-col h-full overflow-hidden">
	{#if db.state.activeQueryTab}
		<QueryToolbar
			isExecuting={db.state.activeQueryTab.isExecuting}
			hasQuery={!!db.state.activeQueryTab.query.trim()}
			{activeResult}
			{liveStatementCount}
			onExecute={handleExecute}
			onExplain={handleExplain}
			onFormat={handleFormat}
			onSave={handleSave}
		/>

		<Resizable.PaneGroup direction="vertical" class="flex-1 min-h-0">
			<!-- Editor Pane -->
			<Resizable.Pane defaultSize={40} minSize={15}>
				<div class="h-full">
					{#key db.state.activeQueryTabId}
						<MonacoEditor
							bind:value={db.state.activeQueryTab.query}
							bind:ref={monacoRef}
							schema={db.state.activeSchema}
							onExecute={handleExecute}
							onToggleSidebar={() => sidebar.toggle()}
							onChange={(newValue) => {
								currentQuery = newValue;
								if (db.state.activeQueryTabId) {
									db.queryTabs.updateContent(db.state.activeQueryTabId, newValue);
								}
							}}
						/>
					{/key}
				</div>
			</Resizable.Pane>

			<Resizable.Handle withHandle />

			<!-- Results Pane -->
			<Resizable.Pane defaultSize={60} minSize={15}>
				<div class="h-full flex flex-col overflow-hidden">
					{#if allResults.length > 0}
						<QueryResultTabs
							results={allResults}
							activeIndex={activeResultIndex}
							onSelectResult={(i) => db.queries.setActiveResult(db.state.activeQueryTabId!, i)}
						/>

						{#if activeResult && !activeResult.isError}
							<QueryExportMenu onExport={handleExport} onCopy={handleCopy} />
						{/if}

						{#if activeResult?.isError}
							<QueryErrorDisplay
								statementIndex={activeResultIndex}
								error={activeResult.error ?? ''}
								statementSql={activeResult.statementSql}
							/>
						{:else if activeResult}
							<VirtualResultsTable
								columns={activeResult.columns}
								rows={activeResult.rows}
								isEditable={!!isEditable}
								onCellSave={handleCellSave}
								onRowDelete={confirmDeleteRow}
								{deletingRowIndex}
								onCopyCell={copyCell}
								onCopyRow={copyRowAsJSON}
								onCopyColumn={copyColumn}
								onCellRightClick={handleCellRightClick}
							/>

							{#if activeResult.totalPages > 1}
								<QueryPagination
									page={activeResult.page}
									pageSize={activeResult.pageSize}
									totalPages={activeResult.totalPages}
									totalRows={activeResult.totalRows}
									isExecuting={db.state.activeQueryTab.isExecuting}
									onGoToPage={(page) => db.queries.goToPage(db.state.activeQueryTabId!, page)}
									onSetPageSize={(size) => db.queries.setPageSize(db.state.activeQueryTabId!, size)}
								/>
							{/if}
						{/if}
					{:else}
						<div class="flex-1 flex items-center justify-center p-6 overflow-auto">
							<div class="w-full max-w-md space-y-6">
								<div class="text-center">
									<PlayIcon class="size-10 mx-auto mb-2 opacity-20" />
									<p class="font-medium">{m.query_no_results()}</p>
									<p class="text-xs text-muted-foreground mt-1">
										{m.query_run_hint({ shortcut: "⌘+Enter" })}
									</p>
								</div>

								{#if activeSampleQueries.length > 0}
									<div class="space-y-3">
										<p class="text-xs text-muted-foreground text-center">
											{m.empty_query_sample_title()}
										</p>
										{#each activeSampleQueries as sampleQuery}
											<QueryExampleCard query={sampleQuery} onTry={handleTrySampleQuery} />
										{/each}
									</div>
								{/if}
							</div>
						</div>
					{/if}
				</div>
			</Resizable.Pane>
		</Resizable.PaneGroup>
	{:else}
		<div class="flex-1 flex items-center justify-center p-6">
			<div class="w-full max-w-md space-y-6">
				<div class="text-center">
					<PlayIcon class="size-10 mx-auto mb-2 opacity-20" />
					<p class="font-medium">{m.query_create_tab()}</p>
				</div>
				<Button class="w-full" onclick={() => db.queryTabs.add()}>
					<PlusIcon class="size-4 me-2" />
					{m.empty_query_new()}
				</Button>
			</div>
		</div>
	{/if}
</div>

{#if db.state.activeQueryTab}
	<SaveQueryDialog
		bind:open={showSaveDialog}
		query={db.state.activeQueryTab.query}
		tabId={db.state.activeQueryTab.id}
	/>

	<ParameterInputDialog
		bind:open={showParamsDialog}
		parameters={pendingParams}
		onExecute={handleParamExecute}
		onCancel={handleParamCancel}
	/>
{/if}

<!-- Delete Row Confirmation Dialog -->
<Dialog.Root bind:open={showDeleteConfirm}>
	<Dialog.Content class="sm:max-w-md">
		<Dialog.Header>
			<Dialog.Title>{m.query_delete_row_title()}</Dialog.Title>
			<Dialog.Description>
				{m.query_delete_row_description()}
			</Dialog.Description>
		</Dialog.Header>
		<Dialog.Footer>
			<Button variant="outline" onclick={() => (showDeleteConfirm = false)}>
				{m.query_cancel()}
			</Button>
			<Button variant="destructive" onclick={handleDeleteRow}>
				{m.query_delete()}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
