<script lang="ts">
  // Shown by the root +layout.svelte when this build is neither the desktop
  // app, the web build nor the demo (e.g. `npm run dev` opened in a plain
  // browser): the app would only half work there, with no Core to talk to.
  import TriangleAlertIcon from "@lucide/svelte/icons/triangle-alert";
  import { m } from "$lib/paraglide/messages.js";
</script>

<main class="bg-background text-foreground flex min-h-screen items-center justify-center p-6">
  <div class="w-full max-w-xl space-y-4" role="alert">
    <div class="flex items-center gap-3">
      <TriangleAlertIcon class="text-destructive size-6 shrink-0" aria-hidden="true" />
      <h1 class="text-xl font-semibold">{m.unsupported_build_title()}</h1>
    </div>
    <p class="text-sm">{m.unsupported_build_message()}</p>
  </div>
</main>
