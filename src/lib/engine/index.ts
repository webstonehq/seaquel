/**
 * Picks the `EngineClient` for a connection: the Rust core for Postgres on
 * desktop and web, the TypeScript adapters everywhere else (other engines,
 * and every engine in the browser demo, which has no Rust core).
 *
 * The provider connection id is read on every call, not when the client is
 * made. `reconnect()` doesn't mutate the connection object: it replaces it in
 * `state.connections` with a copy carrying the new `providerConnectionId`
 * (and `disconnect` replaces it with one whose id is `undefined`). So a
 * client that must survive a reconnect needs the connection list, and looks
 * its connection up by `id` each time. Without one, the client reads the
 * object it was given, which is only right until the next reconnect.
 */

import type { DatabaseConnection } from "$lib/types";
import { isTauri, isWeb } from "$lib/utils/environment";
import { RustEngineClient, type RustEngine } from "./rust-engine-client";
import { TsEngineClient } from "./ts-engine-client";
import type { EngineClient } from "./types";

export type { CastMap, EngineClient, RowRecord, TableMetadata } from "./types";
export { RustEngineClient } from "./rust-engine-client";
export { TsEngineClient, type TsEngineClientOptions } from "./ts-engine-client";

type EngineConnection = Pick<DatabaseConnection, "id" | "type" | "name" | "providerConnectionId">;

/** Engines whose dialect runs in Rust (phase 1: Postgres). */
const RUST_ENGINES: ReadonlySet<string> = new Set<RustEngine>(["postgres"]);

function isRustEngine(type: DatabaseConnection["type"]): type is RustEngine {
  return RUST_ENGINES.has(type);
}

export function usesRustEngine(connection: Pick<DatabaseConnection, "type">): boolean {
  return isRustEngine(connection.type) && (isTauri() || isWeb());
}

/**
 * @param state Where the live connection list is (the app's `DatabaseState`).
 *   Pass it whenever the client may outlive a reconnect.
 */
export function getEngineClient(
  connection: EngineConnection,
  state?: { readonly connections: readonly EngineConnection[] },
): EngineClient {
  const getConnectionId = state
    ? () => state.connections.find((c) => c.id === connection.id)?.providerConnectionId
    : () => connection.providerConnectionId;

  const { type } = connection;
  if (isRustEngine(type) && (isTauri() || isWeb())) {
    return new RustEngineClient(type, getConnectionId);
  }
  return new TsEngineClient({
    type: connection.type,
    connectionName: connection.name,
    getConnectionId,
  });
}
