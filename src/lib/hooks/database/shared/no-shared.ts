/**
 * Web and the demo have no shared projects and no imports (Decision 48):
 * every call answers `NOT_SUPPORTED`, as Core does for a workspace without
 * `LocalFiles`. The GUI hides the entry points there, so these only stand
 * in for a call that slips through.
 */
import { CoreCallError } from "$lib/storage/rust-client";
import { NOT_SUPPORTED, type ImportsService, type SharedService } from "./types";

function notSupported(what: string): Promise<never> {
  return Promise.reject(
    new CoreCallError({ code: NOT_SUPPORTED, message: `${what} isn't supported here` }),
  );
}

export class NoShared implements SharedService {
  listRepos = () => notSupported("Shared projects");
  registerRepo = () => notSupported("Shared projects");
  updateRepo = () => notSupported("Shared projects");
  removeRepo = () => notSupported("Shared projects");
  linkProject = () => notSupported("Shared projects");
  unlinkProject = () => notSupported("Shared projects");
  unlinkPreview = () => notSupported("Shared projects");
  scan = () => notSupported("Shared projects");
  importProjects = () => notSupported("Shared projects");
  sync = () => notSupported("Shared projects");
}

export class NoImports implements ImportsService {
  candidates = () => notSupported("Importing connections");
  create = () => notSupported("Importing connections");
}
