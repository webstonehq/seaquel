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
    import { Input } from "$lib/components/ui/input";
    import { Label } from "$lib/components/ui/label";
    import { errorToast } from "$lib/utils/toast";
    import { toast } from "svelte-sonner";
    import { getVault } from "$lib/services/vault/vault-state.svelte";

    const vault = getVault();

    // The gate mounts this component only while the vault is locked and a
    // waiter is pending. Owning `open` locally lets the Dialog primitive
    // handle ESC / outside-click naturally — we forward close events to
    // `vault.cancelPending()` via `onOpenChange`.
    let open = $state(true);
    let passphrase = $state("");
    let busy = $state(false);
    let confirmReset = $state(false);

    async function handleUnlock() {
        if (!passphrase) return;
        // Capture into a local before awaiting and zero the rune immediately:
        // while `vault.unlock(...)` is in flight (network round-trip + KDF),
        // the plaintext would otherwise sit in reactive state where devtools
        // — or any future component that introspects $state — could read it.
        const pp = passphrase;
        passphrase = "";
        busy = true;
        try {
            await vault.unlock(pp);
            toast.success("Vault unlocked");
        } catch (err) {
            errorToast((err as Error).message);
        } finally {
            busy = false;
        }
    }

    function handleCancel() {
        vault.cancelPending("vault unlock cancelled");
        passphrase = "";
        confirmReset = false;
    }

    async function handleReset() {
        busy = true;
        try {
            await vault.reset();
            toast.success("Vault reset — your saved credentials have been cleared");
            passphrase = "";
            confirmReset = false;
        } catch (err) {
            errorToast(`Failed to reset vault: ${(err as Error).message}`);
        } finally {
            busy = false;
        }
    }

    function onKeydown(e: KeyboardEvent) {
        if (e.key === "Enter" && !busy) handleUnlock();
    }
</script>

<Dialog bind:open onOpenChange={(v) => { if (!v) handleCancel(); }}>
    <DialogContent class="max-w-md">
        <DialogHeader>
            <DialogTitle>Unlock your credential vault</DialogTitle>
            <DialogDescription>
                Enter the passphrase you set when you created your Seaquel
                vault. It will stay unlocked for this browser tab until you
                close it.
            </DialogDescription>
        </DialogHeader>

        {#if !confirmReset}
            <div class="grid gap-4 py-2">
                <div class="grid gap-2">
                    <Label for="vault-passphrase-unlock">Passphrase</Label>
                    <Input
                        id="vault-passphrase-unlock"
                        type="password"
                        autocomplete="off"
                        data-1p-ignore
                        data-lpignore="true"
                        data-form-type="other"
                        bind:value={passphrase}
                        onkeydown={onKeydown}
                        disabled={busy}
                    />
                </div>
                <button
                    type="button"
                    class="text-xs text-muted-foreground hover:text-foreground underline underline-offset-2 text-left"
                    onclick={() => (confirmReset = true)}
                    disabled={busy}
                >
                    Forgot your passphrase?
                </button>
            </div>

            <DialogFooter>
                <Button variant="outline" onclick={handleCancel} disabled={busy}>
                    Cancel
                </Button>
                <Button onclick={handleUnlock} disabled={busy || !passphrase}>
                    {busy ? "Unlocking…" : "Unlock"}
                </Button>
            </DialogFooter>
        {:else}
            <div class="grid gap-4 py-2">
                <p class="text-sm">
                    Resetting the vault deletes every saved credential on this
                    account. You'll need to re-enter database passwords, SSH
                    credentials, and API keys the next time you use them.
                </p>
                <p class="text-sm text-destructive">
                    This cannot be undone.
                </p>
            </div>

            <DialogFooter>
                <Button
                    variant="outline"
                    onclick={() => (confirmReset = false)}
                    disabled={busy}
                >
                    Back
                </Button>
                <Button
                    onclick={handleReset}
                    disabled={busy}
                    class="bg-destructive text-white hover:bg-destructive/90"
                >
                    {busy ? "Resetting…" : "Reset vault"}
                </Button>
            </DialogFooter>
        {/if}
    </DialogContent>
</Dialog>
