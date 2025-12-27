<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Button, buttonVariants } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import { PlayIcon, SaveIcon, DownloadIcon, LoaderIcon, CopyIcon, ChevronDownIcon, WandSparklesIcon } from "@lucide/svelte";
	import { Badge } from "$lib/components/ui/badge";
	import { toast } from "svelte-sonner";
	import SaveQueryDialog from "$lib/components/save-query-dialog.svelte";
	import MonacoEditor from "$lib/components/monaco-editor.svelte";
	import * as Resizable from "$lib/components/ui/resizable";
	import { save } from "@tauri-apps/plugin-dialog";
	import { writeTextFile } from "@tauri-apps/plugin-fs";
	import { format as formatSQL } from "sql-formatter";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";

	const db = useDatabase();
	let showSaveDialog = $state(false);

	const handleExecute = () => {
		if (db.activeQueryTabId) {
			db.executeQuery(db.activeQueryTabId);
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
                        <Badge variant="secondary" class="text-xs"
                            >{db.activeQueryTab.results.rowCount}</Badge
                        >
                        rows
                    </span>
                    <span class="flex items-center gap-1">
                        <Badge variant="secondary" class="text-xs"
                            >{db.activeQueryTab.results.executionTime}ms</Badge
                        >
                    </span>
                {/if}
            </div>
            <div class="flex items-center gap-2">
                <Button
                    size="sm"
                    class="h-7 gap-1"
                    onclick={handleExecute}
                    disabled={!db.activeQueryTab ||
                        db.activeQueryTab.isExecuting}
                >
                    {#if db.activeQueryTab?.isExecuting}
                        <LoaderIcon class="animate-spin size-3" />
                    {:else}
                        <PlayIcon class="size-3" />
                    {/if}
                    Execute
                </Button>
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
                                            {#each db.activeQueryTab.results.columns as column}
                                                <th class="px-4 py-2 text-left font-medium">{column}</th>
                                            {/each}
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {#each db.activeQueryTab.results.rows as row, i}
                                            <tr class={["border-b hover:bg-muted/50", i % 2 === 0 && "bg-muted/20"]}>
                                                {#each db.activeQueryTab.results.columns as column}
                                                    <td
                                                        class="px-4 py-2"
                                                        oncontextmenu={() => handleCellRightClick(row[column], column, row)}
                                                    >
                                                        {row[column]}
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
