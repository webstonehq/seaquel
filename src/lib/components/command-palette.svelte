<script lang="ts">
	import * as Command from "$lib/components/ui/command";
	import { useDatabase } from "$lib/hooks/database.svelte";
	import { useShortcuts } from "$lib/shortcuts/shortcuts.svelte";
	import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte";
	import {
		Plus,
		Play,
		Save,
		Table2,
		Database,
		History,
		FileText,
		Sparkles,
		PanelLeft,
		Download,
		Copy,
		GitBranch,
		Code,
	} from "@lucide/svelte";
	import { m } from "$lib/paraglide/messages.js";

	const db = useDatabase();
	const shortcuts = useShortcuts();

	let open = $state(false);

	// Derived state for dynamic commands
	const tables = $derived(
		db.state.activeConnectionId ? db.state.schemas.get(db.state.activeConnectionId) || [] : []
	);
	const connections = $derived(db.state.connections);
	const savedQueries = $derived(db.state.activeConnectionSavedQueries);
	const recentHistory = $derived(db.state.activeConnectionQueryHistory?.slice(0, 10) || []);
	const openTabs = $derived(db.tabs.ordered);
	const activeResult = $derived(db.state.activeQueryResult);
	const hasResults = $derived((activeResult?.rows?.length ?? 0) > 0);
	const isConnected = $derived(!!db.state.activeConnectionId && !!(db.state.activeConnection?.database || db.state.activeConnection?.mssqlConnectionId));
	const hasActiveQueryTab = $derived(isConnected && !!db.state.activeQueryTab);
	const hasQueryContent = $derived(hasActiveQueryTab && !!db.state.activeQueryTab?.query?.trim());
	const hasConnections = $derived(connections.length > 0);

	// Register shortcut handler
	$effect(() => {
		shortcuts.registerHandler("commandPalette", () => {
			open = !open;
		});
		return () => shortcuts.unregisterHandler("commandPalette");
	});

	function runAndClose(action: () => void) {
		action();
		open = false;
	}

	// Actions
	function newQueryTab() {
		runAndClose(() => db.queryTabs.add());
	}

	function executeQuery() {
		const tab = db.state.activeQueryTab;
		if (tab) {
			runAndClose(() => db.queries.execute(tab.id));
		}
	}

	function saveQuery() {
		// This triggers the save dialog - we just close the palette
		// The save handler needs to be in the parent component
		runAndClose(() => {
			// Dispatch a custom event to open save dialog
			window.dispatchEvent(new CustomEvent("open-save-query-dialog"));
		});
	}

	function toggleAI() {
		runAndClose(() => db.ui.toggleAI());
	}

	function goToTab(tabId: string, type: "query" | "schema" | "explain" | "erd") {
		runAndClose(() => {
			switch (type) {
				case "query":
					db.queryTabs.setActive(tabId);
					db.ui.setActiveView("query");
					break;
				case "schema":
					db.schemaTabs.setActive(tabId);
					db.ui.setActiveView("schema");
					break;
				case "explain":
					db.explainTabs.setActive(tabId);
					db.ui.setActiveView("explain");
					break;
				case "erd":
					db.erdTabs.setActive(tabId);
					db.ui.setActiveView("erd");
					break;
			}
		});
	}

	function toggleSidebar() {
		runAndClose(() => {
			// Trigger the toggle sidebar shortcut
			const handler = shortcuts as any;
			handler.handlers.get("toggleSidebar")?.();
		});
	}

	function openTable(table: { name: string; schema: string }) {
		const schemaTable = tables.find(
			(t) => t.name === table.name && t.schema === table.schema
		);
		if (schemaTable) {
			runAndClose(() => {
				db.schemaTabs.add(schemaTable);
				db.ui.setActiveView("schema");
			});
		}
	}

	function queryTable(table: { name: string; schema: string }) {
		runAndClose(() => {
			const query = `SELECT * FROM "${table.schema}"."${table.name}" LIMIT 100`;
			db.queryTabs.add(`Query: ${table.name}`, query);
			db.ui.setActiveView("query");
		});
	}

	function viewErd() {
		runAndClose(() => db.erdTabs.add());
	}

	function switchConnection(id: string) {
		const connection = connections.find((c) => c.id === id);
		if (!connection) return;

		if (connection.database || connection.mssqlConnectionId) {
			// Already connected, just switch to it
			runAndClose(() => db.connections.setActive(id));
		} else {
			// Disconnected, open the reconnect dialog
			open = false;
			connectionDialogStore.open({
				id: connection.id,
				name: connection.name,
				type: connection.type,
				host: connection.host,
				port: connection.port,
				databaseName: connection.databaseName,
				username: connection.username,
				sslMode: connection.sslMode,
				connectionString: connection.connectionString,
				sshTunnel: connection.sshTunnel,
			});
		}
	}

	function loadSavedQuery(id: string) {
		runAndClose(() => db.queryTabs.loadSaved(id));
	}

	function loadHistoryItem(id: string) {
		runAndClose(() => db.queryTabs.loadFromHistory(id));
	}

	function exportResults(format: "csv" | "json") {
		if (!activeResult) return;

		let content: string;
		let filename: string;
		const timestamp = new Date().toISOString().replace(/[:.]/g, "-");

		if (format === "csv") {
			const headers = activeResult.columns.join(",");
			const rows = activeResult.rows
				.map((row: Record<string, unknown>) =>
					activeResult.columns
						.map((col: string) => {
							const val = row[col];
							if (val === null || val === undefined) return "";
							const str = String(val);
							return str.includes(",") || str.includes('"') || str.includes("\n")
								? `"${str.replace(/"/g, '""')}"`
								: str;
						})
						.join(",")
				)
				.join("\n");
			content = `${headers}\n${rows}`;
			filename = `query-results-${timestamp}.csv`;
		} else {
			content = JSON.stringify(activeResult.rows, null, 2);
			filename = `query-results-${timestamp}.json`;
		}

		const blob = new Blob([content], { type: format === "csv" ? "text/csv" : "application/json" });
		const url = URL.createObjectURL(blob);
		const a = document.createElement("a");
		a.href = url;
		a.download = filename;
		a.click();
		URL.revokeObjectURL(url);

		open = false;
	}

	function copyResults() {
		if (!activeResult) return;

		const content = JSON.stringify(activeResult.rows, null, 2);
		navigator.clipboard.writeText(content);
		open = false;
	}

	function explainQuery() {
		const tab = db.state.activeQueryTab;
		if (tab) {
			runAndClose(() => db.explainTabs.execute(tab.id, false));
		}
	}

	function explainAnalyzeQuery() {
		const tab = db.state.activeQueryTab;
		if (tab) {
			runAndClose(() => db.explainTabs.execute(tab.id, true));
		}
	}

	function getTabName(tab: { type: string; tab: any }): string {
		switch (tab.type) {
			case "query":
				return tab.tab.name || "Query";
			case "schema":
				return tab.tab.table?.name || "Schema";
			case "explain":
				return tab.tab.name || "Explain";
			case "erd":
				return tab.tab.name || "ERD";
			default:
				return "Tab";
		}
	}

	function getTabIcon(type: string) {
		switch (type) {
			case "query":
				return Code;
			case "schema":
				return Table2;
			case "explain":
				return GitBranch;
			case "erd":
				return GitBranch;
			default:
				return FileText;
		}
	}

	function truncateQuery(query: string, maxLength: number = 50): string {
		const cleaned = query.replace(/\s+/g, " ").trim();
		return cleaned.length > maxLength ? cleaned.substring(0, maxLength) + "..." : cleaned;
	}
