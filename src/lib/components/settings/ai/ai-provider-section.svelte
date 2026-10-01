<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import { Button } from "$lib/components/ui/button";
	import DeleteConfirmDialog from "$lib/components/delete-confirm-dialog.svelte";
	import { aiSettingsStore, ProviderKeyNotSavedError } from "$lib/stores/ai-settings.svelte.js";
	import { getKeyringService } from "$lib/services/keyring";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import { libraryErrorMessage } from "$lib/hooks/database/library/messages";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import PencilIcon from "@lucide/svelte/icons/pencil";
	import TrashIcon from "@lucide/svelte/icons/trash-2";
	import CheckIcon from "@lucide/svelte/icons/check";
	import SparklesIcon from "@lucide/svelte/icons/sparkles";

	let deleteProviderDialogOpen = $state(false);
	let providerToDelete = $state<string | null>(null);

	function confirmDeleteProvider(id: string) {
		providerToDelete = id;
		deleteProviderDialogOpen = true;
	}

	async function executeDeleteProvider() {
		if (!providerToDelete) return;
		try {
			await aiSettingsStore.deleteProvider(providerToDelete);
		} catch (error) {
			errorToast(libraryErrorMessage(error));
			return;
		}
		providerToDelete = null;
		deleteProviderDialogOpen = false;
	}

	let isAddingProvider = $state(false);
	let editingProviderId = $state<string | null>(null);
	let providerFormName = $state("");
	let providerFormType = $state<"anthropic" | "openai-compatible">("anthropic");
	let providerFormBaseUrl = $state("");
	let providerFormApiKey = $state("");
	let providerFormHasExistingKey = $state(false);
	let providerFormClearKey = $state(false);
	let isSavingProvider = $state(false);
	let providerTestStatus = $state<Record<string, "idle" | "success" | "failed">>({});
	let isTestingProvider = $state<Record<string, boolean>>({});

	function startAddProvider() {
		editingProviderId = null;
		providerFormName = "";
		providerFormType = "anthropic";
		providerFormBaseUrl = "";
		providerFormApiKey = "";
		providerFormHasExistingKey = false;
		providerFormClearKey = false;
		isAddingProvider = true;
	}

	async function startEditProvider(config: import("$lib/types/ai").AIProvider) {
		isAddingProvider = false;
		providerFormName = config.name;
		providerFormType = config.type;
		providerFormBaseUrl = config.baseUrl ?? "";
		providerFormApiKey = "";
		providerFormHasExistingKey = !!(await getKeyringService().getAIApiKeyForProvider(config.id));
		providerFormClearKey = false;
		editingProviderId = config.id;
	}

	function cancelProviderForm() {
		isAddingProvider = false;
		editingProviderId = null;
	}

	async function saveProviderForm() {
		if (!providerFormName.trim()) return;
		isSavingProvider = true;
		try {
			if (editingProviderId) {
				const existing = aiSettingsStore.settings.providers.find(p => p.id === editingProviderId);
				if (!existing) { isSavingProvider = false; return; }
				await aiSettingsStore.updateProvider({
					...existing,
					name: providerFormName.trim(),
					type: providerFormType,
					baseUrl: providerFormBaseUrl.trim(),
				}, providerFormClearKey ? "" : (providerFormApiKey || undefined));
				if (providerFormApiKey.trim()) providerFormHasExistingKey = true;
			} else {
				// Core gives the provider its id (and, on the desktop, stores the key with it).
				await aiSettingsStore.addProvider({
					name: providerFormName.trim(),
					type: providerFormType,
					baseUrl: providerFormBaseUrl.trim(),
				}, providerFormApiKey || undefined);
			}
			providerFormApiKey = "";
			cancelProviderForm();
			toast.success(m.settings_ai_saved());
		} catch (error) {
			if (error instanceof ProviderKeyNotSavedError) {
				// Saved without its key: the form edits it now, so saving again
				// stores the key instead of adding the provider twice.
				isAddingProvider = false;
				editingProviderId = error.providerId;
				providerFormHasExistingKey = false;
				errorToast(error.message);
			} else {
				errorToast(libraryErrorMessage(error));
			}
		} finally {
			isSavingProvider = false;
		}
	}

	async function testProviderConnection(id: string) {
		isTestingProvider = { ...isTestingProvider, [id]: true };
		providerTestStatus = { ...providerTestStatus, [id]: "idle" };
		try {
			const ok = await aiSettingsStore.testConnection(id);
			providerTestStatus = { ...providerTestStatus, [id]: ok ? "success" : "failed" };
		} catch {
			providerTestStatus = { ...providerTestStatus, [id]: "failed" };
		} finally {
			isTestingProvider = { ...isTestingProvider, [id]: false };
		}
	}
</script>

