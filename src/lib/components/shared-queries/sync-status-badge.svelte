<script lang="ts">
	import { Badge } from "$lib/components/ui/badge";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import type { SyncState } from "$lib/types";
	import type { ProjectSyncStatus } from "./project-sync-status";
	import { m } from "$lib/paraglide/messages.js";
	import CheckCircle2Icon from "@lucide/svelte/icons/check-circle-2";
	import ArrowUpIcon from "@lucide/svelte/icons/arrow-up";
	import ArrowDownIcon from "@lucide/svelte/icons/arrow-down";
	import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
	import AlertCircleIcon from "@lucide/svelte/icons/alert-circle";
	import CircleDashedIcon from "@lucide/svelte/icons/circle-dashed";
	import Loader2Icon from "@lucide/svelte/icons/loader-2";

	interface Props {
		status: ProjectSyncStatus;
		syncState?: SyncState | null;
		showCounts?: boolean;
	}

	let { status, syncState, showCounts = true }: Props = $props();

	const statusConfig = $derived.by(() => {
		if (syncState?.isSyncing) {
			return {
				icon: Loader2Icon,
				label: "Syncing...",
				variant: "secondary" as const,
				iconClass: "animate-spin"
			};
		}

		switch (status) {
			case "synced":
				return {
					icon: CheckCircle2Icon,
					label: "Synced",
					variant: "secondary" as const,
					iconClass: "text-green-500"
				};
			case "ahead":
				return {
					icon: ArrowUpIcon,
					label: `${syncState?.aheadBy ?? 0} ahead`,
					variant: "secondary" as const,
					iconClass: "text-blue-500"
				};
			case "behind":
				return {
					icon: ArrowDownIcon,
					label: `${syncState?.behindBy ?? 0} behind`,
					variant: "secondary" as const,
					iconClass: "text-orange-500"
				};
			case "diverged":
				return {
					icon: AlertTriangleIcon,
					label: "Diverged",
					variant: "destructive" as const,
					iconClass: ""
				};
			case "error":
				return {
					icon: AlertCircleIcon,
					label: "Error",
					variant: "destructive" as const,
					iconClass: ""
				};
			case "skipped":
				return {
					icon: AlertTriangleIcon,
					label: m.shared_sync_status_skipped(),
					variant: "outline" as const,
					iconClass: "text-orange-500"
				};
			case "uninitialized":
				return {
					icon: CircleDashedIcon,
					label: "Not synced",
					variant: "outline" as const,
					iconClass: "text-muted-foreground"
				};
			default:
				return {
					icon: CircleDashedIcon,
					label: "Unknown",
					variant: "outline" as const,
					iconClass: ""
				};
		}
	});

	const tooltipContent = $derived.by(() => {
		const parts: string[] = [];

		if (status === "skipped") {
			parts.push(m.shared_sync_status_skipped_tooltip());
		}

		if (syncState?.lastError) {
			parts.push(`Error: ${syncState.lastError}`);
		}

		if (syncState?.pendingChanges && syncState.pendingChanges > 0) {
			parts.push(`${syncState.pendingChanges} uncommitted changes`);
		}

		if (syncState?.aheadBy && syncState.aheadBy > 0) {
			parts.push(`${syncState.aheadBy} commits to push`);
		}

		if (syncState?.behindBy && syncState.behindBy > 0) {
			parts.push(`${syncState.behindBy} commits to pull`);
		}

		if (syncState?.conflictFiles && syncState.conflictFiles.length > 0) {
			parts.push(`${syncState.conflictFiles.length} conflicting files`);
		}

		return parts.length > 0 ? parts.join("\n") : statusConfig.label;
	});
</script>

<Tooltip.Root>
	<Tooltip.Trigger>
		<Badge variant={statusConfig.variant} class="text-xs gap-1">
			{#if syncState?.isSyncing}
				<Loader2Icon class="size-3 animate-spin" />
			{:else if status === "synced"}
				<CheckCircle2Icon class="size-3 text-green-500" />
			{:else if status === "ahead"}
				<ArrowUpIcon class="size-3 text-blue-500" />
			{:else if status === "behind"}
				<ArrowDownIcon class="size-3 text-orange-500" />
			{:else if status === "diverged" || status === "skipped"}
				<AlertTriangleIcon class="size-3" />
			{:else if status === "error"}
				<AlertCircleIcon class="size-3" />
			{:else}
				<CircleDashedIcon class="size-3 text-muted-foreground" />
			{/if}
			{#if showCounts}
				<span>{statusConfig.label}</span>
			{/if}
		</Badge>
	</Tooltip.Trigger>
	<Tooltip.Content>
		<p class="whitespace-pre-line">{tooltipContent}</p>
	</Tooltip.Content>
</Tooltip.Root>
