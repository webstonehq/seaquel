<script lang="ts">
  // Shown by the root +layout.svelte when seaquel-wasm fails to load, instead
  // of SvelteKit's bare "500 Internal Error". In Tauri there's no console, so
  // the error text is on the page with a copy button.
  import { CheckIcon, CopyIcon, RotateCwIcon, TriangleAlertIcon } from "@lucide/svelte";
  import { invalidateAll } from "$app/navigation";
  import { Button } from "$lib/components/ui/button/index.js";
  import { m } from "$lib/paraglide/messages.js";
  import { isTauri } from "$lib/utils/environment";

  let { error }: { error: string } = $props();

  let copied = $state(false);
  let retrying = $state(false);
  let errorText: HTMLPreElement | undefined = $state();

  // True if the text reached the clipboard. The Tauri plugin inside Tauri;
  // otherwise the browser's clipboard, which only exists in a secure context
  // (https or localhost).
  async function writeClipboard(text: string): Promise<boolean> {
    try {
      if (isTauri()) {
        const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
        await writeText(text);
        return true;
      }
      if (navigator.clipboard) {
        await navigator.clipboard.writeText(text);
        return true;
      }
    } catch (e) {
      console.error("Couldn't copy the seaquel-wasm error", e);
    }
    return false;
  }

  async function copy() {
    if (await writeClipboard(error)) {
      copied = true;
      setTimeout(() => (copied = false), 2000);
    } else if (errorText) {
      // No clipboard: select the text so it can be copied by hand.
      window.getSelection()?.selectAllChildren(errorText);
    }
  }

  // Reruns the root load, which retries the module (a failed attempt isn't cached).
  async function retry() {
    retrying = true;
    try {
      await invalidateAll();
    } catch (e) {
      console.error("Retrying seaquel-wasm failed", e);
    } finally {
      retrying = false;
    }
  }
</script>

<main class="bg-background text-foreground flex min-h-screen items-center justify-center p-6">
  <div class="w-full max-w-xl space-y-4">
    <!-- Announced when it appears; the buttons stay outside the alert. -->
    <div class="space-y-4" role="alert">
      <div class="flex items-center gap-3">
        <TriangleAlertIcon class="text-destructive size-6 shrink-0" aria-hidden="true" />
        <h1 class="text-xl font-semibold">{m.wasm_error_title()}</h1>
      </div>
      <p class="text-sm">{m.wasm_error_description()}</p>
      <p class="text-muted-foreground text-sm">{m.wasm_error_hint()}</p>
      <pre
        class="bg-muted max-h-48 overflow-auto rounded-md p-3 text-xs break-words whitespace-pre-wrap"
        data-testid="wasm-error"
        bind:this={errorText}>{error}</pre>
    </div>
    <div class="flex gap-2">
      <Button onclick={retry} disabled={retrying}>
        <RotateCwIcon class="size-4" />
        {m.wasm_error_retry()}
      </Button>
      <Button variant="outline" onclick={copy}>
        {#if copied}
          <CheckIcon class="size-4" />
          {m.wasm_error_copied()}
        {:else}
          <CopyIcon class="size-4" />
          {m.wasm_error_copy()}
        {/if}
      </Button>
      <span class="sr-only" aria-live="polite">{copied ? m.wasm_error_copied() : ""}</span>
    </div>
  </div>
</main>
