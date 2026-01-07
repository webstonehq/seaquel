<script lang="ts">
	import {
		Dialog,
		DialogContent,
		DialogDescription,
		DialogFooter,
		DialogHeader,
		DialogTitle,
	} from "$lib/components/ui/dialog";
	import { Button } from "$lib/components/ui/button";
	import { m } from "$lib/paraglide/messages.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { connectionWizardStore } from "$lib/stores/connection-wizard.svelte.js";
	import { onboardingStore } from "$lib/stores/onboarding.svelte.js";
	import { toast } from "svelte-sonner";
	import ArrowLeftIcon from "@lucide/svelte/icons/arrow-left";
	import ArrowRightIcon from "@lucide/svelte/icons/arrow-right";

	import WizardProgress from "./wizard-progress.svelte";
	import WizardStepStringChoice from "./wizard-step-string-choice.svelte";
	import WizardStepStringPaste from "./wizard-step-string-paste.svelte";
	import WizardStepType from "./wizard-step-type.svelte";
	import WizardStepHost from "./wizard-step-host.svelte";
	import WizardStepCredentials from "./wizard-step-credentials.svelte";
	import WizardStepAdvanced from "./wizard-step-advanced.svelte";

	const db = useDatabase();
	const wizard = connectionWizardStore;

	const extractErrorMessage = (error: unknown): string => {
		if (error instanceof Error) {
			return error.message;
		}
		if (typeof error === "string") {
			const match = error.match(/error returned from database: (.+)/);
			if (match) {
				return match[1];
			}
			return error;
		}
		return "An unknown error occurred";
	};

	const handleTestConnection = async () => {
		wizard.clearError();

		if (!wizard.formData.name.trim()) {
			wizard.setError(m.connection_dialog_error_name_required());
			return;
		}

		if (wizard.formData.type !== "sqlite" && !wizard.formData.host.trim()) {
			wizard.setError(m.connection_dialog_error_host_required());
			return;
		}

		if (!wizard.formData.databaseName.trim()) {
			wizard.setError(m.connection_dialog_error_database_required());
			return;
		}

		wizard.isTesting = true;
		try {
			const connectionData = wizard.getConnectionData();
			await db.connections.test(connectionData);
			toast.success(m.wizard_test_success());
		} catch (error) {
			wizard.setError(extractErrorMessage(error));
		} finally {
			wizard.isTesting = false;
		}
	};

	const handleConnect = async () => {
		wizard.clearError();

		// Validate required fields
		if (!wizard.formData.name.trim()) {
			wizard.setError(m.connection_dialog_error_name_required());
			return;
		}

		if (wizard.formData.type !== "sqlite" && !wizard.formData.host.trim()) {
			wizard.setError(m.connection_dialog_error_host_required());
			return;
		}

		if (!wizard.formData.databaseName.trim()) {
			wizard.setError(m.connection_dialog_error_database_required());
			return;
		}

		wizard.isConnecting = true;
		try {
			const connectionData = wizard.getConnectionData();

			if (wizard.reconnectingConnectionId) {
				await db.connections.reconnect(wizard.reconnectingConnectionId, connectionData);
			} else {
				await db.connections.add(connectionData);
			}

			// Mark onboarding as complete
			onboardingStore.completeWizard();

			// Show toast and close wizard
			if (db.state.activeSchema.length === 0) {
				toast.warning(m.wizard_connect_empty());
			} else {
				toast.success(m.wizard_connect_success());
			}
			wizard.close();
		} catch (error) {
			wizard.setError(extractErrorMessage(error));
		} finally {
			wizard.isConnecting = false;
		}
	};

	const handleParse = (connStr: string): boolean => {
		return wizard.parseConnectionString(connStr);
	};

	// Determine if we should show navigation buttons
	const showBack = $derived(wizard.currentStep !== "string-choice");
	const showNext = $derived(
		wizard.currentStep !== "string-choice" &&
			wizard.currentStep !== "credentials" &&
			wizard.currentStep !== "string-paste",
	);
	const showConnect = $derived(
		wizard.currentStep === "credentials" ||
			wizard.currentStep === "advanced" ||
			(wizard.currentStep === "string-paste" && wizard.formData.name.trim()),
	);
	const showSkipAdvanced = $derived(wizard.currentStep === "credentials");

	const isReconnecting = $derived(wizard.reconnectingConnectionId !== null);
