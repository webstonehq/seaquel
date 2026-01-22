<script lang="ts">
	import { onMount, onDestroy } from "svelte";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { useShortcuts } from "$lib/shortcuts/index.js";
	import { useSidebar } from "$lib/components/ui/sidebar/context.svelte.js";
	import { Button } from "$lib/components/ui/button";
	import { Badge } from "$lib/components/ui/badge";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import * as Popover from "$lib/components/ui/popover";
	import { Label } from "$lib/components/ui/label";
	import { Input } from "$lib/components/ui/input";
	import { PlayIcon, RefreshCwIcon, XIcon, SettingsIcon, ArrowDownIcon, ArrowUpIcon, ArrowRightIcon, ArrowLeftIcon, DatabaseIcon, NetworkIcon } from "@lucide/svelte";
	import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
	import SaveQueryDialog from "$lib/components/save-query-dialog.svelte";
	import ParameterInputDialog from "$lib/components/parameter-input-dialog.svelte";
	import { SharedQueryEditor } from "$lib/components/shared-queries/index.js";
	import MonacoEditor, { type MonacoEditorRef } from "$lib/components/monaco-editor.svelte";
	import * as Resizable from "$lib/components/ui/resizable";
	import { save } from "@tauri-apps/plugin-dialog";
	import { writeTextFile } from "@tauri-apps/plugin-fs";
	import { format as formatSQL } from "sql-formatter";
	import VirtualResultsTable from "$lib/components/virtual-results-table.svelte";
	import { formatConfig, getExportContent, type ExportFormat } from "$lib/utils/export-formats.js";
	import { m } from "$lib/paraglide/messages.js";
	import { splitSqlStatements, getStatementAtOffset } from "$lib/db/sql-parser.js";
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
		QueryErrorDisplay,
		QueryResultViewToggle,
		ExplainResultPane,
		VisualizeResultPane
	} from "$lib/components/query-editor/index.js";

	// Import chart components
	import { QueryChart, ChartConfigPopover, createDefaultChartConfig } from "$lib/components/charts/index.js";
	import type { ResultViewMode, ChartConfig } from "$lib/types";
	import { DEFAULT_LAYOUT_OPTIONS, type QueryLayoutOptions, type LayoutDirection } from "$lib/utils/query-visual-layout";

	const db = useDatabase();
	const shortcuts = useShortcuts();
	const sidebar = useSidebar();
	let showSaveDialog = $state(false);
	let showShareDialog = $state(false);
	let showParamsDialog = $state(false);
	let pendingParams = $state<QueryParameter[]>([]);
	// Track pending action type: 'query' for execute all, 'query-current' for current statement, or explain details
	let pendingAction = $state<'query' | { type: 'query-current'; cursorOffset: number } | { type: 'explain'; analyze: boolean; cursorOffset: number } | null>(null);
	let deletingRowIndex = $state<number | null>(null);
	let pendingDeleteRow = $state<{ index: number; row: Record<string, unknown> } | null>(null);
	let showDeleteConfirm = $state(false);
	let monacoRef = $state<MonacoEditorRef | null>(null);

	// Get the active result (for multi-statement support)
	const activeResult = $derived(db.state.activeQueryResult);
	const activeResultIndex = $derived(db.state.activeQueryTab?.activeResultIndex ?? 0);

	// Get embedded explain/visualize results
	const explainResult = $derived(db.state.activeQueryTab?.explainResult);
	const visualizeResult = $derived(db.state.activeQueryTab?.visualizeResult);

	// Chart view state (per result, keyed by tab-result combination)
	let viewModeByResult = $state<Record<string, ResultViewMode>>({});
	let chartConfigByResult = $state<Record<string, ChartConfig>>({});

	// Get current view mode and chart config for active result
	const resultKey = $derived(
		db.state.activeQueryTabId && activeResultIndex !== undefined
			? `${db.state.activeQueryTabId}-${activeResultIndex}`
			: null
	);
	const currentViewMode = $derived<ResultViewMode>(
		resultKey ? (viewModeByResult[resultKey] ?? 'table') : 'table'
	);
	const currentChartConfig = $derived<ChartConfig | undefined>(
		resultKey && activeResult
			? (chartConfigByResult[resultKey] ?? createDefaultChartConfig(activeResult.columns, activeResult.rows))
			: undefined
	);

	const handleViewModeChange = (mode: ResultViewMode) => {
		if (resultKey) {
			viewModeByResult[resultKey] = mode;
		}
	};

	const handleChartConfigChange = (config: ChartConfig) => {
		if (resultKey) {
			chartConfigByResult[resultKey] = config;
		}
	};

	// Visualize layout options state
	let visualizeLayoutOptions = $state<QueryLayoutOptions>({ ...DEFAULT_LAYOUT_OPTIONS });

	// Direction options for visualize layout
	const layoutDirections: { value: LayoutDirection; label: string; icon: typeof ArrowDownIcon }[] = [
		{ value: 'TB', label: 'Top to Bottom', icon: ArrowDownIcon },
		{ value: 'BT', label: 'Bottom to Top', icon: ArrowUpIcon },
		{ value: 'LR', label: 'Left to Right', icon: ArrowRightIcon },
		{ value: 'RL', label: 'Right to Left', icon: ArrowLeftIcon }
	];

	const setVisualizeDirection = (dir: LayoutDirection) => {
		visualizeLayoutOptions = { ...visualizeLayoutOptions, direction: dir };
	};

	const resetVisualizeLayout = () => {
		visualizeLayoutOptions = { ...DEFAULT_LAYOUT_OPTIONS };
	};

	const allResults = $derived(db.state.activeQueryTab?.results ?? []);

	// Track query content for live statement count
	let currentQuery = $state(db.state.activeQueryTab?.query ?? '');

	// Update currentQuery when active tab changes
	$effect(() => {
		currentQuery = db.state.activeQueryTab?.query ?? '';
	});

	// Check staleness for explain/visualize results
	const isExplainStale = $derived(
		explainResult?.sourceQuery && currentQuery
			? explainResult.sourceQuery.trim() !== currentQuery.trim()
			: false
	);
	const isVisualizeStale = $derived(
		visualizeResult?.sourceQuery && currentQuery
			? visualizeResult.sourceQuery.trim() !== currentQuery.trim()
			: false
	);

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
			errorToast(m.query_cell_update_failed({ error: result.error || '' }));
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
			errorToast(m.query_row_delete_failed({ error: result.error || '' }));
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

	const handleExecuteCurrent = () => {
		if (!db.state.activeQueryTabId || !db.state.activeQueryTab) return;

		const query = db.state.activeQueryTab.query;
		const cursorOffset = monacoRef?.getCursorOffset() ?? 0;
		const dbType = db.state.activeConnection?.type ?? "postgres";

		// Get the current statement to check for parameters
		const currentStatement = getStatementAtOffset(query, cursorOffset, dbType);

		// Only check parameters on the current statement, not the entire query
		if (currentStatement && hasParameters(currentStatement.sql)) {
			pendingParams = getParameterDefinitions(currentStatement.sql);
			pendingAction = { type: 'query-current', cursorOffset };
			showParamsDialog = true;
		} else {
			// No parameters in current statement, execute directly
			db.queries.executeCurrent(db.state.activeQueryTabId, cursorOffset);
		}
	};

	const handleParamExecute = (values: ParameterValue[]) => {
		if (!db.state.activeQueryTabId) return;

		if (pendingAction === 'query') {
			db.queries.executeWithParams(db.state.activeQueryTabId, values);
		} else if (pendingAction && typeof pendingAction === 'object' && pendingAction.type === 'query-current') {
			db.queries.executeCurrentWithParams(
				db.state.activeQueryTabId,
				pendingAction.cursorOffset,
				values
			);
		} else if (pendingAction && typeof pendingAction === 'object' && pendingAction.type === 'explain') {
			db.explainTabs.executeEmbeddedWithParams(
				db.state.activeQueryTabId,
				values,
				pendingAction.analyze,
				pendingAction.cursorOffset
			);
			// Switch view mode to explain
			if (resultKey) {
				viewModeByResult[resultKey] = 'explain';
			}
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
			// No parameters, execute embedded explain
			db.explainTabs.executeEmbedded(db.state.activeQueryTabId, analyze, cursorOffset);
			// Switch view mode to explain
			if (resultKey) {
				viewModeByResult[resultKey] = 'explain';
			}
		}
	};

	const handleVisualize = () => {
		if (!db.state.activeQueryTabId || !db.state.activeQueryTab) return;
		const cursorOffset = monacoRef?.getCursorOffset() ?? 0;
		const success = db.visualizeTabs.visualizeEmbedded(db.state.activeQueryTabId, cursorOffset);
		// Switch view mode to visualize
		if (success && resultKey) {
			viewModeByResult[resultKey] = 'visualize';
		}
	};

	const handleRefreshExplain = (analyze: boolean) => {
		if (!db.state.activeQueryTabId) return;
		const cursorOffset = monacoRef?.getCursorOffset() ?? 0;
		db.explainTabs.executeEmbedded(db.state.activeQueryTabId, analyze, cursorOffset);
	};

	const handleRefreshVisualize = () => {
		if (!db.state.activeQueryTabId) return;
		const cursorOffset = monacoRef?.getCursorOffset() ?? 0;
		db.visualizeTabs.visualizeEmbedded(db.state.activeQueryTabId, cursorOffset);
	};

	const handleCloseExplain = () => {
		if (!db.state.activeQueryTabId) return;
		db.queryTabs.clearExplainResult(db.state.activeQueryTabId);
		// Switch back to table view
		if (resultKey) {
			viewModeByResult[resultKey] = 'table';
		}
	};

	const handleCloseVisualize = () => {
		if (!db.state.activeQueryTabId) return;
		db.queryTabs.clearVisualizeResult(db.state.activeQueryTabId);
		// Switch back to table view
		if (resultKey) {
			viewModeByResult[resultKey] = 'table';
		}
	};

	const handleSave = () => {
		if (!db.state.activeQueryTab?.query.trim()) return;
		showSaveDialog = true;
	};

	const handleShare = () => {
		if (!db.state.activeQueryTab?.query.trim()) return;
		showShareDialog = true;
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
			errorToast(m.query_format_failed());
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
			errorToast(m.query_copy_failed());
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
			onExecuteCurrent={handleExecuteCurrent}
			onExplain={handleExplain}
			onVisualize={handleVisualize}
			onFormat={handleFormat}
			onSave={handleSave}
			onShare={handleShare}
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
							onExecute={handleExecuteCurrent}
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
							<div class="flex items-center justify-between px-2 py-1.5 border-b bg-muted/20">
								<div class="flex items-center gap-2">
									<QueryResultViewToggle
										mode={currentViewMode}
										onModeChange={handleViewModeChange}
										hasExplainResult={!!explainResult?.result}
										hasVisualizeResult={!!visualizeResult?.parsedQuery || !!visualizeResult?.parseError}
										isExplainStale={isExplainStale}
										isVisualizeStale={isVisualizeStale}
									/>
									{#if currentViewMode === 'explain' && explainResult?.result}
										<Badge variant={explainResult.isAnalyze ? "default" : "secondary"} class="h-5">
											{explainResult.isAnalyze ? "ANALYZE" : "EXPLAIN"}
										</Badge>
									{/if}
								</div>
								<div class="flex items-center gap-1">
									{#if currentViewMode === 'chart' && currentChartConfig}
										<ChartConfigPopover
											config={currentChartConfig}
											columns={activeResult.columns}
											onConfigChange={handleChartConfigChange}
										/>
									{/if}
									{#if currentViewMode === 'table' || currentViewMode === 'chart'}
										<QueryExportMenu onExport={handleExport} onCopy={handleCopy} />
									{/if}
									{#if currentViewMode === 'explain' && explainResult}
										<Button
											size="sm"
											variant="ghost"
											class="h-7 gap-1.5 px-2"
											onclick={() => handleRefreshExplain(explainResult.isAnalyze)}
											title={isExplainStale ? "Refresh with updated query" : "Re-run explain"}
										>
											<RefreshCwIcon class="size-3.5" />
											{isExplainStale ? "Refresh" : "Re-run"}
										</Button>
										<Button
											size="sm"
											variant="ghost"
											class="h-7 px-2"
											onclick={handleCloseExplain}
											title="Close"
										>
											<XIcon class="size-3.5" />
										</Button>
									{/if}
									{#if currentViewMode === 'visualize' && visualizeResult}
										<!-- Layout Settings Popover -->
										<Popover.Root>
											<Popover.Trigger>
												<Button variant="ghost" size="sm" class="h-7 gap-1.5 px-2">
													<SettingsIcon class="size-3.5" />
													Layout
												</Button>
											</Popover.Trigger>
											<Popover.Content class="w-64" align="end">
												<div class="space-y-4">
													<div class="flex items-center justify-between">
														<h4 class="font-medium text-sm">Layout Settings</h4>
														<Button variant="ghost" size="sm" class="h-6 text-xs" onclick={resetVisualizeLayout}>
															Reset
														</Button>
													</div>
													<div class="space-y-2">
														<Label class="text-xs text-muted-foreground">Direction</Label>
														<div class="grid grid-cols-4 gap-1">
															{#each layoutDirections as dir}
																<Button
																	variant={visualizeLayoutOptions.direction === dir.value ? "default" : "outline"}
																	size="sm"
																	class="h-8 px-2"
																	onclick={() => setVisualizeDirection(dir.value)}
																	title={dir.label}
																>
																	<dir.icon class="size-4" />
																</Button>
															{/each}
														</div>
													</div>
													<div class="space-y-2">
														<Label class="text-xs text-muted-foreground">Node Spacing (px)</Label>
														<Input
															type="number"
															min={20}
															max={200}
															step={10}
															value={visualizeLayoutOptions.nodeSpacing}
															oninput={(e) => {
																const val = parseInt(e.currentTarget.value, 10);
																if (!isNaN(val) && val >= 20 && val <= 200) {
																	visualizeLayoutOptions = { ...visualizeLayoutOptions, nodeSpacing: val };
																}
															}}
															class="h-8"
														/>
													</div>
													<div class="space-y-2">
														<Label class="text-xs text-muted-foreground">Level Spacing (px)</Label>
														<Input
															type="number"
															min={40}
															max={300}
															step={10}
															value={visualizeLayoutOptions.rankSpacing}
															oninput={(e) => {
																const val = parseInt(e.currentTarget.value, 10);
																if (!isNaN(val) && val >= 40 && val <= 300) {
																	visualizeLayoutOptions = { ...visualizeLayoutOptions, rankSpacing: val };
																}
															}}
															class="h-8"
														/>
													</div>
												</div>
											</Popover.Content>
										</Popover.Root>
										<Button
											size="sm"
											variant="ghost"
											class="h-7 gap-1.5 px-2"
											onclick={handleRefreshVisualize}
											title={isVisualizeStale ? "Refresh with updated query" : "Re-run visualization"}
										>
											<RefreshCwIcon class="size-3.5" />
											{isVisualizeStale ? "Refresh" : "Re-run"}
										</Button>
										<Button
											size="sm"
											variant="ghost"
											class="h-7 px-2"
											onclick={handleCloseVisualize}
											title="Close"
										>
											<XIcon class="size-3.5" />
										</Button>
									{/if}
								</div>
							</div>
						{:else if (explainResult?.result || explainResult?.isExecuting || visualizeResult?.parsedQuery || visualizeResult?.parseError)}
							<!-- Show view toggle even without query results if explain/visualize exists -->
							<div class="flex items-center justify-between px-2 py-1.5 border-b bg-muted/20">
								<div class="flex items-center gap-2">
									<QueryResultViewToggle
										mode={currentViewMode}
										onModeChange={handleViewModeChange}
										hasExplainResult={!!explainResult?.result || explainResult?.isExecuting}
										hasVisualizeResult={!!visualizeResult?.parsedQuery || !!visualizeResult?.parseError}
										isExplainStale={isExplainStale}
										isVisualizeStale={isVisualizeStale}
									/>
									{#if currentViewMode === 'explain' && explainResult?.result}
										<Badge variant={explainResult.isAnalyze ? "default" : "secondary"} class="h-5">
											{explainResult.isAnalyze ? "ANALYZE" : "EXPLAIN"}
										</Badge>
									{/if}
								</div>
								<div class="flex items-center gap-1">
									{#if currentViewMode === 'explain' && explainResult}
										<Button
											size="sm"
											variant="ghost"
											class="h-7 gap-1.5 px-2"
											onclick={() => handleRefreshExplain(explainResult.isAnalyze)}
											title={isExplainStale ? "Refresh with updated query" : "Re-run explain"}
										>
											<RefreshCwIcon class="size-3.5" />
											{isExplainStale ? "Refresh" : "Re-run"}
										</Button>
										<Button
											size="sm"
											variant="ghost"
											class="h-7 px-2"
											onclick={handleCloseExplain}
											title="Close"
										>
											<XIcon class="size-3.5" />
										</Button>
									{/if}
									{#if currentViewMode === 'visualize' && visualizeResult}
										<!-- Layout Settings Popover -->
										<Popover.Root>
											<Popover.Trigger>
												<Button variant="ghost" size="sm" class="h-7 gap-1.5 px-2">
													<SettingsIcon class="size-3.5" />
													Layout
												</Button>
											</Popover.Trigger>
											<Popover.Content class="w-64" align="end">
												<div class="space-y-4">
													<div class="flex items-center justify-between">
														<h4 class="font-medium text-sm">Layout Settings</h4>
														<Button variant="ghost" size="sm" class="h-6 text-xs" onclick={resetVisualizeLayout}>
															Reset
														</Button>
													</div>
													<div class="space-y-2">
														<Label class="text-xs text-muted-foreground">Direction</Label>
														<div class="grid grid-cols-4 gap-1">
															{#each layoutDirections as dir}
																<Button
																	variant={visualizeLayoutOptions.direction === dir.value ? "default" : "outline"}
																	size="sm"
																	class="h-8 px-2"
																	onclick={() => setVisualizeDirection(dir.value)}
																	title={dir.label}
																>
																	<dir.icon class="size-4" />
																</Button>
															{/each}
														</div>
													</div>
													<div class="space-y-2">
														<Label class="text-xs text-muted-foreground">Node Spacing (px)</Label>
														<Input
															type="number"
															min={20}
															max={200}
															step={10}
															value={visualizeLayoutOptions.nodeSpacing}
															oninput={(e) => {
																const val = parseInt(e.currentTarget.value, 10);
																if (!isNaN(val) && val >= 20 && val <= 200) {
																	visualizeLayoutOptions = { ...visualizeLayoutOptions, nodeSpacing: val };
																}
															}}
															class="h-8"
														/>
													</div>
													<div class="space-y-2">
														<Label class="text-xs text-muted-foreground">Level Spacing (px)</Label>
														<Input
															type="number"
															min={40}
															max={300}
															step={10}
															value={visualizeLayoutOptions.rankSpacing}
															oninput={(e) => {
																const val = parseInt(e.currentTarget.value, 10);
																if (!isNaN(val) && val >= 40 && val <= 300) {
																	visualizeLayoutOptions = { ...visualizeLayoutOptions, rankSpacing: val };
																}
															}}
															class="h-8"
														/>
													</div>
												</div>
											</Popover.Content>
										</Popover.Root>
										<Button
											size="sm"
											variant="ghost"
											class="h-7 gap-1.5 px-2"
											onclick={handleRefreshVisualize}
											title={isVisualizeStale ? "Refresh with updated query" : "Re-run visualization"}
										>
											<RefreshCwIcon class="size-3.5" />
											{isVisualizeStale ? "Refresh" : "Re-run"}
										</Button>
										<Button
											size="sm"
											variant="ghost"
											class="h-7 px-2"
											onclick={handleCloseVisualize}
											title="Close"
										>
											<XIcon class="size-3.5" />
										</Button>
									{/if}
								</div>
							</div>
						{/if}

						{#if currentViewMode === 'explain' && explainResult}
							<ExplainResultPane {explainResult} />
						{:else if currentViewMode === 'explain' && !explainResult}
							<div class="flex-1 flex items-center justify-center p-6">
								<div class="text-center max-w-xs">
									<DatabaseIcon class="size-10 mx-auto mb-3 opacity-20" />
									<p class="font-medium mb-2">No Explain Results</p>
									<p class="text-xs text-muted-foreground mb-4">
										Analyze your query's execution plan to understand how the database processes it.
									</p>
									<div class="flex gap-2 justify-center">
										<Button size="sm" variant="outline" onclick={() => handleExplain(false)}>
											Explain
										</Button>
										<Button size="sm" onclick={() => handleExplain(true)}>
											Explain Analyze
										</Button>
									</div>
								</div>
							</div>
						{:else if currentViewMode === 'visualize' && visualizeResult}
							<VisualizeResultPane
								{visualizeResult}
								layoutOptions={visualizeLayoutOptions}
							/>
						{:else if currentViewMode === 'visualize' && !visualizeResult}
							<div class="flex-1 flex items-center justify-center p-6">
								<div class="text-center max-w-xs">
									<NetworkIcon class="size-10 mx-auto mb-3 opacity-20" />
									<p class="font-medium mb-2">No Visual Results</p>
									<p class="text-xs text-muted-foreground mb-4">
										See a visual representation of your query structure.
									</p>
									<Button size="sm" onclick={handleVisualize}>
										Visualize Query
									</Button>
								</div>
							</div>
						{:else if activeResult?.isError}
							<QueryErrorDisplay
								statementIndex={activeResultIndex}
								error={activeResult.error ?? ''}
								statementSql={activeResult.statementSql}
							/>
						{:else if activeResult}
							{#if currentViewMode === 'chart'}
								<div class="flex-1 min-h-0">
									<QueryChart
										columns={activeResult.columns}
										rows={activeResult.rows}
										config={currentChartConfig}
										onConfigChange={handleChartConfigChange}
									/>
								</div>
							{:else}
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
							{/if}

							{#if activeResult.totalPages > 1 && currentViewMode === 'table'}
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
						<!-- Show view toggle and content even without query results -->
						<div class="flex items-center justify-between px-2 py-1.5 border-b bg-muted/20">
							<div class="flex items-center gap-2">
								<QueryResultViewToggle
									mode={currentViewMode}
									onModeChange={handleViewModeChange}
									hasExplainResult={!!explainResult?.result || explainResult?.isExecuting}
									hasVisualizeResult={!!visualizeResult?.parsedQuery || !!visualizeResult?.parseError}
									isExplainStale={isExplainStale}
									isVisualizeStale={isVisualizeStale}
								/>
								{#if currentViewMode === 'explain' && explainResult?.result}
									<Badge variant={explainResult.isAnalyze ? "default" : "secondary"} class="h-5">
										{explainResult.isAnalyze ? "ANALYZE" : "EXPLAIN"}
									</Badge>
								{/if}
							</div>
							<div class="flex items-center gap-1">
								{#if currentViewMode === 'explain' && explainResult}
									<Button
										size="sm"
										variant="ghost"
										class="h-7 gap-1.5 px-2"
										onclick={() => handleRefreshExplain(explainResult.isAnalyze)}
										title={isExplainStale ? "Refresh with updated query" : "Re-run explain"}
									>
										<RefreshCwIcon class="size-3.5" />
										{isExplainStale ? "Refresh" : "Re-run"}
									</Button>
									<Button
										size="sm"
										variant="ghost"
										class="h-7 px-2"
										onclick={handleCloseExplain}
										title="Close"
									>
										<XIcon class="size-3.5" />
									</Button>
								{/if}
								{#if currentViewMode === 'visualize' && visualizeResult}
									<Button
										size="sm"
										variant="ghost"
										class="h-7 gap-1.5 px-2"
										onclick={handleRefreshVisualize}
										title={isVisualizeStale ? "Refresh with updated query" : "Re-run visualization"}
									>
										<RefreshCwIcon class="size-3.5" />
										{isVisualizeStale ? "Refresh" : "Re-run"}
									</Button>
									<Button
										size="sm"
										variant="ghost"
										class="h-7 px-2"
										onclick={handleCloseVisualize}
										title="Close"
									>
										<XIcon class="size-3.5" />
									</Button>
								{/if}
							</div>
						</div>
						{#if currentViewMode === 'explain' && explainResult}
							<ExplainResultPane {explainResult} />
						{:else if currentViewMode === 'explain' && !explainResult}
							<div class="flex-1 flex items-center justify-center p-6">
								<div class="text-center max-w-xs">
									<DatabaseIcon class="size-10 mx-auto mb-3 opacity-20" />
									<p class="font-medium mb-2">No Explain Results</p>
									<p class="text-xs text-muted-foreground mb-4">
										Analyze your query's execution plan to understand how the database processes it.
									</p>
									<div class="flex gap-2 justify-center">
										<Button size="sm" variant="outline" onclick={() => handleExplain(false)}>
											Explain
										</Button>
										<Button size="sm" onclick={() => handleExplain(true)}>
											Explain Analyze
										</Button>
									</div>
								</div>
							</div>
						{:else if currentViewMode === 'visualize' && visualizeResult}
							<VisualizeResultPane
								{visualizeResult}
								layoutOptions={visualizeLayoutOptions}
							/>
						{:else if currentViewMode === 'visualize' && !visualizeResult}
							<div class="flex-1 flex items-center justify-center p-6">
								<div class="text-center max-w-xs">
									<NetworkIcon class="size-10 mx-auto mb-3 opacity-20" />
									<p class="font-medium mb-2">No Visual Results</p>
									<p class="text-xs text-muted-foreground mb-4">
										See a visual representation of your query structure.
									</p>
									<Button size="sm" onclick={handleVisualize}>
										Visualize Query
									</Button>
								</div>
							</div>
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

									{#if activeSampleQueries.length > 0 && !currentQuery?.trim()}
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

	<SharedQueryEditor
		bind:open={showShareDialog}
		onOpenChange={(open) => showShareDialog = open}
		query={db.state.activeQueryTab.query}
		name={db.state.activeQueryTab.name}
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
