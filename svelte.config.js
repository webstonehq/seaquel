// Seaquel ships three SvelteKit build targets:
//
//   - desktop (default):   adapter-static. Tauri loads the built files from
//                          disk via its own runtime; no Node server involved.
//   - demo  (BUILD_TARGET=demo): adapter-static. Served from seaquel.app/demo
//                          as a pure static bundle with DuckDB-WASM in the browser.
//   - web   (BUILD_TARGET=web):  adapter-node. The tenant container runs a
//                          custom server.js entrypoint that imports the
//                          adapter-node handler, spawns the loopback Rust
//                          service, and attaches a WebSocket upgrade proxy.
import adapterStatic from "@sveltejs/adapter-static";
import adapterNode from "@sveltejs/adapter-node";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

const buildTarget = process.env.BUILD_TARGET; // "demo" | "web" | undefined (desktop)
const isDemo = buildTarget === "demo";
const isWeb = buildTarget === "web";

const outDir = isDemo ? "build-demo" : isWeb ? "build-web" : "build";
// Demo is served under /demo/ on seaquel.app; the web build is served at the
// root of each tenant's subdomain.
const basePath = isDemo ? "/demo" : "";

const adapter = isWeb
  ? adapterNode({
      // Matches the outDir convention used by adapter-static targets, so every
      // target's build output lives next to its siblings at the repo root.
      out: outDir,
      precompress: false,
    })
  : adapterStatic({
      fallback: "index.html",
      pages: outDir,
      assets: outDir,
    });

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter,
    paths: {
      base: basePath,
    },
    alias: {
      $lib: "src/lib",
      "$lib/*": "src/lib/*",
      // Plain JS modules shared between the SvelteKit bundle and `server.js`
      // (which runs outside the bundle). Keep contents framework-agnostic.
      $shared: "shared",
      "$shared/*": "shared/*",
    },
  },
};

export default config;
