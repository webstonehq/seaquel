/**
 * Starts a new DuckDB-WASM instance in its own worker: its bundle chosen
 * (`duckdbBundles`: jsDelivr in the demo, the web build's own copy on web),
 * and a start that fails or hangs reported instead of waited on. The worker
 * script and the wasm are cached by the browser, so a second instance costs
 * a worker and its memory, not a second download.
 */
import { absoluteUrl, duckdbBundles, duckdbWorkerScript, startWithin } from "./duckdb-bundles";

type AsyncDuckDB = import("@duckdb/duckdb-wasm").AsyncDuckDB;

export async function startDuckDb(): Promise<AsyncDuckDB> {
  const duckdb = await import("@duckdb/duckdb-wasm");

  // Web serves its own copy; the demo uses jsDelivr (see duckdbBundles).
  const bundle = await duckdb.selectBundle(await duckdbBundles(duckdb.getJsDelivrBundles));

  const workerUrl = URL.createObjectURL(
    new Blob([duckdbWorkerScript(absoluteUrl(bundle.mainWorker!))], {
      type: "text/javascript",
    }),
  );
  const worker = new Worker(workerUrl);
  // DuckDB's own logger stays quiet: its log lines carry the SQL (and the
  // worker's console is silenced by its script, `duckdbWorkerScript`).
  const db = new duckdb.AsyncDuckDB(new duckdb.VoidLogger(), worker);
  // A worker that can't load its script never answers, so give up on its
  // error event or after a timeout.
  try {
    await startWithin(
      db.instantiate(
        absoluteUrl(bundle.mainModule),
        bundle.pthreadWorker ? absoluteUrl(bundle.pthreadWorker) : undefined,
      ),
      worker,
    );
  } catch (error) {
    worker.terminate();
    throw error;
  } finally {
    URL.revokeObjectURL(workerUrl);
  }
  return db;
}
