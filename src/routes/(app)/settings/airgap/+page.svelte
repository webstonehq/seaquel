<script lang="ts">
  /**
   * Settings → Offline license bundle.
   *
   * Two states:
   *   - No bundle present: same file-upload widget as /airgap-setup, but
   *     embedded in the app shell so an authenticated owner can flip
   *     into air-gap mode without ever signing out.
   *   - Bundle present: status read-out (tier, seats, fingerprint, etc.)
   *     plus owner-only Replace / Remove controls.
   *
   * Owners see write controls. Non-owners get a read-only view with a
   * short "ask your owner" note — the API already enforces this; hiding
   * the buttons just keeps the UI from offering actions that would 403.
   */
  import { invalidateAll } from "$app/navigation";
  import { toast } from "svelte-sonner";
  import { errorToast } from "$lib/utils/toast";
  import { Badge } from "$lib/components/ui/badge/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";

  let { data } = $props();

  let file = $state<File | null>(null);
  let submitting = $state(false);
  let removing = $state(false);
  let confirmOpen = $state(false);

  function pickFile(event: Event) {
    const target = event.currentTarget as HTMLInputElement;
    file = target.files && target.files.length > 0 ? target.files[0] : null;
  }

  function formatUnix(unixSeconds: number): string {
    try {
      return new Date(unixSeconds * 1000).toLocaleString();
    } catch {
      return String(unixSeconds);
    }
  }

  function formatUnixDate(unixSeconds: number): string {
    try {
      return new Date(unixSeconds * 1000).toLocaleDateString();
    } catch {
      return String(unixSeconds);
    }
  }

  function shortFingerprint(fp: string): string {
    return fp.length > 8 ? `${fp.slice(0, 8)}…` : fp;
  }

  function errorMessage(
    body: { error?: string } | null,
    status: number,
  ): string {
    const code = body?.error;
    switch (code) {
      case "malformed_envelope":
        return "Could not read the bundle file. Upload the .bundle file exactly as downloaded from seaquel.app.";
      case "untrusted_signer":
        return "Untrusted signer — this bundle wasn't signed by Seaquel.";
      case "bad_signature":
        return "The bundle's signature doesn't verify. The file may have been corrupted in transit.";
      case "schema_mismatch":
        return "The bundle's schema doesn't match what this install supports. Update Seaquel to the latest version.";
      case "bundle_older_than_current":
        return "This bundle is older than the one currently installed. Download a fresh bundle.";
      case "subscription_mismatch":
        return "This bundle is for a different subscription. Remove the current bundle first before switching subscriptions.";
      default:
        break;
    }
    if (status === 401 || status === 403) {
      return "Only the install owner can manage the offline bundle.";
    }
    if (status === 429) {
      return "Too many bundle operations. Wait a minute and try again.";
    }
    return code ?? `Request failed (HTTP ${status}).`;
  }

  async function uploadBundle(event: SubmitEvent) {
    event.preventDefault();
    if (!file) return;
    submitting = true;
    try {
      const res = await fetch("/api/airgap/bundle", {
        method: "POST",
        headers: { "Content-Type": "application/octet-stream" },
        body: file,
      });
      if (!res.ok) {
        const body = await res
          .json()
          .catch(() => null as { error?: string } | null);
        errorToast(errorMessage(body, res.status));
        return;
      }
      const body = (await res.json()) as { unchanged: boolean };
      toast.success(
        body.unchanged
          ? "Bundle already up to date."
          : "Bundle imported. Install is now in offline mode.",
      );
      file = null;
      await invalidateAll();
    } catch (e) {
      errorToast(e instanceof Error ? e.message : String(e));
    } finally {
      submitting = false;
    }
  }

  async function removeBundle() {
    removing = true;
    try {
      const res = await fetch("/api/airgap/bundle", { method: "DELETE" });
      if (!res.ok) {
        const body = await res
          .json()
          .catch(() => null as { error?: string } | null);
        errorToast(errorMessage(body, res.status));
        return;
      }
      toast.success("Offline bundle removed. Install is back online.");
      confirmOpen = false;
      await invalidateAll();
    } catch (e) {
      errorToast(e instanceof Error ? e.message : String(e));
    } finally {
      removing = false;
    }
  }
</script>

<svelte:head>
  <title>Offline license bundle — Seaquel</title>
</svelte:head>

