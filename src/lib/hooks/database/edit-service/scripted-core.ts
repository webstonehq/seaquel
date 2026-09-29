/**
 * A scripted `CoreClient` for the edit view models' tests (never imported by
 * app code): answers `db.planEdits`, `db.applyChanges` and
 * `db.duckdbExtension` from handlers, streams `db.tablePage` from a
 * function, and records every call's params as sent.
 */
import type { CoreClient, StreamRequest } from "$lib/core";
import { CoreCallError } from "$lib/storage/rust-client";
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import type { CoreResponse } from "$lib/types/generated/CoreResponse";
import type { Edit } from "$lib/types/generated/Edit";
import type { PlannedChange } from "$lib/types/generated/PlannedChange";
import type {
  ApplyChangesParams,
  ApplyOutcome,
  ExtensionAction,
  PlanEditsParams,
  RunEvent,
  TablePageParams,
} from "./types";

export interface ScriptedCall {
  method: string;
  params: unknown;
}

export interface CoreScript {
  planEdits?: (params: PlanEditsParams) => PlannedChange[] | Promise<PlannedChange[]>;
  applyChanges?: (params: ApplyChangesParams) => ApplyOutcome | Promise<ApplyOutcome>;
  duckdbExtension?: (params: { connectionId: string; action: ExtensionAction }) => unknown;
  tablePage?: (params: TablePageParams, signal?: AbortSignal) => AsyncIterable<RunEvent>;
}

/** A refusal as Core sends it: the call rejects with `CODE: message`. */
export function refusal(code: string, message = "refused"): CoreCallError {
  return new CoreCallError({ code, message });
}

/** A plausible planned change for `edit` (tests that don't look at the SQL). */
export function plannedFor(edit: Edit): PlannedChange {
  const table = edit.target.table;
  switch (edit.type) {
    case "updateCell":
      return {
        sql: `UPDATE "${table}" SET "${edit.column}" = $1 WHERE …`,
        params: [edit.value, ...edit.key.map(([, v]) => v)],
        queryType: "update",
        dml: true,
        summary: { verb: "update", table, column: edit.column },
      };
    case "setDefault":
      return {
        sql: `UPDATE "${table}" SET "${edit.column}" = DEFAULT WHERE …`,
        params: edit.key.map(([, v]) => v),
        queryType: "update",
        dml: true,
        summary: { verb: "update", table, column: edit.column },
      };
    case "insertRow":
      return {
        sql: `INSERT INTO "${table}" …`,
        params: edit.values.map(([, v]) => v),
        queryType: "insert",
        dml: true,
        summary: { verb: "insert", table, column: null },
      };
    case "deleteRow":
      return {
        sql: `DELETE FROM "${table}" WHERE …`,
        params: edit.key.map(([, v]) => v),
        queryType: "delete",
        dml: true,
        summary: { verb: "delete", table, column: null },
      };
    case "truncateTable":
      return {
        sql: `TRUNCATE TABLE "${table}"`,
        params: [],
        queryType: "other",
        dml: false,
        summary: { verb: "truncate", table, column: null },
      };
    case "dropObject":
      return {
        sql: `DROP ${edit.kind === "table" ? "TABLE" : "VIEW"} "${table}"`,
        params: [],
        queryType: "other",
        dml: false,
        summary: { verb: edit.kind === "table" ? "dropTable" : "dropView", table, column: null },
      };
  }
}

/** Everything applied: one result per change (`single`/`inOrder`), none for `atomic`. */
export function appliedAll(params: ApplyChangesParams): ApplyOutcome {
  const n = params.changes.length;
  const atomic = n > 1;
  return {
    outcome: "applied",
    mode: n === 1 ? "single" : "atomic",
    applied: n,
    results: atomic ? [] : params.changes.map((c) => ({ id: c.id, rowsAffected: 1 })),
    ddl: false,
    history: [],
  };
}

export function scriptedCore(script: CoreScript = {}) {
  const calls: ScriptedCall[] = [];
  const handlers: CoreScript = {
    planEdits: (p) => p.edits.map(plannedFor),
    applyChanges: appliedAll,
    ...script,
  };
  const client: CoreClient = {
    async call(request: CoreRequest): Promise<CoreResponse> {
      if (request.method !== "db") throw refusal("UNSCRIPTED", request.method);
      const { method, params } = request.params as { method: string; params: unknown };
      calls.push({ method, params: structuredClone(params) });
      const handler = (handlers as Record<string, ((p: unknown) => unknown) | undefined>)[method];
      if (!handler) throw refusal("UNSCRIPTED", method);
      const result = await handler(params);
      return { method: "db", result: { method, result } } as unknown as CoreResponse;
    },
    stream<R extends StreamRequest>(request: R, options?: { signal?: AbortSignal }) {
      const { method, params } = request.params as { method: string; params: unknown };
      calls.push({ method, params: structuredClone(params) });
      if (method !== "tablePage" || !handlers.tablePage) throw refusal("UNSCRIPTED", method);
      return handlers.tablePage(params as TablePageParams, options?.signal) as never;
    },
    events: () => () => {},
    onResubscribed: () => () => {},
    onEventsUnavailable: () => () => {},
  };
  const of = (method: string) => calls.filter((c) => c.method === method).map((c) => c.params);
  return { client, calls, of };
}
