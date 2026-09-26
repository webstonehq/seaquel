<script lang="ts">
	import { onMount, onDestroy } from "svelte";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { useShortcuts } from "$lib/shortcuts/index.js";
	import { useSidebar } from "$lib/components/ui/sidebar/context.svelte.js";
	import { Button } from "$lib/components/ui/button";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import DeleteConfirmDialog from "$lib/components/delete-confirm-dialog.svelte";
	import DestructiveQueryConfirmDialog from "$lib/components/destructive-query-confirm-dialog.svelte";
	import { PlayIcon, DatabaseIcon, NetworkIcon, ColumnsIcon } from "@lucide/svelte";
	import SaveQueryDialog from "$lib/components/save-query-dialog.svelte";
	import ParameterInputDialog from "$lib/components/parameter-input-dialog.svelte";
	import MonacoEditor, { type MonacoEditorRef } from "$lib/components/monaco-editor.svelte";
	import * as Resizable from "$lib/components/ui/resizable";
	import VirtualResultsTable from "$lib/components/virtual-results-table.svelte";
	import { m } from "$lib/paraglide/messages.js";
	import { aiSettingsStore } from "$lib/stores/ai-settings.svelte";
	import QueryExampleCard from "$lib/components/empty-states/query-example-card.svelte";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import { EmptyState } from "$lib/components/ui/empty-state";

	import {
		QueryToolbar,
		QueryResultTabs,
		QueryPagination,
		QueryErrorDisplay,
		QueryResultsControlBar,
		ExplainResultPane,
		VisualizeResultPane,
		VisualQueryPanel,
		AIPromptOverlay,
		DiffView,
		createParamDialog,
		createViewState,
		createExecution,
		createExplainVisualize,
		createSaveFormatExport,
		createCellEditing,
		createAIInlinePrompt,
	} from "$lib/components/query-editor/index.js";
	import { EXECUTE_ACTIVE_QUERY_EVENT } from "$lib/components/query-editor/events.js";

	import { QueryChart } from "$lib/components/charts/index.js";

	let { tabId: propTabId = undefined }: { tabId?: string } = $props();

	const db = useDatabase();
	const shortcuts = useShortcuts();
	const sidebar = useSidebar();
	let monacoRef = $state<MonacoEditorRef | null>(null);
	let queryToolbar = $state<ReturnType<typeof QueryToolbar>>();

	// Core derived state
	const activeTab = $derived(
		propTabId
			? db.state.queryTabs.find(t => t.id === propTabId) ?? null
			: db.state.activeQueryTab
	);
	const activeTabId = $derived(propTabId ?? db.state.activeQueryTabId);
	const activeResultIndex = $derived(activeTab?.activeResultIndex ?? 0);
	const activeResult = $derived(activeTab?.results?.[activeResultIndex] ?? null);
	const resultKey = $derived(
		activeTabId && activeResultIndex !== undefined
			? `${activeTabId}-${activeResultIndex}`
			: null
	);

	// Context for all modules
	const ctx = {
		db,
		getMonacoRef: () => monacoRef,
		getActiveTab: () => activeTab,
		getActiveTabId: () => activeTabId,
		getActiveResult: () => activeResult,
		getActiveResultIndex: () => activeResultIndex,
		getResultKey: () => resultKey,
	};

	// Compose modules
	const viewState = createViewState(ctx, () => queryToolbar?.clearVersionSelection());
	const paramDialog = createParamDialog(ctx);
	const exec = createExecution(ctx, paramDialog, () => {
		if (viewState.visualPanelOpen) viewState.syncVisualBuilderSql();
	});
	const explainViz = createExplainVisualize(ctx, paramDialog, viewState);
	const saveExport = createSaveFormatExport(ctx);
	const cellEdit = createCellEditing(ctx);
	const ai = createAIInlinePrompt(ctx, { onExecute: exec.handleExecute });

	function handleParamExecute(values: import("$lib/types").ParameterValue[]) {
		const result = exec.handleParamExecute(values);
		if (result?.switchViewMode) {
			viewState.handleViewModeChange(result.switchViewMode);
		}
	}

	onMount(() => {
		shortcuts.registerHandler('saveQuery', saveExport.handleSave);
		shortcuts.registerHandler('formatSql', saveExport.handleFormat);
	});

	// Lets the command palette run the active tab through the same destructive-confirm and parameter flow.
	// Every pane's editor listens; only the one showing the globally active tab executes.
	function handleExecuteActiveQuery() {
		if (activeTabId && activeTabId === db.state.activeQueryTabId) exec.handleExecute();
	}

	$effect(() => {
		window.addEventListener(EXECUTE_ACTIVE_QUERY_EVENT, handleExecuteActiveQuery);
		return () => window.removeEventListener(EXECUTE_ACTIVE_QUERY_EVENT, handleExecuteActiveQuery);
	});

	onDestroy(() => {
		shortcuts.unregisterHandler('saveQuery');
		shortcuts.unregisterHandler('formatSql');
	});
