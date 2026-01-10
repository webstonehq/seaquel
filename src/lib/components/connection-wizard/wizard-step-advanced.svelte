<script lang="ts">
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { Button } from "$lib/components/ui/button";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
	} from "$lib/components/ui/select";
	import { m } from "$lib/paraglide/messages.js";
	import type { WizardFormData, DatabaseTypeConfig } from "$lib/stores/connection-wizard.svelte.js";
	import type { SSHAuthMethod } from "$lib/types";
	import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
	import { getKeyringService } from "$lib/services/keyring";
	import ShieldIcon from "@lucide/svelte/icons/shield";
	import NetworkIcon from "@lucide/svelte/icons/network";
	import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";

	const keyring = getKeyringService();
	const keychainAvailable = keyring.isAvailable();

	interface Props {
		formData: WizardFormData;
		selectedDbType: DatabaseTypeConfig | undefined;
		isReconnecting: boolean;
	}

	let { formData = $bindable(), selectedDbType, isReconnecting }: Props = $props();

	const sslModes = ["disable", "allow", "prefer", "require"];

	const supportsSSL =
		formData.type === "postgres" || formData.type === "mysql" || formData.type === "mariadb";

	const selectSshKeyFile = async () => {
		try {
			const selected = await openFileDialog({
				multiple: false,
				title: m.wizard_advanced_select_key(),
			});
			if (selected && typeof selected === "string") {
				formData.sshKeyPath = selected;
			}
		} catch (error) {
			console.error("Failed to select file:", error);
		}
	};

	let sslExpanded = $state(formData.sslMode !== "disable");
	let sshExpanded = $state(formData.sshEnabled);
</script>

