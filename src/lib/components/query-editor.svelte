<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Button } from "$lib/components/ui/button";
	import { PlayIcon, SaveIcon, DownloadIcon, LoaderIcon } from "@lucide/svelte";
	import { Badge } from "$lib/components/ui/badge";
	import SaveQueryDialog from "$lib/components/save-query-dialog.svelte";
	import MonacoEditor from "$lib/components/monaco-editor.svelte";
	import * as Resizable from "$lib/components/ui/resizable";

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

	const downloadFile = (content: string, filename: string, mimeType: string) => {
		const blob = new Blob([content], { type: mimeType });
		const url = URL.createObjectURL(blob);
		const link = document.createElement("a");
		link.href = url;
		link.download = filename;
		document.body.appendChild(link);
		link.click();
		document.body.removeChild(link);
		URL.revokeObjectURL(url);
	};

	const escapeCSVValue = (value: unknown): string => {
		if (value === null || value === undefined) return "";
		const str = String(value);
		if (str.includes(",") || str.includes('"') || str.includes("\n")) {
			return `"${str.replace(/"/g, '""')}"`;
		}
		return str;
	};

	const exportToCSV = () => {
		const results = db.activeQueryTab?.results;
		if (!results) return;

		const header = results.columns.map(escapeCSVValue).join(",");
		const rows = results.rows.map((row) =>
			results.columns.map((col) => escapeCSVValue(row[col])).join(",")
		);
		const csv = [header, ...rows].join("\n");

		const timestamp = new Date().toISOString().slice(0, 19).replace(/[:-]/g, "");
		downloadFile(csv, `query_results_${timestamp}.csv`, "text/csv;charset=utf-8");
	};

	const exportToJSON = () => {
		const results = db.activeQueryTab?.results;
		if (!results) return;

		const json = JSON.stringify(results.rows, null, 2);
		const timestamp = new Date().toISOString().slice(0, 19).replace(/[:-]/g, "");
		downloadFile(json, `query_results_${timestamp}.json`, "application/json");
	};

	const handleExport = (format: "csv" | "json") => {
		if (!db.activeQueryTab?.results) return;
		if (format === "csv") {
			exportToCSV();
		} else {
			exportToJSON();
		}
	};
</script>

<div class="flex flex-col h-full overflow-hidden">
	{#if db.activeQueryTab}
		<!-- Toolbar -->
		<div class="flex items-center justify-between p-2 border-b bg-muted/30 shrink-0">
			<div class="flex items-center gap-3 text-xs text-muted-foreground">
				{#if db.activeQueryTab.results}
					<span class="flex items-center gap-1">
						<Badge variant="secondary" class="text-xs">{db.activeQueryTab.results.rowCount}</Badge>
						rows
					</span>
					<span class="flex items-center gap-1">
						<Badge variant="secondary" class="text-xs">{db.activeQueryTab.results.executionTime}ms</Badge>
					</span>
				{/if}
			</div>
			<div class="flex items-center gap-2">
				<Button size="sm" class="h-7 gap-1" onclick={handleExecute} disabled={!db.activeQueryTab || db.activeQueryTab.isExecuting}>
					{#if db.activeQueryTab?.isExecuting}
						<LoaderIcon class="animate-spin size-3" />
					{:else}
						<PlayIcon class="size-3" />
					{/if}
					Execute
				</Button>
				<Button size="sm" variant="outline" class="h-7 gap-1" onclick={handleSave} disabled={!db.activeQueryTab?.query.trim()}>
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
						<div class="flex items-center justify-end p-2 border-b bg-muted/30 shrink-0">
							<div class="flex items-center gap-2">
								<Button size="sm" variant="outline" class="h-7 gap-1" onclick={() => handleExport("csv")}>
									<DownloadIcon class="size-3" />
									CSV
								</Button>
								<Button size="sm" variant="outline" class="h-7 gap-1" onclick={() => handleExport("json")}>
									<DownloadIcon class="size-3" />
									JSON
								</Button>
							</div>
						</div>

						<div class="flex-1 overflow-auto min-h-0">
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
												<td class="px-4 py-2">{row[column]}</td>
											{/each}
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{:else}
						<div class="flex-1 flex items-center justify-center text-muted-foreground">
							<div class="text-center">
								<PlayIcon class="size-12 mx-auto mb-2 opacity-20" />
								<p class="text-sm">Execute a query to see results</p>
								<p class="text-xs mt-1 text-muted-foreground">Press ⌘+Enter to run</p>
							</div>
						</div>
					{/if}
				</div>
			</Resizable.Pane>
		</Resizable.PaneGroup>
	{:else}
		<div class="flex-1 flex items-center justify-center text-muted-foreground">
			<div class="text-center">
				<PlayIcon class="size-12 mx-auto mb-2 opacity-20" />
				<p class="text-sm">Create a new query tab to get started</p>
			</div>
		</div>
	{/if}
</div>

{#if db.activeQueryTab}
	<SaveQueryDialog bind:open={showSaveDialog} query={db.activeQueryTab.query} tabId={db.activeQueryTab.id} />
{/if}
