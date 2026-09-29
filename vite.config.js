/// <reference types="vitest/config" />
import { paraglideVitePlugin } from "@inlang/paraglide-js";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import { withClientRequest } from "./shared/client-request.js";
import { attachRpcStreamProxy } from "./shared/rpc-stream-proxy.js";

const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig(async ({ mode }) => {
  // Resolve the build target from env var OR --mode flag. Env var wins when
  // both are set (keeps scripted builds predictable). This is load-bearing:
  // `import.meta.env.VITE_BUILD_TARGET` is read at runtime by isWeb() /
  // isDemo(), so a mismatch between "which mode is active" and "which define
  // got baked in" means the UI renders demo chrome in web mode.
  const buildTarget =
    process.env.BUILD_TARGET ?? (mode === "web" ? "web" : mode === "demo" ? "demo" : undefined);
  const isDemoMode = buildTarget === "demo";
  const isWebMode = buildTarget === "web";
  const skipTauriConfig = isDemoMode || isWebMode;

  return {
    plugins: [
      tailwindcss(),
      sveltekit(),
      paraglideVitePlugin({
        project: "./project.inlang",
        outdir: "./src/lib/paraglide",
        strategy: ["localStorage", "cookie", "globalVariable", "baseLocale"],
      }),
      // Web-mode dev: the /api/rpc/stream WebSocket, as server.js serves it
      // in production (session from /api/account/stream-access on this dev
      // server, X-Seaquel-User set, frames piped to seaquel-server's
      // /rpc/stream). /api/rpc itself is a SvelteKit route and needs nothing
      // here. Override the Rust URL with SEAQUEL_RUST_URL.
      ...(isWebMode
        ? [
            {
              name: "seaquel-rpc-stream-proxy",
              /** @param {import("vite").ViteDevServer} server */
              configureServer(server) {
                if (!server.httpServer) return;
                attachRpcStreamProxy(server.httpServer, {
                  rustUrl: process.env.SEAQUEL_RUST_URL ?? "http://127.0.0.1:8788",
                });
              },
            },
            // The dev server passes routes no `platform.req`, so /api/rpc
            // reads the Node request from here to abort its call to Rust
            // when the browser's connection closes (shared/client-request.js).
            {
              name: "seaquel-client-request",
              /** @param {import("vite").ViteDevServer} server */
              configureServer(server) {
                server.middlewares.use((req, _res, next) => withClientRequest(req, next));
              },
            },
          ]
        : []),
    ],

    // Define environment variables. VITE_BUILD_TARGET is read by
    // src/lib/utils/environment.ts to pick between Tauri, web, and demo
    // providers.
    define: {
      "import.meta.env.VITE_IS_DEMO": JSON.stringify(isDemoMode),
      "import.meta.env.VITE_BUILD_TARGET": JSON.stringify(buildTarget ?? "desktop"),
    },

    // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`.
    // Skip for browser-targeted builds (demo, web).
    ...(skipTauriConfig
      ? {}
      : {
          // 1. prevent vite from obscuring rust errors
          clearScreen: false,

          // 2. tauri expects a fixed port, fail if that port is not available
          server: {
            port: 1420,
            strictPort: true,
            host: host || false,
            hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,

            watch: {
              // 3. tell vite to ignore watching `src-tauri` and database files
              // SQLite WAL/SHM files change on connection and trigger full page reloads
              ignored: [
                "**/src-tauri/**",
                "**/*.sqlite",
                "**/*.sqlite-shm",
                "**/*.sqlite-wal",
                "**/*.db",
              ],
            },
          },
        }),

    // Monaco Editor and DuckDB optimization
    optimizeDeps: {
      include: ["monaco-editor", "monaco-sql-languages"],
      // DuckDB-WASM is only needed in demo mode (in-browser DB engine).
      ...(isDemoMode
        ? { include: ["monaco-editor", "monaco-sql-languages", "@duckdb/duckdb-wasm"] }
        : {}),
    },

    // vitest-setup.ts loads seaquel-wasm (src/lib/wasm/pkg, from `npm run
    // wasm:build`) so code that calls it synchronously works in tests.
    test: {
      setupFiles: ["src/lib/wasm/vitest-setup.ts"],
    },

    // Server-side externals. `better-sqlite3` is a native CommonJS module —
    // bundling it as ESM breaks because of `__filename` references. Loading
    // it from node_modules at runtime is the correct shape anyway.
    ssr: {
      external: ["better-sqlite3"],
    },

    // Per-target output directory. The web build is what gets embedded into
    // the seaquel-server binary via rust-embed (Phase 3).
    build: isDemoMode
      ? {
          outDir: "build-demo",
          rollupOptions: {
            // Don't externalize; runtime environment checks keep Tauri code
            // from executing in the browser.
            external: (/** @type {string} */ _id) => false,
          },
        }
      : isWebMode
        ? {
            outDir: "build-web",
            rollupOptions: {
              external: (/** @type {string} */ _id) => false,
            },
          }
        : {},
  };
});
