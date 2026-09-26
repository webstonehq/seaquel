<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { m } from "$lib/paraglide/messages.js";
	import type { WizardFormData } from "$lib/stores/connection-wizard.svelte.js";
	import { availableDatabaseTypes } from "$lib/stores/connection-wizard.svelte.js";
	import type { DatabaseType } from "$lib/types";
	import DatabaseTypeCard from "./database-type-card.svelte";
	import CopyIcon from "@lucide/svelte/icons/copy";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";

	interface Props {
		formData: WizardFormData;
		onParse: (connStr: string) => boolean;
		onSelectType: (type: DatabaseType) => void;
		onContinue: () => void;
		error: string | null;
	}

	let { formData = $bindable(), onParse, onSelectType, onContinue, error }: Props = $props();

	// SQLite and DuckDB aren't offered on web (they'd open files on the server).
	const databaseTypes = availableDatabaseTypes();

	// Auto-parse when connection string contains ://, then advance to step 2
	$effect(() => {
		if (formData.connectionString.includes("://")) {
			const timeout = setTimeout(() => {
				if (formData.connectionString.trim()) {
					const success = onParse(formData.connectionString.trim());
					if (success) onContinue();
				}
			}, 500);
			return () => clearTimeout(timeout);
		}
	});
</script>

<div class="flex flex-col gap-6 py-4">
	<div class="space-y-2 text-center">
		<h2 class="text-lg font-semibold">{m.wizard_method_title()}</h2>
		<p class="text-sm text-muted-foreground">
			{m.wizard_method_description()}
		</p>
	</div>

	<!-- Connection string section -->
	<div class="space-y-4">
		<div class="grid gap-2">
			<Label for="connection-string">{m.connection_dialog_label_connection_string()}</Label>
			<Input
				id="connection-string"
				bind:value={formData.connectionString}
				placeholder={m.connection_dialog_placeholder_connection_string()}
				class="font-mono text-sm"
			/>
			<p class="text-xs text-muted-foreground">
				{m.connection_dialog_help_formats()}
				<code class="text-xs">postgres://user:pass@host:port/db</code>
			</p>
		</div>

		{#if error}
			<div class="flex items-start gap-2 p-3 rounded-lg border border-destructive/50 bg-destructive/10 text-destructive text-sm">
				<span class="flex-1">{error}</span>
				<Button
					variant="ghost"
					size="icon"
					class="shrink-0 size-6 text-destructive/70 hover:text-destructive hover:bg-destructive/20"
					onclick={async () => {
						try {
							await navigator.clipboard.writeText(error ?? '');
							toast.success(m.query_error_copied());
						} catch {
							errorToast(m.query_copy_failed());
						}
					}}
				>
					<CopyIcon class="size-3.5" />
				</Button>
			</div>
		{/if}
	</div>

	<!-- Divider -->
	<div class="flex items-center gap-4">
		<div class="flex-1 h-px bg-border"></div>
		<span class="text-xs text-muted-foreground uppercase">{m.wizard_method_divider()}</span>
		<div class="flex-1 h-px bg-border"></div>
	</div>

	<!-- Database type grid -->
	<div class={["grid gap-3", databaseTypes.length === 4 ? "grid-cols-2" : "grid-cols-3"]}>
		{#each databaseTypes as dbType (dbType.value)}
			<DatabaseTypeCard config={dbType} onclick={() => onSelectType(dbType.value)} />
		{/each}
	</div>
</div>
