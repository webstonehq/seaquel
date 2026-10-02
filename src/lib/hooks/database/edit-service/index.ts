/**
 * `getEditService`: the grid's `EditService` for a connection, picked as
 * `getQueryRunner` picks: every build edits in Core (`CoreEditService`),
 * the demo included (phase 8: Core runs in its page).
 */
import type { DatabaseConnection } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "../state.svelte.js";
import { CoreEditService } from "./core-service";
import type { EditService } from "./types";

export * from "./types";
export * from "./intents";
export { CoreEditService } from "./core-service";

let coreService: CoreEditService | null = null;
let override: EditService | null = null;

/** Replace the service every connection gets (tests); `null` goes back to the default. */
export function setEditService(next: EditService | null): void {
  override = next;
}

export async function getEditService(
  _connection: Pick<DatabaseConnection, "id" | "type">,
  _state: Pick<DatabaseState, "schemas">,
  _providers: ProviderRegistry,
): Promise<EditService> {
  if (override) return override;
  return (coreService ??= new CoreEditService());
}
