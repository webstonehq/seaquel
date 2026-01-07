<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { m } from "$lib/paraglide/messages.js";
	import type { WizardFormData, DatabaseTypeConfig } from "$lib/stores/connection-wizard.svelte.js";
	import CheckCircleIcon from "@lucide/svelte/icons/check-circle";
	import TableIcon from "@lucide/svelte/icons/table";
	import FileCodeIcon from "@lucide/svelte/icons/file-code";
	import NetworkIcon from "@lucide/svelte/icons/network";

	interface Props {
		formData: WizardFormData;
		selectedDbType: DatabaseTypeConfig | undefined;
		schemaCount: number;
		onBrowseTables: () => void;
		onWriteQuery: () => void;
		onViewErd: () => void;
		onClose: () => void;
	}

	let {
		formData,
		selectedDbType,
		schemaCount,
		onBrowseTables,
		onWriteQuery,
		onViewErd,
		onClose,
	}: Props = $props();
</script>

<div class="flex flex-col items-center gap-6 py-8">
	<!-- Success icon with animation -->
	<div class="relative">
		<div
			class="size-20 rounded-full bg-primary/10 flex items-center justify-center animate-in zoom-in duration-300"
		>
			<CheckCircleIcon class="size-10 text-primary" />
		</div>
		<div
			class="absolute inset-0 rounded-full border-2 border-primary/30 animate-ping"
			style="animation-duration: 1.5s; animation-iteration-count: 1;"
		></div>
	</div>

	<!-- Success message -->
	<div class="space-y-2 text-center">
		<h2 class="text-xl font-semibold text-primary">{m.wizard_success_title()}</h2>
		<p class="text-sm text-muted-foreground">
			{m.wizard_success_description({ name: formData.name })}
		</p>
	</div>

	<!-- Connection summary -->
	<div class="w-full max-w-sm p-4 rounded-lg border bg-muted/30">
		<dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
			<dt class="text-muted-foreground">{m.connection_dialog_parsed_type()}</dt>
			<dd class="font-medium text-right">{selectedDbType?.label}</dd>

			{#if formData.type !== "sqlite"}
				<dt class="text-muted-foreground">{m.connection_dialog_parsed_host()}</dt>
				<dd class="font-medium text-right truncate" title="{formData.host}:{formData.port}">
					{formData.host}:{formData.port}
				</dd>
			{/if}

			<dt class="text-muted-foreground">{m.connection_dialog_parsed_database()}</dt>
			<dd class="font-medium text-right truncate" title={formData.databaseName}>
				{formData.databaseName}
			</dd>

			<dt class="text-muted-foreground">{m.wizard_success_tables()}</dt>
			<dd class="font-medium text-right">{schemaCount}</dd>
		</dl>
	</div>

	<!-- Next actions -->
	<div class="space-y-3 w-full max-w-sm">
		<p class="text-sm font-medium text-center">{m.wizard_success_whats_next()}</p>

		<div class="grid gap-2">
			<Button variant="outline" class="w-full justify-start gap-3" onclick={onBrowseTables}>
				<TableIcon class="size-4" />
				{m.wizard_success_browse_tables()}
			</Button>

			<Button variant="outline" class="w-full justify-start gap-3" onclick={onWriteQuery}>
				<FileCodeIcon class="size-4" />
				{m.wizard_success_write_query()}
			</Button>

			<Button variant="outline" class="w-full justify-start gap-3" onclick={onViewErd}>
				<NetworkIcon class="size-4" />
				{m.wizard_success_view_erd()}
			</Button>
		</div>
	</div>

	<Button variant="ghost" onclick={onClose}>
		{m.wizard_success_close()}
	</Button>
</div>
