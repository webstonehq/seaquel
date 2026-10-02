/**
 * The page's DuckDB-WASM instances, each started on first use, each with
 * its own catalog (Decision 12, as amended in Task 6's review):
 *
 * - `pageDuckDb`: the demo's, which its Core drives through the bridge
 *   (`$lib/core/browser`);
 * - `tutorialDuckDb`: the tutorial's (`DuckDBProvider`, web and the demo),
 *   so Learn's tables never show in the demo connection and a visitor's
 *   SQL in either can't break or drop the other's.
 *
 * A failed start is forgotten, so the next call tries again.
 */
import { startDuckDb } from "./duckdb-start";

type AsyncDuckDB = import("@duckdb/duckdb-wasm").AsyncDuckDB;

/** `startDuckDb` once, until it fails. */
function once(): () => Promise<AsyncDuckDB> {
  let starting: Promise<AsyncDuckDB> | null = null;
  return () => {
    starting ??= startDuckDb().catch((error: unknown) => {
      starting = null;
      throw error;
    });
    return starting;
  };
}

/** The demo's DuckDB (its Core's bridge). */
export const pageDuckDb = once();

/** The tutorial's DuckDB, apart from the demo's. */
export const tutorialDuckDb = once();
