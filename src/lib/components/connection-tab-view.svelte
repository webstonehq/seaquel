<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { m } from "$lib/paraglide/messages.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { databaseTypes } from "$lib/stores/connection-wizard.svelte.js";
	import { onboardingStore } from "$lib/stores/onboarding.svelte.js";
	import { toast } from "svelte-sonner";
	import { extractErrorMessage } from "$lib/errors/types";
	import { isFileNotFoundError } from "$lib/providers/wire";
	import {
		getConnectionData,
		parseConnectionString,
		hasAllCredentials,
	} from "$lib/utils/connection-string.js";
	import ArrowLeftIcon from "@lucide/svelte/icons/arrow-left";
	import { Trash2Icon } from "@lucide/svelte";
	import DeleteConfirmDialog from "$lib/components/delete-confirm-dialog.svelte";

	import WizardStepMethod from "./connection-wizard/wizard-step-method.svelte";
	import WizardStepDetails from "./connection-wizard/wizard-step-details.svelte";

	import type { ConnectionTab, ConnectionFormData } from "$lib/types";
	import type { WizardFormData } from "$lib/stores/connection-wizard.svelte.js";
	import type { DatabaseType } from "$lib/types";

	interface Props {
		tab: ConnectionTab;
	}

	let { tab }: Props = $props();

	const db = useDatabase();

	// Local reactive copy of formData for two-way binding with wizard step components.
	// We use $derived.by for initial values that come from the tab prop, then track locally.
	let formDataInitialized = $state(false);
	let formData = $state<WizardFormData>({
		name: "",
		type: "postgres",
		host: "localhost",
		port: 5432,
		databaseName: "",
		username: "",
		password: "",
		sslMode: "disable",
		connectionString: "",
		sshEnabled: false,
		sshHost: "",
		sshPort: 22,
		sshUsername: "",
		sshAuthMethod: "password",
		sshPassword: "",
		sshKeyPath: "",
		sshKeyPassphrase: "",
		savePassword: true,
		saveSshPassword: true,
		saveSshKeyPassphrase: true,
		aiShareSchema: undefined,
		aiShareData: undefined,
	});
	let currentStep = $state<"method" | "details">("method");
	let isConnecting = $state(false);
	let isTesting = $state(false);
	let connectionError = $state<string | null>(null);

	// Initialize local state from the tab prop (runs once since formDataInitialized gates it).
	$effect(() => {
		if (!formDataInitialized) {
			formData = { ...tab.formData } as WizardFormData;
			currentStep = tab.currentStep;
			connectionError = tab.error;
			formDataInitialized = true;
		}
	});

	// When credentials are loaded from keyring, sync non-empty values to local state.
	// This must run only once — otherwise editing a synced field (e.g. deleting a
	// character in the password) would be immediately overwritten by the tab's value.
	let credentialsSynced = $state(false);
	$effect(() => {
		if (!tab.credentialsLoaded || credentialsSynced) return;
		if (tab.formData.password) {
			formData.password = tab.formData.password;
		}
		if (tab.formData.sshPassword) {
			formData.sshPassword = tab.formData.sshPassword;
		}
		if (tab.formData.sshKeyPassphrase) {
			formData.sshKeyPassphrase = tab.formData.sshKeyPassphrase;
		}
		credentialsSynced = true;
	});

	// Derived: selected database type config
	const selectedDbType = $derived(databaseTypes.find((t) => t.value === formData.type));

	// Derived: can proceed to connect
	const canProceed = $derived(
		formData.databaseName.trim().length > 0 && formData.name.trim().length > 0,
	);

	const isReconnecting = $derived(tab.connectionId !== null && tab.mode !== "edit");
	const isEditing = $derived(tab.mode === "edit");

	const showBack = $derived(
		currentStep === "details" && !isReconnecting && !isEditing,
	);

	// Auto-connect when credentials are loaded in reconnect mode
	$effect(() => {
		if (
			tab.mode === "reconnect" &&
			tab.credentialsLoaded &&
			!isConnecting &&
			!connectionError
		) {
			// Check if all credentials are present using local formData
			if (hasAllCredentials(formData as ConnectionFormData)) {
				handleAutoConnect();
			}
		}
	});

	const handleAutoConnect = async () => {
		isConnecting = true;
		// Capture tab identity before any await — the component may unmount
		// during reconnect() when the active pane tab switches,
		// which can invalidate the reactive $props() reference.
		const tabId = tab.id;
		const tabConnectionId = tab.connectionId;
		try {
			const connectionData = getConnectionData(formData as ConnectionFormData);
			if (tabConnectionId) {
				await db.connections.reconnect(tabConnectionId, connectionData);
			}
			// Mark onboarding as complete
			onboardingStore.completeWizard();
			// Show toast
			if (db.state.activeSchema.length === 0) {
				toast.warning(m.wizard_connect_empty());
			} else {
				toast.success(m.wizard_connect_success());
			}
			// Close the connection tab on success
			db.connectionTabs.remove(tabId);
		} catch (error) {
			// Auto-connect failed, show the error and let the user edit
			isConnecting = false;
			connectionError = extractErrorMessage(error);
		}
	};

	const validate = (): boolean => {
		connectionError = null;

		if (!formData.name.trim()) {
			connectionError = m.connection_dialog_error_name_required();
			return false;
		}

		if (formData.type !== "sqlite" && !formData.host.trim()) {
			connectionError = m.connection_dialog_error_host_required();
			return false;
		}

		if (!formData.databaseName.trim()) {
			connectionError = m.connection_dialog_error_database_required();
			return false;
		}

		return true;
	};

	const handleTestConnection = async () => {
		if (!validate()) return;

		isTesting = true;
		try {
			const connectionData = getConnectionData(formData as ConnectionFormData);
			await db.connections.test(connectionData);
			toast.success(m.wizard_test_success());
		} catch (error) {
			connectionError = extractErrorMessage(error);
		} finally {
			isTesting = false;
		}
	};

	// A SQLite path that doesn't exist fails with FILE_NOT_FOUND instead of
	// silently creating an empty database; offer to create it explicitly.
	const canCreateDatabase = $derived(
		formData.type === "sqlite" && tab.mode !== "edit" && isFileNotFoundError(connectionError),
	);

	const handleConnect = async ({ createIfMissing = false } = {}) => {
		if (!validate()) return;

		isConnecting = true;
		// Capture tab identity before any await — the component may unmount
		// during add()/reconnect() when the active pane tab switches,
		// which can invalidate the reactive $props() reference.
		const tabId = tab.id;
		const tabMode = tab.mode;
		const tabConnectionId = tab.connectionId;
		try {
			const connectionData = {
				...getConnectionData(formData as ConnectionFormData),
				createIfMissing,
			};

			if (tabMode === "edit" && tabConnectionId) {
				// Edit mode - just update settings without reconnecting
				await db.connections.update(tabConnectionId, connectionData);
				toast.success(m.wizard_edit_success());
			} else if (tabConnectionId) {
				await db.connections.reconnect(tabConnectionId, connectionData);
				// Mark onboarding as complete
				onboardingStore.completeWizard();
				// Show toast
				if (db.state.activeSchema.length === 0) {
					toast.warning(m.wizard_connect_empty());
				} else {
					toast.success(m.wizard_connect_success());
				}
			} else {
				await db.connections.add(connectionData);
				// Mark onboarding as complete
				onboardingStore.completeWizard();
				// Show toast
				if (db.state.activeSchema.length === 0) {
					toast.warning(m.wizard_connect_empty());
				} else {
					toast.success(m.wizard_connect_success());
				}
			}

			// Close the connection tab on success
			db.connectionTabs.remove(tabId);
		} catch (error) {
			connectionError = extractErrorMessage(error);
		} finally {
			isConnecting = false;
		}
	};

	const handleParse = (connStr: string): boolean => {
		const result = parseConnectionString(connStr);
		if (result.success) {
			Object.assign(formData, result.formData);
			connectionError = null;
			return true;
		} else {
			connectionError = result.error;
			return false;
		}
	};

	const selectDatabaseType = (type: DatabaseType) => {
		formData.type = type;
		const dbType = databaseTypes.find((t) => t.value === type);
		if (dbType) {
			formData.port = dbType.defaultPort;
		}
		currentStep = "details";
	};

	const goBack = () => {
		formData.connectionString = "";
		currentStep = "method";
	};

	let showDeleteConfirm = $state(false);

	const handleDeleteConnection = async () => {
		if (!tab.connectionId) return;
		await db.connections.remove(tab.connectionId);
		showDeleteConfirm = false;
		db.connectionTabs.remove(tab.id);
	};
