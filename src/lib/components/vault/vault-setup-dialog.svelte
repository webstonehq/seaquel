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

    // The gate mounts this component only while setup is needed — so the
    // dialog opens immediately. Binding locally lets the Dialog primitive
    // drive its own open state from ESC / outside-click; we route those
    // into `vault.cancelPending()` via `onOpenChange`.
    let open = $state(true);
    let passphrase = $state("");
    let confirm = $state("");
    let busy = $state(false);

    async function handleSetup() {
        if (passphrase.length < 12) {
            errorToast("Passphrase must be at least 12 characters");
            return;
        }
        if (passphrase !== confirm) {
            errorToast("Passphrases do not match");
            return;
        }
        busy = true;
        try {
            await vault.setup(passphrase);
            toast.success("Vault created — your credentials will be encrypted");
            passphrase = "";
            confirm = "";
        } catch (err) {
            errorToast(`Failed to create vault: ${(err as Error).message}`);
        } finally {
            busy = false;
        }
    }

    function handleCancel() {
        vault.cancelPending("vault setup cancelled");
        passphrase = "";
        confirm = "";
    }

    function onKeydown(e: KeyboardEvent) {
        if (e.key === "Enter" && !busy) handleSetup();
    }
</script>

<Dialog bind:open onOpenChange={(v) => { if (!v) handleCancel(); }}>
    <DialogContent class="max-w-md">
        <DialogHeader>
            <DialogTitle>Create your credential vault</DialogTitle>
            <DialogDescription>
                Seaquel Cloud stores your database passwords encrypted with a
                key derived from a passphrase you choose. The passphrase never
                leaves your browser — if you forget it, your saved credentials
                can't be recovered and you'll have to re-enter them.
            </DialogDescription>
        </DialogHeader>

        <div class="grid gap-4 py-2">
            <div class="grid gap-2">
                <Label for="vault-passphrase">Passphrase</Label>
                <Input
                    id="vault-passphrase"
                    type="password"
                    autocomplete="off"
                    data-1p-ignore
                    data-lpignore="true"
                    data-form-type="other"
                    bind:value={passphrase}
                    onkeydown={onKeydown}
                    disabled={busy}
                    placeholder="At least 12 characters"
                />
            </div>
            <div class="grid gap-2">
                <Label for="vault-confirm">Confirm passphrase</Label>
                <Input
                    id="vault-confirm"
                    type="password"
                    autocomplete="off"
                    data-1p-ignore
                    data-lpignore="true"
                    data-form-type="other"
                    bind:value={confirm}
                    onkeydown={onKeydown}
                    disabled={busy}
                    placeholder="Re-enter to confirm"
                />
            </div>
            <p class="text-xs text-muted-foreground">
                This is <strong>different</strong> from your login password.
                Keeping them separate means a login compromise does not leak
                your stored credentials.
            </p>
        </div>

        <DialogFooter>
            <Button variant="outline" onclick={handleCancel} disabled={busy}>
                Cancel
            </Button>
            <Button onclick={handleSetup} disabled={busy}>
                {busy ? "Creating…" : "Create vault"}
            </Button>
        </DialogFooter>
    </DialogContent>
</Dialog>