</script>

<Command.Dialog bind:open title={m.command_title()} description={m.command_description()}>
	<Command.Input placeholder={m.command_search_placeholder()} />
	<Command.List>
		<Command.Empty>{m.command_no_results()}</Command.Empty>

		<!-- Quick Actions (only show group if there are actions available) -->
		{#if isConnected || hasConnections}
			<Command.Group heading={m.command_group_quick_actions()}>
				{#if isConnected}
					<Command.Item value="new-query-tab" onSelect={newQueryTab}>
						<Plus class="size-4" />
						<span>{m.command_new_query_tab()}</span>
						<Command.Shortcut>⌘T</Command.Shortcut>
					</Command.Item>
				{/if}
				{#if hasQueryContent}
					<Command.Item value="execute-query" onSelect={executeQuery}>
						<Play class="size-4" />
						<span>{m.command_execute_query()}</span>
						<Command.Shortcut>⌘↵</Command.Shortcut>
					</Command.Item>
					<Command.Item value="save-query" onSelect={saveQuery}>
						<Save class="size-4" />
						<span>{m.command_save_query()}</span>
						<Command.Shortcut>⌘S</Command.Shortcut>
					</Command.Item>
					<Command.Item value="explain-query" onSelect={explainQuery}>
						<GitBranch class="size-4" />
						<span>{m.command_explain_query()}</span>
					</Command.Item>
					<Command.Item value="explain-analyze-query" onSelect={explainAnalyzeQuery}>
						<GitBranch class="size-4" />
						<span>{m.command_explain_analyze_query()}</span>
					</Command.Item>
				{/if}
				<Command.Item value="toggle-ai" onSelect={toggleAI}>
					<Sparkles class="size-4" />
					<span>{m.command_toggle_ai()}</span>
				</Command.Item>
			</Command.Group>
		{/if}

		<!-- Navigation -->
		{#if openTabs.length > 0}
			<Command.Group heading={m.command_group_open_tabs()}>
				{#each openTabs as tab}
					{@const TabIcon = getTabIcon(tab.type)}
					<Command.Item value="tab-{tab.id}" onSelect={() => goToTab(tab.id, tab.type)}>
						<TabIcon class="size-4" />
						<span>{getTabName(tab)}</span>
						<span class="text-muted-foreground ms-auto text-xs">{tab.type}</span>
					</Command.Item>
				{/each}
			</Command.Group>
		{/if}

		<Command.Group heading={m.command_group_navigation()}>
			<Command.Item value="toggle-sidebar" onSelect={toggleSidebar}>
				<PanelLeft class="size-4" />
				<span>{m.command_toggle_sidebar()}</span>
				<Command.Shortcut>⌘B</Command.Shortcut>
			</Command.Item>
			{#if isConnected}
				<Command.Item value="view-erd" onSelect={viewErd}>
					<GitBranch class="size-4" />
					<span>{m.command_view_erd()}</span>
				</Command.Item>
			{/if}
		</Command.Group>

		<!-- Tables -->
		{#if isConnected && tables.length > 0}
			<Command.Group heading={m.command_group_tables()}>
				{#each tables.slice(0, 20) as table}
					<Command.Item value="open-table-{table.schema}-{table.name}" onSelect={() => openTable(table)}>
						<Table2 class="size-4" />
						<span>{m.command_open_table({ schema: table.schema, table: table.name })}</span>
					</Command.Item>
				{/each}
				{#each tables.slice(0, 20) as table}
					<Command.Item value="query-table-{table.schema}-{table.name}" onSelect={() => queryTable(table)}>
						<Play class="size-4" />
						<span>{m.command_query_table({ schema: table.schema, table: table.name })}</span>
					</Command.Item>
				{/each}
			</Command.Group>
		{/if}

		<!-- Connections -->
		{#if hasConnections}
			<Command.Group heading={m.command_group_connections()}>
				{#each connections as connection}
					<Command.Item value="connection-{connection.id}" onSelect={() => switchConnection(connection.id)}>
						<Database class="size-4" />
						<span>
							{#if connection.id === db.state.activeConnectionId}
								{connection.name}
							{:else if connection.database}
								{m.command_switch_to({ name: connection.name })}
							{:else}
								{m.command_connect_to({ name: connection.name })}
							{/if}
						</span>
						{#if connection.id === db.state.activeConnectionId}
							<span class="text-muted-foreground ms-auto text-xs">{m.command_status_active()}</span>
						{:else if !(connection.database || connection.mssqlConnectionId)}
							<span class="text-muted-foreground ms-auto text-xs">{m.command_status_disconnected()}</span>
						{/if}
					</Command.Item>
				{/each}
			</Command.Group>
		{/if}

		<!-- Saved Queries -->
		{#if isConnected && savedQueries && savedQueries.length > 0}
			<Command.Group heading={m.command_group_saved_queries()}>
				{#each savedQueries as query}
					<Command.Item value="saved-query-{query.id}" onSelect={() => loadSavedQuery(query.id)}>
						<FileText class="size-4" />
						<span>{query.name}</span>
					</Command.Item>
				{/each}
			</Command.Group>
		{/if}

		<!-- Query History -->
		{#if isConnected && recentHistory.length > 0}
			<Command.Group heading={m.command_group_recent_queries()}>
				{#each recentHistory as item}
					<Command.Item value="history-{item.id}" onSelect={() => loadHistoryItem(item.id)}>
						<History class="size-4" />
						<span class="truncate">{truncateQuery(item.query)}</span>
						<span class="text-muted-foreground ms-auto shrink-0 text-xs"
							>{item.executionTime}ms</span
						>
					</Command.Item>
				{/each}
			</Command.Group>
		{/if}

		<!-- Results Actions -->
		{#if hasResults}
			<Command.Group heading={m.command_group_results()}>
				<Command.Item value="export-csv" onSelect={() => exportResults("csv")}>
					<Download class="size-4" />
					<span>{m.command_export_csv()}</span>
				</Command.Item>
				<Command.Item value="export-json" onSelect={() => exportResults("json")}>
					<Download class="size-4" />
					<span>{m.command_export_json()}</span>
				</Command.Item>
				<Command.Item value="copy-results" onSelect={copyResults}>
					<Copy class="size-4" />
					<span>{m.command_copy_results_json()}</span>
				</Command.Item>
			</Command.Group>
		{/if}
	</Command.List>
</Command.Dialog>
