<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { Badge } from "$lib/components/ui/badge";
	import { m } from "$lib/paraglide/messages.js";
	import { isDemo } from "$lib/features";
	import { buildDeepLinkUrl } from "$lib/services/deep-link";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import { Connections, TabNav, SchemaTab, QueriesTab, DashboardsTab } from "./sidebar/manage/index.js";
	import { templatePath, type ShareResource } from "./sidebar/manage/share-link.js";

	interface Props {
		version?: string;
	}

	let { version = "" }: Props = $props();

	const db = useDatabase();

	const hasActiveConnection = $derived(
		!!db.state.activeConnectionId && !!db.state.activeConnection &&
		!!(db.state.activeConnection.providerConnectionId)
	);

	let sidebarTab = $state<"schema" | "queries" | "dashboards">(db.state.activeConnectionId ? "schema" : "queries");

	// When connection becomes inactive, switch away from schema tab
	$effect(() => {
		if (!hasActiveConnection && sidebarTab === "schema") {
			sidebarTab = "queries";
		}
	});

	/**
	 * A deep link to a shared row's file. The path is Core's: a query's or
	 * dashboard's stored `sharedPath`, a connection's template path inside
	 * its `sharedConnectionId` (`<repoId>:<path>`). The repo is the active
	 * project's own (Decision 42), never another project's.
	 */
	const copyShareLink = async (resource: ShareResource, resourceType: "query" | "dashboard" | "connection" = "query") => {
		const repo = db.sharedRepos.repoForProject(db.state.activeProjectId);
		if (!repo) {
			errorToast(m.share_link_no_repo());
			return;
		}
		if (!repo.remoteUrl) {
			errorToast(m.share_link_no_remote());
			return;
		}
		const filePath = resourceType === "connection" ? templatePath(resource.sharedConnectionId) : resource.sharedPath;
		if (!filePath) {
			errorToast(m.share_link_not_written());
			return;
		}
		const url = buildDeepLinkUrl(repo.remoteUrl, repo.branch, filePath);
		await navigator.clipboard.writeText(url);
		toast.success(m.share_link_copied());
	};
</script>

<!-- Header: Connections section -->
<Sidebar.Header class="p-0 py-1">
	<Connections oncopyShareLink={copyShareLink} />

	<!-- Schema/Queries/Dashboards tabs - show when project has connections -->
	{#if db.state.activeProjectId && db.state.projectConnections.length > 0}
		<TabNav bind:value={sidebarTab} {hasActiveConnection} />
	{/if}
</Sidebar.Header>

<!-- Content -->
<Sidebar.Content>
	{#if db.state.activeConnectionId && db.state.activeConnection && (db.state.activeConnection.providerConnectionId)}
		<!-- Schema Tab Panel (requires active connection) -->
		<div
			class={["flex flex-col", sidebarTab !== "schema" && "hidden"]}
			aria-hidden={sidebarTab !== "schema"}
			inert={sidebarTab !== "schema" ? true : undefined}
		>
			<SchemaTab />
		</div>
	{/if}

	{#if db.state.activeProjectId && db.state.projectConnections.length > 0}
		<!-- Queries Tab Panel -->
		<div
			class={["flex flex-col", sidebarTab !== "queries" && "hidden"]}
			aria-hidden={sidebarTab !== "queries"}
			inert={sidebarTab !== "queries" ? true : undefined}
		>
			<QueriesTab oncopyShareLink={copyShareLink} />
		</div>

		<!-- Dashboards Tab Panel -->
		<div
			class={["flex flex-col", sidebarTab !== "dashboards" && "hidden"]}
			aria-hidden={sidebarTab !== "dashboards"}
			inert={sidebarTab !== "dashboards" ? true : undefined}
		>
			<DashboardsTab oncopyShareLink={copyShareLink} />
		</div>
	{/if}
</Sidebar.Content>

<!-- Footer -->
<Sidebar.Footer class="p-4">
	<div class="text-xs text-muted-foreground flex justify-between">
		<span>
			{#if sidebarTab === "schema" && db.state.activeConnection}
				{m.sidebar_tables_count({ count: db.state.activeSchema.length })}
			{:else if sidebarTab === "dashboards"}
				{@const total = db.state.projectDashboards.length}
				{total} dashboard{total !== 1 ? 's' : ''}
			{:else if sidebarTab === "queries"}
				{m.sidebar_queries_stats({ executed: db.state.activeConnectionQueryHistory.length, saved: db.state.projectQueries.length })}
			{:else}
				{m.sidebar_no_connection_footer()}
			{/if}
		</span>
		{#if isDemo()}
			<Badge variant="secondary" class="text-xs">Demo</Badge>
		{:else if version}
			<span>v{version}</span>
		{/if}
	</div>
</Sidebar.Footer>
