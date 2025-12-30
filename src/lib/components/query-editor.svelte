<script lang="ts">
	import { onMount, onDestroy } from "svelte";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { useShortcuts, findShortcut } from "$lib/shortcuts/index.js";
	import ShortcutKeys from "$lib/components/shortcut-keys.svelte";
	import { Button, buttonVariants } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { PlayIcon, SaveIcon, DownloadIcon, LoaderIcon, CopyIcon, ChevronDownIcon, WandSparklesIcon, ChevronLeftIcon, ChevronRightIcon, ChevronsLeftIcon, ChevronsRightIcon, PlusIcon, SearchIcon, ActivityIcon } from "@lucide/svelte";
	import { Badge } from "$lib/components/ui/badge";
	import { toast } from "svelte-sonner";
	import SaveQueryDialog from "$lib/components/save-query-dialog.svelte";
	import MonacoEditor from "$lib/components/monaco-editor.svelte";
	import * as Resizable from "$lib/components/ui/resizable";
	import { save } from "@tauri-apps/plugin-dialog";
	import { writeTextFile } from "@tauri-apps/plugin-fs";
	import { format as formatSQL } from "sql-formatter";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import EditableCell from "$lib/components/editable-cell.svelte";
	import RowActions from "$lib/components/row-actions.svelte";
	import InsertRowDialog from "$lib/components/insert-row-dialog.svelte";
	import VirtualResultsTable from "$lib/components/virtual-results-table.svelte";
	import { formatConfig, getExportContent, type ExportFormat } from "$lib/utils/export-formats.js";
	import { m } from "$lib/paraglide/messages.js";

	const db = useDatabase();
	const shortcuts = useShortcuts();
	let showSaveDialog = $state(false);
	let showInsertDialog = $state(false);
	let deletingRowIndex = $state<number | null>(null);
	let pendingDeleteRow = $state<{ index: number; row: Record<string, unknown> } | null>(null);
	let showDeleteConfirm = $state(false);

	// Get columns for the source table (for insert dialog)
	const sourceTableColumns = $derived.by(() => {
		const sourceTable = db.state.activeQueryTab?.results?.sourceTable;
		if (!sourceTable || !db.state.activeConnectionId) return [];
		const tables = db.state.schemas.get(db.state.activeConnectionId) || [];
		const table = tables.find(t => t.name === sourceTable.name && t.schema === sourceTable.schema);
		return table?.columns || [];
	});

	// Check if results are editable (have source table with primary keys)
	const isEditable = $derived(
		db.state.activeQueryTab?.results?.sourceTable &&
		db.state.activeQueryTab?.results?.sourceTable.primaryKeys.length > 0
	);

	async function handleCellSave(rowIndex: number, column: string, newValue: string) {
		if (!db.state.activeQueryTabId || !db.state.activeQueryTab?.results?.sourceTable) return;

		const result = await db.queries.updateCell(
			db.state.activeQueryTabId,
			rowIndex,
			column,
			newValue,
			db.state.activeQueryTab.results.sourceTable
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
		if (!pendingDeleteRow || !db.state.activeQueryTabId || !db.state.activeQueryTab?.results?.sourceTable) return;

		deletingRowIndex = pendingDeleteRow.index;
		showDeleteConfirm = false;

		const result = await db.queries.deleteRow(
			db.state.activeQueryTab.results.sourceTable,
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
			db.explainTabs.execute(db.state.activeQueryTabId, analyze);
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
		const results = db.state.activeQueryTab?.results;
		if (!results) return format === "json" ? "[]" : "";
		return getExportContent(format, results.columns, results.rows);
	};

	const handleExport = async (format: ExportFormat) => {
		if (!db.state.activeQueryTab?.results) return;

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
		if (!db.state.activeQueryTab?.results) return;

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

	// Threshold for enabling virtualization (rows)
	const VIRTUALIZATION_THRESHOLD = 100;
	const shouldVirtualize = $derived(
		(db.state.activeQueryTab?.results?.rows.length ?? 0) > VIRTUALIZATION_THRESHOLD
	);

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
		if (!contextCell || !db.state.activeQueryTab?.results) return;
		const col = contextCell.column;
		const values = db.state.activeQueryTab.results.rows
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
                {#if db.state.activeQueryTab.results}
                    <span class="flex items-center gap-1">
                        {#if db.state.activeQueryTab.results.queryType && ['insert', 'update', 'delete'].includes(db.state.activeQueryTab.results.queryType)}
                            <Badge variant="secondary" class="text-xs"
                                >{db.state.activeQueryTab.results.affectedRows ?? 0}</Badge
                            >
                            {m.query_rows_affected()}
                            {#if db.state.activeQueryTab.results.lastInsertId}
                                <Badge variant="outline" class="text-xs ms-1"
                                    >ID: {db.state.activeQueryTab.results.lastInsertId}</Badge
                                >
                            {/if}
                        {:else}
                            <Badge variant="secondary" class="text-xs"
                                >{db.state.activeQueryTab.results.totalRows.toLocaleString()}</Badge
                            >
                            {m.query_total_rows()}
                        {/if}
                    </span>
                    <span class="flex items-center gap-1">
                        <Badge variant="secondary" class="text-xs"
                            >{db.state.activeQueryTab.results.executionTime}ms</Badge
                        >
                    </span>
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
                {#if isEditable && sourceTableColumns.length > 0}
                    <Button
                        size="sm"
                        variant="outline"
                        class="h-7 gap-1"
                        onclick={() => showInsertDialog = true}
                    >
                        <PlusIcon class="size-3" />
                        {m.query_add_row()}
                    </Button>
                {/if}
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
                            schema={db.state.activeSchema}
                            onExecute={handleExecute}
                            onChange={(newValue) => {
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
                    {#if db.state.activeQueryTab.results}
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

                        {#if shouldVirtualize}
                            <VirtualResultsTable
                                columns={db.state.activeQueryTab.results.columns}
                                rows={db.state.activeQueryTab.results.rows}
                                isEditable={!!isEditable}
                                onCellSave={handleCellSave}
                                onRowDelete={confirmDeleteRow}
                                {deletingRowIndex}
                                onCopyCell={copyCell}
                                onCopyRow={copyRowAsJSON}
                                onCopyColumn={copyColumn}
                                onCellRightClick={handleCellRightClick}
                            />
                        {:else}
                            <ContextMenu.Root>
                                <ContextMenu.Trigger class="flex-1 overflow-auto min-h-0 block">
                                    <table class="w-full text-sm">
                                        <thead class="sticky top-0 bg-muted border-b">
                                            <tr>
                                                {#if isEditable}
                                                    <th class="px-2 py-2 w-8"></th>
                                                {/if}
                                                {#each db.state.activeQueryTab.results.columns as column}
                                                    <th class="px-4 py-2 text-left font-medium">{column}</th>
                                                {/each}
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {#each db.state.activeQueryTab.results.rows as row, i}
                                                <tr class={["border-b hover:bg-muted/50", i % 2 === 0 && "bg-muted/20"]}>
                                                    {#if isEditable}
                                                        <td class="px-2 py-1">
                                                            <RowActions
                                                                onDelete={async () => confirmDeleteRow(i, row)}
                                                                isDeleting={deletingRowIndex === i}
                                                            />
                                                        </td>
                                                    {/if}
                                                    {#each db.state.activeQueryTab.results.columns as column}
                                                        <td
                                                            class="px-4 py-2"
                                                            oncontextmenu={() => handleCellRightClick(row[column], column, row)}
                                                        >
                                                            <EditableCell
                                                                value={row[column]}
                                                                isEditable={isEditable}
                                                                onSave={(newValue) => handleCellSave(i, column, newValue)}
                                                            />
                                                        </td>
                                                    {/each}
                                                </tr>
                                            {/each}
                                        </tbody>
                                    </table>
                                </ContextMenu.Trigger>
                                <ContextMenu.Portal>
                                    <ContextMenu.Content class="w-48">
                                        <ContextMenu.Item onclick={copyCell}>
                                            <CopyIcon class="size-4 me-2" />
                                            {m.query_copy_cell()}
                                        </ContextMenu.Item>
                                        <ContextMenu.Item onclick={copyRowAsJSON}>
                                            <CopyIcon class="size-4 me-2" />
                                            {m.query_copy_row()}
                                        </ContextMenu.Item>
                                        <ContextMenu.Item onclick={copyColumn}>
                                            <CopyIcon class="size-4 me-2" />
                                            {m.query_copy_column()}
                                        </ContextMenu.Item>
                                    </ContextMenu.Content>
                                </ContextMenu.Portal>
                            </ContextMenu.Root>
                        {/if}

                        <!-- Pagination Controls -->
                        {#if db.state.activeQueryTab.results.totalPages > 1}
                            {@const results = db.state.activeQueryTab.results}
                            {@const start = (results.page - 1) * results.pageSize + 1}
                            {@const end = Math.min(results.page * results.pageSize, results.totalRows)}
                            <div class="flex items-center justify-between p-2 border-t bg-muted/30 shrink-0 text-xs">
                                <div class="text-muted-foreground">
                                    {m.query_showing_rows({ start, end, total: results.totalRows.toLocaleString() })}
                                </div>
                                <div class="flex items-center gap-2">
                                    <DropdownMenu.Root>
                                        <DropdownMenu.Trigger
                                            class={buttonVariants({
                                                variant: "outline",
                                                size: "sm",
                                            }) + " h-7 gap-1 text-xs"}
                                        >
                                            {db.state.activeQueryTab.results.pageSize} rows
                                            <ChevronDownIcon class="size-3" />
                                        </DropdownMenu.Trigger>
                                        <DropdownMenu.Content align="end">
                                            {#each [25, 50, 100, 250, 500, 1000] as size}
                                                <DropdownMenu.Item
                                                    onclick={() => db.queries.setPageSize(db.state.activeQueryTabId!, size)}
                                                    class={db.state.activeQueryTab.results.pageSize === size ? "bg-accent" : ""}
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
                                            disabled={db.state.activeQueryTab.results.page === 1 || db.state.activeQueryTab.isExecuting}
                                        >
                                            <ChevronsLeftIcon class="size-3" />
                                        </Button>
                                        <Button
                                            size="icon"
                                            variant="outline"
                                            class="size-7"
                                            onclick={() => db.queries.goToPage(db.state.activeQueryTabId!, db.state.activeQueryTab!.results!.page - 1)}
                                            disabled={db.state.activeQueryTab.results.page === 1 || db.state.activeQueryTab.isExecuting}
                                        >
                                            <ChevronLeftIcon class="size-3" />
                                        </Button>
                                        <span class="px-2 text-muted-foreground">
                                            {m.query_page_of({ page: db.state.activeQueryTab.results.page, total: db.state.activeQueryTab.results.totalPages })}
                                        </span>
                                        <Button
                                            size="icon"
                                            variant="outline"
                                            class="size-7"
                                            onclick={() => db.queries.goToPage(db.state.activeQueryTabId!, db.state.activeQueryTab!.results!.page + 1)}
                                            disabled={db.state.activeQueryTab.results.page === db.state.activeQueryTab.results.totalPages || db.state.activeQueryTab.isExecuting}
                                        >
                                            <ChevronRightIcon class="size-3" />
                                        </Button>
                                        <Button
                                            size="icon"
                                            variant="outline"
                                            class="size-7"
                                            onclick={() => db.queries.goToPage(db.state.activeQueryTabId!, db.state.activeQueryTab!.results!.totalPages)}
                                            disabled={db.state.activeQueryTab.results.page === db.state.activeQueryTab.results.totalPages || db.state.activeQueryTab.isExecuting}
                                        >
                                            <ChevronsRightIcon class="size-3" />
                                        </Button>
                                    </div>
                                </div>
                            </div>
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

<!-- Insert Row Dialog -->
{#if db.state.activeQueryTab?.results?.sourceTable && sourceTableColumns.length > 0}
    <InsertRowDialog
        bind:open={showInsertDialog}
        sourceTable={db.state.activeQueryTab.results.sourceTable}
        columns={sourceTableColumns}
        onClose={() => showInsertDialog = false}
        onSuccess={() => db.state.activeQueryTabId && db.queries.execute(db.state.activeQueryTabId)}
    />
{/if}
