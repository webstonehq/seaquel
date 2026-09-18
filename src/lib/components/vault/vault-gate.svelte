<script lang="ts">
    /**
     * Mounted once at the top of the web app layout. Watches the reactive
     * vault status and shows the setup (uninitialized) or unlock (locked)
     * dialog whenever some caller is waiting for the vault to unlock.
     *
     * On Tauri desktop and demo builds this component's dialogs never
     * open because `vault.waitersPending` only flips true when a
     * `VaultKeyringService` call asks for unlock — and the web build is
     * the only runtime that uses `VaultKeyringService`.
     */
    import { onMount } from "svelte";
    import VaultSetupDialog from "./vault-setup-dialog.svelte";
    import VaultUnlockDialog from "./vault-unlock-dialog.svelte";
    import { getVault } from "$lib/services/vault/vault-state.svelte";

    const vault = getVault();

    onMount(() => {
        // Read vault_state once so the gate knows whether we need setup or
        // unlock when the first waiter lands.
        void vault.refresh();
    });

    const showSetup = $derived(
        vault.waitersPending && vault.status === "uninitialized",
    );
    const showUnlock = $derived(
        vault.waitersPending && vault.status === "locked",
    );

    // Reset the auto-lock timer only on real user input. Routing this
    // through `notifyUserActivity` (rather than bumping the timer on every
    // `requireKey` call) keeps "idle" meaning "the user is idle" — not
    // "no background decrypt has happened".
    function handleActivity() {
        vault.notifyUserActivity();
    }
</script>

<svelte:window onkeydown={handleActivity} onpointerdown={handleActivity} />

{#if showSetup}
    <VaultSetupDialog />
{/if}
{#if showUnlock}
    <VaultUnlockDialog />
{/if}
