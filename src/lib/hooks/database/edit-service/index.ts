/**
 * `getEditService`: the grid's `EditService` for a connection, picked as
 * `getQueryRunner` picks: desktop and web edit in Core (`CoreEditService`);
 * the demo, which has no Core, keeps the TypeScript path (`TsEditService`)
 * until phase 8.
 */
import { isTauri, isWeb } from "$lib/utils/environment";
import { getAdapter } from "$lib/db";
import { getStorage } from "$lib/storage";
import type { DatabaseConnection } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "../state.svelte.js";
import { CoreEditService } from "./core-service";
import { TsEditService } from "./ts-service";
import type { EditService } from "./types";

export * from "./types";
export * from "./intents";
export { CoreEditService } from "./core-service";
export { TsEditService, type TsEditServiceContext } from "./ts-service";

let coreService: CoreEditService | null = null;
let override: EditService | null = null;

/** Replace the service every connection gets (tests); `null` goes back to the default. */
export function setEditService(next: EditService | null): void {
  override = next;
}

export async function getEditService(
  connection: Pick<DatabaseConnection, "id" | "type">,
  state: Pick<DatabaseState, "schemas">,
  providers: ProviderRegistry,
): Promise<EditService> {
  if (override) return override;
  if (isTauri() || isWeb()) return (coreService ??= new CoreEditService());
  const provider = await providers.getForType(connection.type);
  return new TsEditService({
    provider,
    engine: connection.type,
    adapter: getAdapter(connection.type),
    columnsOf: (schema, table) =>
      (state.schemas[connection.id] ?? [])
        .find((t) => t.schema === schema && t.name === table)
        ?.columns.map((c) => ({ name: c.name, type: c.type })) ?? [],
    appendHistory: (item) => getStorage().queryHistory.append(item),
  });
}
