<script lang="ts">
	import * as Command from "$lib/components/ui/command";
	import { useDatabase } from "$lib/hooks/database.svelte";
	import { useShortcuts } from "$lib/shortcuts/shortcuts.svelte";
	import { EXECUTE_ACTIVE_QUERY_EVENT } from "$lib/components/query-editor/events.js";
	import { goto } from "$app/navigation";
	import { LESSONS, LESSON_SECTIONS } from "$lib/tutorial/lessons";
	import {
		Plus,
		Play,
		Save,
		Table2,
		Database,
		Loader,
		History,
		FileText,
		FileCode,
		Sparkles,
		PanelLeft,
		Download,
		Copy,
		GitBranch,
		Code,
		BarChart3,
		LayoutGrid,
		LayoutDashboard,
		BookOpen,
		GraduationCap,
		Keyboard,
		Activity,
		Network,
		Workflow,
		Cable,
		Rocket,
		Settings,
		PlusSquare,
		Puzzle,
	} from "@lucide/svelte";
	import { m } from "$lib/paraglide/messages.js";
	import { getKeySymbols } from "$lib/shortcuts/platform";

	const keys = getKeySymbols();
	import { Link } from "@lucide/svelte";
	import { handleDeepLink } from "$lib/services/deep-link";
	import { rowsToObjects } from "$lib/utils/row-access";
	import { errorToast } from "$lib/utils/toast";
	import { onboardingStore } from "$lib/stores/onboarding.svelte";
	import { aiSettingsStore } from "$lib/stores/ai-settings.svelte";

	const db = useDatabase();
	const shortcuts = useShortcuts();

	let open = $state(false);

	// Derived state for dynamic commands
	const tables = $derived(
		db.state.activeConnectionId ? db.state.schemas[db.state.activeConnectionId] ?? [] : []
	);
	const connections = $derived(db.state.connections);
	const savedQueries = $derived(db.state.projectQueries);
	const recentHistory = $derived(db.state.activeConnectionQueryHistory?.slice(0, 10) || []);
	const openTabs = $derived(db.tabs.ordered);
	const activeResult = $derived(db.state.activeQueryResult);
	const hasResults = $derived((activeResult?.rows?.length ?? 0) > 0);
	const isConnected = $derived(!!db.state.activeConnectionId && !!(db.state.activeConnection?.providerConnectionId));
	const hasActiveQueryTab = $derived(isConnected && !!db.state.activeQueryTab);
	const hasQueryContent = $derived(hasActiveQueryTab && !!db.state.activeQueryTab?.query?.trim());
	const dashboards = $derived(db.state.projectDashboards);
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
		// Goes through the query editor so destructive statements and parameters are confirmed first.
		runAndClose(() => window.dispatchEvent(new CustomEvent(EXECUTE_ACTIVE_QUERY_EVENT)));
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

	const tabManagerByType = {
		query: db.queryTabs,
		schema: db.schemaTabs,
		explain: db.explainTabs,
		erd: db.erdTabs,
		statistics: db.statisticsTabs,
		workflow: db.workflowTabs,
		visualize: db.visualizeTabs,
		connection: db.connectionTabs,
		dashboard: db.dashboardTabs,
		starter: db.starterTabs,
		settings: db.settingsTabs,
		createTable: db.createTableTabs,
		data: db.dataTabs,
		extensionsDuckdb: db.extensionsDuckdbTabs,
	} as const;

	type TabType = keyof typeof tabManagerByType;

	function goToTab(tabId: string, type: import('$lib/types').ActiveViewType) {
		runAndClose(() => {
			tabManagerByType[type].setActive(tabId);
			db.ui.setActiveView(type);
		});
	}

	function showKeyboardShortcuts() {
		runAndClose(() => {
			shortcuts.showHelp = true;
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

	function viewWorkflow() {
		runAndClose(() => db.workflowTabs.add());
	}

	async function newDashboard() {
		runAndClose(async () => {
			const dashboard = await db.dashboards.createDashboard("New Dashboard");
			if (dashboard) {
				db.dashboardTabs.add(dashboard.id, dashboard.name);
			}
		});
	}

	function openDashboard(dashboardId: string, name: string) {
		runAndClose(() => db.dashboardTabs.add(dashboardId, name));
	}

	function goToLearn(path: string) {
		runAndClose(() => goto(path));
	}

	async function switchConnection(id: string) {
		const connection = connections.find((c) => c.id === id);
		if (!connection) return;

		if (connection.providerConnectionId) {
			// Already connected, just switch to it
			runAndClose(() => db.connections.setActive(id));
		} else {
			// Try auto-reconnect first if password is saved
			open = false;
			const autoReconnected = await db.connections.autoReconnect(id);
			if (autoReconnected) {
				return; // Successfully reconnected
			}

			// Fall back to dialog if auto-reconnect fails or password not saved
			void db.connectionTabs.open({
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
				savePassword: connection.savePassword,
				saveSshPassword: connection.saveSshPassword,
				saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
			});
		}
	}

	function loadSavedQuery(id: string) {
		runAndClose(() => db.queryTabs.loadQuery(id));
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
				.map((row: unknown[]) =>
					row
						.map((val: unknown) => {
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
			// JSON export needs `{col: value}` objects — materialize from
			// columnar storage on demand (user-initiated, so fine).
			content = JSON.stringify(
				rowsToObjects(activeResult.rows, activeResult.columns),
				null,
				2,
			);
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

		const content = JSON.stringify(
			rowsToObjects(activeResult.rows, activeResult.columns),
			null,
			2,
		);
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

	function getTabName(tab: { type: TabType; tab: any }): string {
		switch (tab.type) {
			case "query":
				return tab.tab.name || "Query";
			case "schema":
				return tab.tab.table?.name || "Schema";
			case "explain":
				return tab.tab.name || "Explain";
			case "erd":
				return tab.tab.name || "ERD";
			case "statistics":
				return tab.tab.name || "Statistics";
			case "dashboard":
				return tab.tab.name || "Dashboard";
			case "workflow":
				return tab.tab.name || "Workflow";
			case "visualize":
				return tab.tab.name || "Visualize";
			case "connection":
				return tab.tab.name || "Connection";
			case "starter":
				return "Get Started";
			case "settings":
				return "Settings";
			case "createTable":
				return tab.tab.name || "Create Table";
			case "data":
				return tab.tab.name || "Data";
			case "extensionsDuckdb":
				return "Extensions";
			default: {
				const _exhaustive: never = tab.type;
				return _exhaustive;
			}
		}
	}

	function getTabIcon(type: TabType) {
		switch (type) {
			case "query":
				return FileCode;
			case "schema":
				return Table2;
			case "explain":
				return Activity;
			case "erd":
				return Network;
			case "statistics":
				return BarChart3;
			case "workflow":
				return Workflow;
			case "visualize":
				return GitBranch;
			case "connection":
				return Cable;
			case "dashboard":
				return LayoutDashboard;
			case "starter":
				return Rocket;
			case "settings":
				return Settings;
			case "createTable":
				return PlusSquare;
			case "data":
				return Database;
			case "extensionsDuckdb":
				return Puzzle;
			default: {
				const _exhaustive: never = type;
				return FileText;
			}
		}
	}

	async function openDeepLink() {
		open = false;
		try {
			const text = await navigator.clipboard.readText();
			if (text.startsWith("seaquel://")) {
				handleDeepLink(text, db);
			} else {
				errorToast("Clipboard does not contain a seaquel:// link");
			}
		} catch {
			errorToast("Failed to read clipboard");
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
						<Command.Shortcut>{keys.mod}T</Command.Shortcut>
					</Command.Item>
					<Command.Item value="new-dashboard" onSelect={newDashboard}>
						<LayoutDashboard class="size-4" />
						<span>{m.command_new_dashboard()}</span>
					</Command.Item>
				{/if}
				{#if hasQueryContent}
					<Command.Item value="execute-query" onSelect={executeQuery}>
						<Play class="size-4" />
						<span>{m.command_execute_query()}</span>
						<Command.Shortcut>{keys.mod}{keys.enter}</Command.Shortcut>
					</Command.Item>
					<Command.Item value="save-query" onSelect={saveQuery}>
						<Save class="size-4" />
						<span>{m.command_save_query()}</span>
						<Command.Shortcut>{keys.mod}S</Command.Shortcut>
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
				{#if aiSettingsStore.settings.enabled}
				<Command.Item value="toggle-ai" onSelect={toggleAI}>
					<Sparkles class="size-4" />
					<span>{m.command_toggle_ai()}</span>
				</Command.Item>
			{/if}
			</Command.Group>
		{/if}

		<!-- Navigation -->
		{#if openTabs.length > 0}
			<Command.Group heading={m.command_group_open_tabs()}>
				{#each openTabs as tab, i}
					{@const TabIcon = getTabIcon(tab.type)}
					<Command.Item value="tab-{tab.id}" onSelect={() => goToTab(tab.id, tab.type)}>
						<TabIcon class="size-4" />
						<span>{getTabName(tab)}</span>
						{#if i < 9}
							<Command.Shortcut>{keys.mod}{i + 1}</Command.Shortcut>
						{/if}
					</Command.Item>
				{/each}
			</Command.Group>
		{/if}

		<Command.Group heading={m.command_group_navigation()}>
			<Command.Item value="toggle-sidebar" onSelect={toggleSidebar}>
				<PanelLeft class="size-4" />
				<span>{m.command_toggle_sidebar()}</span>
				<Command.Shortcut>{keys.mod}B</Command.Shortcut>
			</Command.Item>
			<Command.Item value="keyboard-shortcuts" onSelect={showKeyboardShortcuts}>
				<Keyboard class="size-4" />
				<span>{m.command_keyboard_shortcuts()}</span>
				<Command.Shortcut>?</Command.Shortcut>
			</Command.Item>
			{#if isConnected}
				<Command.Item value="view-erd" onSelect={viewErd}>
					<GitBranch class="size-4" />
					<span>{m.command_view_erd()}</span>
				</Command.Item>
				<Command.Item value="view-workflow" onSelect={viewWorkflow}>
					<LayoutGrid class="size-4" />
					<span>{m.command_open_workflow()}</span>
				</Command.Item>
			{/if}
		</Command.Group>

		<!-- Learning -->
		{#if onboardingStore.learnEnabled}
			<Command.Group heading={m.command_group_learning()}>
				<Command.Item value="learn-sandbox" onSelect={() => goToLearn('/learn/sandbox')}>
					<BookOpen class="size-4" />
					<span>{m.command_learn_sandbox()}</span>
				</Command.Item>
				{#each LESSON_SECTIONS as section}
					{#each section.lessons as lessonId}
						{@const lesson = LESSONS[lessonId]}
						{#if lesson}
							<Command.Item value="learn-{lessonId}" onSelect={() => goToLearn(`/learn/${lessonId}`)}>
								<GraduationCap class="size-4" />
								<span>{lesson.title}</span>
								<span class="text-muted-foreground ms-auto text-xs">{section.title}</span>
							</Command.Item>
						{/if}
					{/each}
				{/each}
			</Command.Group>
		{/if}

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

		<!-- Dashboards -->
		{#if isConnected && dashboards.length > 0}
			<Command.Group heading={m.command_group_dashboards()}>
				{#each dashboards as dashboard}
					<Command.Item value="open-dashboard-{dashboard.id}" onSelect={() => openDashboard(dashboard.id, dashboard.name)}>
						<LayoutDashboard class="size-4" />
						<span>{m.command_open_dashboard({ name: dashboard.name })}</span>
					</Command.Item>
				{/each}
			</Command.Group>
		{/if}

		<!-- Connections -->
		{#if hasConnections}
			<Command.Group heading={m.command_group_connections()}>
				{#each connections as connection}
					<Command.Item value="connection-{connection.id}" onSelect={() => switchConnection(connection.id)}>
						{#if db.connections.connectingIds.has(connection.id)}
							<Loader class="size-4 animate-spin" />
						{:else}
							<Database class="size-4" />
						{/if}
						<span>
							{#if connection.id === db.state.activeConnectionId}
								{connection.name}
							{:else if connection.providerConnectionId}
								{m.command_switch_to({ name: connection.name })}
							{:else}
								{m.command_connect_to({ name: connection.name })}
							{/if}
						</span>
						{#if connection.id === db.state.activeConnectionId}
							<span class="text-muted-foreground ms-auto text-xs">{m.command_status_active()}</span>
						{:else if !(connection.providerConnectionId)}
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

		<!-- Dev Tools (only visible during development) -->
		{#if import.meta.env.DEV}
			<Command.Group heading="Dev Tools">
				<Command.Item value="open-deep-link" onSelect={openDeepLink}>
					<Link class="size-4" />
					<span>Open Deep Link (from clipboard)</span>
				</Command.Item>
			</Command.Group>
		{/if}
	</Command.List>
</Command.Dialog>
