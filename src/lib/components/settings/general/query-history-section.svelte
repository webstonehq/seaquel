<script lang="ts">
	import { onMount } from "svelte";
	import { m } from "$lib/paraglide/messages.js";
	import { libraryErrorMessage } from "$lib/hooks/database/library/messages";
	import { errorToast } from "$lib/utils/toast";
	import { MIN_VERSION_LIMIT, clampVersionLimit } from "$lib/utils/version-limit";
	import { VersionLimitsStore, type VersionLimitKey } from "$lib/stores/version-limits.svelte";

	// Follows other windows' changes; a save before the read answers isn't undone by it.
	const limits = new VersionLimitsStore();

	/** Stores a limit (clamped, as the setting shows it); a refusal is shown. */
	async function saveLimit(key: VersionLimitKey, value: number): Promise<void> {
		try {
			await limits.save(key, value);
		} catch (error) {
			errorToast(libraryErrorMessage(error));
		}
	}

	onMount(() => {
		limits.load().catch((error: unknown) => errorToast(libraryErrorMessage(error)));
		return () => limits.dispose();
	});
</script>

<div class="space-y-6" data-section="query-history">
	<div>
		<h2 class="text-lg font-medium">{m.settings_query_history()}</h2>
		<p class="text-sm text-muted-foreground mt-1">{m.settings_query_history_description()}</p>
	</div>
	<div class="space-y-4">
		<div class="flex items-center justify-between">
			<div>
				<p class="text-sm font-medium">{m.settings_query_version_limit()}</p>
				<p class="text-sm text-muted-foreground">{m.settings_query_version_limit_description()}</p>
			</div>
			<input
				type="number"
				min={MIN_VERSION_LIMIT}
				max="1000"
				bind:value={limits.query}
				onchange={() => saveLimit("query_version_limit", clampVersionLimit(limits.query))}
				class="w-24 rounded-md border border-input bg-background px-3 py-1.5 text-sm"
			/>
		</div>
		<div class="flex items-center justify-between">
			<div>
				<p class="text-sm font-medium">{m.settings_dashboard_version_limit()}</p>
				<p class="text-sm text-muted-foreground">{m.settings_dashboard_version_limit_description()}</p>
			</div>
			<input
				type="number"
				min={MIN_VERSION_LIMIT}
				max="1000"
				bind:value={limits.dashboard}
				onchange={() => saveLimit("dashboard_version_limit", clampVersionLimit(limits.dashboard))}
				class="w-24 rounded-md border border-input bg-background px-3 py-1.5 text-sm"
			/>
		</div>
	</div>
</div>
