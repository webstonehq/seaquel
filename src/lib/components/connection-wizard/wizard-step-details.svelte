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
	import { Switch } from "$lib/components/ui/switch";
	import { aiSettingsStore } from "$lib/stores/ai-settings.svelte.js";
	import CopyIcon from "@lucide/svelte/icons/copy";
	import ServerIcon from "@lucide/svelte/icons/server";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import UserIcon from "@lucide/svelte/icons/user";
	import KeyIcon from "@lucide/svelte/icons/key";
	import FolderIcon from "@lucide/svelte/icons/folder";
	import ShieldIcon from "@lucide/svelte/icons/shield";
	import NetworkIcon from "@lucide/svelte/icons/network";
	import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
	import SparklesIcon from "@lucide/svelte/icons/sparkles";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import { getUsername } from "$lib/api/tauri";
	import { isTauri } from "$lib/utils/environment";
	import { isFeatureEnabled } from "$lib/features";

	const keyring = getKeyringService();
	const keychainAvailable = keyring.isAvailable();

	let osUsername = $state("");

	if (isTauri()) {
		getUsername().then((name) => { osUsername = name; });
	}

	interface Props {
		formData: WizardFormData;
		selectedDbType: DatabaseTypeConfig | undefined;
		isReconnecting: boolean;
		isEditing: boolean;
		isTesting: boolean;
		onTest: () => void;
		error: string | null;
	}

	let { formData = $bindable(), selectedDbType, isReconnecting, isEditing, isTesting, onTest, error }: Props =
		$props();

	const isFileBasedDb = $derived(formData.type === "sqlite" || formData.type === "duckdb");

	// Track whether the user has manually edited the connection name field.
	let nameManuallyEdited = $state(formData.name !== "");

	// Quick host presets
	const quickPresets = [
		{ label: "localhost", host: "localhost" },
		{ label: "127.0.0.1", host: "127.0.0.1" },
		{ label: "Docker", host: "host.docker.internal" },
	];

	const sslModes = ["disable", "allow", "prefer", "require"];

	const supportsSSL = $derived(
		formData.type === "postgres" || formData.type === "mysql" || formData.type === "mariadb" || formData.type === "mssql",
	);

	let advancedExpanded = $state(formData.sslMode !== "disable" || formData.sshEnabled);
	let sshExpanded = $state(formData.sshEnabled);
	let aiPrivacyExpanded = $state(formData.aiShareSchema !== undefined || formData.aiShareData !== undefined);

	const selectDatabaseFile = async () => {
		try {
			const isDuckDB = formData.type === "duckdb";
			const filters = isDuckDB
				? [{ name: "DuckDB Database", extensions: ["duckdb", "db"] }]
				: [{ name: "SQLite Database", extensions: ["db", "sqlite", "sqlite3"] }];

			const selected = await openFileDialog({
				multiple: false,
				title: m.wizard_credentials_select_database(),
				filters,
			});
			if (selected && typeof selected === "string") {
				formData.databaseName = selected;
				if (!nameManuallyEdited) {
					const fileName = selected.split("/").pop() || "database";
					formData.name = isDuckDB ? `DuckDB - ${fileName}` : `SQLite - ${fileName}`;
				}
			}
		} catch (error) {
			console.error("Failed to select file:", error);
		}
	};

	const useInMemoryDatabase = () => {
		formData.databaseName = ":memory:";
		if (!nameManuallyEdited) {
			formData.name = "DuckDB - In-Memory";
		}
	};

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

	// Auto-generate connection name from database name
	$effect(() => {
		if (formData.databaseName && !formData.name && !isReconnecting && !nameManuallyEdited) {
			formData.name = formData.databaseName;
		}
	});

</script>

