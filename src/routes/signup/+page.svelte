<script lang="ts">
  import { page } from "$app/state";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import * as Card from "$lib/components/ui/card/index.js";

  let { data } = $props();

  const isSuspended = $derived(data.tenant?.status === "suspended");

  let name = $state("");
  let email = $state("");
  let password = $state("");
  let licenseKey = $state(page.url.searchParams.get("key") ?? "");
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    loading = true;
    error = null;

    try {
      const res = await fetch("/api/signup", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          email,
          password,
          name,
          licenseKey,
        }),
      });

      if (!res.ok) {
        const body = await res
          .json()
          .catch(() => ({ error: "unknown_error" }));
        error = errorMessage(body, res.status);
        loading = false;
        return;
      }

      // Better Auth set the session cookie on the response. Full-page
      // navigation so the (app) layout re-runs with the new session.
      window.location.href = "/";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      loading = false;
    }
  }

  function errorMessage(
    body: { error?: string; message?: string },
    status: number,
  ): string {
    if (body.error) {
      switch (body.error) {
        case "license_not_found":
          return "We don't recognize that license key.";
        case "license_inactive":
          return "That license is no longer active. Check your subscription.";
        case "wrong_subscription":
          return "That license isn't part of this tenant's subscription.";
        case "license_already_in_other_tenant":
          return "That license is already in use by another tenant.";
        default:
          return body.error;
      }
    }
    if (body.message) return body.message;
    if (status === 409) return "An account with this email already exists.";
    return "Unable to create account.";
  }
</script>

<svelte:head>
  <title>Sign up — Seaquel</title>
</svelte:head>

<div class="flex min-h-svh items-center justify-center p-4">
  <Card.Root class="w-full max-w-sm">
    {#if isSuspended}
      <Card.Header>
        <Card.Title>Subscription paused</Card.Title>
        <Card.Description>
          This workspace is currently suspended. Ask the workspace owner
          to update billing at seaquel.app/dashboard before signing up.
        </Card.Description>
      </Card.Header>
      <Card.Footer class="flex justify-center text-sm">
        <a href="/login" class="text-foreground underline">Back to sign in</a>
      </Card.Footer>
    {:else}
      <Card.Header>
        <Card.Title>
          {#if data.tenant}
            Join {data.tenant.slug}
          {:else}
            Set up Seaquel
          {/if}
        </Card.Title>
        <Card.Description>
          {#if data.tenant}
            Enter the license key you received from Seaquel and your
            sign-in details.
          {:else}
            Enter your owner license key to get started.
          {/if}
        </Card.Description>
      </Card.Header>
      <Card.Content>
        <form onsubmit={submit} class="flex flex-col gap-4">
          <div class="flex flex-col gap-2">
            <Label for="licenseKey">License key</Label>
            <Input
              id="licenseKey"
              type="text"
              autocomplete="off"
              required
              bind:value={licenseKey}
              placeholder="XXXX-XXXX-XXXX-XXXX"
              disabled={loading}
            />
          </div>
          <div class="flex flex-col gap-2">
            <Label for="name">Your name</Label>
            <Input
              id="name"
              type="text"
              autocomplete="name"
              required
              bind:value={name}
              disabled={loading}
            />
          </div>
          <div class="flex flex-col gap-2">
            <Label for="email">Email</Label>
            <Input
              id="email"
              type="email"
              autocomplete="email"
              required
              bind:value={email}
              disabled={loading}
            />
          </div>
          <div class="flex flex-col gap-2">
            <Label for="password">Password</Label>
            <Input
              id="password"
              type="password"
              autocomplete="new-password"
              required
              minlength={8}
              bind:value={password}
              disabled={loading}
            />
          </div>
          {#if error}
            <p class="text-destructive text-sm" role="alert">{error}</p>
          {/if}
          <Button
            type="submit"
            disabled={loading ||
              !name ||
              !email ||
              !password ||
              !licenseKey}
          >
            {loading ? "Creating account…" : "Create account"}
          </Button>
        </form>
      </Card.Content>
      <Card.Footer class="flex justify-center text-sm">
        <span class="text-muted-foreground">
          Already have an account?
          <a href="/login" class="text-foreground underline">Sign in</a>
        </span>
      </Card.Footer>
    {/if}
  </Card.Root>
</div>
