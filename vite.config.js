import { paraglideVitePlugin } from "@inlang/paraglide-js";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";

const host = process.env.TAURI_DEV_HOST;
const isDemo = process.env.BUILD_TARGET === "demo";

// https://vitejs.dev/config/
export default defineConfig(async ({ mode }) => {
  const isDemoMode = mode === "demo" || isDemo;

  return {
    plugins: [
      tailwindcss(),
      sveltekit(),
      paraglideVitePlugin({ project: "./project.inlang", outdir: "./src/lib/paraglide" })
    ],

    // Define environment variables
    define: {
      "import.meta.env.VITE_IS_DEMO": JSON.stringify(isDemoMode)
    },

    // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
    // Skip Tauri-specific config in demo mode
    ...(isDemoMode
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
              // 3. tell vite to ignore watching `src-tauri`
              ignored: ["**/src-tauri/**"]
            }
          }
        }),

    // Monaco Editor and DuckDB optimization
    optimizeDeps: {
      include: ["monaco-editor", "monaco-sql-languages"],
      // Include DuckDB-WASM in demo mode
      ...(isDemoMode ? { include: ["monaco-editor", "monaco-sql-languages", "@duckdb/duckdb-wasm"] } : {})
    },

    // Build configuration for demo mode
    build: isDemoMode
      ? {
          outDir: "build-demo",
          // Externalize Tauri packages in demo mode (they won't be used)
          rollupOptions: {
            external: (/** @type {string} */ id) => {
              // Don't externalize - let the dynamic imports handle it
              // The environment checks will prevent Tauri code from running
              return false;
            }
          }
        }
      : {}
  };
});
