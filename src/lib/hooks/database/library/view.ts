/**
 * Small helpers the view models share when they apply library rows.
 */
import type { DatabaseConnection } from "$lib/types";
import { reportProjection } from "../shared/projection.js";
import { log } from "$lib/utils/logger";
import { errorToast } from "$lib/utils/toast";
import { m } from "$lib/paraglide/messages.js";
import type { DatabaseState } from "../state.svelte.js";
import { connectionFromWire, queryVersionFromWire } from "./convert";
import { getLibrary } from "./index";
import { libraryError, libraryErrorMessage } from "./messages";
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
    const answer = await state.librarySeqs.write([rowKey("connection", id)], () =>
      getLibrary().updateConnection(id, patch),
    );
    const connection = applyConnectionRow(state, answer.value, answer.seq);
    // Turning local-only on takes a linked connection's template out.
    reportProjection(answer, connection.projectId, { removal: patch.isLocalOnly === true });
    return connection;
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

/**
 * Show a project's connection order taken at `seq`, if it is newer than the
 * one shown (the order is shared by the project's windows).
 */
export function applyConnectionOrder(
  state: DatabaseState,
  projectId: string,
  order: readonly string[],
  seq: ChangeSeq,
): void {
  if (!state.librarySeqs.take(rowKey("projectSidebar", projectId), seq)) return;
  state.connectionOrderByProject = { ...state.connectionOrderByProject, [projectId]: [...order] };
  state.connectionOrderStored.set(projectId, [...order]);
}

/**
 * Store a project's connection order if the page's differs from the one it
 * last read or stored (a connection added since, which only appends to the
 * page's order): what the project state's save used to carry. A project
 * whose order was never read is left alone.
 */
export async function storeConnectionOrderIfChanged(
  state: DatabaseState,
  projectId: string,
): Promise<void> {
  const stored = state.connectionOrderStored.get(projectId);
  const shown = state.connectionOrderByProject[projectId];
  if (!stored || !shown) return;
  if (stored.length === shown.length && stored.every((id, i) => id === shown[i])) return;
  await storeConnectionOrder(state, projectId);
}

/**
 * Read a project's connection order (`projectSidebarGet`) and show it by
 * the `seq` rule, after this page's own order writes for it have answered.
 * A failed read is logged and leaves the order shown.
 */
export async function refreshConnectionOrder(
  state: DatabaseState,
  projectId: string,
): Promise<void> {
  const key = rowKey("projectSidebar", projectId);
  try {
    await state.librarySeqs.settled(key);
    const { value, seq } = await getLibrary().getProjectSidebar(projectId);
    applyConnectionOrder(state, projectId, value, seq);
  } catch (error) {
    void log.warn(`Reading the connection order of project ${projectId} failed:`, error);
  }
}

/**
 * Store a project's connection order as the page shows it, at once
 * (`projectSidebarSet`; it isn't part of a window's view state). Another
 * window of the project sees it through its `project` event. A failure is
 * shown and leaves the page's order as it is.
 */
export async function storeConnectionOrder(state: DatabaseState, projectId: string): Promise<void> {
  const key = rowKey("projectSidebar", projectId);
  const order = [...(state.connectionOrderByProject[projectId] ?? [])];
  try {
    const { seq } = await state.librarySeqs.write([key], () =>
      getLibrary().setProjectSidebar(projectId, order),
    );
    state.librarySeqs.note(key, seq);
    state.connectionOrderStored.set(projectId, order);
  } catch (error) {
    void log.warn(`Storing the connection order of project ${projectId} failed:`, error);
    errorToast(
      m.connection_order_save_failed({
        message: libraryErrorMessage(error, (id) => libraryNameOf(state, id)),
      }),
    );
  }
}
