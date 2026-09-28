/**
 * The seam between the editor's view model (`QueryExecutionManager`) and
 * whatever runs its SQL (phase 5b, Decision 13).
 *
 * - `CoreQueryRunner` (desktop and web): `db.run` and `db.page` in Core.
 * - `TsQueryRunner` (the demo): the TypeScript runner the GUI used before
 *   5b, over DuckDB-WASM, until phase 8 runs Core in the browser.
 *
 * Both yield the generated `RunEvent`s, in Core's order and cell wire
 * format (`$lib/values`: rows and bind values tagged, decoded by the view
 * model), and end with exactly one `done` or `error`. Aborting `signal`
 * cancels the run and ends the iterator with an `error` whose code is
 * `CANCELLED`.
 */
import type { PageParams } from "$lib/types/generated/PageParams";
import type { RunEvent } from "$lib/types/generated/RunEvent";
import type { RunParams } from "$lib/types/generated/RunParams";

export type { PageParams, RunEvent, RunParams };

export interface QueryRunner {
  run(params: RunParams, signal: AbortSignal): AsyncIterable<RunEvent>;
  page(params: PageParams, signal: AbortSignal): AsyncIterable<RunEvent>;
}

/** Core's code for a run that holds destructive statements and wasn't confirmed. */
export const CONFIRM_REQUIRED = "CONFIRM_REQUIRED";
/** How many destructive statements a `CONFIRM_REQUIRED` lists (`MAX_DESTRUCTIVE_LISTED` in Core). */
export const MAX_DESTRUCTIVE_LISTED = 100;
/** A parameter value that can't be substituted (at the cursor: the whole run). */
export const INVALID_PARAMETERS = "INVALID_PARAMETERS";
/** A page size past the cap, page 0, or a `db.page` of something other than one SELECT. */
export const INVALID_ARGUMENT = "INVALID_ARGUMENT";
