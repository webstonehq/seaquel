/**
 * `QueryRunner` over Seaquel Core (desktop and web): `db.run` and `db.page`
 * through the page's `CoreClient`. Core plans the run (split, statement at
 * the cursor, `{{param}}` substitution, query type, destructive check),
 * executes it, times it and records history; this only carries the events.
 */
import { getCoreClient, type CoreClient } from "$lib/core";
import type { PageParams, QueryRunner, RunEvent, RunParams } from "./types";

export class CoreQueryRunner implements QueryRunner {
  /** `getClient` is read per call, so the page's client can be swapped (tests). */
  constructor(private readonly getClient: () => CoreClient = getCoreClient) {}

  run(params: RunParams, signal: AbortSignal): AsyncIterable<RunEvent> {
    return this.getClient().stream({ method: "db", params: { method: "run", params } }, { signal });
  }

  page(params: PageParams, signal: AbortSignal): AsyncIterable<RunEvent> {
    return this.getClient().stream(
      { method: "db", params: { method: "page", params } },
      { signal },
    );
  }
}
