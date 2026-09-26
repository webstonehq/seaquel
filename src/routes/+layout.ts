import { initSeaquelWasm } from "$lib/wasm";
import type { LayoutLoad } from "./$types";

// Tauri doesn't have a Node.js server to do proper SSR
// so we will use adapter-static to prerender the app (SSG)
// See: https://v2.tauri.app/start/frontend/sveltekit/ for more info
export const prerender = true;
export const ssr = false;

// seaquel-wasm is loaded before anything renders, so the editor and the rest
// of the app can call it synchronously. `load`'s fetch avoids SvelteKit's
// warning about `window.fetch` in `load`. A failure doesn't throw, which would
// show SvelteKit's bare "500 Internal Error" (and in Tauri there's no console
// to explain it): +layout.svelte shows `wasmError` instead of the app.
export const load: LayoutLoad = async ({ fetch }) => {
  try {
    await initSeaquelWasm(fetch);
    return { wasmError: null };
  } catch (e) {
    console.error("seaquel-wasm failed to load", e);
    return { wasmError: e instanceof Error ? `${e.name}: ${e.message}` : String(e) };
  }
};
