<script lang="ts">
	import { onMount } from "svelte";
	import { getVersion } from "@tauri-apps/api/app";
	import { m } from "$lib/paraglide/messages.js";
	import { Switch } from "$lib/components/ui/switch";
	import { updateStore, type UpdateChannel } from "$lib/stores/update.svelte.js";

	const uid = $props.id();
	const labelId = `${uid}-label`;
	const helpId = `${uid}-help`;

	// Null until the version is known (or if it can't be read): the channel row
	// stays hidden so a beta build never briefly shows or acts as Stable.
	let appVersion = $state<string | null>(null);

	onMount(async () => {
		try {
			appVersion = await getVersion();
		} catch (error) {
			console.error("Failed to read the app version:", error);
		}
	});

	// A pre-release build (2026.10.0-beta.2) follows beta until a channel is chosen.
	// Build metadata (after "+") doesn't count, as in the updater's semver check.
	const isPrerelease = $derived(
		appVersion !== null && appVersion.split("+")[0].includes("-")
	);
	const current = $derived<UpdateChannel>(
		updateStore.channel ?? (isPrerelease ? "beta" : "stable")
	);

	function handleBetaChange(checked: boolean) {
		const value: UpdateChannel = checked ? "beta" : "stable";
		// Picking the effective channel again must not forget a download.
		if (value === current) return;
		void updateStore.setChannel(value);
	}
</script>

<div class="space-y-6" data-section="updates">
	<div>
		<h2 class="text-lg font-medium">{m.settings_updates()}</h2>
		<p class="text-sm text-muted-foreground mt-1">
			{m.settings_updates_description()}
		</p>
	</div>

	{#if appVersion !== null}
		<div class="flex items-center justify-between gap-4">
			<div>
				<p id={labelId} class="text-sm font-medium">{m.settings_update_beta()}</p>
				<p id={helpId} class="text-xs text-muted-foreground">
					{m.settings_update_channel_beta_help()}
				</p>
				{#if current === "stable" && isPrerelease}
					<p class="text-xs text-muted-foreground mt-1">
						{m.settings_update_channel_stay({ version: appVersion })}
					</p>
				{/if}
			</div>
			<Switch
				checked={current === "beta"}
				onCheckedChange={handleBetaChange}
				aria-labelledby={labelId}
				aria-describedby={helpId}
			/>
		</div>
	{/if}
</div>
