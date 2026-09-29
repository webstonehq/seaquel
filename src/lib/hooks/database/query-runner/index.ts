/**
 * `getQueryRunner`: the editor's `QueryRunner` for a connection, picked
 * with the provider registry's rule. Desktop and web run in Core
 * (`CoreQueryRunner`); the demo, which has no Core, keeps the TypeScript
 * runner (`TsQueryRunner`) until phase 8.
 */
import { isTauri, isWeb } from "$lib/utils/environment";
import { getAdapter } from "$lib/db";
import { getStorage } from "$lib/storage";
import type { DatabaseConnection } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "../state.svelte.js";
import { CoreQueryRunner } from "./core-runner";
import { TsQueryRunner } from "./ts-runner";
import type { QueryRunner } from "./types";

export * from "./types";
export { CoreQueryRunner } from "./core-runner";
export { TsQueryRunner, SQL_CHECK_FAILED, type TsRunnerContext } from "./ts-runner";

let coreRunner: CoreQueryRunner | null = null;

export async function getQueryRunner(
  connection: DatabaseConnection,
  _state: DatabaseState,
  providers: ProviderRegistry,
): Promise<QueryRunner> {
  if (isTauri() || isWeb()) return (coreRunner ??= new CoreQueryRunner());
  const provider = await providers.getForType(connection.type);
  return new TsQueryRunner({
    provider,
    engine: connection.type,
    // The demo's dialect pages (phase 5c, Decision 14: the engine client has no `paginate`).
    paginate: async (sql, limit, offset) =>
      getAdapter(connection.type).paginateQuery(sql, limit, offset),
    appendHistory: (item) => getStorage().queryHistory.append(item),
  });
}
