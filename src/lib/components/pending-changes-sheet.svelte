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
	import { confirmedFor, listDestructive } from "$lib/hooks/database/pending-changes.svelte.js";
	import { m } from "$lib/paraglide/messages.js";
	import type { DestructiveStatement } from "$lib/types/generated/DestructiveStatement";

	const db = useDatabase();

	let viewMode = $state<PendingChangeViewMode>("visual");
	let isExecuting = $state(false);
	let showConfirmDialog = $state(false);
	/** The change the last Execute All stopped at, and why; it stays pending. */
	let failure = $state<{ change: PendingChange; error: string } | null>(null);
	/**
	 * The destructive statements Core asked to confirm (the dialog's own list
	 * missed them): the dialog shows these, and Execute All confirms them.
	 */
	let coreDestructive = $state<{ statements: DestructiveStatement[]; total: number } | null>(null);

	const changes = $derived(db.state.activePendingChanges);
	/**
	 * The failure to show, while its change is still pending as it was:
	 * editing the change replaces its object and removing it drops it, and
	 * either makes the error stale.
	 */
	const shownFailure = $derived(
		failure && changes.includes(failure.change) ? failure : null,
	);
	// The queue shown is the focused data tab's or result's connection's
	// (`pendingConnectionId`), which may not be the active one.
	const connectionName = $derived(
		db.state.connections.find((c) => c.id === db.state.pendingConnectionId)?.name ?? "Unknown",
	);

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
			case "drop-view":
				return "Drop View";
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

	/** A bind value as the SQL view lists it beside the statement. */
	function formatValue(value: unknown): string {
		if (value === null || value === undefined) return "NULL";
		if (typeof value === "string") return `'${value}'`;
		return cellText(value);
	}

	/** The statement's values, numbered in bind order, e.g. `1: 'a'  2: 5`. */
	function formatValues(bindValues?: unknown[]): string {
		return (bindValues ?? []).map((v, i) => `${i + 1}: ${formatValue(v)}`).join("  ");
	}

	function truncateSql(sql: string, maxLength = 120): string {
		const oneLine = sql.replace(/\s+/g, " ").trim();
		if (oneLine.length <= maxLength) return oneLine;
		return oneLine.slice(0, maxLength) + "…";
	}

	/** The queue's destructive statements for the dialog (`null`: the check failed). */
	const destructive = $derived(
		listDestructive(
			changes,
			db.state.connections.find((c) => c.id === db.state.pendingConnectionId)?.type,
		),
	);

	const shownDestructive = $derived(coreDestructive?.statements ?? destructive ?? []);
	const interrupted = $derived(
		!!db.state.pendingConnectionId && !!db.state.pendingChangesInterrupted[db.state.pendingConnectionId],
	);

	function handleRemove(changeId: string) {
		const connectionId = db.state.pendingConnectionId;
		if (connectionId) {
			db.pendingChanges.remove(connectionId, changeId);
		}
	}

	function handleClear() {
		const connectionId = db.state.pendingConnectionId;
		if (connectionId) {
			db.pendingChanges.clear(connectionId);
		}
	}

	function openConfirm() {
		coreDestructive = null;
		showConfirmDialog = true;
	}

	/**
	 * Apply the queue. The dialog listed its destructive statements, so they
	 * go confirmed; without any listed (none found, or the check failed) Core
	 * decides, and a `confirmRequired` reopens the dialog with Core's list.
	 */
	async function handleExecuteAll() {
		showConfirmDialog = false;
		const connectionId = db.state.pendingConnectionId;
		if (!connectionId) return;
		const confirmed = confirmedFor(destructive, coreDestructive !== null);
		coreDestructive = null;

		isExecuting = true;
		try {
			failure = null;
			const result = await db.pendingChanges.apply(connectionId, confirmed);
			const plural = (n: number) => `${n} statement${n === 1 ? "" : "s"}`;
			switch (result.kind) {
				case "empty":
					break;
				case "applied":
					toast.success(`${plural(result.applied)} executed successfully`);
					db.pendingChanges.closeSheet();
					break;
				case "confirmRequired":
					coreDestructive = { statements: result.destructive, total: result.total };
					showConfirmDialog = true;
					break;
				case "failed": {
					const at = result.index === undefined ? "" : ` at statement ${result.index + 1}`;
					errorToast(`Failed${at}: ${result.error}`);
					if (result.mode === "atomic") {
						toast.info(m.pending_changes_rolled_back());
					} else if (result.applied > 0) {
						toast.info(`${plural(result.applied)} executed before failure`);
					}
					// Keep the sheet open on the failed change, still pending.
					const failed = (db.state.pendingChangesByConnection[connectionId] ?? []).find(
						(c) => c.id === result.changeId,
					);
					if (failed) failure = { change: failed, error: result.error };
					break;
				}
				case "refused":
					errorToast(result.error);
					break;
				case "interrupted":
					errorToast(m.pending_changes_interrupted({ error: result.error }));
					break;
			}
		} catch (error) {
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

	{#if interrupted && changes.length > 0}
		<p class="border-b border-destructive/40 bg-destructive/5 px-4 py-2 text-xs text-destructive">
			{m.pending_changes_maybe_applied()}
		</p>
	{/if}

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
					{@const failed = shownFailure?.change === change}
					<div
						class={["group relative rounded-md border px-3 py-2.5 text-sm", failed && "border-destructive bg-destructive/5"]}
					>
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
										{truncateSql(change.sql, 200)}
									</code>
									{#if change.bindValues?.length}
										<code class="mt-0.5 block text-[11px] text-muted-foreground/80 break-all whitespace-pre-wrap">
											{m.pending_changes_values({ values: truncateSql(formatValues(change.bindValues), 200) })}
										</code>
									{/if}
								{/if}
								{#if failed && shownFailure}
									<p class="mt-1 text-xs text-destructive break-words">{shownFailure.error}</p>
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
				onclick={openConfirm}
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
				These statements will be executed on {connectionName}. This action cannot be undone.
			</AlertDialog.Description>
		</AlertDialog.Header>
		{#if shownDestructive.length > 0}
			<div class="rounded-md border border-destructive/40 bg-destructive/5 px-3 py-2 text-xs text-destructive">
				<p class="font-medium">
					{m.pending_changes_destructive({ count: coreDestructive?.total ?? shownDestructive.length })}
				</p>
				<ul class="mt-1 flex flex-col gap-1">
					{#each shownDestructive as statement (statement.index)}
						<li class="flex flex-col">
							<span>{statement.index + 1}. {statement.reason.replaceAll("_", " ")}</span>
							<code class="block overflow-x-auto whitespace-nowrap scrollbar-hide">{truncateSql(statement.sql)}</code>
						</li>
					{/each}
				</ul>
			</div>
		{/if}
		<div class="flex max-h-48 flex-col gap-2 overflow-y-auto py-2">
			{#each changes as change (change.id)}
				{@const Icon2 = getIcon(change)}
				<div class="bg-muted rounded-md px-3 py-2 text-sm">
					<div class="flex items-center gap-2">
						<Icon2 class="size-3.5 text-muted-foreground" />
						<span class="font-medium text-xs">{change.description}</span>
					</div>
					<code class="text-muted-foreground mt-1 block overflow-x-auto whitespace-nowrap scrollbar-hide text-xs">{change.sql}</code>
					{#if change.bindValues?.length}
						<code class="text-muted-foreground/80 block overflow-x-auto whitespace-nowrap scrollbar-hide text-[11px]">{m.pending_changes_values({ values: formatValues(change.bindValues) })}</code>
					{/if}
				</div>
			{/each}
		</div>
		<AlertDialog.Footer>
			<AlertDialog.Cancel onclick={() => (coreDestructive = null)}>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action onclick={handleExecuteAll}>
				Execute All
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
