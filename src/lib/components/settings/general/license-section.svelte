<script lang="ts">
	import { onMount } from "svelte";
	import { m } from "$lib/paraglide/messages.js";
	import { Button } from "$lib/components/ui/button";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import { licenseStore } from "$lib/stores/license.svelte.js";
	import { tenantLicenseStore } from "$lib/stores/tenant-license.svelte.js";
	import { isTauri, isWeb } from "$lib/utils/environment";
	import CopyIcon from "@lucide/svelte/icons/copy";

	let showActivationInput = $state(false);
	let licenseKeyInput = $state("");
	let isRetrying = $state(false);

	// Web build: read-only, sourced from the control plane via
	// /api/account/tenant + /api/team. Tauri keeps the existing
	// keychain-backed activate/deactivate flow.
	onMount(() => {
		if (isWeb()) void tenantLicenseStore.refresh();
	});

	async function handleRetryValidation() {
		isRetrying = true;
		try {
			const success = await licenseStore.retryValidation();
			if (success) {
				toast.success(m.license_revalidated_success());
			} else {
				errorToast(m.license_revalidation_failed());
			}
		} finally {
			isRetrying = false;
		}
	}

	async function handleActivate() {
		if (!licenseKeyInput.trim()) return;
		const success = await licenseStore.activate(licenseKeyInput.trim());
		if (success) {
			toast.success(m.license_activate_success());
			licenseKeyInput = "";
			showActivationInput = false;
		}
	}

	async function handleDeactivate() {
		const success = await licenseStore.deactivate();
		if (success) {
			toast.success(m.license_deactivate_success());
		} else {
			errorToast(m.license_deactivate_failed());
		}
	}

	function openExternal(url: string) {
		if (isTauri()) {
			import("$lib/api/tauri").then(({ openPath }) => {
				openPath(url);
			});
		} else {
			window.open(url, "_blank");
		}
	}
</script>

