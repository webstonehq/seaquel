<script lang="ts">
	import { onMount } from "svelte";
	import { m } from "$lib/paraglide/messages.js";
	import { getStorage } from "$lib/storage";
	import { MIN_VERSION_LIMIT, clampVersionLimit } from "$lib/utils/version-limit";

	let queryVersionLimit = $state<number>(100);
	let dashboardVersionLimit = $state<number>(100);

	onMount(async () => {
		const savedLimit = await getStorage().appState.get("query_version_limit");
		if (savedLimit) {
			const parsed = parseInt(savedLimit, 10);
			if (!isNaN(parsed)) queryVersionLimit = parsed;
		}
		const savedDashboardLimit = await getStorage().appState.get("dashboard_version_limit");
		if (savedDashboardLimit) {
			const parsed = parseInt(savedDashboardLimit, 10);
			if (!isNaN(parsed)) dashboardVersionLimit = parsed;
		}
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
				bind:value={queryVersionLimit}
				onchange={async () => {
					queryVersionLimit = clampVersionLimit(queryVersionLimit);
					await getStorage().appState.set("query_version_limit", String(queryVersionLimit));
				}}
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
				bind:value={dashboardVersionLimit}
				onchange={async () => {
					dashboardVersionLimit = clampVersionLimit(dashboardVersionLimit);
					await getStorage().appState.set("dashboard_version_limit", String(dashboardVersionLimit));
				}}
				class="w-24 rounded-md border border-input bg-background px-3 py-1.5 text-sm"
			/>
		</div>
	</div>
</div>
