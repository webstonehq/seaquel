/**
 * DuckDB-WASM served from this app's own assets, for the web build.
 *
 * A self-hosted install may have no internet access, so web doesn't load
 * DuckDB from jsDelivr. Vite copies these files from the npm package into
 * the build (`?url`). Only the MVP and exception-handling builds: the COI
 * build needs cross-origin isolation, which the web app doesn't set up, so
 * `selectBundle` would never pick it.
 *
 * Imported only when `VITE_BUILD_TARGET` is "web" (see `duckdbBundles`), so
 * the desktop and demo builds don't carry the ~73 MB of WASM.
 */
import type { DuckDBBundles } from "@duckdb/duckdb-wasm";
import mvpModule from "@duckdb/duckdb-wasm/dist/duckdb-mvp.wasm?url";
import mvpWorker from "@duckdb/duckdb-wasm/dist/duckdb-browser-mvp.worker.js?url";
import ehModule from "@duckdb/duckdb-wasm/dist/duckdb-eh.wasm?url";
import ehWorker from "@duckdb/duckdb-wasm/dist/duckdb-browser-eh.worker.js?url";

export const LOCAL_BUNDLES: DuckDBBundles = {
  mvp: { mainModule: mvpModule, mainWorker: mvpWorker },
  eh: { mainModule: ehModule, mainWorker: ehWorker },
};
