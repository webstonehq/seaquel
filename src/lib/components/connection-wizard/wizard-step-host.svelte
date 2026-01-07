<script lang="ts">
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { m } from "$lib/paraglide/messages.js";
	import type { WizardFormData, DatabaseTypeConfig } from "$lib/stores/connection-wizard.svelte.js";
	import ServerIcon from "@lucide/svelte/icons/server";

	interface Props {
		formData: WizardFormData;
		selectedDbType: DatabaseTypeConfig | undefined;
	}

	let { formData = $bindable(), selectedDbType }: Props = $props();

	// Common localhost shortcuts
	const quickPresets = [
		{ label: "localhost", host: "localhost" },
		{ label: "127.0.0.1", host: "127.0.0.1" },
		{ label: "Docker", host: "host.docker.internal" },
	];
</script>

<div class="flex flex-col gap-6 py-4">
	<div class="space-y-2 text-center">
		<h2 class="text-lg font-semibold">{m.wizard_host_title()}</h2>
		<p class="text-sm text-muted-foreground">
			{m.wizard_host_description()}
		</p>
	</div>

	<div class="space-y-4">
		<!-- Quick presets -->
		<div class="flex items-center gap-2">
			<span class="text-xs text-muted-foreground">{m.wizard_host_quick()}</span>
			{#each quickPresets as preset}
				<button
					type="button"
					class="px-2 py-1 text-xs rounded border hover:bg-accent transition-colors {formData.host === preset.host
						? 'border-primary bg-primary/10'
						: 'border-border'}"
					onclick={() => (formData.host = preset.host)}
				>
					{preset.label}
				</button>
			{/each}
		</div>

		<div class="grid gap-4">
			<div class="grid gap-2">
				<Label for="host">{m.connection_dialog_label_host()}</Label>
				<div class="relative">
					<ServerIcon class="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
					<Input
						id="host"
						bind:value={formData.host}
						placeholder={m.connection_dialog_placeholder_host()}
						class="pl-9"
					/>
				</div>
			</div>

			<div class="grid gap-2">
				<Label for="port">{m.connection_dialog_label_port()}</Label>
				<Input
					id="port"
					type="number"
					bind:value={formData.port}
					placeholder={String(selectedDbType?.defaultPort || 5432)}
				/>
				{#if selectedDbType}
					<p class="text-xs text-muted-foreground">
						{m.wizard_host_default_port({ port: selectedDbType.defaultPort })}
					</p>
				{/if}
			</div>
		</div>
	</div>
</div>
