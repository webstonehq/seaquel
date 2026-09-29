<script lang="ts">
  // Shown by the app shell instead of the app when the first storage call
  // fails in a way retrying can't fix (`storageGate.blocked`): legacy JSON
  // data from before 2026.4.5, a metadata file that isn't SQLite, or no data
  // dir at all. There is no retry button; the user has to act outside the
  // app first.
  import DatabaseIcon from "@lucide/svelte/icons/database";
  import ExternalLinkIcon from "@lucide/svelte/icons/external-link";
  import { m } from "$lib/paraglide/messages.js";
  import type { BlockingStorageError } from "$lib/storage/storage-gate.svelte";
  import { isTauri } from "$lib/utils/environment";

  const RELEASES_URL = "https://github.com/webstonehq/seaquel/releases";

  let { error }: { error: BlockingStorageError } = $props();

  const title = $derived(
    error.kind === "legacy"
      ? m.storage_error_legacy_title()
      : error.kind === "corrupt"
        ? m.storage_error_corrupt_title()
        : m.storage_error_no_data_dir_title(),
  );

  // A plain link works on web; the desktop webview doesn't open new windows,
  // so it hands the URL to the OS.
  async function openReleases(event: MouseEvent) {
    if (!isTauri()) return;
    event.preventDefault();
    try {
      const { openPath } = await import("$lib/api/tauri");
      await openPath(RELEASES_URL);
    } catch (e) {
      console.error("Couldn't open the releases page", e);
    }
  }
</script>

<main class="bg-background text-foreground flex min-h-screen items-center justify-center p-6">
  <div class="w-full max-w-xl space-y-4" role="alert">
    <div class="flex items-center gap-3">
      <DatabaseIcon class="text-destructive size-6 shrink-0" aria-hidden="true" />
      <h1 class="text-xl font-semibold">{title}</h1>
    </div>

    {#if error.kind === "legacy"}
      <p class="text-sm">{m.storage_error_legacy_description()}</p>
      <a
        href={RELEASES_URL}
        target="_blank"
        rel="noopener noreferrer"
        onclick={openReleases}
        class="text-primary inline-flex items-center gap-1.5 text-sm font-medium underline-offset-4 hover:underline"
      >
        {m.storage_error_releases_link()}
        <ExternalLinkIcon class="size-4" aria-hidden="true" />
      </a>
    {:else if error.kind === "no-data-dir"}
      <p class="text-sm">{m.storage_error_no_data_dir_description()}</p>
    {:else}
      {#if error.path}
        <p class="text-sm">{m.storage_error_corrupt_description({ path: error.path })}</p>
      {/if}
      {#if error.untouched}
        <p class="text-sm">{m.storage_error_corrupt_untouched()}</p>
      {/if}
      <p class="text-muted-foreground text-sm">
        {isTauri() ? m.storage_error_corrupt_hint_desktop() : m.storage_error_corrupt_hint_web()}
      </p>
    {/if}

    <div class="space-y-1">
      <p class="text-muted-foreground text-xs font-medium">{m.storage_error_details()}</p>
      <pre
        class="bg-muted max-h-48 overflow-auto rounded-md p-3 text-xs break-words whitespace-pre-wrap select-text"
        data-testid="storage-error">{error.detail}</pre>
    </div>
  </div>
</main>
