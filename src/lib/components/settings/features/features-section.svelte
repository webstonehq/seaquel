<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import { Switch } from "$lib/components/ui/switch";
	import { aiSettingsStore } from "$lib/stores/ai-settings.svelte.js";
	import { onboardingStore } from "$lib/stores/onboarding.svelte.js";
	import { pendingChangesSettingsStore } from "$lib/stores/pending-changes-settings.svelte.js";
	import { getDatabase } from "$lib/storage/db";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { goto } from "$app/navigation";
	import { resolve } from "$app/paths";
	import { page } from "$app/state";

	const db = useDatabase();

	async function handleAIToggle(checked: boolean) {
		const database = await getDatabase();
		await aiSettingsStore.setEnabled(database, checked);
		if (!checked && db.state.isAIOpen) {
			db.ui.toggleAI();
		}
	}

	function handleLearnToggle(checked: boolean) {
		onboardingStore.setLearnEnabled(checked);
		if (!checked && page.url.pathname.startsWith(resolve("/(app)/learn"))) {
			goto(resolve("/(app)/manage"));
		}
	}
</script>

<div class="space-y-4" data-section="ai-feature">
	<div class="flex items-center justify-between">
		<div>
			<p class="text-sm font-medium">{m.settings_ai_feature()}</p>
			<p class="text-xs text-muted-foreground">
				{m.settings_ai_feature_enabled_description()}
			</p>
		</div>
		<Switch
			checked={aiSettingsStore.settings.enabled}
			onCheckedChange={handleAIToggle}
		/>
	</div>
	<div class="flex items-center justify-between">
		<div>
			<p class="text-sm font-medium">{m.settings_learn()}</p>
			<p class="text-xs text-muted-foreground">
				{m.settings_learn_enabled_description()}
			</p>
		</div>
		<Switch
			checked={onboardingStore.learnEnabled}
			onCheckedChange={handleLearnToggle}
		/>
	</div>
	<div class="flex items-center justify-between">
		<div>
			<p class="text-sm font-medium">Pending Changes</p>
			<p class="text-xs text-muted-foreground">
				Queue write and DDL queries for review before executing them
			</p>
		</div>
		<Switch
			checked={pendingChangesSettingsStore.enabled}
			onCheckedChange={(v) => pendingChangesSettingsStore.setEnabled(v)}
		/>
	</div>
</div>
