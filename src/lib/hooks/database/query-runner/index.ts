/**
 * `getQueryRunner`: the editor's `QueryRunner` for a connection. Every
 * build runs in Core (`CoreQueryRunner`): desktop and web, and the demo,
 * whose Core runs in the page (phase 8).
 */
import type { DatabaseConnection } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "../state.svelte.js";
import { CoreQueryRunner } from "./core-runner";
import type { QueryRunner } from "./types";

export * from "./types";
export { CoreQueryRunner } from "./core-runner";

let coreRunner: CoreQueryRunner | null = null;

export async function getQueryRunner(
  _connection: DatabaseConnection,
  _state: DatabaseState,
  _providers: ProviderRegistry,
): Promise<QueryRunner> {
  return (coreRunner ??= new CoreQueryRunner());
}
