<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
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

	const db = useDatabase();
	let showSaveDialog = $state(false);
	let showInsertDialog = $state(false);
	let deletingRowIndex = $state<number | null>(null);
	let pendingDeleteRow = $state<{ index: number; row: Record<string, unknown> } | null>(null);
	let showDeleteConfirm = $state(false);

	// Get columns for the source table (for insert dialog)
	const sourceTableColumns = $derived.by(() => {
		const sourceTable = db.activeQueryTab?.results?.sourceTable;
		if (!sourceTable || !db.activeConnectionId) return [];
		const tables = db.schemas.get(db.activeConnectionId) || [];
		const table = tables.find(t => t.name === sourceTable.name && t.schema === sourceTable.schema);
		return table?.columns || [];
	});

	// Check if results are editable (have source table with primary keys)
	const isEditable = $derived(
		db.activeQueryTab?.results?.sourceTable &&
		db.activeQueryTab?.results?.sourceTable.primaryKeys.length > 0
	);

	async function handleCellSave(rowIndex: number, column: string, newValue: string) {
		if (!db.activeQueryTabId || !db.activeQueryTab?.results?.sourceTable) return;

		const result = await db.updateCell(
			db.activeQueryTabId,
			rowIndex,
			column,
			newValue,
			db.activeQueryTab.results.sourceTable
		);

		if (result.success) {
			toast.success('Cell updated');
		} else {
			toast.error(`Failed to update: ${result.error}`);
		}
	}

	function confirmDeleteRow(rowIndex: number, row: Record<string, unknown>) {
		pendingDeleteRow = { index: rowIndex, row };
		showDeleteConfirm = true;
	}

	async function handleDeleteRow() {
		if (!pendingDeleteRow || !db.activeQueryTabId || !db.activeQueryTab?.results?.sourceTable) return;

		deletingRowIndex = pendingDeleteRow.index;
		showDeleteConfirm = false;

		const result = await db.deleteRow(
			db.activeQueryTab.results.sourceTable,
			pendingDeleteRow.row
		);

		if (result.success) {
			toast.success('Row deleted');
			// Refresh data
			await db.executeQuery(db.activeQueryTabId);
		} else {
			toast.error(`Failed to delete: ${result.error}`);
		}

		deletingRowIndex = null;
		pendingDeleteRow = null;
	}

	const handleExecute = () => {
		if (db.activeQueryTabId) {
			db.executeQuery(db.activeQueryTabId);
		}
	};

	const handleExplain = (analyze: boolean) => {
		if (db.activeQueryTabId) {
			db.executeExplain(db.activeQueryTabId, analyze);
		}
	};

	const handleSave = () => {
		if (!db.activeQueryTab?.query.trim()) return;
		showSaveDialog = true;
	};

	const handleFormat = () => {
		if (!db.activeQueryTab?.query.trim()) return;
		try {
			const formatted = formatSQL(db.activeQueryTab.query, {
				language: "postgresql",
				tabWidth: 2,
				keywordCase: "upper"
			});
			db.activeQueryTab.query = formatted;
		} catch {
			toast.error("Failed to format SQL");
		}
	};

	const escapeCSVValue = (value: unknown): string => {
		if (value === null || value === undefined) return "";
		const str = String(value);
		if (str.includes(",") || str.includes('"') || str.includes("\n")) {
			return `"${str.replace(/"/g, '""')}"`;
		}
		return str;
	};

	const generateCSV = (): string => {
		const results = db.activeQueryTab?.results;
		if (!results) return "";

		const header = results.columns.map(escapeCSVValue).join(",");
		const rows = results.rows.map((row) =>
			results.columns.map((col) => escapeCSVValue(row[col])).join(",")
		);
		return [header, ...rows].join("\n");
	};

	const generateJSON = (): string => {
		const results = db.activeQueryTab?.results;
		if (!results) return "[]";
		return JSON.stringify(results.rows, null, 2);
	};

	const generateSQL = (tableName: string = "table_name"): string => {
		const results = db.activeQueryTab?.results;
		if (!results || results.rows.length === 0) return "";

		const escapeValue = (value: unknown): string => {
			if (value === null || value === undefined) return "NULL";
			if (typeof value === "number") return String(value);
			if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
			const str = String(value);
			return `'${str.replace(/'/g, "''")}'`;
		};

		const columns = results.columns.join(", ");
		const inserts = results.rows.map((row) => {
			const values = results.columns.map((col) => escapeValue(row[col])).join(", ");
			return `INSERT INTO ${tableName} (${columns}) VALUES (${values});`;
		});

		return inserts.join("\n");
	};

	const generateMarkdown = (): string => {
		const results = db.activeQueryTab?.results;
		if (!results || results.rows.length === 0) return "";

		const escapeMarkdown = (value: unknown): string => {
			if (value === null || value === undefined) return "";
			return String(value).replace(/\|/g, "\\|").replace(/\n/g, " ");
		};

		const header = `| ${results.columns.join(" | ")} |`;
		const separator = `| ${results.columns.map(() => "---").join(" | ")} |`;
		const rows = results.rows.map(
			(row) => `| ${results.columns.map((col) => escapeMarkdown(row[col])).join(" | ")} |`
		);

		return [header, separator, ...rows].join("\n");
	};

	type ExportFormat = "csv" | "json" | "sql" | "markdown";

	const formatConfig: Record<ExportFormat, { extension: string; name: string }> = {
		csv: { extension: "csv", name: "CSV" },
		json: { extension: "json", name: "JSON" },
		sql: { extension: "sql", name: "SQL" },
		markdown: { extension: "md", name: "Markdown" }
	};

	const getContent = (format: ExportFormat): string => {
		switch (format) {
			case "csv":
				return generateCSV();
			case "json":
				return generateJSON();
			case "sql":
				return generateSQL();
			case "markdown":
				return generateMarkdown();
		}
	};

	const handleExport = async (format: ExportFormat) => {
		if (!db.activeQueryTab?.results) return;

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
		if (!db.activeQueryTab?.results) return;

		const content = getContent(format);
		const formatNames: Record<ExportFormat, string> = {
			csv: "CSV",
			json: "JSON",
			sql: "SQL INSERT",
			markdown: "Markdown"
		};

		try {
			await navigator.clipboard.writeText(content);
			toast.success(`${formatNames[format]} copied to clipboard`);
		} catch {
			toast.error("Failed to copy to clipboard");
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
		toast.success("Cell value copied");
	};

	const copyRowAsJSON = async () => {
		if (!contextCell) return;
		await navigator.clipboard.writeText(JSON.stringify(contextCell.row, null, 2));
		toast.success("Row copied as JSON");
	};

	const copyColumn = async () => {
		if (!contextCell || !db.activeQueryTab?.results) return;
		const col = contextCell.column;
		const values = db.activeQueryTab.results.rows
			.map(row => row[col])
			.map(v => v === null || v === undefined ? "" : String(v))
			.join("\n");
		await navigator.clipboard.writeText(values);
		toast.success("Column values copied");
	};

	// Keyboard shortcuts
	const handleKeydown = (e: KeyboardEvent) => {
		const isMod = e.metaKey || e.ctrlKey;

		// Cmd+S: Save query
		if (isMod && e.key === 's') {
			e.preventDefault();
			handleSave();
			return;
		}

		// Cmd+Shift+F: Format SQL
		if (isMod && e.shiftKey && e.key === 'f') {
			e.preventDefault();
			handleFormat();
			return;
		}
	};
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="flex flex-col h-full overflow-hidden">
    {#if db.activeQueryTab}
        <!-- Toolbar -->
        <div
            class="flex items-center justify-between p-2 border-b bg-muted/30 shrink-0"
        >
            <div class="flex items-center gap-3 text-xs text-muted-foreground">
                {#if db.activeQueryTab.results}
                    <span class="flex items-center gap-1">
                        {#if db.activeQueryTab.results.queryType && ['insert', 'update', 'delete'].includes(db.activeQueryTab.results.queryType)}
                            <Badge variant="secondary" class="text-xs"
                                >{db.activeQueryTab.results.affectedRows ?? 0}</Badge
                            >
                            rows affected
                            {#if db.activeQueryTab.results.lastInsertId}
                                <Badge variant="outline" class="text-xs ml-1"
                                    >ID: {db.activeQueryTab.results.lastInsertId}</Badge
                                >
                            {/if}
                        {:else}
                            <Badge variant="secondary" class="text-xs"
                                >{db.activeQueryTab.results.totalRows.toLocaleString()}</Badge
                            >
                            total rows
                        {/if}
                    </span>
                    <span class="flex items-center gap-1">
                        <Badge variant="secondary" class="text-xs"
                            >{db.activeQueryTab.results.executionTime}ms</Badge
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
                        disabled={!db.activeQueryTab || db.activeQueryTab.isExecuting}
                    >
                        {#if db.activeQueryTab?.isExecuting}
                            <LoaderIcon class="animate-spin size-3" />
                        {:else}
                            <PlayIcon class="size-3" />
                        {/if}
                        Execute
                    </Button>
                    <DropdownMenu.Root>
                        <DropdownMenu.Trigger
                            class={buttonVariants({ size: "sm", variant: "default" }) + " h-7 px-1.5 rounded-l-none border-l border-primary-foreground/20"}
                            disabled={!db.activeQueryTab || db.activeQueryTab.isExecuting}
                        >
                            <ChevronDownIcon class="size-3" />
                        </DropdownMenu.Trigger>
                        <DropdownMenu.Content align="end">
                            <DropdownMenu.Item onclick={handleExecute}>
                                <PlayIcon class="size-4 mr-2" />
                                Execute
                            </DropdownMenu.Item>
                            <DropdownMenu.Separator />
                            <DropdownMenu.Item onclick={() => handleExplain(false)}>
                                <SearchIcon class="size-4 mr-2" />
                                Explain
                            </DropdownMenu.Item>
                            <DropdownMenu.Item onclick={() => handleExplain(true)}>
                                <ActivityIcon class="size-4 mr-2" />
                                Explain Analyze
                            </DropdownMenu.Item>
                        </DropdownMenu.Content>
                    </DropdownMenu.Root>
                </div>
                <Button
                    size="sm"
                    variant="outline"
                    class="h-7 gap-1"
                    onclick={handleFormat}
                    disabled={!db.activeQueryTab?.query.trim()}
                >
                    <WandSparklesIcon class="size-3" />
                    Format
                </Button>
                {#if isEditable && sourceTableColumns.length > 0}
                    <Button
                        size="sm"
                        variant="outline"
                        class="h-7 gap-1"
                        onclick={() => showInsertDialog = true}
                    >
                        <PlusIcon class="size-3" />
                        Add Row
                    </Button>
                {/if}
                <Button
                    size="sm"
                    variant="outline"
                    class="h-7 gap-1"
                    onclick={handleSave}
                    disabled={!db.activeQueryTab?.query.trim()}
                >
                    <SaveIcon class="size-3" />
                    Save
                </Button>
            </div>
        </div>

        <Resizable.PaneGroup direction="vertical" class="flex-1 min-h-0">
            <!-- Editor Pane -->
            <Resizable.Pane defaultSize={40} minSize={15}>
                <div class="h-full">
                    {#key db.activeQueryTabId}
                        <MonacoEditor
                            bind:value={db.activeQueryTab.query}
                            schema={db.activeSchema}
                            onExecute={handleExecute}
                        />
                    {/key}
                </div>
            </Resizable.Pane>

            <Resizable.Handle withHandle />

            <!-- Results Pane -->
            <Resizable.Pane defaultSize={60} minSize={15}>
                <div class="h-full flex flex-col overflow-hidden">
                    {#if db.activeQueryTab.results}
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
                                    Export
                                    <ChevronDownIcon class="size-3" />
                                </DropdownMenu.Trigger>
                                <DropdownMenu.Content align="end" class="w-48">
                                    <DropdownMenu.Group>
                                        <DropdownMenu.GroupHeading
                                            >Download</DropdownMenu.GroupHeading
                                        >
                                        <DropdownMenu.Item
                                            onclick={() => handleExport("csv")}
                                        >
                                            <DownloadIcon class="size-4 mr-2" />
                                            CSV
                                        </DropdownMenu.Item>
                                        <DropdownMenu.Item
                                            onclick={() => handleExport("json")}
                                        >
                                            <DownloadIcon class="size-4 mr-2" />
                                            JSON
                                        </DropdownMenu.Item>
                                        <DropdownMenu.Item
                                            onclick={() => handleExport("sql")}
                                        >
                                            <DownloadIcon class="size-4 mr-2" />
                                            SQL INSERT
                                        </DropdownMenu.Item>
                                        <DropdownMenu.Item
                                            onclick={() =>
                                                handleExport("markdown")}
                                        >
                                            <DownloadIcon class="size-4 mr-2" />
                                            Markdown
                                        </DropdownMenu.Item>
                                    </DropdownMenu.Group>
                                    <DropdownMenu.Separator />
                                    <DropdownMenu.Group>
                                        <DropdownMenu.GroupHeading
                                            >Copy to Clipboard</DropdownMenu.GroupHeading
                                        >
                                        <DropdownMenu.Item
                                            onclick={() => handleCopy("csv")}
                                        >
                                            <CopyIcon class="size-4 mr-2" />
                                            CSV
                                        </DropdownMenu.Item>
                                        <DropdownMenu.Item
                                            onclick={() => handleCopy("json")}
                                        >
                                            <CopyIcon class="size-4 mr-2" />
                                            JSON
                                        </DropdownMenu.Item>
                                        <DropdownMenu.Item
                                            onclick={() => handleCopy("sql")}
                                        >
                                            <CopyIcon class="size-4 mr-2" />
                                            SQL INSERT
                                        </DropdownMenu.Item>
                                        <DropdownMenu.Item
                                            onclick={() =>
                                                handleCopy("markdown")}
                                        >
                                            <CopyIcon class="size-4 mr-2" />
                                            Markdown
                                        </DropdownMenu.Item>
                                    </DropdownMenu.Group>
                                </DropdownMenu.Content>
                            </DropdownMenu.Root>
                        </div>

                        <ContextMenu.Root>
                            <ContextMenu.Trigger class="flex-1 overflow-auto min-h-0 block">
                                <table class="w-full text-sm">
                                    <thead class="sticky top-0 bg-muted border-b">
                                        <tr>
                                            {#if isEditable}
                                                <th class="px-2 py-2 w-8"></th>
                                            {/if}
                                            {#each db.activeQueryTab.results.columns as column}
                                                <th class="px-4 py-2 text-left font-medium">{column}</th>
                                            {/each}
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {#each db.activeQueryTab.results.rows as row, i}
                                            <tr class={["border-b hover:bg-muted/50", i % 2 === 0 && "bg-muted/20"]}>
                                                {#if isEditable}
                                                    <td class="px-2 py-1">
                                                        <RowActions
                                                            onDelete={async () => confirmDeleteRow(i, row)}
                                                            isDeleting={deletingRowIndex === i}
                                                        />
                                                    </td>
                                                {/if}
                                                {#each db.activeQueryTab.results.columns as column}
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
                                        <CopyIcon class="size-4 mr-2" />
                                        Copy Cell Value
                                    </ContextMenu.Item>
                                    <ContextMenu.Item onclick={copyRowAsJSON}>
                                        <CopyIcon class="size-4 mr-2" />
                                        Copy Row as JSON
                                    </ContextMenu.Item>
                                    <ContextMenu.Item onclick={copyColumn}>
                                        <CopyIcon class="size-4 mr-2" />
                                        Copy Column Values
                                    </ContextMenu.Item>
                                </ContextMenu.Content>
                            </ContextMenu.Portal>
                        </ContextMenu.Root>

                        <!-- Pagination Controls -->
                        {#if db.activeQueryTab.results.totalPages > 1}
                            {@const results = db.activeQueryTab.results}
                            {@const start = (results.page - 1) * results.pageSize + 1}
                            {@const end = Math.min(results.page * results.pageSize, results.totalRows)}
                            <div class="flex items-center justify-between p-2 border-t bg-muted/30 shrink-0 text-xs">
                                <div class="text-muted-foreground">
                                    Showing {start}-{end} of {results.totalRows.toLocaleString()} rows
                                </div>
                                <div class="flex items-center gap-2">
                                    <DropdownMenu.Root>
                                        <DropdownMenu.Trigger
                                            class={buttonVariants({
                                                variant: "outline",
                                                size: "sm",
                                            }) + " h-7 gap-1 text-xs"}
                                        >
                                            {db.activeQueryTab.results.pageSize} rows
                                            <ChevronDownIcon class="size-3" />
                                        </DropdownMenu.Trigger>
                                        <DropdownMenu.Content align="end">
                                            {#each [25, 50, 100, 250, 500, 1000] as size}
                                                <DropdownMenu.Item
                                                    onclick={() => db.setPageSize(db.activeQueryTabId!, size)}
                                                    class={db.activeQueryTab.results.pageSize === size ? "bg-accent" : ""}
                                                >
                                                    {size} rows
                                                </DropdownMenu.Item>
                                            {/each}
                                        </DropdownMenu.Content>
                                    </DropdownMenu.Root>

                                    <div class="flex items-center gap-1">
                                        <Button
                                            size="icon"
                                            variant="outline"
                                            class="size-7"
                                            onclick={() => db.goToPage(db.activeQueryTabId!, 1)}
                                            disabled={db.activeQueryTab.results.page === 1 || db.activeQueryTab.isExecuting}
                                        >
                                            <ChevronsLeftIcon class="size-3" />
                                        </Button>
                                        <Button
                                            size="icon"
                                            variant="outline"
                                            class="size-7"
                                            onclick={() => db.goToPage(db.activeQueryTabId!, db.activeQueryTab!.results!.page - 1)}
                                            disabled={db.activeQueryTab.results.page === 1 || db.activeQueryTab.isExecuting}
                                        >
                                            <ChevronLeftIcon class="size-3" />
                                        </Button>
                                        <span class="px-2 text-muted-foreground">
                                            Page {db.activeQueryTab.results.page} of {db.activeQueryTab.results.totalPages}
                                        </span>
                                        <Button
                                            size="icon"
                                            variant="outline"
                                            class="size-7"
                                            onclick={() => db.goToPage(db.activeQueryTabId!, db.activeQueryTab!.results!.page + 1)}
                                            disabled={db.activeQueryTab.results.page === db.activeQueryTab.results.totalPages || db.activeQueryTab.isExecuting}
                                        >
                                            <ChevronRightIcon class="size-3" />
                                        </Button>
                                        <Button
                                            size="icon"
                                            variant="outline"
                                            class="size-7"
                                            onclick={() => db.goToPage(db.activeQueryTabId!, db.activeQueryTab!.results!.totalPages)}
                                            disabled={db.activeQueryTab.results.page === db.activeQueryTab.results.totalPages || db.activeQueryTab.isExecuting}
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
                                    Execute a query to see results
                                </p>
                                <p class="text-xs mt-1 text-muted-foreground">
                                    Press ⌘+Enter to run
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
                <p class="text-sm">Create a new query tab to get started</p>
            </div>
        </div>
    {/if}
</div>

{#if db.activeQueryTab}
    <SaveQueryDialog
        bind:open={showSaveDialog}
        query={db.activeQueryTab.query}
        tabId={db.activeQueryTab.id}
    />
{/if}

<!-- Delete Row Confirmation Dialog -->
<Dialog.Root bind:open={showDeleteConfirm}>
    <Dialog.Content class="sm:max-w-md">
        <Dialog.Header>
            <Dialog.Title>Delete Row</Dialog.Title>
            <Dialog.Description>
                Are you sure you want to delete this row? This action cannot be undone.
            </Dialog.Description>
        </Dialog.Header>
        <Dialog.Footer>
            <Button variant="outline" onclick={() => showDeleteConfirm = false}>
                Cancel
            </Button>
            <Button variant="destructive" onclick={handleDeleteRow}>
                Delete
            </Button>
        </Dialog.Footer>
    </Dialog.Content>
</Dialog.Root>

<!-- Insert Row Dialog -->
{#if db.activeQueryTab?.results?.sourceTable && sourceTableColumns.length > 0}
    <InsertRowDialog
        bind:open={showInsertDialog}
        sourceTable={db.activeQueryTab.results.sourceTable}
        columns={sourceTableColumns}
        onClose={() => showInsertDialog = false}
        onSuccess={() => db.activeQueryTabId && db.executeQuery(db.activeQueryTabId)}
    />
{/if}
