<script lang="ts">
  import * as Card from "$lib/components/ui/card/index.js";
  import { Button } from "$lib/components/ui/button/index.js";

  let { data } = $props();
</script>

<svelte:head>
  <title>Subscription paused — Seaquel</title>
</svelte:head>

<div class="flex min-h-svh items-center justify-center p-4">
  <Card.Root class="w-full max-w-md">
    <Card.Header>
      <Card.Title>Subscription paused</Card.Title>
      <Card.Description>
        {#if data.tenant?.slug}
          The subscription that funds <strong>{data.tenant.slug}</strong>
          has lapsed. Your data is preserved — sign-in resumes as soon as
          billing is updated.
        {:else}
          The subscription that funds this workspace has lapsed.
        {/if}
      </Card.Description>
    </Card.Header>
    <Card.Content class="text-sm text-muted-foreground">
      <p>Workspace owners can update billing in the Seaquel dashboard.</p>
    </Card.Content>
    <Card.Footer class="flex gap-2 justify-end">
      <Button href="https://seaquel.app/dashboard" target="_blank" rel="noopener">
        Open dashboard
      </Button>
    </Card.Footer>
  </Card.Root>
</div>
