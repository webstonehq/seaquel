<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import { Switch } from "$lib/components/ui/switch";
	import { aiSettingsStore } from "$lib/stores/ai-settings.svelte.js";
	import { errorToast } from "$lib/utils/toast";
	import type { AISettings } from "$lib/types/ai";

	async function savePrivacy(patch: Pick<AISettings, "shareSchemaGlobally" | "shareDataGlobally">) {
		try {
			await aiSettingsStore.savePrivacySettings(patch);
		} catch (error) {
			errorToast(error instanceof Error ? error.message : String(error));
		}
	}
</script>

<div class="space-y-6" data-section="ai-privacy">
	<div>
		<h2 class="text-lg font-medium">{m.settings_ai_privacy()}</h2>
		<p class="text-sm text-muted-foreground mt-1">
			{m.settings_ai_privacy_description()}
		</p>
	</div>

	<div class="space-y-4">
		<div class="flex items-center justify-between">
			<div>
				<p class="text-sm font-medium">{m.settings_ai_share_schema_globally()}</p>
				<p class="text-xs text-muted-foreground">
					{m.settings_ai_share_schema_globally_description()}
				</p>
			</div>
			<Switch
				checked={aiSettingsStore.settings.shareSchemaGlobally}
				onCheckedChange={async (checked) => {
					await savePrivacy({ shareSchemaGlobally: checked, shareDataGlobally: aiSettingsStore.settings.shareDataGlobally });
				}}
			/>
		</div>

		<div class="flex items-center justify-between">
			<div>
				<p class="text-sm font-medium">{m.settings_ai_share_data_globally()}</p>
				<p class="text-xs text-muted-foreground">
					{m.settings_ai_share_data_globally_description()}
				</p>
			</div>
			<Switch
				checked={aiSettingsStore.settings.shareDataGlobally}
				onCheckedChange={async (checked) => {
					await savePrivacy({ shareSchemaGlobally: aiSettingsStore.settings.shareSchemaGlobally, shareDataGlobally: checked });
				}}
			/>
		</div>
	</div>

	{#if !aiSettingsStore.settings.shareSchemaGlobally && !aiSettingsStore.settings.shareDataGlobally}
		<p class="text-xs text-muted-foreground bg-muted rounded-md px-3 py-2">
			{m.settings_ai_no_context_note()}
		</p>
	{/if}
</div>
