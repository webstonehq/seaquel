import { paraglideVitePlugin } from "@inlang/paraglide-js";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";

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

    // Web-mode dev proxy: forward /api/db/* (including the WebSocket
    // upgrade for /api/db/stream) to the local seaquel-server. The rest of
    // /api (e.g. /api/auth, /api/meta) stays inside SvelteKit so hooks +
    // Better Auth + Kysely routes work during HMR.
    //
    // /health is NOT proxied — SvelteKit has its own /health route and in
    // dev we want that path to reflect the Node app's liveness, consistent
    // with the production shape.
    //
    // Override the target via SEAQUEL_SERVER_URL if the Rust server runs elsewhere.
    ...(isWebMode
      ? {
          server: {
            proxy: {
              "/api/db": {
                target: process.env.SEAQUEL_SERVER_URL || "http://127.0.0.1:8788",
                changeOrigin: true,
                ws: true,
              },
            },
          },
        }
      : {}),

    // Monaco Editor and DuckDB optimization
    optimizeDeps: {
      include: ["monaco-editor", "monaco-sql-languages"],
      // DuckDB-WASM is only needed in demo mode (in-browser DB engine).
      ...(isDemoMode
        ? { include: ["monaco-editor", "monaco-sql-languages", "@duckdb/duckdb-wasm"] }
        : {}),
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
