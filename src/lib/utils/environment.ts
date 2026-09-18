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
 * Check if running in browser demo mode.
 *
 * "Demo" means in-browser with WASM DB engines (DuckDB-WASM, sql.js), no
 * real backend — i.e. not Tauri AND not the web server build.
 */
export function isDemo(): boolean {
  return !isTauri() && !isWeb();
}

/**
 * Check if running in a browser environment (vs SSR).
 */
export function isBrowser(): boolean {
  return typeof window !== "undefined";
}
