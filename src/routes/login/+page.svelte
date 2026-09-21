<script lang="ts">
  import { page } from "$app/state";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Label } from "$lib/components/ui/label/index.js";
  import * as Card from "$lib/components/ui/card/index.js";
  import { getAuthClient } from "$lib/auth-client";

  let email = $state("");
  let password = $state("");
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    loading = true;
    error = null;

    try {
      const res = await getAuthClient().signIn.email({ email, password });
      if (res.error) {
        error = res.error.message ?? "Unable to sign in";
        loading = false;
        return;
      }
      // Full-page navigation so the root layout re-runs setDatabase() with
      // the new session cookie in place. A client-side goto() would preserve
      // the current component instance and miss the database bootstrap.
      //
      // Reject anything that isn't a same-origin absolute path: `//host/...`
      // is a protocol-relative URL the browser treats as off-origin, and
      // anything with a scheme is obviously off-origin. Falling back to "/"
      // prevents an attacker-crafted `?redirect=https://evil.com` from
      // bouncing an authenticated user straight into a phishing site.
      const raw = page.url.searchParams.get("redirect") ?? "/";
      const redirectTo = raw.startsWith("/") && !raw.startsWith("//") ? raw : "/";
      window.location.href = redirectTo;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      loading = false;
    }
  }
</script>

<svelte:head>
  <title>Sign in — Seaquel</title>
</svelte:head>

<div class="flex min-h-svh items-center justify-center p-4">
  <Card.Root class="w-full max-w-sm">
    <Card.Header>
      <Card.Title>Sign in to Seaquel</Card.Title>
      <Card.Description>Welcome back. Enter your email and password below.</Card.Description>
    </Card.Header>
    <Card.Content>
      <form onsubmit={submit} class="flex flex-col gap-4">
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
            autocomplete="current-password"
            required
            minlength={8}
            bind:value={password}
            disabled={loading}
          />
        </div>
        {#if error}
          <p class="text-destructive text-sm" role="alert">{error}</p>
        {/if}
        <Button type="submit" disabled={loading || !email || !password}>
          {loading ? "Signing in…" : "Sign in"}
        </Button>
      </form>
    </Card.Content>
    <Card.Footer class="flex justify-center text-sm">
      <span class="text-muted-foreground">
        New here?
        <a href="/signup" class="text-foreground underline">Create an account</a>
      </span>
    </Card.Footer>
  </Card.Root>
</div>
