<script lang="ts">
  /**
   * Unauthenticated entry point for importing an offline license bundle.
   *
   * Two callers land here:
   *   1. A fresh self-hosted install whose operator can't reach the control
   *      plane — they need to import a bundle before any user can sign up.
   *   2. An install whose grace window expired and which got redirected
   *      here from `/revalidate` (or directly from the app shell layout).
   *
   * Renders a single file-input + submit pair. POST to /api/airgap/bundle
   * is the same endpoint the in-app settings page uses; the API enforces
   * its own auth policy (loose during first-run, owner-only afterwards),
   * so this page doesn't need a server load. Result/error display is
   * inline rather than via toast because the page is otherwise empty and
   * the result is the only thing on screen.
   */
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Card from "$lib/components/ui/card/index.js";

  let file = $state<File | null>(null);
  let submitting = $state(false);
  let result = $state<
    | { kind: "idle" }
    | {
        kind: "success";
        tier: string;
        seats: number;
        notAfter: number;
        unchanged: boolean;
      }
    | { kind: "error"; message: string }
  >({ kind: "idle" });

  function pickFile(event: Event) {
    const target = event.currentTarget as HTMLInputElement;
    file = target.files && target.files.length > 0 ? target.files[0] : null;
    if (result.kind !== "idle") result = { kind: "idle" };
  }

  function errorMessage(
    body: { error?: string } | null,
    status: number,
  ): string {
    const code = body?.error;
    switch (code) {
      case "malformed_envelope":
        return "Could not read the bundle file. Make sure you're uploading the .bundle file downloaded from seaquel.app, unmodified.";
      case "untrusted_signer":
        return "Untrusted signer — this bundle wasn't signed by Seaquel. Download a fresh bundle from your seaquel.app dashboard.";
      case "bad_signature":
        return "The bundle's signature doesn't verify. The file may have been corrupted in transit — download it again.";
      case "schema_mismatch":
        return "The bundle's schema doesn't match what this install supports. Update Seaquel to the latest version.";
      case "bundle_older_than_current":
        return "This bundle is older than the one currently installed. Download a fresh bundle from your seaquel.app dashboard.";
      case "subscription_mismatch":
        return "This bundle is for a different subscription. To switch subscriptions, an owner needs to remove the current bundle first from Settings → Offline license bundle.";
      default:
        break;
    }
    if (status === 401 || status === 403) {
      return "An owner is already bound on this install. Sign in first, then import a new bundle from the Settings page.";
    }
    if (status === 429) {
      return "Too many upload attempts. Wait a minute and try again.";
    }
    return code ?? `Upload failed (HTTP ${status}).`;
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!file) return;
    submitting = true;
    result = { kind: "idle" };
    try {
      // Stream the raw .bundle bytes. The endpoint accepts either JSON or
      // octet-stream — both are read as bytes and re-decoded by the
      // verifier; the content-type just documents intent.
      const res = await fetch("/api/airgap/bundle", {
        method: "POST",
        headers: { "Content-Type": "application/octet-stream" },
        body: file,
      });
      if (!res.ok) {
        const body = await res
          .json()
          .catch(() => null as { error?: string } | null);
        result = { kind: "error", message: errorMessage(body, res.status) };
        return;
      }
      const data = (await res.json()) as {
        ok: boolean;
        unchanged: boolean;
        tier: string;
        seats: number;
        notAfter: number;
      };
      result = {
        kind: "success",
        tier: data.tier,
        seats: data.seats,
        notAfter: data.notAfter,
        unchanged: data.unchanged,
      };
    } catch (e) {
      result = {
        kind: "error",
        message: e instanceof Error ? e.message : String(e),
      };
    } finally {
      submitting = false;
    }
  }

  function formatNotAfter(unixSeconds: number): string {
    try {
      return new Date(unixSeconds * 1000).toISOString().slice(0, 10);
    } catch {
      return String(unixSeconds);
    }
  }
</script>

<svelte:head>
  <title>Import offline license bundle — Seaquel</title>
</svelte:head>

<div class="flex min-h-svh items-center justify-center p-4">
  <Card.Root class="w-full max-w-lg">
    <Card.Header>
      <Card.Title>Import offline license bundle</Card.Title>
      <Card.Description>
        Upload a <code class="text-foreground font-mono">.bundle</code> file
        downloaded from your seaquel.app dashboard. This puts the instance
        into offline mode using the seats and revocations encoded in the
        bundle.
      </Card.Description>
    </Card.Header>
    <Card.Content>
      <form onsubmit={submit} class="flex flex-col gap-4">
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

        {#if result.kind === "success"}
          <div
            class="rounded-md border border-green-500/40 bg-green-50 p-3 text-sm dark:bg-green-950/40"
            role="status"
          >
            <p class="font-medium text-green-900 dark:text-green-200">
              {result.unchanged
                ? "Bundle already up to date."
                : "Bundle imported."}
            </p>
            <p class="text-green-800 dark:text-green-300 mt-1">
              Tier: <span class="font-mono">{result.tier}</span> · seats:
              <span class="font-mono">{result.seats}</span> · valid until
              <span class="font-mono">{formatNotAfter(result.notAfter)}</span>
            </p>
            <div class="mt-3 flex flex-wrap gap-2">
              <Button size="sm" href="/signup">Continue to sign up</Button>
              <Button size="sm" variant="outline" href="/login">
                I already have an account
              </Button>
            </div>
          </div>
        {:else if result.kind === "error"}
          <div
            class="text-destructive border-destructive/40 bg-destructive/5 rounded-md border p-3 text-sm"
            role="alert"
          >
            <p class="font-medium">Could not import bundle</p>
            <p class="mt-1 break-words">{result.message}</p>
          </div>
        {/if}

        <Button type="submit" disabled={submitting || !file}>
          {submitting ? "Uploading…" : "Import bundle"}
        </Button>
      </form>
    </Card.Content>
    <Card.Footer class="text-muted-foreground flex justify-center text-xs">
      <span>
        Already have an account on this install?
        <a href="/login" class="text-foreground underline">Sign in</a>
      </span>
    </Card.Footer>
  </Card.Root>
</div>
