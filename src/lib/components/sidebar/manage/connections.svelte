<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { showErrorUnlessShown } from "$lib/errors";
	import { dndzone } from "svelte-dnd-action";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { Button } from "$lib/components/ui/button";
	import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import PlugIcon from "@lucide/svelte/icons/plug";
	import UnplugIcon from "@lucide/svelte/icons/unplug";
	import TagIcon from "@lucide/svelte/icons/tag";
	import BarChart3Icon from "@lucide/svelte/icons/bar-chart-3";
	import NetworkIcon from "@lucide/svelte/icons/network";
	import WorkflowIcon from "@lucide/svelte/icons/workflow";
	import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";
	import GitBranchIcon from "@lucide/svelte/icons/git-branch";
	import PencilIcon from "@lucide/svelte/icons/pencil";
	import LoaderIcon from "@lucide/svelte/icons/loader";
	import LayoutDashboardIcon from "@lucide/svelte/icons/layout-dashboard";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import PuzzleIcon from "@lucide/svelte/icons/puzzle";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import DeleteConfirmDialog from "$lib/components/delete-confirm-dialog.svelte";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import { m } from "$lib/paraglide/messages.js";
	import { isDemo, getFeatures } from "$lib/features";
	import ConnectionLabelPicker from "$lib/components/connection-label-picker.svelte";
	import LinkIcon from "@lucide/svelte/icons/link";

	interface Props {
		oncopyShareLink: (resource: { repoId?: string; filePath?: string; name?: string; folder?: string; id?: string }, resourceType: "connection") => Promise<void>;
	}

	let { oncopyShareLink }: Props = $props();

	const db = useDatabase();
	const features = getFeatures();

	let connectionsExpanded = $state(true);

	// Remove connection confirmation dialog state
	let showRemoveDialog = $state(false);
	let connectionToRemove = $state<string | null>(null);
	let connectionToRemoveName = $state("");

	// Labels dialog state
	let showLabelsDialog = $state(false);
	let connectionToEditLabels = $state<string | null>(null);
	let connectionToEditLabelsName = $state("");

	// Drag & drop reordering state
	type ProjectConnection = (typeof db.state.projectConnections)[number];
	let isDragging = $state(false);
	let draggedConnections = $state<ProjectConnection[]>([]);
	const displayConnections = $derived(
		isDragging ? draggedConnections : db.state.projectConnections,
	);

	function handleDndConsider(e: CustomEvent<{ items: ProjectConnection[] }>) {
		isDragging = true;
		draggedConnections = e.detail.items;
	}

	function handleDndFinalize(e: CustomEvent<{ items: ProjectConnection[] }>) {
		isDragging = false;
		draggedConnections = [];
		const projectId = db.state.activeProjectId;
		if (!projectId) return;
		db.connections.reorder(
			projectId,
			e.detail.items.map((c) => c.id),
		);
	}

	const handleConnectionClick = async (connection: typeof db.state.connections[0]) => {
		if (connection.providerConnectionId) {
			db.connections.setActive(connection.id);
		} else {
			const autoReconnected = await db.connections.autoReconnect(connection.id);
			if (autoReconnected) {
				return;
			}
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
	};

	const confirmRemoveConnection = (connectionId: string, name: string) => {
		connectionToRemove = connectionId;
		connectionToRemoveName = name;
		showRemoveDialog = true;
	};

	const handleRemoveConnection = async () => {
		const id = connectionToRemove;
		connectionToRemove = null;
		connectionToRemoveName = "";
		showRemoveDialog = false;
		if (!id) return;
		try {
			await db.connections.remove(id);
		} catch (error) {
			showErrorUnlessShown(error);
		}
	};

	const getConnectionLabels = (connection: typeof db.state.connections[0]) => {
		return db.labels.getConnectionLabelsById(connection.id);
	};

	const openLabelsDialog = (connectionId: string, name: string) => {
		connectionToEditLabels = connectionId;
		connectionToEditLabelsName = name;
		showLabelsDialog = true;
	};
</script>

<Sidebar.Group class="py-2">
	<Collapsible bind:open={connectionsExpanded}>
		<div class="flex items-center justify-between px-3 py-1">
			<CollapsibleTrigger class="flex items-center gap-1 text-xs font-medium text-sidebar-foreground/70 hover:text-sidebar-foreground">
				<ChevronRightIcon class={["size-3 transition-transform", connectionsExpanded && "rotate-90"]} />
				{m.sidebar_connections()}
			</CollapsibleTrigger>
			{#if features.newConnections}
				<Button
					size="icon"
					variant="ghost"
					class="size-5 [&_svg:not([class*='size-'])]:size-3"
					onclick={() => void db.connectionTabs.open()}
					title={m.sidebar_connections_add()}
				>
					<PlusIcon />
				</Button>
			{/if}
		</div>
		<CollapsibleContent>
			<Sidebar.GroupContent>
				<Sidebar.Menu class="px-2">
					<div
						use:dndzone={{
							items: displayConnections,
							type: 'connections',
							dropTargetStyle: {},
							flipDurationMs: 150,
						}}
						onconsider={handleDndConsider}
						onfinalize={handleDndFinalize}
					>
					{#each displayConnections as connection (connection.id)}
						<div id={connection.id}>
						<ContextMenu.Root>
							<ContextMenu.Trigger class="w-full">
								<Sidebar.MenuItem>
									<Sidebar.MenuButton
										class={[
											"flex items-center gap-2 cursor-pointer",
											db.state.activeConnectionId === connection.id && "bg-sidebar-accent"
										]}
										onclick={() => handleConnectionClick(connection)}
									>
											<span class="flex size-2 items-center justify-center shrink-0">
											{#if db.connections.connectingIds.has(connection.id)}
												<LoaderIcon class="size-3 animate-spin text-muted-foreground" />
											{:else}
												<span
													class={[
														"size-2 rounded-full",
														(connection.providerConnectionId) ? "bg-green-500" : "bg-gray-400"
													]}
												></span>
											{/if}
										</span>
										<span class="flex-1 truncate text-sm">{connection.name}</span>
										{#if getConnectionLabels(connection).length > 0}
											<Tooltip.Root>
												<Tooltip.Trigger
													class="flex items-center cursor-pointer"
													onclick={(e: MouseEvent) => { e.stopPropagation(); openLabelsDialog(connection.id, connection.name); }}
												>
													{#each getConnectionLabels(connection) as label, i (label.id)}
														<span
															class="size-2.5 rounded-full shrink-0 ring-1 ring-sidebar-background"
															style="background-color: {label.color}; {i > 0 ? 'margin-left: -4px;' : ''}"
														></span>
													{/each}
												</Tooltip.Trigger>
												<Tooltip.Content side="right">
													<div class="flex flex-col gap-1">
														{#each getConnectionLabels(connection) as label (label.id)}
															<div class="flex items-center gap-1.5 text-xs">
																<span
																	class="size-2 rounded-full"
																	style="background-color: {label.color};"
																></span>
																{label.name}
															</div>
														{/each}
													</div>
												</Tooltip.Content>
											</Tooltip.Root>
										{/if}
										{#if db.state.activeProjectHasGit}
											<Tooltip.Root>
											<Tooltip.Trigger>
												{#snippet child({ props })}
													<button
														{...props}
														type="button"
														class="shrink-0 cursor-pointer"
														onclick={(e) => { e.stopPropagation(); void db.connections.toggleLocalOnly(connection.id).catch(showErrorUnlessShown); }}
													>
														<GitBranchIcon class={["size-3!", connection.isLocalOnly ? "text-muted-foreground/40" : "text-green-500"]} />
													</button>
												{/snippet}
											</Tooltip.Trigger>
											<Tooltip.Content>
												{connection.isLocalOnly ? m.connection_share() : m.connection_mark_local_only()}
											</Tooltip.Content>
										</Tooltip.Root>
										{/if}
									</Sidebar.MenuButton>
								</Sidebar.MenuItem>
							</ContextMenu.Trigger>
							<ContextMenu.Content class="w-48">
								{#if connection.providerConnectionId}
									<ContextMenu.Item onclick={() => db.connections.toggle(connection.id)}>
										<UnplugIcon class="size-4 me-2" />
										{m.sidebar_connection_disconnect()}
									</ContextMenu.Item>
									<ContextMenu.Separator />
									<ContextMenu.Item onclick={() => {
										db.connections.setActive(connection.id);
										db.statisticsTabs.add();
									}}>
										<BarChart3Icon class="size-4 me-2" />
										{m.sidebar_database_statistics()}
									</ContextMenu.Item>
									{#if connection.type === "duckdb"}
										<ContextMenu.Item onclick={() => {
											db.connections.setActive(connection.id);
											db.extensionsDuckdbTabs.add();
										}}>
											<PuzzleIcon class="size-4 me-2" />
											{m.ext_sidebar_extensions()}
										</ContextMenu.Item>
									{/if}
									<ContextMenu.Item onclick={() => {
										db.connections.setActive(connection.id);
										db.erdTabs.add();
									}}>
										<NetworkIcon class="size-4 me-2" />
										{m.sidebar_erd_diagram()}
									</ContextMenu.Item>
									<ContextMenu.Item onclick={() => {
										db.connections.setActive(connection.id);
										db.workflowTabs.add();
									}}>
										<WorkflowIcon class="size-4 me-2" />
										{m.sidebar_workflows()}
									</ContextMenu.Item>
									<ContextMenu.Item onclick={async () => {
										db.connections.setActive(connection.id);
										const dashboard = await db.dashboards.createDashboard("New Dashboard");
										if (dashboard) {
											db.dashboardTabs.add(dashboard.id, dashboard.name);
										}
									}}>
										<LayoutDashboardIcon class="size-4 me-2" />
										Dashboards
									</ContextMenu.Item>
								{:else}
									<ContextMenu.Item onclick={() => handleConnectionClick(connection)}>
										<PlugIcon class="size-4 me-2" />
										{m.sidebar_connection_connect()}
									</ContextMenu.Item>
								{/if}
								<ContextMenu.Separator />
								<ContextMenu.Item onclick={() => void db.connectionTabs.open(connection, "edit")}>
									<PencilIcon class="size-4 me-2" />
									{m.sidebar_connection_edit()}
								</ContextMenu.Item>
								<ContextMenu.Item onclick={() => openLabelsDialog(connection.id, connection.name)}>
									<TagIcon class="size-4 me-2" />
									{m.sidebar_connection_labels()}
								</ContextMenu.Item>
								{#if connection.sharedConnectionId}
									{@const sharedConn = db.state.allSharedConnections.find((c) => c.id === connection.sharedConnectionId)}
									{#if sharedConn}
										<ContextMenu.Item onclick={() => oncopyShareLink(sharedConn, "connection")}>
											<LinkIcon class="size-4 me-2" />
											{m.share_connection()}
										</ContextMenu.Item>
									{/if}
								{/if}
								{#if !(isDemo() && connection.id === "demo-connection")}
									<ContextMenu.Separator />
									<ContextMenu.Item
										class="text-destructive focus:text-destructive"
										onclick={() => confirmRemoveConnection(connection.id, connection.name)}
									>
										<Trash2Icon class="size-4 me-2" />
										{m.sidebar_connection_delete()}
									</ContextMenu.Item>
								{/if}
							</ContextMenu.Content>
						</ContextMenu.Root>
						</div>
					{/each}
					</div>
					{#if db.state.projectConnections.length === 0}
						<div class="text-center py-2 text-muted-foreground">
							<p class="text-xs">{m.sidebar_no_connection()}</p>
						</div>
					{/if}
				</Sidebar.Menu>
			</Sidebar.GroupContent>
		</CollapsibleContent>
	</Collapsible>
</Sidebar.Group>

<!-- Delete Connection Dialog -->
<DeleteConfirmDialog
	bind:open={showRemoveDialog}
	title={m.header_delete_dialog_title()}
	description={m.header_delete_dialog_description({ name: connectionToRemoveName })}
	cancelText={m.header_button_cancel()}
	confirmText={m.header_button_remove()}
	onconfirm={handleRemoveConnection}
/>

<!-- Labels Dialog -->
<Dialog.Root bind:open={showLabelsDialog}>
	<Dialog.Content class="max-w-md">
		<Dialog.Header>
			<Dialog.Title>{m.labels_dialog_title({ name: connectionToEditLabelsName })}</Dialog.Title>
		</Dialog.Header>
		<div class="py-4">
			{#if connectionToEditLabels}
				<ConnectionLabelPicker connectionId={connectionToEditLabels} />
			{/if}
		</div>
		<Dialog.Footer>
			<Button onclick={() => showLabelsDialog = false}>
				{m.labels_done()}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
