/**
 * Environment detection utilities.
 * Used to determine runtime context (Tauri desktop vs web browser).
 */

/**
 * Check if running inside Tauri desktop app.
 * Uses __TAURI_INTERNALS__ which is available synchronously in Tauri v2,
 * or falls back to checking __TAURI__ for compatibility.
 */
export function isTauri(): boolean {
  if (typeof window === "undefined") return false;

  // __TAURI_INTERNALS__ is available synchronously in Tauri v2
  if ("__TAURI_INTERNALS__" in window) return true;

  // Fallback: check for __TAURI__ (may be async in some cases)
  if ("__TAURI__" in window) return true;

  // Check if loaded via tauri:// protocol (Tauri v2 default)
  if (window.location.protocol === "tauri:") return true;

  return false;
}

/**
 * Check if this build targets the hosted / self-hosted web deployment — i.e.
 * talks to a real `seaquel-server` over HTTP + WebSocket, rather than running
 * the DB entirely in the browser (demo) or embedded in Tauri (desktop).
 *
 * Set at build time via `BUILD_TARGET=web`, which vite.config.js forwards to
 * `import.meta.env.VITE_BUILD_TARGET`.
 */
export function isWeb(): boolean {
  if (typeof import.meta === "undefined" || !import.meta.env) return false;
  return import.meta.env.VITE_BUILD_TARGET === "web";
}

/**
 * Check if this is the browser demo build: Seaquel Core running in the
 * page (phase 8: the browser module over DuckDB-WASM), no backend.
 *
 * A build-time constant (`BUILD_TARGET=demo`, which `vite.config.js` turns
 * into `VITE_BUILD_TARGET` and `VITE_IS_DEMO`), so a desktop or dev build
 * opened in a plain browser is not the demo (see `isSupportedBuild`).
 */
export function isDemo(): boolean {
  if (typeof import.meta === "undefined" || !import.meta.env) return false;
  return (
    import.meta.env.VITE_BUILD_TARGET === "demo" || String(import.meta.env.VITE_IS_DEMO) === "true"
  );
}

/**
 * Whether this build can run where it is: inside the desktop app, as the
 * web build, or as the demo build. A desktop build opened in a plain
 * browser (`npm run dev` in Chrome) is none of them; the root layout then
 * shows how to run the demo instead of the app.
 */
export function isSupportedBuild(): boolean {
  return isTauri() || isWeb() || isDemo();
}

/**
 * Check if running in a browser environment (vs SSR).
 */
export function isBrowser(): boolean {
  return typeof window !== "undefined";
}