<div class="space-y-6" data-section="ai-provider">
	<div>
		<h2 class="text-lg font-medium">{m.settings_ai_provider()}</h2>
		<p class="text-sm text-muted-foreground mt-1">
			{m.settings_ai_provider_description()}
		</p>
	</div>

	<div class="space-y-3">
		{#if aiSettingsStore.settings.providers.length === 0 && !isAddingProvider}
			<p class="text-sm text-muted-foreground">{m.settings_ai_no_providers()}</p>
		{/if}

		{#each aiSettingsStore.settings.providers as config (config.id)}
			<div class="flex items-center justify-between rounded-lg border px-3 py-2.5" class:opacity-50={editingProviderId !== null && editingProviderId !== config.id}>
				<div class="flex items-center gap-3">
					<div>
						<p class="text-sm font-medium">{config.name}</p>
						<p class="text-xs text-muted-foreground">{config.type === "anthropic" ? m.settings_ai_provider_anthropic() : m.settings_ai_provider_openai_compatible()}</p>
					</div>
				</div>
				<div class="flex items-center gap-1">
					{#if providerTestStatus[config.id] === "success"}
						<span class="text-xs text-green-600 dark:text-green-400 flex items-center gap-1 mr-1">
							<CheckIcon class="size-3" />{m.settings_ai_test_success()}
						</span>
					{:else if providerTestStatus[config.id] === "failed"}
						<span class="text-xs text-destructive mr-1">{m.settings_ai_test_failed()}</span>
					{/if}
					<Button
						size="icon"
						variant="ghost"
						class="size-7"
						disabled={isTestingProvider[config.id]}
						onclick={() => testProviderConnection(config.id)}
						title={m.settings_ai_test_connection()}
					>
						<SparklesIcon class="size-3.5" />
					</Button>
					<Button
						size="icon"
						variant="ghost"
						class="size-7"
						onclick={() => startEditProvider(config)}
						title={m.settings_ai_edit_provider()}
					>
						<PencilIcon class="size-3.5" />
					</Button>
					<Button
						size="icon"
						variant="ghost"
						class="size-7 text-destructive hover:text-destructive"
						onclick={() => confirmDeleteProvider(config.id)}
						title={m.settings_ai_delete_provider()}
					>
						<TrashIcon class="size-3.5" />
					</Button>
				</div>
			</div>
		{/each}

		{#if isAddingProvider || editingProviderId}
			<div class="rounded-lg border p-4 space-y-3">
				<p class="text-sm font-medium">{isAddingProvider ? m.settings_ai_add_provider() : m.settings_ai_edit_provider()}</p>
				<div class="space-y-1">
					<p class="text-xs font-medium text-muted-foreground">{m.settings_ai_provider_name()}</p>
					<input
						type="text"
						class="w-full px-3 py-2 border rounded-md bg-background text-sm"
						placeholder={m.settings_ai_provider_name_placeholder()}
						bind:value={providerFormName}
					/>
				</div>
				<select
					class="w-full px-3 py-2 h-10 border rounded-md bg-background text-sm"
					bind:value={providerFormType}
				>
					<option value="anthropic">{m.settings_ai_provider_anthropic()}</option>
					<option value="openai-compatible">{m.settings_ai_provider_openai_compatible()}</option>
				</select>
				<div class="space-y-1">
					{#if providerFormHasExistingKey}
						<div class="flex items-center gap-2">
							<p class="text-xs text-green-600 dark:text-green-400">{m.settings_ai_key_saved()}</p>
							<button
								type="button"
								class="text-xs text-muted-foreground hover:text-foreground underline"
								onclick={() => { providerFormHasExistingKey = false; providerFormClearKey = true; }}
							>{m.settings_ai_clear_key()}</button>
						</div>
					{/if}
					<input
						type="password"
						class="w-full px-3 py-2 border rounded-md bg-background text-sm"
						placeholder={providerFormHasExistingKey ? "***" : m.settings_ai_api_key_placeholder()}
						bind:value={providerFormApiKey}
					/>
				</div>
				{#if providerFormType === "openai-compatible"}
					<input
						type="text"
						class="w-full px-3 py-2 border rounded-md bg-background text-sm"
						placeholder={m.settings_ai_base_url_placeholder()}
						bind:value={providerFormBaseUrl}
					/>
				{/if}
				<div class="flex items-center gap-2">
					<Button size="sm" onclick={saveProviderForm} disabled={isSavingProvider || !providerFormName.trim()}>
						{m.settings_ai_save()}
					</Button>
					<Button size="sm" variant="ghost" onclick={cancelProviderForm}>{m.common_cancel()}</Button>
				</div>
			</div>
		{:else}
			<Button variant="outline" size="sm" onclick={startAddProvider}>
				<PlusIcon class="size-3.5 mr-1" />
				{m.settings_ai_add_provider()}
			</Button>
		{/if}
	</div>
</div>

<DeleteConfirmDialog
	bind:open={deleteProviderDialogOpen}
	title={m.settings_ai_delete_provider()}
	cancelText={m.common_cancel()}
	confirmText={m.theme_delete_confirm()}
	onconfirm={executeDeleteProvider}
>
	{#if providerToDelete}
		{@const name = aiSettingsStore.settings.providers.find(p => p.id === providerToDelete)?.name ?? "this provider"}
		This will permanently delete "{name}" and its API key. This cannot be undone.
	{/if}
</DeleteConfirmDialog>
