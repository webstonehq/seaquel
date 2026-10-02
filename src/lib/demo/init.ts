/**
 * The demo's start (phase 8, Decision 19), once the page's Core is open
 * (`$lib/demo/core`, from `src/routes/+layout.ts`) and the page has loaded
 * its projects and saved connections:
 *
 * 1. Core stores `demo-connection` the first time and afterwards only marks
 *    it connected (`ensureDemoConnection`), keeping the visitor's labels,
 *    AI flags and project;
 * 2. it is connected as a saved target (`db.connect`), on the page's
 *    DuckDB-WASM, which starts empty on every load;
 * 3. the sample tables are created and filled through `db.execute`, one
 *    statement at a time;
 * 4. the page shows the row, loads the schema and makes it active;
 * 5. the sample dashboard is created if the project has none of its name
 *    (Decision 20).
 *
 * It imports no part of the module or the transport: the page passes its
 * Core in, so this file costs nothing outside the demo build.
 */

import type { WireConnection } from "$lib/hooks/database/library/index.js";
import type { ChangeSeq } from "$lib/hooks/database/library/index.js";
import type { ConnectRequest, DatabaseProvider } from "$lib/providers";
import { log } from "$lib/utils/logger";
import { createDemoDashboard } from "./sample-dashboard";
import { SAMPLE_DATA, SAMPLE_SCHEMA } from "./sample-data";

/** The demo's fixed connection id (Core's `DEMO_CONNECTION_ID`). */
export const DEMO_CONNECTION_ID = "demo-connection";

/** What the start needs of the page's Core (`BrowserCore`). */
export interface DemoCore {
  ensureDemoConnection(): Promise<unknown>;
}

/** What the start needs of the page (`UseDatabase`). */
export type DemoPage = Parameters<typeof createDemoDashboard>[0] & {
  whenReady(): Promise<void>;
  connections: {
    addDemoConnection(
      stored: { value: WireConnection; seq: ChangeSeq },
      providerConnectionId: string,
    ): Promise<string>;
  };
};

/** The sample SQL as statements, in order. */
export function sampleStatements(): string[] {
  return [SAMPLE_SCHEMA, SAMPLE_DATA].flatMap((sql) =>
    sql
      .split(";")
      .map((s) => s.trim())
      .filter((s) => s.length > 0),
  );
}

/**
 * Runs the demo's start on `page`. Throws when the connection can't be
 * stored, connected or shown (the layout says the demo failed to start); a
 * sample statement that fails is logged by its index and skipped.
 */
export async function startDemo(
  page: DemoPage,
  core: DemoCore,
  provider: Pick<DatabaseProvider, "connect" | "execute">,
): Promise<void> {
  // The page's projects and saved connections first, so the stored row is
  // shown with what the page loaded for it (history, chats).
  await page.whenReady();
  const stored = (await core.ensureDemoConnection()) as { value: WireConnection; seq: ChangeSeq };

  const request: ConnectRequest = { target: { type: "saved", id: DEMO_CONNECTION_ID } };
  const providerConnectionId = await provider.connect(request);

  for (const [index, statement] of sampleStatements().entries()) {
    try {
      await provider.execute(providerConnectionId, statement);
    } catch (error) {
      void log.warn(`[Demo] Sample statement ${index} failed:`, errorCode(error));
    }
  }

  await page.connections.addDemoConnection(stored, providerConnectionId);
  await createDemoDashboard(page);
}

/** An error's code for the log, never its message (it can quote the SQL). */
function errorCode(error: unknown): string {
  const code = (error as { code?: unknown } | null)?.code;
  if (typeof code === "string") return code;
  return error instanceof Error ? error.name : typeof error;
}
