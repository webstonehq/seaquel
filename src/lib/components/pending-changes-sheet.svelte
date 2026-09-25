<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import type { PendingChange, PendingChangeViewMode } from "$lib/types";
	import { Button } from "$lib/components/ui/button";
	import { Badge } from "$lib/components/ui/badge";
	import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
	import PlusCircleIcon from "@lucide/svelte/icons/plus-circle";
	import PencilIcon from "@lucide/svelte/icons/pencil";
	import TrashIcon from "@lucide/svelte/icons/trash-2";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import PlayIcon from "@lucide/svelte/icons/play";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import { cellText } from "$lib/values";

	const db = useDatabase();

	let viewMode = $state<PendingChangeViewMode>("visual");
	let isExecuting = $state(false);
	let showConfirmDialog = $state(false);

	const changes = $derived(db.state.activePendingChanges);
	const connectionName = $derived(db.state.activeConnection?.name ?? "Unknown");

	function getIcon(change: PendingChange) {
		switch (change.queryType) {
			case "insert":
				return PlusCircleIcon;
			case "update":
				return PencilIcon;
			case "delete":
				return TrashIcon;
			default:
				return DatabaseIcon;
		}
	}

	function getOriginLabel(change: PendingChange): string {
		switch (change.origin) {
			case "query-editor":
				return "Query Editor";
			case "inline-edit":
				return "Inline Edit";
			case "insert-row":
				return "Insert Row";
			case "delete-row":
				return "Delete Row";
			case "set-default":
				return "Set Default";
			case "create-table":
				return "Create Table";
			case "alter-table":
				return "Alter Table";
			case "drop-table":
				return "Drop Table";
			case "truncate-table":
				return "Truncate";
			default:
				return "Query";
		}
	}

	function formatTimeAgo(date: Date): string {
		const seconds = Math.floor((Date.now() - date.getTime()) / 1000);
		if (seconds < 60) return "just now";
		const minutes = Math.floor(seconds / 60);
		if (minutes < 60) return `${minutes}m ago`;
		const hours = Math.floor(minutes / 60);
		return `${hours}h ago`;
	}

	function formatSqlWithValues(sql: string, bindValues?: unknown[]): string {
		if (!bindValues?.length) return sql;
		return sql.replace(/\$(\d+)/g, (match, index) => {
			const i = parseInt(index, 10) - 1;
			if (i < 0 || i >= bindValues.length) return match;
			const val = bindValues[i];
			if (val === null || val === undefined) return "NULL";
			if (typeof val === "string") return `'${val}'`;
			return cellText(val);
		});
	}

	function truncateSql(sql: string, maxLength = 120, bindValues?: unknown[]): string {
		const resolved = formatSqlWithValues(sql, bindValues);
		const oneLine = resolved.replace(/\s+/g, " ").trim();
		if (oneLine.length <= maxLength) return oneLine;
		return oneLine.slice(0, maxLength) + "…";
	}

	function handleRemove(changeId: string) {
		const connectionId = db.state.activeConnectionId;
		if (connectionId) {
			db.pendingChanges.remove(connectionId, changeId);
		}
	}

	function handleClear() {
		const connectionId = db.state.activeConnectionId;
		if (connectionId) {
			db.pendingChanges.clear(connectionId);
		}
	}

	async function handleExecuteAll() {
		showConfirmDialog = false;
		const connectionId = db.state.activeConnectionId;
		if (!connectionId) return;

		isExecuting = true;
		try {
			const result = await db.pendingChanges.executeAll(connectionId);
			if (result.failed > 0) {
				errorToast(`Failed at statement ${(result.failedAt ?? 0) + 1}: ${result.error}`);
				if (result.executed > 0) {
					toast.info(`${result.executed} statement${result.executed > 1 ? "s" : ""} executed before failure`);
				}
			} else {
				toast.success(`${result.executed} statement${result.executed > 1 ? "s" : ""} executed successfully`);
			}
			if (result.hasDdl) {
				await db.connections.refreshSchema(connectionId);
			}
			if (result.hasDdl || result.executed > 0) {
				await db.dataTabs.refreshAllForConnection(connectionId);
			}
			db.pendingChanges.clear(connectionId);
			db.pendingChanges.closeSheet();
		} catch (error) {
			db.pendingChanges.closeSheet();
			errorToast(error instanceof Error ? error.message : String(error));
		} finally {
			isExecuting = false;
		}
	}
</script>

