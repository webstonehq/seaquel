/**
 * Small helpers the view models share when they apply library rows.
 */
import type { DatabaseConnection } from "$lib/types";
import { log } from "$lib/utils/logger";
import type { DatabaseState } from "../state.svelte.js";
import { connectionFromWire, queryVersionFromWire } from "./convert";
import { getLibrary } from "./index";
import { libraryError } from "./messages";
import { rowKey, type ChangeSeq, type ConnectionPatch, type WireConnection } from "./types";

/** The name of the library row `id`, if the page holds it (for `NAME_TAKEN`). */
export function libraryNameOf(state: DatabaseState, id: string): string | undefined {
  const connection = state.connections.find((c) => c.id === id);
  if (connection) return connection.name;
  const project = state.projects.find((p) => p.id === id);
  if (project) return project.name;
  for (const p of state.projects) {
    const label = p.customLabels.find((l) => l.id === id);
    if (label) return label.name;
  }
  for (const queries of Object.values(state.queriesByProject)) {
    const query = queries.find((q) => q.id === id);
    if (query) return query.name;
  }
  return undefined;
}

/** The stored fields of a shown connection, for telling whether a refetch changed it. */
function stored(c: DatabaseConnection): unknown[] {
  return [
    c.name,
    c.type,
    c.host,
    c.port,
    c.databaseName,
    c.username,
    c.sslMode,
    c.connectionString,
    c.sshTunnel,
    c.savePassword,
    c.saveSshPassword,
    c.saveSshKeyPassphrase,
    c.labelIds,
    c.isLocalOnly,
    c.aiShareSchema,
    c.aiShareData,
    c.activeAIProviderId,
    c.activeAIModel,
  ];
}

/** Whether another window's refetch changed what a form edits (not `lastConnected`). */
export function storedFieldsDiffer(a: DatabaseConnection, b: DatabaseConnection): boolean {
  return JSON.stringify(stored(a)) !== JSON.stringify(stored(b));
}

/** Count another window's change to each of `keys`, for the forms editing them. */
export function bumpRevisions(state: DatabaseState, keys: readonly string[]): void {
  if (keys.length === 0) return;
  const next = { ...state.libraryRemoteRevision };
  for (const key of keys) next[key] = (next[key] ?? 0) + 1;
  state.libraryRemoteRevision = next;
}

/**
 * Show one of this page's write answers for connection `row`, taken at
 * `seq`: the row replaces (or joins) the page's copy, keeping its runtime
 * fields, and `keep`'s fields on top.
 */
export function applyConnectionRow(
  state: DatabaseState,
  row: WireConnection,
  seq: ChangeSeq,
  keep?: Partial<DatabaseConnection>,
): DatabaseConnection {
  state.librarySeqs.note(rowKey("connection", row.id), seq);
  const current = state.connections.find((c) => c.id === row.id);
  const next = { ...connectionFromWire(row, current), ...keep };
  state.connections = current
    ? state.connections.map((c) => (c.id === row.id ? next : c))
    : [...state.connections, next];
  return next;
}

/**
 * Change a saved connection's stored fields with `patch` (labels, the AI
 * model, the local-only flag): one targeted call, then the page shows
 * Core's row. A refusal throws a `LibraryError` worded for the user, and
 * changes nothing.
 */
export async function patchConnection(
  state: DatabaseState,
  id: string,
  patch: ConnectionPatch,
): Promise<DatabaseConnection> {
  try {
    const { value, seq } = await state.librarySeqs.write([rowKey("connection", id)], () =>
      getLibrary().updateConnection(id, patch),
    );
    return applyConnectionRow(state, value, seq);
  } catch (error) {
    throw libraryError(error, (other) => libraryNameOf(state, other));
  }
}

/**
 * Read a project's saved-query versions again and show them, if the list is
 * newer than the one shown. After an update that appended or pruned
 * versions: its answer names only that query's changes, so recording its
 * `seq` for the whole list would hide another window's earlier version of
 * another query (C1). A failed read is logged.
 */
export async function refreshQueryVersions(state: DatabaseState, projectId: string): Promise<void> {
  try {
    const { value, seq } = await getLibrary().listQueryVersions(projectId);
    if (!state.librarySeqs.take(rowKey("queryVersion", projectId), seq)) return;
    state.queryVersionsByProject = {
      ...state.queryVersionsByProject,
      [projectId]: value.map(queryVersionFromWire),
    };
  } catch (error) {
    void log.warn("Reading the saved query versions again failed:", error);
  }
}
