<script lang="ts">
  // Minimal root layout.
  //
  // The full app shell (database context, sidebar, header, Tauri listeners,
  // etc.) lives in `(app)/+layout.svelte`. Only routes inside the `(app)`
  // route group get that layout. Public routes like `/login` and `/signup`
  // use this layout directly, so they never instantiate `UseDatabase` and
  // never fire the storage calls that would 401 on an unauthenticated user.
  import "./layout.css";
  import { ModeWatcher } from "mode-watcher";
  import { page } from "$app/state";
  import { Toaster } from "$lib/components/ui/sonner/index.js";
  import WasmLoadError from "$lib/wasm/load-error.svelte";
  let { data, children } = $props();

  // Routes that never touch SQL still work when seaquel-wasm failed to load.
  const WASM_FREE_ROUTES = new Set(["/login", "/signup", "/airgap-setup"]);
  const showWasmError = $derived(
    data.wasmError !== null && !WASM_FREE_ROUTES.has(page.route.id ?? ""),
  );
</script>

<ModeWatcher />
<Toaster position="bottom-right" richColors expand />
{#if showWasmError && data.wasmError}
  <WasmLoadError error={data.wasmError} />
{:else}
  {@render children()}
{/if}