<div class="space-y-6" data-section="license">
	<div>
		<h2 class="text-lg font-medium">{m.settings_license()}</h2>
		<p class="text-sm text-muted-foreground mt-1">
			{m.settings_license_description()}
		</p>
	</div>

	{#if isWeb()}
		<!-- Cloud / per-tenant container: license is implicit from
		     membership. No activate/deactivate controls — billing changes
		     happen at seaquel.app/dashboard, propagate here via webhook +
		     cache TTL. -->
		{#if !tenantLicenseStore.loaded}
			<p class="text-sm text-muted-foreground">Loading…</p>
		{:else if tenantLicenseStore.loadError}
			<p class="text-sm text-destructive">{tenantLicenseStore.loadError}</p>
		{:else}
			<div class="space-y-4 border rounded-lg p-4">
				<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
					<span class="text-muted-foreground">{m.license_status_label()}</span>
					<span>
						{#if tenantLicenseStore.status === "active"}
							<span
								class="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-200"
							>
								{m.license_status_active()}
							</span>
						{:else if tenantLicenseStore.status === "suspended"}
							<span
								class="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-destructive/10 text-destructive"
							>
								Suspended
							</span>
						{:else}
							<span class="text-muted-foreground">{tenantLicenseStore.status}</span>
						{/if}
					</span>
				</div>
				<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
					<span class="text-muted-foreground">{m.license_tier_label()}</span>
					<span class="font-medium">
						{#if tenantLicenseStore.tier === "individual"}
							{m.license_tier_individual()}
						{:else if tenantLicenseStore.tier === "business"}
							{m.license_tier_business()}
						{:else}
							{m.license_tier_personal()}
						{/if}
					</span>
				</div>
				{#if tenantLicenseStore.maskedLicenseKey}
					<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
						<span class="text-muted-foreground">{m.license_key_label()}</span>
						<span class="font-mono">{tenantLicenseStore.maskedLicenseKey}</span>
					</div>
				{/if}
				<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
					<span class="text-muted-foreground">Seats</span>
					<span>{tenantLicenseStore.memberCount} / {tenantLicenseStore.seatLimit}</span>
				</div>
				{#if tenantLicenseStore.currentPeriodEnd}
					<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
						<span class="text-muted-foreground">Renews</span>
						<span>
							{tenantLicenseStore.currentPeriodEnd.toLocaleDateString()}
						</span>
					</div>
				{/if}
				{#if tenantLicenseStore.role === "owner"}
					<div class="pt-2">
						<Button
							variant="outline"
							size="sm"
							href="https://seaquel.app/dashboard"
							target="_blank"
							rel="noopener"
						>
							Manage billing
						</Button>
					</div>
				{/if}
			</div>
		{/if}
	{:else if licenseStore.status === "active"}
		<div class="space-y-4 border rounded-lg p-4">
			<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
				<span class="text-muted-foreground">{m.license_status_label()}</span>
				<span>
					<span class="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-200">
						{m.license_status_active()}
					</span>
				</span>
			</div>
			<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
				<span class="text-muted-foreground">{m.license_tier_label()}</span>
				<span class="font-medium">
					{#if licenseStore.tier === "individual"}
						{m.license_tier_individual()}
					{:else if licenseStore.tier === "business"}
						{m.license_tier_business()}
					{:else}
						{m.license_tier_personal()}
					{/if}
				</span>
			</div>
			{#if licenseStore.maskedKey}
				<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
					<span class="text-muted-foreground">{m.license_key_label()}</span>
					<span class="font-mono">{licenseStore.maskedKey}</span>
				</div>
			{/if}
			{#if licenseStore.expiresAt}
				<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
					<span class="text-muted-foreground">{m.license_expires_at()}</span>
					<span>{new Date(licenseStore.expiresAt).toLocaleDateString()}</span>
				</div>
			{/if}
			{#if licenseStore.lastValidatedAt}
				<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
					<span class="text-muted-foreground">{m.license_last_validated()}</span>
					<span>{new Date(licenseStore.lastValidatedAt).toLocaleString()}</span>
				</div>
			{/if}
			<div class="pt-2">
				<Button variant="outline" size="sm" onclick={handleDeactivate}>
					{m.license_deactivate_button()}
				</Button>
			</div>
		</div>
	{:else if licenseStore.status === "expired" || licenseStore.status === "invalid"}
		<div class="space-y-4 border border-destructive/50 rounded-lg p-4">
			<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
				<span class="text-muted-foreground">{m.license_status_label()}</span>
				<span>
					<span class="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-destructive/10 text-destructive">
						{licenseStore.status === "expired" ? m.license_status_expired() : m.license_status_invalid()}
					</span>
				</span>
			</div>
			<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
				<span class="text-muted-foreground">{m.license_tier_label()}</span>
				<span class="font-medium">
					{#if licenseStore.tier === "individual"}
						{m.license_tier_individual()}
					{:else if licenseStore.tier === "business"}
						{m.license_tier_business()}
					{:else}
						{m.license_tier_personal()}
					{/if}
				</span>
			</div>
			{#if licenseStore.maskedKey}
				<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
					<span class="text-muted-foreground">{m.license_key_label()}</span>
					<span class="font-mono">{licenseStore.maskedKey}</span>
				</div>
			{/if}
			{#if licenseStore.expiresAt}
				<div class="grid grid-cols-[140px_1fr] gap-2 text-sm">
					<span class="text-muted-foreground">{m.license_expires_at()}</span>
					<span>{new Date(licenseStore.expiresAt).toLocaleDateString()}</span>
				</div>
			{/if}
			<p class="text-sm text-muted-foreground">
				{licenseStore.status === "expired" ? m.license_expired_message() : m.license_invalid_message()}
			</p>
			<div class="flex gap-2 pt-2">
				<Button variant="default" size="sm" onclick={handleRetryValidation} disabled={isRetrying}>
					{isRetrying ? m.license_retrying() : m.license_retry_validation()}
				</Button>
				<Button variant="outline" size="sm" onclick={() => showActivationInput = true}>
					{m.license_enter_new_key()}
				</Button>
				<Button variant="ghost" size="sm" onclick={handleDeactivate}>
					{m.license_deactivate_button()}
				</Button>
			</div>
			{#if showActivationInput}
				<div class="space-y-2 min-w-0 border-t pt-4">
					<input
						type="text"
						class="w-full px-3 py-2 border rounded-md bg-background text-sm font-mono"
						placeholder={m.license_key_placeholder()}
						bind:value={licenseKeyInput}
						onkeydown={(e) => e.key === "Enter" && handleActivate()}
					/>
					{#if licenseStore.activationError}
						<div class="flex items-start gap-1.5 text-sm text-destructive">
							<p class="break-all flex-1">{licenseStore.activationError}</p>
							<button
								class="shrink-0 p-0.5 rounded hover:bg-muted transition-colors cursor-pointer"
								title="Copy error"
								onclick={() => navigator.clipboard.writeText(licenseStore.activationError ?? "")}
							>
								<CopyIcon class="size-3.5" />
							</button>
						</div>
					{/if}
					<Button
						size="sm"
						onclick={handleActivate}
						disabled={licenseStore.isActivating || !licenseKeyInput.trim()}
					>
						{m.license_activate_button()}
					</Button>
				</div>
			{/if}
		</div>
	{:else}
		<div class="space-y-4">
			<p class="text-sm text-muted-foreground">
				{m.license_personal_explainer()}
			</p>
			<p class="text-sm text-muted-foreground">
				{m.license_activate_description()}
			</p>
			{#if showActivationInput}
				<div class="space-y-2 min-w-0">
					<input
						type="text"
						class="w-full px-3 py-2 border rounded-md bg-background text-sm font-mono"
						placeholder={m.license_key_placeholder()}
						bind:value={licenseKeyInput}
						onkeydown={(e) => e.key === "Enter" && handleActivate()}
					/>
					{#if licenseStore.activationError}
						<div class="flex items-start gap-1.5 text-sm text-destructive">
							<p class="break-all flex-1">{licenseStore.activationError}</p>
							<button
								class="shrink-0 p-0.5 rounded hover:bg-muted transition-colors cursor-pointer"
								title="Copy error"
								onclick={() => navigator.clipboard.writeText(licenseStore.activationError ?? "")}
							>
								<CopyIcon class="size-3.5" />
							</button>
						</div>
					{/if}
					<Button
						size="sm"
						onclick={handleActivate}
						disabled={licenseStore.isActivating || !licenseKeyInput.trim()}
					>
						{m.license_activate_button()}
					</Button>
				</div>
			{:else}
				<div class="flex gap-2">
					<Button variant="outline" size="sm" onclick={() => showActivationInput = true}>
						{m.license_activate()}
					</Button>
					<Button size="sm" onclick={() => openExternal("https://seaquel.app/pricing")}>
						{m.license_purchase()}
					</Button>
				</div>
			{/if}
		</div>
	{/if}
</div>
