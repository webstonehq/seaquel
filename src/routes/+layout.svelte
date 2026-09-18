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
  import { Toaster } from "$lib/components/ui/sonner/index.js";
  let { children } = $props();
</script>

<ModeWatcher />
<Toaster position="bottom-right" richColors expand />
{@render children()}