<div class="flex flex-col gap-6 py-4">
	<div class="space-y-2 text-center">
		<h2 class="text-lg font-semibold">{m.wizard_advanced_title()}</h2>
		<p class="text-sm text-muted-foreground">
			{m.wizard_advanced_description()}
		</p>
	</div>

	<div class="space-y-4">
		<!-- SSL Section -->
		{#if supportsSSL}
			<div class="border rounded-lg">
				<button
					type="button"
					class="w-full flex items-center justify-between p-4 text-left hover:bg-accent/50 transition-colors"
					onclick={() => (sslExpanded = !sslExpanded)}
				>
					<div class="flex items-center gap-3">
						<ShieldIcon class="size-5 text-muted-foreground" />
						<div>
							<h3 class="font-medium text-sm">{m.wizard_advanced_ssl_title()}</h3>
							<p class="text-xs text-muted-foreground">{m.wizard_advanced_ssl_description()}</p>
						</div>
					</div>
					<ChevronDownIcon
						class="size-5 text-muted-foreground transition-transform {sslExpanded
							? 'rotate-180'
							: ''}"
					/>
				</button>

				{#if sslExpanded}
					<div class="p-4 pt-0 border-t">
						<div class="grid gap-2">
							<Label for="sslmode">{m.connection_dialog_label_ssl_mode()}</Label>
							<Select
								type="single"
								value={formData.sslMode}
								onValueChange={(value) => (formData.sslMode = value)}
							>
								<SelectTrigger id="sslmode" class="w-full">
									{formData.sslMode}
								</SelectTrigger>
								<SelectContent>
									{#each sslModes as mode}
										<SelectItem value={mode}>{mode}</SelectItem>
									{/each}
								</SelectContent>
							</Select>
						</div>
					</div>
				{/if}
			</div>
		{/if}

		<!-- SSH Tunnel Section -->
		{#if formData.type !== "sqlite"}
			<div class="border rounded-lg">
				<button
					type="button"
					class="w-full flex items-center justify-between p-4 text-left hover:bg-accent/50 transition-colors"
					onclick={() => {
						sshExpanded = !sshExpanded;
						formData.sshEnabled = sshExpanded;
					}}
				>
					<div class="flex items-center gap-3">
						<NetworkIcon class="size-5 text-muted-foreground" />
						<div>
							<h3 class="font-medium text-sm">{m.connection_dialog_label_ssh_tunnel()}</h3>
							<p class="text-xs text-muted-foreground">{m.wizard_advanced_ssh_description()}</p>
						</div>
					</div>
					<div class="flex items-center gap-2">
						{#if formData.sshEnabled}
							<span class="text-xs text-primary">{m.wizard_advanced_enabled()}</span>
						{/if}
						<ChevronDownIcon
							class="size-5 text-muted-foreground transition-transform {sshExpanded
								? 'rotate-180'
								: ''}"
						/>
					</div>
				</button>

				{#if sshExpanded}
					<div class="p-4 pt-0 border-t space-y-4">
						<div class="grid grid-cols-3 gap-2">
							<div class="col-span-2 grid gap-2">
								<Label for="ssh-host">{m.connection_dialog_label_ssh_host()}</Label>
								<Input
									id="ssh-host"
									bind:value={formData.sshHost}
									placeholder={m.connection_dialog_placeholder_ssh_host()}
								/>
							</div>
							<div class="grid gap-2">
								<Label for="ssh-port">{m.connection_dialog_label_ssh_port()}</Label>
								<Input id="ssh-port" type="number" bind:value={formData.sshPort} />
							</div>
						</div>

						<div class="grid gap-2">
							<Label for="ssh-username">{m.connection_dialog_label_ssh_username()}</Label>
							<Input
								id="ssh-username"
								bind:value={formData.sshUsername}
								placeholder={m.connection_dialog_placeholder_ssh_username()}
							/>
						</div>

						<div class="grid gap-2">
							<Label>{m.connection_dialog_label_auth_method()}</Label>
							<Select
								type="single"
								value={formData.sshAuthMethod}
								onValueChange={(value) => (formData.sshAuthMethod = value as SSHAuthMethod)}
							>
								<SelectTrigger class="w-full">
									{formData.sshAuthMethod === "password"
										? m.connection_dialog_auth_method_password()
										: m.connection_dialog_auth_method_ssh_key()}
								</SelectTrigger>
								<SelectContent>
									<SelectItem value="password"
										>{m.connection_dialog_auth_method_password()}</SelectItem
									>
									<SelectItem value="key">{m.connection_dialog_auth_method_ssh_key()}</SelectItem>
								</SelectContent>
							</Select>
						</div>

						{#if formData.sshAuthMethod === "password"}
							<div class="grid gap-2">
								<Label for="ssh-password">{m.connection_dialog_label_ssh_password()}</Label>
								<Input
									id="ssh-password"
									type="password"
									bind:value={formData.sshPassword}
									placeholder={m.connection_dialog_placeholder_ssh_password()}
								/>
								{#if keychainAvailable}
									<div class="flex items-center gap-2 mt-1">
										<Checkbox
											id="save-ssh-password"
											checked={formData.saveSshPassword}
											onCheckedChange={(checked) => formData.saveSshPassword = !!checked}
										/>
										<Label for="save-ssh-password" class="text-xs font-normal cursor-pointer">
											{m.connection_dialog_save_ssh_password()}
										</Label>
									</div>
								{/if}
							</div>
						{:else}
							<div class="grid gap-2">
								<Label for="ssh-key-path">{m.connection_dialog_label_ssh_key_file()}</Label>
								<div class="flex gap-2">
									<Input
										id="ssh-key-path"
										bind:value={formData.sshKeyPath}
										placeholder={m.connection_dialog_placeholder_ssh_key_path()}
										class="flex-1"
									/>
									<Button variant="outline" type="button" onclick={selectSshKeyFile}>
										{m.connection_dialog_button_browse()}
									</Button>
								</div>
							</div>
							<div class="grid gap-2">
								<Label for="ssh-key-passphrase">{m.connection_dialog_label_key_passphrase()}</Label>
								<Input
									id="ssh-key-passphrase"
									type="password"
									bind:value={formData.sshKeyPassphrase}
									placeholder={m.connection_dialog_placeholder_optional()}
								/>
								{#if keychainAvailable}
									<div class="flex items-center gap-2 mt-1">
										<Checkbox
											id="save-ssh-passphrase"
											checked={formData.saveSshKeyPassphrase}
											onCheckedChange={(checked) => formData.saveSshKeyPassphrase = !!checked}
										/>
										<Label for="save-ssh-passphrase" class="text-xs font-normal cursor-pointer">
											{m.connection_dialog_save_ssh_passphrase()}
										</Label>
									</div>
								{/if}
							</div>
						{/if}

						{#if isReconnecting && (!formData.sshPassword && !formData.sshKeyPassphrase)}
							<p class="text-xs text-amber-600 dark:text-amber-500">
								{m.connection_dialog_warning_ssh()}
							</p>
						{/if}
					</div>
				{/if}
			</div>
		{/if}

		{#if !supportsSSL && formData.type === "sqlite"}
			<div class="text-center py-4 text-sm text-muted-foreground">
				{m.wizard_advanced_sqlite_none()}
			</div>
		{/if}
	</div>
</div>
