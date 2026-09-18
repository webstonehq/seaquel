<script lang="ts">
  import * as Card from "$lib/components/ui/card/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { goto } from "$app/navigation";

  let { data } = $props();

  const lastValidated = $derived(
    data.lastValidatedAt ? new Date(data.lastValidatedAt * 1000) : null,
  );
  const graceExpired = $derived(
    data.graceUntil ? new Date(data.graceUntil * 1000) : null,
  );
  const bundleNotAfter = $derived(
    data.bundleNotAfter ? new Date(data.bundleNotAfter) : null,
  );

  const variant = $derived.by(() => {
    if (data.mode === "airgap" && data.bundleExpired) return "airgap-expired";
    if (data.mode === "airgap" && data.bundlePresent) return "airgap-present";
    return "online";
  });

  const importBundleHref = $derived(
    data.signedIn ? "/settings/airgap" : "/airgap-setup",
  );

  function formatDate(d: Date | null): string {
    if (!d) return "unknown";
    return d.toLocaleString();
  }

  function tryAgain() {
    location.reload();
  }

  function reenterLicense() {
    goto("/signup?reason=revalidate");
  }

  function importBundle() {
    goto(importBundleHref);
  }

  function continueToSignup() {
    goto("/signup");
  }
</script>

<svelte:head>
  <title>Revalidation required — Seaquel</title>
</svelte:head>

<div class="flex min-h-svh items-center justify-center p-4">
  <Card.Root class="w-full max-w-md">
    {#if variant === "airgap-expired"}
      <Card.Header>
        <Card.Title>
          Offline bundle expired on {formatDate(bundleNotAfter)}.
        </Card.Title>
        <Card.Description>
          Download a fresh bundle from your seaquel.app dashboard and
          import it to continue.
        </Card.Description>
      </Card.Header>
      <Card.Content class="text-sm text-muted-foreground">
        <p>
          Bundles expire 30 days after the end of each billing period.
          Once a new bundle is imported, license checks resume locally
          against the bundle — no outbound connection required.
        </p>
      </Card.Content>
      <Card.Footer class="flex gap-2 justify-end">
        <Button variant="outline" onclick={importBundle}>Import bundle</Button>
        <Button onclick={tryAgain}>Try again</Button>
      </Card.Footer>
    {:else if variant === "airgap-present"}
      <Card.Header>
        <Card.Title>Continue to signup</Card.Title>
        <Card.Description>
          An offline bundle is loaded but the install isn't bound to a
          user yet. Continue to signup to bind this install.
        </Card.Description>
      </Card.Header>
      <Card.Content class="text-sm text-muted-foreground">
        <p>
          If you were expecting to land somewhere else, try again — the
          license state may have updated since this page loaded.
        </p>
      </Card.Content>
      <Card.Footer class="flex gap-2 justify-end">
        <Button variant="outline" onclick={tryAgain}>Try again</Button>
        <Button onclick={continueToSignup}>Continue to signup</Button>
      </Card.Footer>
    {:else}
      <Card.Header>
        <Card.Title>Revalidation required</Card.Title>
        <Card.Description>
          We couldn't reach the Seaquel licensing service to confirm your
          license is still valid, and the offline grace window has expired.
          Last successful validation: <strong>{formatDate(lastValidated)}</strong>.
          Grace period ended: <strong>{formatDate(graceExpired)}</strong>.
        </Card.Description>
      </Card.Header>
      <Card.Content class="text-sm text-muted-foreground space-y-3">
        <p>
          Reconnect to the internet and try again, or re-enter your license
          key to restore access.
        </p>
        <p class="text-xs">
          If your network is permanently restricted, import an offline
          bundle from your seaquel.app dashboard.
          <a class="underline" href="/airgap-setup">Import a bundle.</a>
        </p>
      </Card.Content>
      <Card.Footer class="flex gap-2 justify-end">
        <Button variant="outline" onclick={reenterLicense}>
          Re-enter license key
        </Button>
        <Button onclick={tryAgain}>Try again</Button>
      </Card.Footer>
    {/if}
  </Card.Root>
</div>
