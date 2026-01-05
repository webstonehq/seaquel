<script lang="ts">
	import { onMount, onDestroy } from "svelte";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { useShortcuts, findShortcut } from "$lib/shortcuts/index.js";
	import ShortcutKeys from "$lib/components/shortcut-keys.svelte";
	import { Button, buttonVariants } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { PlayIcon, SaveIcon, DownloadIcon, LoaderIcon, CopyIcon, ChevronDownIcon, WandSparklesIcon, ChevronLeftIcon, ChevronRightIcon, ChevronsLeftIcon, ChevronsRightIcon, SearchIcon, ActivityIcon, XCircleIcon, TableIcon, ZapIcon } from "@lucide/svelte";
	import { Badge } from "$lib/components/ui/badge";
	import { toast } from "svelte-sonner";
	import SaveQueryDialog from "$lib/components/save-query-dialog.svelte";
	import MonacoEditor, { type MonacoEditorRef } from "$lib/components/monaco-editor.svelte";
	import * as Resizable from "$lib/components/ui/resizable";
	import { save } from "@tauri-apps/plugin-dialog";
	import { writeTextFile } from "@tauri-apps/plugin-fs";
	import { format as formatSQL } from "sql-formatter";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import EditableCell from "$lib/components/editable-cell.svelte";
	import RowActions from "$lib/components/row-actions.svelte";
	import VirtualResultsTable from "$lib/components/virtual-results-table.svelte";
	import { formatConfig, getExportContent, type ExportFormat } from "$lib/utils/export-formats.js";
	import { m } from "$lib/paraglide/messages.js";
	import { cn } from "$lib/utils.js";
	import { splitSqlStatements } from "$lib/db/sql-parser.js";

	const db = useDatabase();
	const shortcuts = useShortcuts();
	let showSaveDialog = $state(false);
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
			// Refresh data
			await db.queries.execute(db.state.activeQueryTabId);
		} else {
			toast.error(m.query_row_delete_failed({ error: result.error || '' }));
		}

		deletingRowIndex = null;
		pendingDeleteRow = null;
	}

	const handleExecute = () => {
		if (db.state.activeQueryTabId) {
			db.queries.execute(db.state.activeQueryTabId);
		}
	};

	const handleExplain = (analyze: boolean) => {
		if (db.state.activeQueryTabId) {
			const cursorOffset = monacoRef?.getCursorOffset() ?? 0;
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
        <!-- Toolbar -->
        <div
            class="flex items-center justify-between p-2 border-b bg-muted/30 shrink-0"
        >
            <div class="flex items-center gap-3 text-xs text-muted-foreground">
                <!-- Live statement count (always shown when > 1) -->
                {#if liveStatementCount > 1}
                    <span class="flex items-center gap-1">
                        <Badge variant="outline" class="text-xs"
                            >{m.query_statements_count({ count: liveStatementCount })}</Badge
                        >
                    </span>
                {/if}
                <!-- Results stats (shown after execution) -->
                {#if activeResult}
                    {#if activeResult.isError}
                        <span class="flex items-center gap-1 text-destructive">
                            <XCircleIcon class="size-3" />
                            {m.query_error()}
                        </span>
                    {:else if activeResult.queryType && ['insert', 'update', 'delete'].includes(activeResult.queryType)}
                        <span class="flex items-center gap-1">
                            <Badge variant="secondary" class="text-xs"
                                >{activeResult.affectedRows ?? 0}</Badge
                            >
                            {m.query_rows_affected()}
                            {#if activeResult.lastInsertId}
                                <Badge variant="outline" class="text-xs ms-1"
                                    >ID: {activeResult.lastInsertId}</Badge
                                >
                            {/if}
                        </span>
                    {/if}
                {/if}
            </div>
            <div class="flex items-center gap-2">
                <!-- Execute Button with Dropdown -->
                <div class="flex">
                    <Button
                        size="sm"
                        class="h-8 gap-1 rounded-r-none border-r-0"
                        onclick={handleExecute}
                        disabled={!db.state.activeQueryTab || db.state.activeQueryTab.isExecuting}
                    >
                        {#if db.state.activeQueryTab?.isExecuting}
                            <LoaderIcon class="animate-spin size-3" />
                        {:else}
                            <PlayIcon class="size-3" />
                        {/if}
                        {m.query_execute()}
                    </Button>
                    <DropdownMenu.Root>
                        <DropdownMenu.Trigger
                            class={buttonVariants({ size: "sm", variant: "default" }) + " h-7 px-1.5 rounded-l-none border-l border-primary-foreground/20"}
                            disabled={!db.state.activeQueryTab || db.state.activeQueryTab.isExecuting}
                        >
                            <ChevronDownIcon class="size-3" />
                        </DropdownMenu.Trigger>
                        <DropdownMenu.Content align="end">
                            <DropdownMenu.Item onclick={handleExecute}>
                                <PlayIcon class="size-4 me-2" />
                                {m.query_execute()}
                                {#if findShortcut('executeQuery')}
                                    <ShortcutKeys keys={findShortcut('executeQuery')!.keys} class="ms-auto" />
                                {/if}
                            </DropdownMenu.Item>
                            <DropdownMenu.Separator />
                            <DropdownMenu.Item onclick={() => handleExplain(false)}>
                                <SearchIcon class="size-4 me-2" />
                                {m.query_explain()}
                            </DropdownMenu.Item>
                            <DropdownMenu.Item onclick={() => handleExplain(true)}>
                                <ActivityIcon class="size-4 me-2" />
                                {m.query_explain_analyze()}
                            </DropdownMenu.Item>
                        </DropdownMenu.Content>
                    </DropdownMenu.Root>
                </div>
                <Button
                    size="sm"
                    variant="outline"
                    class="h-7 gap-1"
                    onclick={handleFormat}
                    disabled={!db.state.activeQueryTab?.query.trim()}
                >
                    <WandSparklesIcon class="size-3" />
                    {m.query_format()}
                    {#if findShortcut('formatSql')}
                        <ShortcutKeys keys={findShortcut('formatSql')!.keys} class="ms-1" />
                    {/if}
                </Button>
                <Button
                    size="sm"
                    variant="outline"
                    class="h-7 gap-1"
                    onclick={handleSave}
                    disabled={!db.state.activeQueryTab?.query.trim()}
                >
                    <SaveIcon class="size-3" />
                    {m.query_save()}
                    {#if findShortcut('saveQuery')}
                        <ShortcutKeys keys={findShortcut('saveQuery')!.keys} class="ms-1" />
                    {/if}
                </Button>
            </div>
        </div>

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
                        <!-- Result Tabs -->
                        <div class="flex items-center gap-1 p-2 border-b bg-muted/20 overflow-x-auto shrink-0">
                            {#each allResults as result, i}
                                <button
                                    class={cn(
                                        "px-3 py-1.5 text-xs rounded-md shrink-0 flex items-center gap-1.5 transition-colors",
                                        activeResultIndex === i
                                            ? "bg-primary text-primary-foreground"
                                            : "bg-muted hover:bg-muted/80",
                                        result.isError && activeResultIndex !== i && "text-destructive"
                                    )}
                                    onclick={() => db.queries.setActiveResult(db.state.activeQueryTabId!, i)}
                                >
                                    {#if result.isError}
                                        <XCircleIcon class="size-3" />
                                    {:else if result.queryType === 'select'}
                                        <TableIcon class="size-3" />
                                    {:else}
                                        <ZapIcon class="size-3" />
                                    {/if}
                                    {m.query_statement_n({ n: i + 1 })}
                                    {#if result.isError}
                                        <span class="opacity-70">{m.query_result_error()}</span>
                                    {:else if result.queryType === 'select'}
                                        <span class="opacity-70">{m.query_result_rows_time({ rows: result.totalRows, time: result.executionTime })}</span>
                                    {:else if result.affectedRows !== undefined}
                                        <span class="opacity-70">{m.query_result_affected_time({ affected: result.affectedRows, time: result.executionTime })}</span>
                                    {:else}
                                        <span class="opacity-70">{m.query_result_time({ time: result.executionTime })}</span>
                                    {/if}
                                </button>
                            {/each}
                        </div>

                        <!-- Export toolbar -->
                        {#if activeResult && !activeResult.isError}
                            <div
                                class="flex items-center justify-end p-2 border-b bg-muted/30 shrink-0"
                            >
                                <DropdownMenu.Root>
                                    <DropdownMenu.Trigger
                                        class={buttonVariants({
                                            variant: "outline",
                                            size: "sm",
                                        }) + " h-7 gap-1"}
                                    >
                                        <DownloadIcon class="size-3" />
                                        {m.query_export()}
                                        <ChevronDownIcon class="size-3" />
                                    </DropdownMenu.Trigger>
                                    <DropdownMenu.Content align="end" class="w-48">
                                        <DropdownMenu.Group>
                                            <DropdownMenu.GroupHeading
                                                >{m.query_download()}</DropdownMenu.GroupHeading
                                            >
                                            <DropdownMenu.Item
                                                onclick={() => handleExport("csv")}
                                            >
                                                <DownloadIcon class="size-4 me-2" />
                                                CSV
                                            </DropdownMenu.Item>
                                            <DropdownMenu.Item
                                                onclick={() => handleExport("json")}
                                            >
                                                <DownloadIcon class="size-4 me-2" />
                                                JSON
                                            </DropdownMenu.Item>
                                            <DropdownMenu.Item
                                                onclick={() => handleExport("sql")}
                                            >
                                                <DownloadIcon class="size-4 me-2" />
                                                SQL INSERT
                                            </DropdownMenu.Item>
                                            <DropdownMenu.Item
                                                onclick={() =>
                                                    handleExport("markdown")}
                                            >
                                                <DownloadIcon class="size-4 me-2" />
                                                Markdown
                                            </DropdownMenu.Item>
                                        </DropdownMenu.Group>
                                        <DropdownMenu.Separator />
                                        <DropdownMenu.Group>
                                            <DropdownMenu.GroupHeading
                                                >{m.query_copy_to_clipboard()}</DropdownMenu.GroupHeading
                                            >
                                            <DropdownMenu.Item
                                                onclick={() => handleCopy("csv")}
                                            >
                                                <CopyIcon class="size-4 me-2" />
                                                CSV
                                            </DropdownMenu.Item>
                                            <DropdownMenu.Item
                                                onclick={() => handleCopy("json")}
                                            >
                                                <CopyIcon class="size-4 me-2" />
                                                JSON
                                            </DropdownMenu.Item>
                                            <DropdownMenu.Item
                                                onclick={() => handleCopy("sql")}
                                            >
                                                <CopyIcon class="size-4 me-2" />
                                                SQL INSERT
                                            </DropdownMenu.Item>
                                            <DropdownMenu.Item
                                                onclick={() =>
                                                    handleCopy("markdown")}
                                            >
                                                <CopyIcon class="size-4 me-2" />
                                                Markdown
                                            </DropdownMenu.Item>
                                        </DropdownMenu.Group>
                                    </DropdownMenu.Content>
                                </DropdownMenu.Root>
                            </div>
                        {/if}

                        <!-- Error Display for failed statements -->
                        {#if activeResult?.isError}
                            <div class="flex-1 p-4 bg-destructive/10 overflow-auto">
                                <div class="flex items-start gap-3">
                                    <XCircleIcon class="size-5 text-destructive shrink-0 mt-0.5" />
                                    <div class="space-y-3">
                                        <div>
                                            <h4 class="font-semibold text-destructive">{m.query_statement_failed({ n: activeResultIndex + 1 })}</h4>
                                            <pre class="mt-2 text-sm whitespace-pre-wrap text-destructive/90 font-mono">{activeResult.error}</pre>
                                        </div>
                                        <details class="text-sm">
                                            <summary class="cursor-pointer text-muted-foreground hover:text-foreground">{m.query_show_sql()}</summary>
                                            <pre class="mt-2 text-xs bg-muted p-3 rounded-md overflow-x-auto font-mono">{activeResult.statementSql}</pre>
                                        </details>
                                    </div>
                                </div>
                            </div>
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

                            <!-- Pagination Controls -->
                            {#if activeResult.totalPages > 1}
                                {@const start = (activeResult.page - 1) * activeResult.pageSize + 1}
                                {@const end = Math.min(activeResult.page * activeResult.pageSize, activeResult.totalRows)}
                                <div class="flex items-center justify-between p-2 border-t bg-muted/30 shrink-0 text-xs">
                                    <div class="text-muted-foreground">
                                        {m.query_showing_rows({ start, end, total: activeResult.totalRows.toLocaleString() })}
                                    </div>
                                    <div class="flex items-center gap-2">
                                        <DropdownMenu.Root>
                                            <DropdownMenu.Trigger
                                                class={buttonVariants({
                                                    variant: "outline",
                                                    size: "sm",
                                                }) + " h-7 gap-1 text-xs"}
                                            >
                                                {activeResult.pageSize} rows
                                                <ChevronDownIcon class="size-3" />
                                            </DropdownMenu.Trigger>
                                            <DropdownMenu.Content align="end">
                                                {#each [25, 50, 100, 250, 500, 1000] as size}
                                                    <DropdownMenu.Item
                                                        onclick={() => db.queries.setPageSize(db.state.activeQueryTabId!, size)}
                                                        class={activeResult.pageSize === size ? "bg-accent" : ""}
                                                    >
                                                        {m.query_rows_count({ count: size })}
                                                    </DropdownMenu.Item>
                                                {/each}
                                            </DropdownMenu.Content>
                                        </DropdownMenu.Root>

                                        <div class="flex items-center gap-1">
                                            <Button
                                                size="icon"
                                                variant="outline"
                                                class="size-7"
                                                onclick={() => db.queries.goToPage(db.state.activeQueryTabId!, 1)}
                                                disabled={activeResult.page === 1 || db.state.activeQueryTab.isExecuting}
                                            >
                                                <ChevronsLeftIcon class="size-3" />
                                            </Button>
                                            <Button
                                                size="icon"
                                                variant="outline"
                                                class="size-7"
                                                onclick={() => db.queries.goToPage(db.state.activeQueryTabId!, activeResult.page - 1)}
                                                disabled={activeResult.page === 1 || db.state.activeQueryTab.isExecuting}
                                            >
                                                <ChevronLeftIcon class="size-3" />
                                            </Button>
                                            <span class="px-2 text-muted-foreground">
                                                {m.query_page_of({ page: activeResult.page, total: activeResult.totalPages })}
                                            </span>
                                            <Button
                                                size="icon"
                                                variant="outline"
                                                class="size-7"
                                                onclick={() => db.queries.goToPage(db.state.activeQueryTabId!, activeResult.page + 1)}
                                                disabled={activeResult.page === activeResult.totalPages || db.state.activeQueryTab.isExecuting}
                                            >
                                                <ChevronRightIcon class="size-3" />
                                            </Button>
                                            <Button
                                                size="icon"
                                                variant="outline"
                                                class="size-7"
                                                onclick={() => db.queries.goToPage(db.state.activeQueryTabId!, activeResult.totalPages)}
                                                disabled={activeResult.page === activeResult.totalPages || db.state.activeQueryTab.isExecuting}
                                            >
                                                <ChevronsRightIcon class="size-3" />
                                            </Button>
                                        </div>
                                    </div>
                                </div>
                            {/if}
                        {/if}
                    {:else}
                        <div
                            class="flex-1 flex items-center justify-center text-muted-foreground"
                        >
                            <div class="text-center">
                                <PlayIcon
                                    class="size-12 mx-auto mb-2 opacity-20"
                                />
                                <p class="text-sm">
                                    {m.query_no_results()}
                                </p>
                                <p class="text-xs mt-1 text-muted-foreground">
                                    {m.query_run_hint({ shortcut: "⌘+Enter" })}
                                </p>
                            </div>
                        </div>
                    {/if}
                </div>
            </Resizable.Pane>
        </Resizable.PaneGroup>
    {:else}
        <div
            class="flex-1 flex items-center justify-center text-muted-foreground"
        >
            <div class="text-center">
                <PlayIcon class="size-12 mx-auto mb-2 opacity-20" />
                <p class="text-sm">{m.query_create_tab()}</p>
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
            <Button variant="outline" onclick={() => showDeleteConfirm = false}>
                {m.query_cancel()}
            </Button>
            <Button variant="destructive" onclick={handleDeleteRow}>
                {m.query_delete()}
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>
