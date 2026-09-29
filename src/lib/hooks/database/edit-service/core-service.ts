/**
 * `EditService` over Seaquel Core (desktop and web): `db.planEdits`,
 * `db.applyChanges`, `db.tablePage` and `db.duckdbExtension` through the
 * page's `CoreClient`. Core reads the table's metadata, checks the key,
 * builds with the connection's dialect, classifies and runs; this only
 * carries the calls. Intents and typed changes arrive already in the cell
 * wire format (`./intents`); an extension listing's rows come back decoded.
 */
import { callDb, getCoreClient, type CoreClient } from "$lib/core";
import { toRowObjects } from "$lib/providers/wire";
import { decodeRows } from "$lib/values";
import type {
  ApplyChangesParams,
  ApplyOutcome,
  EditService,
  ExtensionAction,
  PlanEditsParams,
  PlannedChange,
  RunEvent,
  TablePageParams,
} from "./types";

export class CoreEditService implements EditService {
  /** `getClient` is read per call, so the page's client can be swapped (tests). */
  constructor(private readonly getClient: () => CoreClient = getCoreClient) {}

  plan(params: PlanEditsParams): Promise<PlannedChange[]> {
    return callDb(this.getClient(), "planEdits", params);
  }

  apply(params: ApplyChangesParams): Promise<ApplyOutcome> {
    return callDb(this.getClient(), "applyChanges", params);
  }

  tablePage(params: TablePageParams, signal: AbortSignal): AsyncIterable<RunEvent> {
    return this.getClient().stream(
      { method: "db", params: { method: "tablePage", params } },
      { signal },
    );
  }

  async duckdbExtension(
    connectionId: string,
    action: ExtensionAction,
  ): Promise<Record<string, unknown>[] | null> {
    const result = await callDb(this.getClient(), "duckdbExtension", { connectionId, action });
    return result ? toRowObjects(result.columns, decodeRows(result.rows)) : null;
  }
}