</script>

<Dialog bind:open={wizard.isOpen}>
	<DialogContent class="max-w-md max-h-[90vh] overflow-y-auto">
		<DialogHeader>
			<DialogTitle>
				{#if isReconnecting}
					{m.connection_dialog_title_reconnect()}
				{:else}
					{m.wizard_dialog_title()}
				{/if}
			</DialogTitle>
			{#if wizard.currentStep !== "string-choice"}
				<div class="pt-2">
					<WizardProgress currentStep={wizard.stepNumber} totalSteps={wizard.totalSteps} />
				</div>
			{/if}
		</DialogHeader>

		<!-- Step Content -->
		<div class="min-h-[300px]">
			{#if wizard.currentStep === "string-choice"}
				<WizardStepStringChoice
					onChooseString={() => wizard.chooseConnectionString()}
					onChooseManual={() => wizard.chooseManual()}
				/>
			{:else if wizard.currentStep === "string-paste"}
				<WizardStepStringPaste
					bind:formData={wizard.formData}
					selectedDbType={wizard.selectedDbType}
					onParse={handleParse}
					error={wizard.connectionError}
				/>
			{:else if wizard.currentStep === "type"}
				<WizardStepType
					selectedType={wizard.formData.type}
					onSelect={(type) => wizard.setDatabaseType(type)}
				/>
			{:else if wizard.currentStep === "host"}
				<WizardStepHost bind:formData={wizard.formData} selectedDbType={wizard.selectedDbType} />
			{:else if wizard.currentStep === "credentials"}
				<WizardStepCredentials
					bind:formData={wizard.formData}
					selectedDbType={wizard.selectedDbType}
					{isReconnecting}
					isTesting={wizard.isTesting}
					onTest={handleTestConnection}
					error={wizard.connectionError}
				/>
			{:else if wizard.currentStep === "advanced"}
				<WizardStepAdvanced
					bind:formData={wizard.formData}
					selectedDbType={wizard.selectedDbType}
					{isReconnecting}
				/>
			{/if}
		</div>

		<!-- Footer -->
		{#if wizard.currentStep !== "string-choice"}
			<DialogFooter class="flex-row justify-between gap-2">
				<div>
					{#if showBack}
						<Button
							variant="ghost"
							onclick={() => wizard.prevStep()}
							disabled={wizard.isConnecting}
						>
							<ArrowLeftIcon class="size-4 me-2" />
							{m.wizard_back()}
						</Button>
					{/if}
				</div>

				<div class="flex gap-2">
					{#if showSkipAdvanced}
						<Button
							variant="outline"
							onclick={() => wizard.nextStep()}
							disabled={wizard.isConnecting}
						>
							{m.wizard_advanced_options()}
						</Button>
					{/if}

					{#if showNext}
						<Button
							onclick={() => wizard.nextStep()}
							disabled={!wizard.canProceed || wizard.isConnecting}
						>
							{m.wizard_next()}
							<ArrowRightIcon class="size-4 ms-2" />
						</Button>
					{/if}

					{#if showConnect}
						<Button
							onclick={handleConnect}
							disabled={!wizard.canProceed || wizard.isConnecting || wizard.isTesting}
						>
							{#if wizard.isConnecting}
								{m.connection_dialog_button_connecting()}
							{:else if isReconnecting}
								{m.connection_dialog_button_reconnect()}
							{:else}
								{m.wizard_connect()}
							{/if}
						</Button>
					{/if}
				</div>
			</DialogFooter>
		{/if}
	</DialogContent>
</Dialog>
