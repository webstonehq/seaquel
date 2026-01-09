<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { Select, SelectContent, SelectItem, SelectTrigger } from "$lib/components/ui/select";
	import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
	import { m } from "$lib/paraglide/messages.js";
	import type { SSHAuthMethod } from "$lib/types";

	type Props = {
		enabled: boolean;
		host: string;
		port: number;
		username: string;
		authMethod: SSHAuthMethod;
		password: string;
		keyPath: string;
		keyPassphrase: string;
		isReconnecting: boolean;
		onEnabledChange: (enabled: boolean) => void;
		onHostChange: (host: string) => void;
		onPortChange: (port: number) => void;
		onUsernameChange: (username: string) => void;
		onAuthMethodChange: (method: SSHAuthMethod) => void;
		onPasswordChange: (password: string) => void;
		onKeyPathChange: (path: string) => void;
		onKeyPassphraseChange: (passphrase: string) => void;
	};

	let {
		enabled,
		host,
		port,
		username,
		authMethod,
		password,
		keyPath,
		keyPassphrase,
		isReconnecting,
		onEnabledChange,
		onHostChange,
		onPortChange,
		onUsernameChange,
		onAuthMethodChange,
		onPasswordChange,
		onKeyPathChange,
		onKeyPassphraseChange
	}: Props = $props();

	const selectSshKeyFile = async () => {
		try {
			const selected = await openFileDialog({
				multiple: false,
				title: "Select SSH Key File"
			});
			if (selected && typeof selected === "string") {
				onKeyPathChange(selected);
			}
		} catch (error) {
			console.error("Failed to select file:", error);
		}
	};
</script>

<div class="border-t pt-4 mt-2">
	<div class="flex items-center gap-3 mb-4">
		<input
			type="checkbox"
			id="ssh-enabled"
			checked={enabled}
			onchange={(e) => onEnabledChange(e.currentTarget.checked)}
			class="h-4 w-4 rounded border-gray-300"
		/>
		<Label for="ssh-enabled" class="cursor-pointer">{m.connection_dialog_label_ssh_tunnel()}</Label>
	</div>

	{#if enabled}
		<div class="flex flex-col gap-4">
			<div class="grid grid-cols-3 gap-2">
				<div class="col-span-2 grid gap-2">
					<Label for="ssh-host">{m.connection_dialog_label_ssh_host()}</Label>
					<Input
						id="ssh-host"
						value={host}
						oninput={(e) => onHostChange(e.currentTarget.value)}
						placeholder={m.connection_dialog_placeholder_ssh_host()}
					/>
				</div>
				<div class="grid gap-2">
					<Label for="ssh-port">{m.connection_dialog_label_ssh_port()}</Label>
					<Input
						id="ssh-port"
						type="number"
						value={port}
						oninput={(e) => onPortChange(parseInt(e.currentTarget.value) || 22)}
					/>
				</div>
			</div>

			<div class="grid gap-2">
				<Label for="ssh-username">{m.connection_dialog_label_ssh_username()}</Label>
				<Input
					id="ssh-username"
					value={username}
					oninput={(e) => onUsernameChange(e.currentTarget.value)}
					placeholder={m.connection_dialog_placeholder_ssh_username()}
				/>
			</div>

			<div class="grid gap-2">
				<Label>{m.connection_dialog_label_auth_method()}</Label>
				<Select
					type="single"
					value={authMethod}
					onValueChange={(value) => onAuthMethodChange(value as SSHAuthMethod)}
				>
					<SelectTrigger class="w-full">
						{authMethod === "password"
							? m.connection_dialog_auth_method_password()
							: m.connection_dialog_auth_method_ssh_key()}
					</SelectTrigger>
					<SelectContent>
						<SelectItem value="password">{m.connection_dialog_auth_method_password()}</SelectItem>
						<SelectItem value="key">{m.connection_dialog_auth_method_ssh_key()}</SelectItem>
					</SelectContent>
				</Select>
			</div>

			{#if authMethod === "password"}
				<div class="grid gap-2">
					<Label for="ssh-password">{m.connection_dialog_label_ssh_password()}</Label>
					<Input
						id="ssh-password"
						type="password"
						value={password}
						oninput={(e) => onPasswordChange(e.currentTarget.value)}
						placeholder={m.connection_dialog_placeholder_ssh_password()}
					/>
				</div>
			{:else}
				<div class="grid gap-2">
					<Label for="ssh-key-path">{m.connection_dialog_label_ssh_key_file()}</Label>
					<div class="flex gap-2">
						<Input
							id="ssh-key-path"
							value={keyPath}
							oninput={(e) => onKeyPathChange(e.currentTarget.value)}
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
						value={keyPassphrase}
						oninput={(e) => onKeyPassphraseChange(e.currentTarget.value)}
						placeholder={m.connection_dialog_placeholder_optional()}
					/>
				</div>
			{/if}

			{#if isReconnecting && enabled}
				<p class="text-xs text-amber-600 dark:text-amber-500">
					⚠ {m.connection_dialog_warning_ssh()}
				</p>
			{/if}
		</div>
	{/if}
</div>