</script>

<!-- Reusable snippets -->

{#snippet explainEmptyState()}
	<EmptyState
		icon={DatabaseIcon}
		title="No Explain Results"
		description="Analyze your query's execution plan to understand how the database processes it."
	>
		{#snippet actions()}
			<div class="flex gap-2 justify-center">
				<Button size="sm" variant="outline" onclick={() => explainViz.handleExplain(false)}>Explain</Button>
				<Button size="sm" onclick={() => explainViz.handleExplain(true)}>Explain Analyze</Button>
			</div>
		{/snippet}
	</EmptyState>
{/snippet}

{#snippet visualizeEmptyState()}
	<EmptyState
		icon={NetworkIcon}
		title="No Visual Results"
		description="See a visual representation of your query structure."
	>
		{#snippet actions()}
			<Button size="sm" onclick={explainViz.handleVisualize}>Visualize Query</Button>
		{/snippet}
	</EmptyState>
{/snippet}

{#snippet resultControlBar(opts: { withChartAndExport?: boolean })}
	<QueryResultsControlBar
		currentViewMode={viewState.currentViewMode}
		onViewModeChange={viewState.handleViewModeChange}
		explainResult={viewState.explainResult}
		visualizeResult={viewState.visualizeResult}
		isExplainStale={viewState.isExplainStale}
		isVisualizeStale={viewState.isVisualizeStale}
		currentChartConfig={opts.withChartAndExport ? viewState.currentChartConfig : undefined}
		activeResult={opts.withChartAndExport ? activeResult : undefined}
		onChartConfigChange={opts.withChartAndExport ? viewState.handleChartConfigChange : undefined}
		onExport={opts.withChartAndExport ? saveExport.handleExport : undefined}
		onCopy={opts.withChartAndExport ? saveExport.handleCopy : undefined}
		onRefreshExplain={explainViz.handleRefreshExplain}
		onCloseExplain={explainViz.handleCloseExplain}
		visualizeLayoutOptions={viewState.visualizeLayoutOptions}
		onVisualizeLayoutChange={(opts) => viewState.visualizeLayoutOptions = opts}
		onRefreshVisualize={explainViz.handleRefreshVisualize}
		onCloseVisualize={explainViz.handleCloseVisualize}
	/>
{/snippet}

{#snippet resultContentByViewMode()}
	{#if viewState.currentViewMode === 'explain'}
		{#if viewState.explainResult}
			<ExplainResultPane explainResult={viewState.explainResult} />
		{:else}
			{@render explainEmptyState()}
		{/if}
	{:else if viewState.currentViewMode === 'visualize'}
		{#if viewState.visualizeResult}
			<VisualizeResultPane visualizeResult={viewState.visualizeResult} layoutOptions={viewState.visualizeLayoutOptions} />
		{:else}
			{@render visualizeEmptyState()}
		{/if}
	{/if}
{/snippet}

{#snippet noResultsEmptyState()}
	<div class="flex-1 overflow-auto p-6">
		<div class="w-full max-w-md space-y-6 mx-auto my-auto min-h-full flex flex-col justify-center">
			<div class="text-center">
				<PlayIcon class="size-10 mx-auto mb-2 opacity-20" />
				<p class="font-medium">{m.query_no_results()}</p>
				<p class="text-xs text-muted-foreground mt-1">{m.query_run_hint({ shortcut: "\u2318+Enter" })}</p>
			</div>

			{#if viewState.activeSampleQueries.length > 0 && !viewState.currentQuery?.trim()}
				<div class="space-y-3">
					<p class="text-xs text-muted-foreground text-center">{m.empty_query_sample_title()}</p>
					{#each viewState.activeSampleQueries as sampleQuery (sampleQuery.id)}
						<QueryExampleCard query={sampleQuery} onTry={viewState.handleTrySampleQuery} />
					{/each}
				</div>
			{/if}
		</div>
	</div>
{/snippet}

{#snippet noTabEmptyState()}
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
{/snippet}

<!-- Main layout -->

<div class="flex flex-col h-full overflow-hidden">
	{#if activeTab}
		<!-- Toolbar -->
		<div class="flex items-center justify-between border-b bg-muted/30 shrink-0">
			<div class="flex-1">
				<QueryToolbar bind:this={queryToolbar}
					isExecuting={activeTab.isExecuting}
					hasQuery={!!activeTab.query.trim()}
					{activeResult}
					liveStatementCount={viewState.liveStatementCount}
					onExecute={exec.handleExecute}
					onExecuteCurrent={exec.handleExecuteCurrent}
					onExplain={explainViz.handleExplain}
					onVisualize={explainViz.handleVisualize}
					onFormat={saveExport.handleFormat}
					onSave={saveExport.handleSave}
					onSaveAs={saveExport.handleSaveAs}
					queryId={activeTab.queryId}
					tabId={activeTab.id}
					versions={viewState.savedQueryVersions}
					onDiffVersions={viewState.handleDiffVersions}
				/>
			</div>
			{#if viewState.queryBuilderSchema.length > 0}
				<div class="pe-2 shrink-0">
					<Tooltip.Root>
						<Tooltip.Trigger>
							{#snippet child({ props })}
								<Button
									{...props}
									size="sm"
									variant="outline"
									class="h-7 px-2 {viewState.visualPanelOpen ? 'bg-secondary border-secondary-foreground/30' : ''}"
									onclick={viewState.toggleVisualPanel}
								>
									<ColumnsIcon class="size-3.5" />
								</Button>
							{/snippet}
						</Tooltip.Trigger>
						<Tooltip.Content>{m.query_visual_builder()}</Tooltip.Content>
					</Tooltip.Root>
				</div>
			{/if}
		</div>

		<Resizable.PaneGroup direction="vertical" class="flex-1 min-h-0">
			<!-- Editor Pane -->
			<Resizable.Pane defaultSize={40} minSize={15}>
				{#if viewState.visualPanelOpen && viewState.queryBuilderSchema.length > 0}
					<div class="h-full">
						{#key activeTabId}
							<VisualQueryPanel
								schema={viewState.queryBuilderSchema}
								monacoSchema={db.state.activeSchema ?? undefined}
								engine={db.state.activeConnection?.type ?? 'postgres'}
								initialSql={activeTab.query}
								bind:getSql={viewState.visualPanelGetSql}
							/>
						{/key}
					</div>
				{:else if viewState.diffMode}
					<DiffView
						diffMode={viewState.diffMode}
						bind:originalWidth={viewState.diffOriginalWidth}
						onRestore={viewState.restoreVersion}
						onClose={viewState.closeDiff}
					/>
				{:else}
					<div class="relative h-full">
						{#key activeTabId}
							<MonacoEditor
								bind:value={activeTab.query}
								bind:ref={monacoRef}
								schema={db.state.activeSchema}
								onExecute={exec.handleExecuteCurrent}
								onToggleSidebar={() => sidebar.toggle()}
								onAIInlinePrompt={aiSettingsStore.settings.enabled ? ai.handleOpen : undefined}
								onChange={(newValue) => {
									if (activeTabId) {
										db.queryTabs.updateContent(activeTabId, newValue);
									}
								}}
							/>
						{/key}
						{#if ai.open}
							<AIPromptOverlay
								bind:text={ai.text}
								loading={ai.loading}
								bind:error={ai.error}
								onSubmit={ai.submit}
								onClose={ai.close}
								focusOnMount={ai.focusOnMount}
							/>
						{/if}
					</div>
				{/if}
			</Resizable.Pane>

			<Resizable.Handle withHandle />

			<!-- Results Pane -->
			<Resizable.Pane defaultSize={60} minSize={15}>
				<div class="h-full flex flex-col overflow-hidden">
					{#if viewState.allResults.length > 0}
						<QueryResultTabs
							results={viewState.allResults}
							activeIndex={activeResultIndex}
							onSelectResult={(i) => db.queries.setActiveResult(activeTabId!, i)}
						/>

						{#if activeResult && !activeResult.isError}
							{@render resultControlBar({ withChartAndExport: true })}
						{:else if viewState.explainResult?.result || viewState.explainResult?.isExecuting || viewState.visualizeResult?.parsedQuery || viewState.visualizeResult?.parseError}
							{@render resultControlBar({})}
						{/if}

						{#if viewState.currentViewMode === 'explain' || viewState.currentViewMode === 'visualize'}
							{@render resultContentByViewMode()}
						{:else if activeResult?.isError}
							<QueryErrorDisplay
								statementIndex={activeResultIndex}
								error={activeResult.error ?? ''}
								statementSql={activeResult.statementSql}
							/>
						{:else if activeResult}
							{#if viewState.currentViewMode === 'chart'}
								<div class="flex-1 min-h-0">
									<QueryChart
										columns={activeResult.columns}
										rows={activeResult.rows}
										config={viewState.currentChartConfig}
										onConfigChange={viewState.handleChartConfigChange}
									/>
								</div>
							{:else}
								<VirtualResultsTable
									columns={activeResult.columns}
									rows={activeResult.rows}
									isEditable={!!viewState.isEditable}
									onCellSave={cellEdit.handleCellSave}
									onRowDelete={cellEdit.confirmDeleteRow}
									deletingRowIndex={cellEdit.deletingRowIndex}
									onCopyCell={cellEdit.copyCell}
									onCopyRow={cellEdit.copyRowAsJSON}
									onCopyColumn={cellEdit.copyColumn}
									onCellRightClick={cellEdit.handleCellRightClick}
									onSetNull={cellEdit.setNull}
									onSetDefault={cellEdit.setDefault}
									pendingCellEdits={viewState.qePendingCellEdits}
									pendingRowDeletes={viewState.qePendingRowDeletes}
								/>
							{/if}

							{#if (activeResult.totalPages > 1 || activeResult.pageSize === 0 || activeResult.isStreaming) && viewState.currentViewMode === 'table'}
								<QueryPagination
									page={activeResult.page}
									pageSize={activeResult.pageSize}
									totalPages={activeResult.totalPages}
									totalRows={activeResult.totalRows}
									isExecuting={activeTab.isExecuting}
									isStreaming={activeResult.isStreaming ?? false}
									onGoToPage={(page) => db.queries.goToPage(activeTabId!, page)}
									onSetPageSize={(size) => db.queries.setPageSize(activeTabId!, size)}
									onCancelStream={() => db.queries.cancelStream(activeTabId!)}
								/>
							{/if}
						{/if}
					{:else}
						{@render resultControlBar({})}
						{#if viewState.currentViewMode === 'explain' || viewState.currentViewMode === 'visualize'}
							{@render resultContentByViewMode()}
						{:else}
							{@render noResultsEmptyState()}
						{/if}
					{/if}
				</div>
			</Resizable.Pane>
		</Resizable.PaneGroup>
	{:else}
		{@render noTabEmptyState()}
	{/if}
</div>

<!-- Dialogs -->

{#if activeTab}
	<SaveQueryDialog bind:open={saveExport.showSaveDialog} query={activeTab.query} tabId={activeTab.id} />
	<SaveQueryDialog bind:open={saveExport.showSaveAsDialog} query={activeTab.query} tabId={activeTab.id} saveAsNew />
	<ParameterInputDialog bind:open={paramDialog.show} parameters={paramDialog.params} onExecute={handleParamExecute} onCancel={exec.handleParamCancel} />
{/if}

<DeleteConfirmDialog
	bind:open={cellEdit.showDeleteConfirm}
	title={m.query_delete_row_title()}
	description={m.query_delete_row_description()}
	cancelText={m.query_cancel()}
	confirmText={m.query_delete()}
	onconfirm={cellEdit.handleDeleteRow}
/>

<DestructiveQueryConfirmDialog
	bind:open={exec.showDestructiveConfirm}
	statements={exec.destructiveStatements}
	onconfirm={exec.handleDestructiveConfirm}
/>
