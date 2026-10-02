/**
 * Picks the `EngineClient` for a connection: the Rust core for every
 * engine, on desktop, on web and in the demo (phase 8: Core runs in the
 * demo's page).
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
import { RustEngineClient } from "./rust-engine-client";
import type { EngineClient } from "./types";

export type { EngineClient, TableMetadata } from "./types";
export { RustEngineClient } from "./rust-engine-client";
export { editorQualifiedTable, quoteIdent, selectPreview } from "./qualified-table";

type EngineConnection = Pick<DatabaseConnection, "id" | "type" | "providerConnectionId">;

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
  return new RustEngineClient(connection.type, getConnectionId);
}