<div class="flex-1 overflow-y-auto p-6">
  <div class="mx-auto flex w-full max-w-2xl flex-col gap-6">
    <div class="flex items-center justify-between gap-4">
      <div>
        <h1 class="text-xl font-semibold">Offline license bundle</h1>
        <p class="text-muted-foreground text-sm">
          Run this install without calling the seaquel.app license server.
        </p>
      </div>
      <Badge variant={data.mode === "airgap" ? "default" : "secondary"}>
        {data.mode === "airgap" ? "AIRGAP" : "ONLINE"}
      </Badge>
    </div>

    {#if data.bundle.present}
      <Card.Root>
        <Card.Header>
          <Card.Title>Active bundle</Card.Title>
          <Card.Description>
            This install validates its license against the bundle below
            instead of the seaquel.app control plane.
          </Card.Description>
        </Card.Header>
        <Card.Content class="space-y-3 text-sm">
          <div class="grid grid-cols-[160px_1fr] gap-2">
            <span class="text-muted-foreground">Tier</span>
            <span class="font-medium">{data.bundle.tier}</span>
          </div>
          <div class="grid grid-cols-[160px_1fr] gap-2">
            <span class="text-muted-foreground">Seats</span>
            <span class="font-medium">{data.bundle.seats}</span>
          </div>
          <div class="grid grid-cols-[160px_1fr] gap-2">
            <span class="text-muted-foreground">Issued</span>
            <span>{formatUnix(data.bundle.issuedAt)}</span>
          </div>
          <div class="grid grid-cols-[160px_1fr] gap-2">
            <span class="text-muted-foreground">Expires</span>
            <span>
              {formatUnixDate(data.bundle.notAfter)}
              {#if data.bundle.expired}
                <span class="text-destructive ml-1">(expired)</span>
              {/if}
            </span>
          </div>
          <div class="grid grid-cols-[160px_1fr] gap-2">
            <span class="text-muted-foreground">Imported</span>
            <span>{formatUnix(data.bundle.importedAt)}</span>
          </div>
          <div class="grid grid-cols-[160px_1fr] gap-2">
            <span class="text-muted-foreground">Signing key</span>
            <span class="font-mono">
              {shortFingerprint(data.bundle.pubkeyFingerprint)}
            </span>
          </div>
          <div class="grid grid-cols-[160px_1fr] gap-2">
            <span class="text-muted-foreground">Revoked keys</span>
            <span>{data.bundle.revokedKeyCount}</span>
          </div>
        </Card.Content>
        {#if data.isOwner}
          <Card.Footer class="flex flex-col items-stretch gap-3">
            <form onsubmit={uploadBundle} class="flex flex-col gap-2">
              <label
                for="replace-bundle-file"
                class="text-sm font-medium"
              >
                Replace bundle
              </label>
              <input
                id="replace-bundle-file"
                type="file"
                accept=".bundle,application/octet-stream,application/json"
                onchange={pickFile}
                disabled={submitting}
                class="file:bg-secondary file:text-secondary-foreground hover:file:bg-secondary/80 file:mr-3 file:cursor-pointer file:rounded-md file:border-0 file:px-3 file:py-1.5 file:text-sm file:font-medium block w-full text-sm text-muted-foreground"
              />
              {#if file}
                <p class="text-muted-foreground text-xs">
                  {file.name} · {(file.size / 1024).toFixed(1)} KB
                </p>
              {/if}
              <div>
                <Button
                  type="submit"
                  size="sm"
                  disabled={submitting || !file}
                >
                  {submitting ? "Uploading…" : "Upload replacement"}
                </Button>
              </div>
            </form>

            <div class="border-t pt-3">
              <Button
                variant="outline"
                size="sm"
                onclick={() => (confirmOpen = true)}
                disabled={removing}
              >
                Remove offline bundle
              </Button>
            </div>
          </Card.Footer>
        {:else}
          <Card.Footer>
            <p class="text-muted-foreground text-xs">
              Only the install owner can replace or remove the offline
              bundle.
            </p>
          </Card.Footer>
        {/if}
      </Card.Root>
    {:else}
      <Card.Root>
        <Card.Header>
          <Card.Title>No bundle imported</Card.Title>
          <Card.Description>
            This install is validating licenses against seaquel.app. Upload
            a bundle to switch to offline mode — useful for air-gapped
            networks or to ride out a planned control-plane outage.
          </Card.Description>
        </Card.Header>
        {#if data.isOwner}
          <Card.Content>
            <form onsubmit={uploadBundle} class="flex flex-col gap-3">
              <div class="flex flex-col gap-2">
                <label for="bundle-file" class="text-sm font-medium">
                  Bundle file
                </label>
                <input
                  id="bundle-file"
                  type="file"
                  accept=".bundle,application/octet-stream,application/json"
                  onchange={pickFile}
                  disabled={submitting}
                  class="file:bg-secondary file:text-secondary-foreground hover:file:bg-secondary/80 file:mr-3 file:cursor-pointer file:rounded-md file:border-0 file:px-3 file:py-1.5 file:text-sm file:font-medium block w-full text-sm text-muted-foreground"
                />
                {#if file}
                  <p class="text-muted-foreground text-xs">
                    {file.name} · {(file.size / 1024).toFixed(1)} KB
                  </p>
                {/if}
              </div>
              <div>
                <Button
                  type="submit"
                  size="sm"
                  disabled={submitting || !file}
                >
                  {submitting ? "Uploading…" : "Import bundle"}
                </Button>
              </div>
            </form>
          </Card.Content>
        {:else}
          <Card.Content>
            <p class="text-muted-foreground text-sm">
              Only the install owner can import an offline bundle.
            </p>
          </Card.Content>
        {/if}
      </Card.Root>
    {/if}
  </div>
</div>

<AlertDialog.Root bind:open={confirmOpen}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Remove offline bundle?</AlertDialog.Title>
      <AlertDialog.Description>
        This install will resume calling the seaquel.app license server.
        Make sure connectivity is restored before removing the bundle —
        otherwise the install will fall into the offline grace window and
        eventually require revalidation.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel disabled={removing}>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action onclick={removeBundle} disabled={removing}>
        {removing ? "Removing…" : "Remove bundle"}
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>
