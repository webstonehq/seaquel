/**
 * The seam between the grid's view models (`QueryCrudManager`,
 * `PendingChangesManager`, `DataTabManager`) and whatever builds and runs
 * their edits (phase 5c, Decision 16), like 5b's `QueryRunner`.
 *
 * `CoreEditService` (every build; the demo's Core runs in the page since
 * phase 8): `db.planEdits`, `db.applyChanges`, `db.tablePage` and
 * `db.duckdbExtension` in Core, which reads the table's metadata, checks the
 * key, builds the SQL with the connection's dialect and runs it.
 *
 * It takes and return the generated wire types. Values in an `Edit` and a
 * typed `Change` are in the cell wire format (`$lib/values` `encodeParam`,
 * see `./intents`), and so are a planned change's `params` and a table
 * page's rows. `plan`, `apply` and `duckdbExtension` reject with a
 * `CoreCallError` (`"CODE: message"`) when Core refuses the call before
 * anything runs; a refusal inside a batch is an `applied` outcome with
 * `failed`. `tablePage` yields `RunEvent`s ending with one `done` or
 * `error`, and aborting `signal` ends it with `CANCELLED`.
 */
import type { ApplyChangesParams } from "$lib/types/generated/ApplyChangesParams";
import type { ApplyOutcome } from "$lib/types/generated/ApplyOutcome";
import type { Change } from "$lib/types/generated/Change";
import type { ChangeSummary } from "$lib/types/generated/ChangeSummary";
import type { Edit } from "$lib/types/generated/Edit";
import type { ExtensionAction } from "$lib/types/generated/ExtensionAction";
import type { PlanEditsParams } from "$lib/types/generated/PlanEditsParams";
import type { PlannedChange } from "$lib/types/generated/PlannedChange";
import type { RunEvent } from "$lib/types/generated/RunEvent";
import type { TablePageParams } from "$lib/types/generated/TablePageParams";
import type { TableQuery } from "$lib/types/generated/TableQuery";
import type { TableTarget } from "$lib/types/generated/TableTarget";

export type {
  ApplyChangesParams,
  ApplyOutcome,
  Change,
  ChangeSummary,
  Edit,
  ExtensionAction,
  PlanEditsParams,
  PlannedChange,
  RunEvent,
  TablePageParams,
  TableQuery,
  TableTarget,
};

export interface EditService {
  /** The queue entries' display fields for `edits`, in order. Runs nothing. */
  plan(params: PlanEditsParams): Promise<PlannedChange[]>;
  /** Apply the queue (or one immediate change) as Decision 5 says. */
  apply(params: ApplyChangesParams): Promise<ApplyOutcome>;
  /** One page of a data tab: a one-statement run's events. */
  tablePage(params: TablePageParams, signal: AbortSignal): AsyncIterable<RunEvent>;
  /**
   * A DuckDB extensions tab action, one statement at a time. `list` returns
   * `duckdb_extensions()`'s rows as objects; the others return nothing.
   */
  duckdbExtension(
    connectionId: string,
    action: ExtensionAction,
  ): Promise<Record<string, unknown>[] | null>;
}

/** A keyed edit matched no row (Decision 4): the GUI words it from the change's target. */
export const NO_ROWS_AFFECTED = "NO_ROWS_AFFECTED";
/** The key isn't the table's primary key, or the table has none (Decision 4). */
export const NOT_EDITABLE = "NOT_EDITABLE";
/**
 * The connection has a transaction the user opened by hand: an atomic
 * apply won't start inside it. The GUI asks to commit or roll it back.
 */
export const TRANSACTION_OPEN = "TRANSACTION_OPEN";
