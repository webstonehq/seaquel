<script lang="ts">
	import AiModelSwitcher from "$lib/components/ai-model-switcher.svelte";
	import { aiSettingsStore } from "$lib/stores/ai-settings.svelte";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { showErrorUnlessShown } from "$lib/errors";

	let {
		text = $bindable(""),
		loading,
		error = $bindable<{ message: string; action?: { label: string; fn: () => void } } | null>(null),
		notice = null,
		onSubmit,
		onClose,
		focusOnMount,
	}: {
		text: string;
		loading: boolean;
		error: { message: string; action?: { label: string; fn: () => void } } | null;
		/** After an insert: how to run it (the prompt never runs it). */
		notice?: string | null;
		onSubmit: () => void;
		onClose: () => void;
		focusOnMount: () => (el: HTMLInputElement) => void;
	} = $props();

	const db = useDatabase();

	async function handleModelSelect(pid: string, mod: string) {
		const conn = db.state.activeConnection;
		if (!conn) return;
		await db.setConnectionAIModel(conn.id, pid, mod).catch(showErrorUnlessShown);
	}
</script>

<div
	class="absolute top-12 left-4 right-4 z-50 flex items-center gap-2 rounded-lg border border-border bg-background px-3 py-2 shadow-lg"
	role="dialog"
	aria-label="AI query prompt"
>
	<svg class="h-4 w-4 shrink-0 text-muted-foreground" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2a10 10 0 0 1 10 10 10 10 0 0 1-10 10A10 10 0 0 1 2 12 10 10 0 0 1 12 2"/><path d="M12 8v8"/><path d="M8 12h8"/></svg>
	<input
		type="text"
		class="flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
		placeholder="Ask AI to write a query..."
		bind:value={text}
		disabled={loading}
		oninput={() => { error = null; }}
		onkeydown={(e) => {
			if (e.key === "Enter") { e.preventDefault(); onSubmit(); }
			if (e.key === "Escape") { e.preventDefault(); onClose(); }
		}}
		{@attach focusOnMount()}
	/>
	{#if loading}
		<span class="text-xs text-muted-foreground">Thinking...</span>
	{:else if error}
		<span class="text-xs text-destructive">
			{error.message}
			{#if error.action}
				<button class="underline hover:no-underline" onclick={error.action.fn}>{error.action.label}</button>
			{/if}
		</span>
		{#if !error.action && aiSettingsStore.settings.providers.length > 0}
			<AiModelSwitcher
				providerId={db.state.activeConnection?.activeAIProviderId ?? null}
				model={db.state.activeConnection?.activeAIModel ?? null}
				onSelect={async (pid, mod) => {
					await handleModelSelect(pid, mod);
					error = null;
					onSubmit();
				}}
			/>
		{/if}
	{:else}
		{#if notice}
			<span class="text-xs text-muted-foreground" role="status">{notice}</span>
		{/if}
		<AiModelSwitcher
			providerId={db.state.activeConnection?.activeAIProviderId ?? null}
			model={db.state.activeConnection?.activeAIModel ?? null}
			onSelect={handleModelSelect}
		/>
	{/if}
	<button
		class="shrink-0 text-xs text-muted-foreground hover:text-foreground"
		onclick={onClose}
		aria-label="Close"
	>ESC</button>
</div>
