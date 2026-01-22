<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { Textarea } from "$lib/components/ui/textarea";
	import { m } from "$lib/paraglide/messages.js";
	import type { WizardFormData, DatabaseTypeConfig } from "$lib/stores/connection-wizard.svelte.js";
	import CheckCircleIcon from "@lucide/svelte/icons/check-circle";
	import CopyIcon from "@lucide/svelte/icons/copy";
	import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";

	interface Props {
		formData: WizardFormData;
		selectedDbType: DatabaseTypeConfig | undefined;
		onParse: (connStr: string) => boolean;
		error: string | null;
	}

	let { formData = $bindable(), selectedDbType, onParse, error }: Props = $props();

	let isParsed = $state(false);

	const handlePaste = () => {
		if (formData.connectionString.trim()) {
			isParsed = onParse(formData.connectionString.trim());
		}
	};

	// Auto-parse when connection string changes significantly
	$effect(() => {
		if (formData.connectionString.trim().length > 10 && formData.connectionString.includes("://")) {
			// Debounced auto-parse
			const timeout = setTimeout(() => {
				if (formData.connectionString.trim()) {
					isParsed = onParse(formData.connectionString.trim());
				}
			}, 500);
			return () => clearTimeout(timeout);
		} else {
			isParsed = false;
		}
	});
</script>

<div class="flex flex-col gap-6 py-4">
	<div class="space-y-2 text-center">
		<h2 class="text-lg font-semibold">{m.wizard_string_paste_title()}</h2>
		<p class="text-sm text-muted-foreground">
			{m.wizard_string_paste_description()}
		</p>
	</div>

	<div class="space-y-4">
		<div class="grid gap-2">
			<Label for="connection-string">{m.connection_dialog_label_connection_string()}</Label>
			<Textarea
				id="connection-string"
				bind:value={formData.connectionString}
				placeholder={m.connection_dialog_placeholder_connection_string()}
				class="font-mono text-sm min-h-[100px]"
			/>
			<p class="text-xs text-muted-foreground">
				{m.connection_dialog_help_formats()}
				<br />
				<code class="text-xs">postgres://user:pass@host:port/db</code>
			</p>
		</div>

		{#if !isParsed}
			<Button
				type="button"
				variant="outline"
				class="w-full"
				onclick={handlePaste}
				disabled={!formData.connectionString.trim()}
			>
				{m.connection_dialog_button_parse()}
			</Button>
		{/if}

		{#if isParsed && formData.name}
			<div class="p-4 rounded-lg border bg-muted/30 flex flex-col gap-3">
				<div class="flex items-center gap-2 text-sm font-medium text-primary">
					<CheckCircleIcon class="size-4" />
					{m.wizard_string_paste_parsed()}
				</div>
				<div class="grid grid-cols-2 gap-3 text-sm">
					<div>
						<span class="text-muted-foreground">{m.connection_dialog_parsed_type()}</span>
						<span class="font-medium">{selectedDbType?.label}</span>
					</div>
					<div>
						<span class="text-muted-foreground">{m.connection_dialog_parsed_host()}</span>
						<span class="font-medium">{formData.host}</span>
					</div>
					<div>
						<span class="text-muted-foreground">{m.connection_dialog_parsed_database()}</span>
						<span class="font-medium">{formData.databaseName}</span>
					</div>
					<div>
						<span class="text-muted-foreground">{m.connection_dialog_parsed_username()}</span>
						<span class="font-medium">{formData.username || m.connection_dialog_parsed_none()}</span>
					</div>
				</div>

				<div class="grid gap-2 pt-2 border-t">
					<Label for="connection-name">{m.connection_dialog_label_connection_name()}</Label>
					<Input
						id="connection-name"
						bind:value={formData.name}
						placeholder={m.connection_dialog_placeholder_connection_name()}
					/>
				</div>
			</div>
		{/if}

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
</div>
