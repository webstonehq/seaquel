<script lang="ts">
	import { onMount } from "svelte";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import TrashIcon from "@lucide/svelte/icons/trash-2";
	import { Button } from "$lib/components/ui/button/index.js";

	interface MemberView {
		tenantMemberId: string;
		containerUserId: string;
		email: string;
		role: "owner" | "member";
		boundAt: string | null;
		maskedLicenseKey: string;
	}

	let loading = $state(true);
	let loadError = $state<string | null>(null);
	let members = $state<MemberView[]>([]);
	let currentUserRole = $state<"owner" | "member">("member");
	let removing = $state<string | null>(null);

	const isOwner = $derived(currentUserRole === "owner");

	async function refresh() {
		try {
			const res = await fetch("/api/team");
			if (!res.ok) {
				loadError = `Couldn't load team (${res.status})`;
				loading = false;
				return;
			}
			const data = (await res.json()) as {
				members: MemberView[];
				currentUserRole: "owner" | "member";
			};
			members = data.members;
			currentUserRole = data.currentUserRole;
			loading = false;
		} catch (e) {
			loadError = e instanceof Error ? e.message : String(e);
			loading = false;
		}
	}

	onMount(refresh);

	async function removeMember(member: MemberView) {
		if (
			!confirm(
				`Remove ${member.email}? They'll be signed out and their license seat will free up.`,
			)
		)
			return;
		removing = member.containerUserId;
		try {
			const res = await fetch(
				`/api/team/${encodeURIComponent(member.containerUserId)}`,
				{ method: "DELETE" },
			);
			if (!res.ok) {
				errorToast(`Couldn't remove ${member.email} (${res.status})`);
				return;
			}
			toast.success(`Removed ${member.email}`);
			await refresh();
		} finally {
			removing = null;
		}
	}

	function formatDate(v: string | null): string {
		if (!v) return "";
		return new Date(v).toLocaleDateString();
	}
</script>

<div class="space-y-6" data-section="team">
	<div>
		<h3 class="text-lg font-medium">Team</h3>
		<p class="text-muted-foreground text-sm">
			People with a license from this subscription can join by visiting
			this URL and entering their license key.
		</p>
	</div>

	{#if loading}
		<p class="text-muted-foreground text-sm">Loading…</p>
	{:else if loadError}
		<p class="text-destructive text-sm">{loadError}</p>
	{:else}
		<div class="space-y-2">
			<h4 class="text-sm font-medium">Members ({members.length})</h4>
			{#each members as m (m.tenantMemberId)}
				<div
					class="flex items-center justify-between gap-3 py-2 px-3 rounded-lg border"
				>
					<div class="flex-1 min-w-0">
						<p class="text-sm truncate">{m.email}</p>
						<p class="text-muted-foreground text-xs">
							{m.role === "owner" ? "Owner" : "Member"}
							{#if m.maskedLicenseKey}· {m.maskedLicenseKey}{/if}
							{#if m.boundAt}· joined {formatDate(m.boundAt)}{/if}
						</p>
					</div>
					{#if isOwner && m.role !== "owner"}
						<Button
							variant="ghost"
							size="icon"
							class="size-8"
							aria-label="Remove member"
							disabled={removing === m.containerUserId}
							onclick={() => removeMember(m)}
						>
							<TrashIcon class="size-3.5" />
						</Button>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>
