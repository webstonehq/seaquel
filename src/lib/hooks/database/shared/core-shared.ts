/**
 * `SharedService` and `ImportsService` over Seaquel Core (desktop only): the
 * `shared` and `imports` RPC groups, through the page's `RustStorageClient`,
 * so a write joins the storage writes' queue and lands in the order this
 * page issued it. Core does the work (paths, files, pairing, the three-way
 * sync, the repo lock); this only carries the calls. A refusal rejects with
 * a `CoreCallError` whose `code` is Core's.
 */
import type {
  ImportsMethod,
  ImportsParams,
  ImportsResult,
  RustStorageClient,
  SharedMethod,
  SharedParams,
  SharedResult,
} from "$lib/storage/rust-client";
import type { ImportSource, ImportsService, RepoPatch, SharedService, SyncTarget } from "./types";

/** What `CoreShared` needs of the storage client: its queued `shared` call. */
export type SharedCaller = Pick<RustStorageClient, "shared">;
/** What `CoreImports` needs of the storage client: its queued `imports` call. */
export type ImportsCaller = Pick<RustStorageClient, "imports">;

export class CoreShared implements SharedService {
  /** `getCaller` is read per call, so the page's client can be swapped (tests). */
  constructor(private readonly getCaller: () => SharedCaller) {}

  private call<M extends SharedMethod>(
    method: M,
    params: SharedParams<M>,
  ): Promise<SharedResult<M>> {
    return this.getCaller().shared(method, params);
  }

  listRepos() {
    return this.call("reposList", undefined);
  }
  registerRepo(path: string, init: { name?: string; remoteUrl?: string } = {}) {
    return this.call("repoRegister", {
      path,
      ...(init.name !== undefined ? { name: init.name } : {}),
      ...(init.remoteUrl !== undefined ? { remoteUrl: init.remoteUrl } : {}),
    });
  }
  updateRepo(id: string, patch: RepoPatch) {
    return this.call("repoUpdate", { id, patch });
  }
  removeRepo(id: string) {
    return this.call("repoRemove", { id });
  }
  linkProject(projectId: string, path: string, share: string[]) {
    return this.call("linkProject", { projectId, path, share });
  }
  unlinkProject(projectId: string, removeImported: boolean) {
    return this.call("unlinkProject", { projectId, removeImported });
  }
  unlinkPreview(projectId: string) {
    return this.call("unlinkPreview", { projectId });
  }
  scan(path: string) {
    return this.call("scan", { path });
  }
  importProjects(path: string, dirs: string[]) {
    return this.call("importProjects", { path, dirs });
  }
  sync(target: SyncTarget) {
    return "projectId" in target
      ? this.call("sync", { projectId: target.projectId })
      : this.call("syncRepo", { repoId: target.repoId });
  }
}

export class CoreImports implements ImportsService {
  constructor(private readonly getCaller: () => ImportsCaller) {}

  private call<M extends ImportsMethod>(
    method: M,
    params: ImportsParams<M>,
  ): Promise<ImportsResult<M>> {
    return this.getCaller().imports(method, params);
  }

  candidates(source: ImportSource, projectId: string) {
    return this.call("candidates", { source, projectId });
  }
  create(source: ImportSource, projectId: string, keys: string[]) {
    return this.call("create", { source, projectId, keys });
  }
}
