<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Button } from "$lib/components/ui/button";
	import { Textarea } from "$lib/components/ui/textarea";
	import { Input } from "$lib/components/ui/input";
	import { PlayIcon, PlusIcon, XIcon, SaveIcon, DownloadIcon, LoaderIcon } from "@lucide/svelte";
	import { Badge } from "$lib/components/ui/badge";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import SaveQueryDialog from "$lib/components/save-query-dialog.svelte";

	const db = useDatabase();
	let editingTabId = $state<string | null>(null);
	let editingTabName = $state("");
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

	const handleExport = (format: "csv" | "json") => {
		if (!db.activeQueryTab?.results) return;
		// Simulate export
		console.log(`Exporting as ${format}`, db.activeQueryTab.results);
	};

	const startEditing = (tabId: string, currentName: string) => {
		editingTabId = tabId;
		editingTabName = currentName;
	};

	const finishEditing = () => {
		if (editingTabId && editingTabName.trim()) {
			db.renameQueryTab(editingTabId, editingTabName.trim());
		}
		editingTabId = null;
		editingTabName = "";
	};

	const handleKeydown = (e: KeyboardEvent) => {
		if (e.key === "Enter") {
			finishEditing();
		} else if (e.key === "Escape") {
			editingTabId = null;
			editingTabName = "";
		}
	};

	const handleTextareaKeydown = (e: KeyboardEvent) => {
		if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
			e.preventDefault();
			handleExecute();
		}
	};
</script>

<div class="flex flex-col h-full">
	<div class="flex items-center justify-between p-2 border-b bg-muted/30">
		<div class="flex items-center gap-2 flex-1 overflow-hidden min-w-0">
			{#if db.queryTabs.length > 0}
				<ScrollArea orientation="horizontal" class="flex-1">
					<div class="flex items-center gap-1 pb-1">
						{#each db.queryTabs as tab (tab.id)}
						    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
							<div class={["relative group shrink-0 flex items-center gap-2 px-3 h-7 text-xs rounded-md transition-colors", db.activeQueryTabId === tab.id ? "bg-background shadow-sm" : "hover:bg-muted"]} onclick={() => db.setActiveQueryTab(tab.id)}>
								{#if editingTabId === tab.id}
									<Input bind:value={editingTabName} class="h-5 px-1 text-xs w-24" onblur={finishEditing} onkeydown={handleKeydown} onclick={(e) => e.stopPropagation()} />
								{:else}
									<span
										class="pr-4"
										ondblclick={(e) => {
											e.stopPropagation();
											startEditing(tab.id, tab.name);
										}}
									>
										{tab.name}
									</span>
								{/if}
								<Button
									size="icon"
									variant="ghost"
									class="absolute right-0 top-1/2 -translate-y-1/2 size-5 opacity-0 group-hover:opacity-100 transition-opacity [&_svg:not([class*='size-'])]:size-3"
									onclick={(e) => {
										e.stopPropagation();
										db.removeQueryTab(tab.id);
									}}
								>
									<XIcon />
								</Button>
							</div>
						{/each}
					</div>
				</ScrollArea>
			{/if}
			<Button size="icon" variant="ghost" class="size-7 shrink-0 [&_svg:not([class*='size-'])]:size-4" onclick={() => db.addQueryTab()}>
				<PlusIcon />
			</Button>
		</div>

		<div class="flex items-center gap-2 shrink-0">
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

	{#if db.activeQueryTab}
		<div class="flex-1 flex flex-col">
			<div class="h-64 border-b">
				<Textarea bind:value={db.activeQueryTab.query} placeholder="Enter your SQL query here..." class="h-full resize-none rounded-none border-0 font-mono text-sm focus-visible:ring-0" onchange={(e) => db.updateQueryTabContent(db.activeQueryTab!.id, e.currentTarget.value)} onkeydown={handleTextareaKeydown} />
			</div>

			<div class="h-full flex flex-col">
				{#if db.activeQueryTab.results}
					<div class="flex items-center justify-between p-2 border-b bg-muted/30">
						<div class="flex items-center gap-3 text-xs text-muted-foreground">
							<span class="flex items-center gap-1">
								<Badge variant="secondary" class="text-xs">{db.activeQueryTab.results.rowCount}</Badge>
								rows
							</span>
							<span class="flex items-center gap-1">
								<Badge variant="secondary" class="text-xs">{db.activeQueryTab.results.executionTime}ms</Badge>
								execution time
							</span>
						</div>
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

					<div class="flex-1 overflow-auto">
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
						</div>
					</div>
				{/if}
			</div>
		</div>
	{:else}
		<div class="flex-1 flex items-center justify-center text-muted-foreground">
			<div class="text-center">
				<PlusIcon class="size-12 mx-auto mb-2 opacity-20" />
				<p class="text-sm">Create a new query tab to get started</p>
			</div>
		</div>
	{/if}
</div>

{#if db.activeQueryTab}
	<SaveQueryDialog bind:open={showSaveDialog} query={db.activeQueryTab.query} tabId={db.activeQueryTab.id} />
{/if}