</script>

<div class="flex-1 flex flex-col min-h-0">
	<div class="flex-1 overflow-y-auto">
		<div class="max-w-lg mx-auto py-8 px-4">
			<!-- Title -->
			<div class="mb-6">
				<h1 class="text-xl font-semibold text-center">
					{#if isEditing}
						{m.wizard_dialog_title_edit()}
					{:else if isReconnecting}
						{m.connection_dialog_title_reconnect()}
					{:else}
						{m.wizard_dialog_title()}
					{/if}
				</h1>
			</div>

			<!-- Step Content -->
			<div class="min-h-[300px]">
				{#if currentStep === "method"}
					<WizardStepMethod
						bind:formData
						onParse={handleParse}
						onSelectType={selectDatabaseType}
						onContinue={() => (currentStep = "details")}
						error={connectionError}
					/>
				{:else if currentStep === "details"}
					<WizardStepDetails
						bind:formData
						{selectedDbType}
						{isReconnecting}
						{isEditing}
						{isTesting}
						onTest={handleTestConnection}
						onCreateDatabase={canCreateDatabase
							? () => handleConnect({ createIfMissing: true })
							: undefined}
						error={connectionError}
					/>
				{/if}
			</div>

			<!-- Footer (details step only) -->
			{#if currentStep === "details"}
				<div class="flex justify-between gap-2 mt-6">
					<div>
						{#if showBack}
							<Button
								variant="ghost"
								onclick={goBack}
								disabled={isConnecting}
							>
								<ArrowLeftIcon class="size-4 me-2" />
								{m.wizard_back()}
							</Button>
						{:else if isEditing && tab.connectionId}
							<Button
								variant="ghost"
								class="text-destructive hover:text-destructive"
								onclick={() => { showDeleteConfirm = true; }}
								disabled={isConnecting}
							>
								<Trash2Icon class="size-4 me-1" />
								{m.header_delete_connection()}
							</Button>
						{/if}
					</div>

					<div class="flex gap-2">
						<Button
							onclick={() => handleConnect()}
							disabled={!canProceed || isConnecting || isTesting}
						>
							{#if isConnecting}
								{m.connection_dialog_button_connecting()}
							{:else if isEditing}
								{m.wizard_save()}
							{:else if isReconnecting}
								{m.connection_dialog_button_reconnect()}
							{:else}
								{m.wizard_connect()}
							{/if}
						</Button>
					</div>
				</div>
			{/if}
		</div>
	</div>
</div>

<!-- Delete Connection Confirmation -->
<DeleteConfirmDialog
	bind:open={showDeleteConfirm}
	title={m.header_delete_dialog_title()}
	description={m.header_delete_dialog_description({ name: formData.name })}
	cancelText={m.header_button_cancel()}
	confirmText={m.header_delete_connection()}
	onconfirm={handleDeleteConnection}
/>
