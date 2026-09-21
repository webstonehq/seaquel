<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import { toast } from "svelte-sonner";
	import { Button } from "$lib/components/ui/button/index.js";
	import { licenseNudgeStore } from "$lib/stores/license-nudge.svelte.js";
	import { openPath } from "$lib/api/tauri";
	import XIcon from "@lucide/svelte/icons/x";
	import ArrowUpRightIcon from "@lucide/svelte/icons/arrow-up-right";

	const title = $derived(
		licenseNudgeStore.isWorkReminder
			? m.license_nudge_reminder_title()
			: licenseNudgeStore.milestone.kind === "queries"
				? m.license_nudge_title_queries({ count: licenseNudgeStore.milestone.count.toLocaleString() })
				: m.license_nudge_title_days({ count: licenseNudgeStore.milestone.count }),
	);

	function handleWork() {
		const from = licenseNudgeStore.isWorkReminder ? "license-reminder" : "license-nudge";
		licenseNudgeStore.respond("work");
		void openPath(`https://seaquel.app/pricing?from=${from}`);
	}

	function handlePersonal() {
		licenseNudgeStore.respond("personal");
		toast.success(m.license_nudge_thanks_personal());
	}
</script>

{#if licenseNudgeStore.shouldShow}
	<div
		class="fixed bottom-4 left-4 z-50 w-80 rounded-lg border bg-popover text-popover-foreground shadow-lg"
		role="dialog"
		aria-labelledby="license-nudge-title"
	>
		<div class="p-4 flex flex-col gap-3">
			<div class="flex items-start gap-2">
				<h4 id="license-nudge-title" class="flex-1 text-sm font-semibold">{title}</h4>
				<button
					class="shrink-0 -m-1 p-1 rounded text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer"
					title={m.license_nudge_dismiss()}
					aria-label={m.license_nudge_dismiss()}
					onclick={() => licenseNudgeStore.snooze()}
				>
					<XIcon class="size-4" />
				</button>
			</div>
			{#if licenseNudgeStore.isWorkReminder}
				<p class="text-sm text-muted-foreground">{m.license_nudge_reminder_body()}</p>
				<div class="flex gap-2">
					<Button size="sm" onclick={handleWork}>{m.license_nudge_reminder_yes()}</Button>
					<Button size="sm" variant="outline" onclick={handlePersonal}>{m.license_nudge_reminder_no()}</Button>
				</div>
			{:else}
				<p class="text-sm text-muted-foreground">{m.license_nudge_body()}</p>
				<div class="flex gap-2">
					<Button size="sm" onclick={handleWork}>{m.license_nudge_yes()}</Button>
					<Button size="sm" variant="outline" onclick={handlePersonal}>{m.license_nudge_no()}</Button>
				</div>
			{/if}
		</div>
		<!-- Links to the web form rather than collecting an email here, same
		     as the update popover: consent capture stays in one place. -->
		<button
			class="flex items-center gap-2 w-full px-4 py-2.5 border-t text-xs text-muted-foreground hover:text-foreground hover:bg-muted/50 transition-colors cursor-pointer text-left"
			onclick={() => openPath("https://seaquel.app/feedback?from=license-nudge")}
		>
			<span class="flex-1">{m.license_nudge_feedback()}</span>
			<ArrowUpRightIcon class="size-3.5 shrink-0" />
		</button>
	</div>
{/if}
