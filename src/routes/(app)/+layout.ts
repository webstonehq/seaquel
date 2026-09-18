// Override the root layout's prerender setting for the (app) group.
//
// The root layout sets `prerender = true` because the desktop (Tauri) and
// demo builds run under adapter-static — they need every page baked to HTML.
//
// The web build (BUILD_TARGET=web → adapter-node) runs the multi-tenant
// container, where +layout.server.ts in this directory enforces the auth +
// license-state gate. SvelteKit serves prerendered files directly without
// re-running server `load` functions, so prerendering this group would
// bypass the gate on every (app) URL.
//
// Inherit `ssr = false` from the root layout — the app shell is built around
// browser-only runes/state, and the server load runs server-side regardless.
const isWeb = import.meta.env.VITE_BUILD_TARGET === "web";
export const prerender = !isWeb;
