<script lang="ts">
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { Button } from "$lib/components/ui/button";
	import { m } from "$lib/paraglide/messages.js";
	import type { WizardFormData, DatabaseTypeConfig } from "$lib/stores/connection-wizard.svelte.js";
	import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import UserIcon from "@lucide/svelte/icons/user";
	import KeyIcon from "@lucide/svelte/icons/key";
	import FolderIcon from "@lucide/svelte/icons/folder";
	import CopyIcon from "@lucide/svelte/icons/copy";
	import { toast } from "svelte-sonner";

	interface Props {
		formData: WizardFormData;
		selectedDbType: DatabaseTypeConfig | undefined;
		isReconnecting: boolean;
		isTesting: boolean;
		onTest: () => void;
		error: string | null;
	}

	let { formData = $bindable(), selectedDbType, isReconnecting, isTesting, onTest, error }: Props =
		$props();

	const selectDatabaseFile = async () => {
		try {
			const selected = await openFileDialog({
				multiple: false,
				title: m.wizard_credentials_select_database(),
				filters: [{ name: "SQLite Database", extensions: ["db", "sqlite", "sqlite3"] }],
			});
			if (selected && typeof selected === "string") {
				formData.databaseName = selected;
				// Auto-generate name from file
				if (!formData.name) {
					formData.name = `SQLite - ${selected.split("/").pop() || "database"}`;
				}
			}
		} catch (error) {
			console.error("Failed to select file:", error);
		}
	};

	// Auto-generate connection name from database name
	$effect(() => {
		if (formData.databaseName && !formData.name && !isReconnecting) {
			formData.name = formData.databaseName;
		}
	});
</script>

<div class="flex flex-col gap-6 py-4">
	<div class="space-y-2 text-center">
		<h2 class="text-lg font-semibold">{m.wizard_credentials_title()}</h2>
		<p class="text-sm text-muted-foreground">
			{m.wizard_credentials_description()}
		</p>
	</div>

	<div class="space-y-4">
		<!-- Database name / path -->
		<div class="grid gap-2">
			<Label for="database">{m.connection_dialog_label_database()}</Label>
			{#if formData.type === "sqlite"}
				<div class="flex gap-2">
					<div class="relative flex-1">
						<FolderIcon
							class="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-muted-foreground"
						/>
						<Input
							id="database"
							bind:value={formData.databaseName}
							placeholder={m.connection_dialog_placeholder_database_path()}
							class="pl-9"
						/>
					</div>
					<Button variant="outline" type="button" onclick={selectDatabaseFile}>
						{m.connection_dialog_button_browse()}
					</Button>
				</div>
			{:else}
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
			{/if}
		</div>

		{#if formData.type !== "sqlite"}
			<!-- Username -->
			<div class="grid gap-2">
				<Label for="username">{m.connection_dialog_label_username()}</Label>
				<div class="relative">
					<UserIcon class="absolute left-3 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
					<Input id="username" bind:value={formData.username} placeholder="postgres" class="pl-9" />
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
				{#if isReconnecting}
					<p class="text-xs text-amber-600 dark:text-amber-500">
						{m.connection_dialog_warning_password()}
					</p>
				{/if}
			</div>
		{/if}

		<!-- Connection name -->
		<div class="grid gap-2">
			<Label for="name">{m.connection_dialog_label_connection_name()}</Label>
			<Input
				id="name"
				bind:value={formData.name}
				placeholder={m.connection_dialog_placeholder_connection_name()}
			/>
			<p class="text-xs text-muted-foreground">{m.wizard_credentials_name_hint()}</p>
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
							toast.error(m.query_copy_failed());
						}
					}}
				>
					<CopyIcon class="size-3.5" />
				</Button>
			</div>
		{/if}
	</div>
</div>
