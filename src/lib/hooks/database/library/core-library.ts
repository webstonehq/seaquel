/**
 * `LibraryService` over Seaquel Core (desktop and web): the `library` RPC
 * group, through the page's `RustStorageClient` so every write joins the
 * storage writes' queue and lands in the order this page issued it
 * (Decision 3). Core checks, assigns ids and times, writes one transaction
 * (secrets first, on the desktop) and emits `storageChanged`; this only
 * carries the calls. A refusal rejects with a `CoreCallError` whose `code`
 * is Core's and, for `NAME_TAKEN`, whose `takenBy` names the row.
 */
import type {
  LibraryMethod,
  LibraryParams,
  LibraryResult,
  RustStorageClient,
} from "$lib/storage/rust-client";
import type {
  ConnectionDraft,
  ConnectionPatch,
  LabelDraft,
  LabelPatch,
  LibraryService,
  ProjectDraft,
  ProjectPatch,
  SavedQueryDraft,
  SavedQueryPatch,
  SecretChanges,
} from "./types";

/** What `CoreLibrary` needs of the storage client: its queued `library` call. */
export type LibraryCaller = Pick<RustStorageClient, "library">;

/** `secrets` only when it holds a change: an empty object is left out. */
function withSecrets<T extends object>(params: T, secrets?: SecretChanges) {
  return secrets && Object.keys(secrets).length > 0 ? { ...params, secrets } : params;
}

export class CoreLibrary implements LibraryService {
  /** `getCaller` is read per call, so the page's client can be swapped (tests). */
  constructor(private readonly getCaller: () => LibraryCaller) {}

  private call<M extends LibraryMethod>(
    method: M,
    params: LibraryParams<M>,
  ): Promise<LibraryResult<M>> {
    return this.getCaller().library(method, params);
  }

  listConnections() {
    return this.call("connectionsList", undefined);
  }
  listProjects() {
    return this.call("projectsList", undefined);
  }
  listSavedQueries(projectId: string) {
    return this.call("savedQueriesList", { projectId });
  }
  listQueryVersions(projectId: string) {
    return this.call("queryVersionsList", { projectId });
  }

  createConnection(connection: ConnectionDraft, secrets?: SecretChanges) {
    return this.call("connectionCreate", withSecrets({ connection }, secrets));
  }
  updateConnection(id: string, patch: ConnectionPatch, secrets?: SecretChanges) {
    return this.call("connectionUpdate", withSecrets({ id, patch }, secrets));
  }
  removeConnection(id: string) {
    return this.call("connectionRemove", { id });
  }

  createProject(project: ProjectDraft) {
    return this.call("projectCreate", { project });
  }
  ensureDefaultProject() {
    return this.call("projectEnsureDefault", undefined);
  }
  updateProject(id: string, patch: ProjectPatch) {
    return this.call("projectUpdate", { id, patch });
  }
  removeProject(id: string) {
    return this.call("projectRemove", { id });
  }

  createLabel(projectId: string, label: LabelDraft) {
    return this.call("labelCreate", { projectId, label });
  }
  updateLabel(projectId: string, labelId: string, patch: LabelPatch) {
    return this.call("labelUpdate", { projectId, labelId, patch });
  }
  removeLabel(projectId: string, labelId: string) {
    return this.call("labelRemove", { projectId, labelId });
  }

  createSavedQuery(query: SavedQueryDraft) {
    return this.call("savedQueryCreate", { query });
  }
  updateSavedQuery(id: string, patch: SavedQueryPatch) {
    return this.call("savedQueryUpdate", { id, patch });
  }
  removeSavedQuery(id: string) {
    return this.call("savedQueryRemove", { id });
  }
}
