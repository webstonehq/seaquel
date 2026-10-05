<script lang="ts">
	/**
	 * DuckDB support's install dialog (desktop only; desktop DuckDB helper
	 * plan, Task 5). Opened by `withDuckdbHelper` when a DuckDB connect finds
	 * this version's helper missing; the steps and their wording live in
	 * `duckdbInstallStore`.
	 */
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { Button } from "$lib/components/ui/button";
	import { m } from "$lib/paraglide/messages.js";
	import { duckdbInstallStore as store, sizeText } from "$lib/stores/duckdb-install.svelte.js";
	import DownloadIcon from "@lucide/svelte/icons/download";
	import CircleAlertIcon from "@lucide/svelte/icons/circle-alert";
	import LoaderIcon from "@lucide/svelte/icons/loader-circle";

	const stage = $derived(store.stage);
	const percent = $derived(
		stage.step === "downloading" && stage.total > 0
			? Math.min(100, Math.round((stage.bytes / stage.total) * 100))
			: 0,
	);

	const step = $derived(stage.step);
	/** A download or file install is running: a stray click outside mustn't cancel it. */
	const busy = $derived(step === "downloading" || step === "installingFile");
	const progressText = $derived(
		stage.step === "downloading" && stage.bytes > 0
			? m.duckdb_install_progress({ bytes: sizeText(stage.bytes), total: sizeText(stage.total) })
			: m.duckdb_install_starting(),
	);

	/** The step's primary button (or its Cancel), focused when the step changes. */
	let primary = $state<HTMLElement | null>(null);
	$effect(() => {
		void step;
		primary?.focus();
	});

	function handleOpenChange(open: boolean) {
		if (!open) store.dismiss();
	}

	function handleInteractOutside(event: Event) {
		// Esc and Cancel still cancel.
		if (busy) event.preventDefault();
	}
</script>

<Dialog.Root bind:open={store.open} onOpenChange={handleOpenChange}>
	<Dialog.Content class="sm:max-w-md" onInteractOutside={handleInteractOutside}>
		{#if stage.step === "checking"}
			<Dialog.Header>
				<Dialog.Title>{m.duckdb_install_title()}</Dialog.Title>
				<Dialog.Description class="flex items-center gap-2">
					<LoaderIcon class="size-4 animate-spin" aria-hidden="true" />
					{m.duckdb_install_checking()}
				</Dialog.Description>
			</Dialog.Header>
			<Dialog.Footer>
				<Button bind:ref={primary} variant="outline" onclick={() => store.dismiss()}>
					{m.common_cancel()}
				</Button>
			</Dialog.Footer>
		{:else if stage.step === "ask"}
			<Dialog.Header>
				<Dialog.Title class="flex items-center gap-2">
					<DownloadIcon class="size-4" aria-hidden="true" />
					{m.duckdb_install_ask_title()}
				</Dialog.Title>
				<Dialog.Description>
					{m.duckdb_install_ask({ size: sizeText(stage.size) })}
				</Dialog.Description>
			</Dialog.Header>
			<div class="space-y-2 text-sm text-muted-foreground">
				<p>{m.duckdb_install_for_version({ version: stage.version })}</p>
				{#if stage.repair}
					<p>{m.duckdb_install_repair()}</p>
				{/if}
				<p class="text-xs">{m.duckdb_install_checked()}</p>
			</div>
			<Dialog.Footer>
				<Button variant="outline" onclick={() => store.dismiss()}>{m.duckdb_install_not_now()}</Button>
				<Button bind:ref={primary} onclick={() => store.download()}>
					{m.duckdb_install_download()}
				</Button>
			</Dialog.Footer>
		{:else if stage.step === "downloading"}
			<Dialog.Header>
				<Dialog.Title>{m.duckdb_install_downloading()}</Dialog.Title>
				<Dialog.Description>{progressText}</Dialog.Description>
			</Dialog.Header>
			<div
				class="h-2 w-full overflow-hidden rounded bg-muted"
				role="progressbar"
				aria-label={m.duckdb_install_downloading()}
				aria-valuemin={0}
				aria-valuemax={100}
				aria-valuenow={percent}
				aria-valuetext={progressText}
			>
				<div class="h-full bg-primary transition-[width]" style:width="{percent}%"></div>
			</div>
			<Dialog.Footer>
				<Button bind:ref={primary} variant="outline" onclick={() => store.dismiss()}>
					{m.common_cancel()}
				</Button>
			</Dialog.Footer>
		{:else if stage.step === "installingFile"}
			<Dialog.Header>
				<Dialog.Title>{m.duckdb_install_title()}</Dialog.Title>
				<Dialog.Description class="flex items-center gap-2">
					<LoaderIcon class="size-4 animate-spin" aria-hidden="true" />
					{m.duckdb_install_installing_file()}
				</Dialog.Description>
			</Dialog.Header>
			<Dialog.Footer>
				<Button bind:ref={primary} variant="outline" onclick={() => store.dismiss()}>
					{m.common_cancel()}
				</Button>
			</Dialog.Footer>
		{:else if stage.step === "failed"}
			{@const failure = stage.failure}
			<Dialog.Header>
				<Dialog.Title class="flex items-center gap-2">
					<CircleAlertIcon class="size-4 text-destructive" aria-hidden="true" />
					{failure.title}
				</Dialog.Title>
				<Dialog.Description>{failure.hint}</Dialog.Description>
			</Dialog.Header>
			<p class="rounded border bg-muted/50 p-3 font-mono text-xs break-words select-text">
				{failure.code}{failure.message ? `: ${failure.message}` : ""}
			</p>
			<Dialog.Footer>
				{#if failure.retry || failure.fromFile}
					<Button variant="outline" onclick={() => store.dismiss()}>
						{m.duckdb_install_close()}
					</Button>
				{:else}
					<Button bind:ref={primary} onclick={() => store.dismiss()}>
						{m.duckdb_install_close()}
					</Button>
				{/if}
				{#if failure.fromFile && failure.retry}
					<Button variant="outline" onclick={() => store.installFromFile()}>
						{m.duckdb_install_from_file()}
					</Button>
				{:else if failure.fromFile}
					<Button bind:ref={primary} onclick={() => store.installFromFile()}>
						{m.duckdb_install_from_file()}
					</Button>
				{/if}
				{#if failure.retry}
					<Button bind:ref={primary} onclick={() => store.retry()}>
						{m.duckdb_install_try_again()}
					</Button>
				{/if}
			</Dialog.Footer>
		{:else if stage.step === "unusable"}
			<Dialog.Header>
				<Dialog.Title class="flex items-center gap-2">
					<CircleAlertIcon class="size-4 text-destructive" aria-hidden="true" />
					{m.duckdb_install_unusable_title()}
				</Dialog.Title>
				<Dialog.Description>{m.duckdb_helper_unusable({ reason: stage.reason })}</Dialog.Description>
			</Dialog.Header>
			<Dialog.Footer>
				<Button bind:ref={primary} onclick={() => store.dismiss()}>
					{m.duckdb_install_close()}
				</Button>
			</Dialog.Footer>
		{/if}
	</Dialog.Content>
</Dialog.Root>