<div class="flex flex-col h-full">
	<!-- Header -->
	<div class="flex items-center justify-between gap-2 border-b px-4 py-3">
		<div class="flex items-center gap-2">
			<p class="text-sm font-semibold">Pending Changes</p>
			{#if changes.length > 0}
				<Badge variant="secondary">{changes.length}</Badge>
			{/if}
		</div>
		<Button size="icon" variant="ghost" class="size-6 [&_svg:not([class*='size-'])]:size-4" aria-label="Close" onclick={() => db.pendingChanges.toggleSheet()}>
			<ChevronRightIcon />
		</Button>
	</div>

	<!-- Sub-header: connection + view toggle -->
	<div class="flex items-center gap-2 px-4 py-2 border-b">
		<span class="text-xs text-muted-foreground">Connection: {connectionName}</span>
		<div class="ml-auto flex items-center gap-1">
			<Button
				size="sm"
				variant={viewMode === "visual" ? "secondary" : "ghost"}
				class="h-6 px-2 text-xs"
				onclick={() => (viewMode = "visual")}
			>
				Visual
			</Button>
			<Button
				size="sm"
				variant={viewMode === "sql" ? "secondary" : "ghost"}
				class="h-6 px-2 text-xs"
				onclick={() => (viewMode = "sql")}
			>
				SQL
			</Button>
		</div>
	</div>

	<!-- Content -->
	<div class="flex-1 min-h-0 overflow-y-auto px-4 py-2">
		{#if changes.length === 0}
			<div class="flex flex-col items-center justify-center h-full text-muted-foreground">
				<DatabaseIcon class="size-8 mb-2 opacity-50" />
				<p class="text-sm">No pending changes</p>
				<p class="text-xs mt-1">Write and DDL queries will appear here for review</p>
			</div>
		{:else}
			<div class="space-y-2">
				{#each changes as change (change.id)}
					{@const Icon = getIcon(change)}
					<div class="group relative rounded-md border px-3 py-2.5 text-sm">
						<div class="flex items-start gap-2">
							<Icon class="size-4 mt-0.5 shrink-0 text-muted-foreground" />
							<div class="flex-1 min-w-0">
								<div class="flex items-center gap-2">
									<Badge variant="outline" class="text-[10px] px-1.5 py-0 shrink-0">
										{getOriginLabel(change)}
									</Badge>
									<span class="text-[10px] text-muted-foreground">{formatTimeAgo(change.addedAt)}</span>
								</div>
								{#if viewMode === "visual"}
									<p class="mt-1 text-sm">{change.description}</p>
								{:else}
									<code class="mt-1 block text-xs text-muted-foreground break-all whitespace-pre-wrap">
										{truncateSql(change.sql, 200, change.bindValues)}
									</code>
								{/if}
							</div>
							<button
								class="shrink-0 opacity-0 group-hover:opacity-100 transition-opacity p-0.5 rounded-sm hover:bg-muted"
								onclick={() => handleRemove(change.id)}
								title="Remove"
							>
								<Trash2Icon class="size-3.5 text-muted-foreground" />
							</button>
						</div>
					</div>
				{/each}
			</div>
		{/if}
	</div>

	<!-- Footer -->
	{#if changes.length > 0}
		<div class="shrink-0 flex gap-2 px-4 py-3 border-t">
			<Button
				variant="ghost"
				size="sm"
				onclick={handleClear}
				disabled={isExecuting}
			>
				<Trash2Icon class="size-3.5 mr-1.5" />
				Clear All
			</Button>
			<div class="flex-1"></div>
			<Button
				size="sm"
				onclick={() => (showConfirmDialog = true)}
				disabled={isExecuting}
			>
				<PlayIcon class="size-3.5 mr-1.5" />
				{isExecuting ? "Executing…" : "Execute All"}
			</Button>
		</div>
	{/if}
</div>

<AlertDialog.Root bind:open={showConfirmDialog}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Execute {changes.length} pending change{changes.length !== 1 ? "s" : ""}?</AlertDialog.Title>
			<AlertDialog.Description>
				These statements will be executed sequentially on {connectionName}. This action cannot be undone.
			</AlertDialog.Description>
		</AlertDialog.Header>
		<div class="flex max-h-48 flex-col gap-2 overflow-y-auto py-2">
			{#each changes as change (change.id)}
				{@const Icon2 = getIcon(change)}
				<div class="bg-muted rounded-md px-3 py-2 text-sm">
					<div class="flex items-center gap-2">
						<Icon2 class="size-3.5 text-muted-foreground" />
						<span class="font-medium text-xs">{change.description}</span>
					</div>
					<code class="text-muted-foreground mt-1 block overflow-x-auto whitespace-nowrap scrollbar-hide text-xs">{formatSqlWithValues(change.sql, change.bindValues)}</code>
				</div>
			{/each}
		</div>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action onclick={handleExecuteAll}>
				Execute All
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
