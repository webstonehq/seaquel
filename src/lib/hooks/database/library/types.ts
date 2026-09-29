/**
 * The seam between the library's view models (`ConnectionManager`,
 * `ProjectManager`, `LabelManager`, `SavedQueryManager`) and whatever
 * stores the library (phase 5d-1, Decision 6), like 5c's `EditService`.
 *
 * - `CoreLibrary` (desktop and web): the `library` RPC group. Core checks
 *   the input, assigns ids and times, writes in one transaction (and, on the
 *   desktop, the keychain in the same call), and tells the user's other
 *   windows and tabs (`storageChanged`).
 * - `TsLibrary` (the demo): the same rules in TypeScript over sql.js, until
 *   phase 8.
 *
 * Both take and return the generated wire types: rows are the
 * `seaquel_types::storage` rows (`lastConnected` is text), and every result
 * carries the change `seq` it is at least as new as (Decision 17). A
 * refusal rejects with a `LibraryCallError` (`code`, and `takenBy` for
 * `NAME_TAKEN`). The GUI builds no ids, versions or names: an import's
 * draft sends `renameIfTaken` and Core picks the free name.
 */
import type { ChangeSeq } from "$lib/types/generated/ChangeSeq";
import type { ConnectionDraft } from "$lib/types/generated/ConnectionDraft";
import type { ConnectionLabel } from "$lib/types/generated/ConnectionLabel";
import type { ConnectionPatch } from "$lib/types/generated/ConnectionPatch";
import type { LabelDraft } from "$lib/types/generated/LabelDraft";
import type { LabelPatch } from "$lib/types/generated/LabelPatch";
import type { LabelRemoved } from "$lib/types/generated/LabelRemoved";
import type { PersistedConnection } from "$lib/types/generated/PersistedConnection";
import type { PersistedProject } from "$lib/types/generated/PersistedProject";
import type { PersistedQueryVersion } from "$lib/types/generated/PersistedQueryVersion";
import type { PersistedSavedQuery } from "$lib/types/generated/PersistedSavedQuery";
import type { ProjectDraft } from "$lib/types/generated/ProjectDraft";
import type { ProjectPatch } from "$lib/types/generated/ProjectPatch";
import type { ProjectRemoved } from "$lib/types/generated/ProjectRemoved";
import type { SavedQueryDraft } from "$lib/types/generated/SavedQueryDraft";
import type { SavedQueryPatch } from "$lib/types/generated/SavedQueryPatch";
import type { SavedQueryUpdated } from "$lib/types/generated/SavedQueryUpdated";
import type { SecretChanges } from "$lib/types/generated/SecretChanges";
import type { Seqd } from "$lib/types/generated/Seqd";
import type { StoredKind } from "$lib/types/generated/StoredKind";

export type {
  ChangeSeq,
  ConnectionDraft,
  ConnectionLabel,
  ConnectionPatch,
  LabelDraft,
  LabelPatch,
  LabelRemoved,
  PersistedConnection as WireConnection,
  PersistedProject as WireProject,
  PersistedQueryVersion as WireQueryVersion,
  PersistedSavedQuery as WireSavedQuery,
  ProjectDraft,
  ProjectPatch,
  ProjectRemoved,
  SavedQueryDraft,
  SavedQueryPatch,
  SavedQueryUpdated,
  SecretChanges,
  Seqd,
  StoredKind,
};

export interface LibraryService {
  // -------- Reads --------
  listConnections(): Promise<Seqd<PersistedConnection[]>>;
  listProjects(): Promise<Seqd<PersistedProject[]>>;
  listSavedQueries(projectId: string): Promise<Seqd<PersistedSavedQuery[]>>;
  /** Every version of the project's saved queries, oldest first per query. */
  listQueryVersions(projectId: string): Promise<Seqd<PersistedQueryVersion[]>>;

  // -------- Connections --------
  /** `secrets` only on desktop: web keeps its vault in the browser. */
  createConnection(
    draft: ConnectionDraft,
    secrets?: SecretChanges,
  ): Promise<Seqd<PersistedConnection>>;
  updateConnection(
    id: string,
    patch: ConnectionPatch,
    secrets?: SecretChanges,
  ): Promise<Seqd<PersistedConnection>>;
  removeConnection(id: string): Promise<Seqd<null>>;

  // -------- Projects --------
  createProject(draft: ProjectDraft): Promise<Seqd<PersistedProject>>;
  /** Makes the default project on a file with none; lists the projects. */
  ensureDefaultProject(): Promise<Seqd<PersistedProject[]>>;
  updateProject(id: string, patch: ProjectPatch): Promise<Seqd<PersistedProject>>;
  /** Refuses the last project (`LAST_PROJECT`); returns the connections it removed. */
  removeProject(id: string): Promise<Seqd<ProjectRemoved>>;

  // -------- Custom labels --------
  createLabel(projectId: string, label: LabelDraft): Promise<Seqd<ConnectionLabel>>;
  updateLabel(
    projectId: string,
    labelId: string,
    patch: LabelPatch,
  ): Promise<Seqd<ConnectionLabel>>;
  /** Also strips the label from every connection that had it (returned). */
  removeLabel(projectId: string, labelId: string): Promise<Seqd<LabelRemoved>>;

  // -------- Saved queries --------
  createSavedQuery(draft: SavedQueryDraft): Promise<Seqd<PersistedSavedQuery>>;
  /** A changed text appends a keyframe of the previous text and prunes. */
  updateSavedQuery(id: string, patch: SavedQueryPatch): Promise<Seqd<SavedQueryUpdated>>;
  removeSavedQuery(id: string): Promise<Seqd<null>>;
}

// -------- Error codes the GUI words --------

/** Another row of the scope has the name (after trimming and case folding). */
export const NAME_TAKEN = "NAME_TAKEN";
/** The last project can't be removed. */
export const LAST_PROJECT = "LAST_PROJECT";
export const PROJECT_NOT_FOUND = "PROJECT_NOT_FOUND";
export const SAVED_QUERY_NOT_FOUND = "SAVED_QUERY_NOT_FOUND";
export const LABEL_NOT_FOUND = "LABEL_NOT_FOUND";
/** A saved connection that isn't there (the wire code of `SAVED_CONNECTION_NOT_FOUND`). */
export const CONNECTION_NOT_FOUND = "CONNECTION_NOT_FOUND";
export const INVALID_ARGUMENT = "INVALID_ARGUMENT";

/**
 * A refused or failed library call: `"CODE: message"`, with the code, and
 * for `NAME_TAKEN` the id of the row that has the name.
 */
export class LibraryCallError extends Error {
  readonly code: string;
  readonly takenBy?: string;
  constructor(code: string, message: string, takenBy?: string) {
    super(`${code}: ${message}`);
    this.name = "LibraryCallError";
    this.code = code;
    if (takenBy !== undefined) this.takenBy = takenBy;
  }
}

/** The per-row key the change feed and the `seq` rule use. */
export type RowKind = "connection" | "project" | "savedQuery" | "queryVersion";
export function rowKey(kind: RowKind, id: string): string {
  return `${kind}:${id}`;
}

/** For `NAME_TAKEN`: the id of the row that has the name, if the error says. */
export function takenByOf(error: unknown): string | undefined {
  if (typeof error === "object" && error !== null && "takenBy" in error) {
    const { takenBy } = error as { takenBy?: unknown };
    if (typeof takenBy === "string") return takenBy;
  }
  return undefined;
}
