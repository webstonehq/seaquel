import type { SchemaTable } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";

/**
 * Put freshly loaded metadata for `table` into the connection's schema cache
 * (`state.schemas`), replacing the entry with the same schema and name. A
 * table that isn't in the cache's table list is left out.
 */
export function storeTableMetadata(
  state: Pick<DatabaseState, "schemas">,
  connectionId: string,
  table: SchemaTable,
): void {
  const connectionSchemas = [...(state.schemas[connectionId] ?? [])];
  const tableIndex = connectionSchemas.findIndex(
    (t) => t.name === table.name && t.schema === table.schema,
  );
  if (tableIndex >= 0) {
    connectionSchemas[tableIndex] = table;
  }
  state.schemas = {
    ...state.schemas,
    [connectionId]: connectionSchemas,
  };
}