<div class="flex flex-col gap-6 py-4">
	<div class="space-y-2 text-center">
		<h2 class="text-lg font-semibold">
			{#if isEditing}
				{m.wizard_dialog_title_edit()}
			{:else}
				{m.wizard_details_title()}
			{/if}
		</h2>
		<p class="text-sm text-muted-foreground">
			{m.wizard_details_description()}
		</p>
	</div>

	<div class="space-y-4">
		<!-- Connection name -->
		<div class="grid gap-2">
			<Label for="name">{m.connection_dialog_label_connection_name()}</Label>
			<Input
				id="name"
				bind:value={formData.name}
				oninput={() => nameManuallyEdited = true}
				placeholder={m.connection_dialog_placeholder_connection_name()}
			/>
			<p class="text-xs text-muted-foreground">{m.wizard_credentials_name_hint()}</p>
		</div>

		{#if isFileBasedDb}
			<!-- File-based DB: file path -->
			<div class="grid gap-2">
				<Label for="database">{m.connection_dialog_label_database()}</Label>
				<div class="flex gap-2">
					<div class="relative flex-1">
						<FolderIcon
							class="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-muted-foreground"
						/>
						<Input
							id="database"
							bind:value={formData.databaseName}
							placeholder={formData.type === "duckdb" ? "/path/to/database.duckdb or :memory:" : m.connection_dialog_placeholder_database_path()}
							class="pl-9"
						/>
					</div>
					<Button variant="outline" type="button" onclick={selectDatabaseFile}>
						{m.connection_dialog_button_browse()}
					</Button>
				</div>
				{#if formData.type === "duckdb"}
					<button
						type="button"
						class="text-xs text-primary hover:underline text-left"
						onclick={useInMemoryDatabase}
					>
						Use in-memory database
					</button>
				{/if}
			</div>
		{:else}
			<!-- Network DB: host + port -->
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

			<div class="grid grid-cols-3 gap-3">
				<div class="col-span-2 grid gap-2">
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
				</div>
			</div>

			<!-- Database name -->
			<div class="grid gap-2">
				<Label for="database">{m.connection_dialog_label_database()}</Label>
				<div class="relative">
					<DatabaseIcon
						class="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-muted-foreground"
					/>
					<Input
						id="database"
						bind:value={formData.databaseName}
						placeholder={m.connection_dialog_placeholder_database_name()}
						class="pl-9"
					/>
				</div>
			</div>

			<!-- Username -->
			<div class="grid gap-2">
				<Label for="username">{m.connection_dialog_label_username()}</Label>
				<div class="relative">
					<UserIcon class="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
					<Input id="username" bind:value={formData.username} placeholder={osUsername} class="pl-9" />
				</div>
			</div>

			<!-- Password -->
			<div class="grid gap-2">
				<Label for="password">{m.connection_dialog_label_password()}</Label>
				<div class="relative">
					<KeyIcon class="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
					<Input
						id="password"
						type="password"
						bind:value={formData.password}
						placeholder={isReconnecting
							? m.connection_dialog_placeholder_password_reconnect()
							: m.connection_dialog_placeholder_password()}
						class="pl-9"
					/>
				</div>
				{#if isReconnecting && !formData.password}
					<p class="text-xs text-amber-600 dark:text-amber-500">
						{m.connection_dialog_warning_password()}
					</p>
				{/if}
				{#if keychainAvailable}
					<div class="flex items-center gap-2 mt-1">
						<Checkbox
							id="save-password"
							checked={formData.savePassword}
							onCheckedChange={(checked) => formData.savePassword = !!checked}
						/>
						<Label for="save-password" class="text-xs font-normal cursor-pointer">
							{m.connection_dialog_save_password()}
						</Label>
					</div>
				{/if}
			</div>
		{/if}

		<!-- Advanced section (collapsible) -->
		{#if !isFileBasedDb || formData.type === "duckdb"}
			<div class="border rounded-lg">
				<button
					type="button"
					class="w-full flex items-center justify-between p-3 text-left hover:bg-accent/50 transition-colors rounded-lg"
					onclick={() => (advancedExpanded = !advancedExpanded)}
				>
					<span class="text-sm font-medium">{m.wizard_advanced_options()}</span>
					<ChevronDownIcon
						class="size-4 text-muted-foreground transition-transform {advancedExpanded
							? 'rotate-180'
							: ''}"
					/>
				</button>

				{#if advancedExpanded}
					<div class="px-3 pt-1 pb-3 space-y-4">
						<!-- SSL Mode -->
						{#if supportsSSL}
							<div class="grid gap-2">
								<Label for="sslmode" class="flex items-center gap-2">
									<ShieldIcon class="size-4 text-muted-foreground" />
									{m.connection_dialog_label_ssl_mode()}
								</Label>
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
						{/if}

						<!-- SSH Tunnel — Tauri-only runtime; hidden in web/demo (see
							 $lib/features/index.ts → sshTunnels). The whole subtree
							 calls `createSshTunnel` via `invoke()` which has no
							 implementation outside the Tauri shell. -->
						{#if formData.type !== "sqlite" && isFeatureEnabled("sshTunnels")}
							<div class="space-y-4">
								<div class="flex items-center justify-between">
									<Label class="flex items-center gap-2 cursor-pointer" for="ssh-toggle">
										<NetworkIcon class="size-4 text-muted-foreground" />
										{m.connection_dialog_label_ssh_tunnel()}
									</Label>
									<Checkbox
										id="ssh-toggle"
										checked={formData.sshEnabled}
										onCheckedChange={(checked) => {
											formData.sshEnabled = !!checked;
											sshExpanded = !!checked;
										}}
									/>
								</div>

								{#if sshExpanded}
									<div class="space-y-4">
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
					</div>
				{/if}
			</div>
		{/if}

		<!-- AI Privacy overrides -->
		<div class="border rounded-lg">
			<button
				type="button"
				class="w-full flex items-center justify-between p-3 text-left hover:bg-accent/50 transition-colors rounded-lg"
				onclick={() => (aiPrivacyExpanded = !aiPrivacyExpanded)}
			>
				<div class="flex items-center gap-2">
					<SparklesIcon class="size-4 text-muted-foreground" />
					<span class="text-sm font-medium">{m.wizard_ai_privacy()}</span>
					{#if formData.aiShareSchema !== undefined || formData.aiShareData !== undefined}
						<span class="size-1.5 rounded-full bg-primary inline-block"></span>
					{/if}
				</div>
				<ChevronDownIcon
					class="size-4 text-muted-foreground transition-transform {aiPrivacyExpanded ? 'rotate-180' : ''}"
				/>
			</button>

			{#if aiPrivacyExpanded}
				<div class="px-3 pt-1 pb-3 space-y-4">
					<p class="text-xs text-muted-foreground">
						{m.wizard_ai_privacy_description()}
					</p>

					<div class="flex items-start justify-between gap-4">
						<div class="flex-1">
							<p class="text-sm font-medium">{m.wizard_ai_share_schema()}</p>
							<p class="text-xs text-muted-foreground">{m.wizard_ai_share_schema_description()}</p>
							<div class="flex items-center gap-1.5 mt-1.5">
								<Checkbox
									id="ai-schema-use-default"
									checked={formData.aiShareSchema === undefined}
									onCheckedChange={(checked) => {
										formData.aiShareSchema = checked ? undefined : aiSettingsStore.settings.shareSchemaGlobally;
									}}
								/>
								<Label for="ai-schema-use-default" class="text-xs font-normal text-muted-foreground cursor-pointer">
									{m.wizard_ai_share_default()}
								</Label>
							</div>
						</div>
						<Switch
							checked={formData.aiShareSchema ?? aiSettingsStore.settings.shareSchemaGlobally}
							onCheckedChange={(checked) => (formData.aiShareSchema = checked)}
						/>
					</div>

					<div class="flex items-start justify-between gap-4">
						<div class="flex-1">
							<p class="text-sm font-medium">{m.wizard_ai_share_data()}</p>
							<p class="text-xs text-muted-foreground">{m.wizard_ai_share_data_description()}</p>
							<div class="flex items-center gap-1.5 mt-1.5">
								<Checkbox
									id="ai-data-use-default"
									checked={formData.aiShareData === undefined}
									onCheckedChange={(checked) => {
										formData.aiShareData = checked ? undefined : aiSettingsStore.settings.shareDataGlobally;
									}}
								/>
								<Label for="ai-data-use-default" class="text-xs font-normal text-muted-foreground cursor-pointer">
									{m.wizard_ai_share_default()}
								</Label>
							</div>
						</div>
						<Switch
							checked={formData.aiShareData ?? aiSettingsStore.settings.shareDataGlobally}
							onCheckedChange={(checked) => (formData.aiShareData = checked)}
						/>
					</div>
				</div>
			{/if}
		</div>

		<!-- Test connection button -->
		<Button variant="outline" class="w-full" onclick={onTest} disabled={isTesting}>
			{#if isTesting}
				{m.connection_dialog_button_testing()}
			{:else}
				{m.connection_dialog_button_test()}
			{/if}
		</Button>

		{#if error}
			<div
				class="flex items-start gap-2 p-3 rounded-lg border border-destructive/50 bg-destructive/10 text-destructive text-sm"
			>
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
