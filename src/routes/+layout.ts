import { initSeaquelWasm } from "$lib/wasm";
import { isSupportedBuild } from "$lib/utils/environment";
import type { LayoutLoad } from "./$types";

// Tauri doesn't have a Node.js server to do proper SSR
// so we will use adapter-static to prerender the app (SSG)
// See: https://v2.tauri.app/start/frontend/sveltekit/ for more info
export const prerender = true;
export const ssr = false;

/**
 * The demo's Core in the page (phase 8), opened before anything
 * renders, like the editor module: every storage call goes to it. The branch
 * is on the build-time constant itself, so Rollup drops the import (the
 * module, the bridge and the transport) from the desktop and web builds.
 */
const openDemoCore =
  import.meta.env.VITE_BUILD_TARGET === "demo"
    ? async () => (await import("$lib/demo/core")).openDemoCore()
    : null;

// seaquel-wasm is loaded before anything renders, so the editor and the rest
// of the app can call it synchronously. `load`'s fetch avoids SvelteKit's
// warning about `window.fetch` in `load`. A failure doesn't throw, which would
// show SvelteKit's bare "500 Internal Error" (and in Tauri there's no console
// to explain it): +layout.svelte shows `wasmError` instead of the app.
//
// A build that is neither the desktop app, the web build nor the demo (a
// desktop or dev build opened in a plain browser) loads nothing: the layout
// shows `unsupported-build.svelte`, and no Core call or socket starts.
export const load: LayoutLoad = async ({ fetch }) => {
  if (!isSupportedBuild()) return { wasmError: null, unsupportedBuild: true };
  try {
    await initSeaquelWasm(fetch);
  } catch (e) {
    console.error("seaquel-wasm failed to load", e);
    return {
      wasmError: e instanceof Error ? `${e.name}: ${e.message}` : String(e),
      unsupportedBuild: false,
    };
  }
  if (openDemoCore) {
    try {
      await openDemoCore();
    } catch (e) {
      // The module's own errors are `RpcError`s (`CORE_FAILED`, …); nothing
      // here quotes stored data.
      console.error("Seaquel Core failed to start in the page", e);
      return {
        wasmError: `Seaquel Core didn't start: ${coreErrorText(e)}`,
        unsupportedBuild: false,
      };
    }
  }
  return { wasmError: null, unsupportedBuild: false };
};

function coreErrorText(e: unknown): string {
  if (e instanceof Error) return `${e.name}: ${e.message}`;
  const code = (e as { code?: unknown } | null)?.code;
  return typeof code === "string" ? code : String(e);
}
