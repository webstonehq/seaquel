/**
 * Git service for shared query repositories: the `git` group of the
 * workspace RPC, sent through `core_call` (desktop only; web and the demo
 * have no shared projects).
 *
 * Failures reject with a `CoreCallError` whose `code` is the git code
 * (`CLONE_ERROR`, `PUSH_ERROR`, …) and whose message reads `"CODE: message"`,
 * as the old Tauri commands' errors did through `extractErrorMessage`.
 */

import { CoreCallError, encodeCoreRequest, tauriCoreTransport } from "$lib/storage/rust-client";
import type { GitCredentials, RepoStatus, SyncResult, ConflictContent } from "$lib/types";
import type { CoreResponse } from "$lib/types/generated/CoreResponse";
import type { GitCredentials as WireCredentials } from "$lib/types/generated/GitCredentials";
import type { GitRepoStatus } from "$lib/types/generated/GitRepoStatus";
import type { GitRequest } from "$lib/types/generated/GitRequest";
import type { GitResponse } from "$lib/types/generated/GitResponse";
import type { GitSyncResult } from "$lib/types/generated/GitSyncResult";

type GitMethod = GitRequest["method"];
type GitResult<M extends GitMethod> = Extract<GitResponse, { method: M }>["result"];

/** One git call. */
async function callGit<M extends GitMethod>(
  request: Extract<GitRequest, { method: M }>,
): Promise<GitResult<M>> {
  const response = (await tauriCoreTransport(
    encodeCoreRequest({ method: "git", params: request }),
  )) as CoreResponse;
  if (response?.method !== "git" || response.result?.method !== request.method) {
    // Never echo the response: it can hold file contents.
    throw new CoreCallError({
      code: "PROTOCOL_ERROR",
      message: `expected a git ${request.method} response`,
    });
  }
  return response.result.result as GitResult<M>;
}

/**
 * Clone a Git repository to a local path.
 */
export async function cloneRepo(
  url: string,
  path: string,
  credentials?: GitCredentials,
): Promise<void> {
  await callGit({
    method: "clone",
    params: { url, path, ...(credentials ? { credentials: toWireCredentials(credentials) } : {}) },
  });
}

/**
 * Initialize a new Git repository at the given path.
 */
export async function initRepo(path: string): Promise<void> {
  await callGit({ method: "init", params: { path } });
}

/**
 * Pull changes from remote repository.
 */
export async function pullRepo(path: string, credentials?: GitCredentials): Promise<SyncResult> {
  const result = await callGit({
    method: "pull",
    params: { path, ...(credentials ? { credentials: toWireCredentials(credentials) } : {}) },
  });
  return fromWireSyncResult(result);
}

/**
 * Push local changes to remote repository.
 */
export async function pushRepo(path: string, credentials?: GitCredentials): Promise<SyncResult> {
  const result = await callGit({
    method: "push",
    params: { path, ...(credentials ? { credentials: toWireCredentials(credentials) } : {}) },
  });
  return fromWireSyncResult(result);
}

/**
 * Get the current status of a repository.
 */
export async function getRepoStatus(path: string): Promise<RepoStatus> {
  return fromWireRepoStatus(await callGit({ method: "status", params: { path } }));
}

/**
 * Commit all changes in the repository. Returns the commit id.
 */
export async function commitChanges(path: string, message: string): Promise<string> {
  return callGit({ method: "commit", params: { path, message } });
}

/**
 * Resolve a merge conflict by writing the resolved content.
 */
export async function resolveConflict(
  path: string,
  filePath: string,
  resolution: string | null,
): Promise<void> {
  // `null` keeps the side that deleted the file: Core deletes it.
  await callGit({
    method: "resolveConflict",
    params:
      resolution === null
        ? { path, filePath, resolution: "", delete: true }
        : { path, filePath, resolution },
  });
}

/**
 * Get the conflict content for a file (base, ours, theirs).
 */
export async function getConflictContent(path: string, filePath: string): Promise<ConflictContent> {
  return callGit({ method: "conflictContent", params: { path, filePath } });
}

/**
 * Set or update the remote URL for the repository.
 */
export async function setRemote(path: string, url: string): Promise<void> {
  await callGit({ method: "setRemote", params: { path, url } });
}

/**
 * Get the current remote URL, if any.
 */
export async function getRemoteUrl(path: string): Promise<string | null> {
  return callGit({ method: "remoteUrl", params: { path } });
}

// === Wire mapping (Rust keeps the snake_case it always had) ===

function toWireCredentials(creds: GitCredentials): WireCredentials {
  return {
    username: creds.username ?? null,
    password: creds.password ?? null,
    ssh_key_path: creds.sshKeyPath ?? null,
    ssh_passphrase: creds.sshPassphrase ?? null,
  };
}

function fromWireSyncResult(result: GitSyncResult): SyncResult {
  return {
    success: result.success,
    message: result.message,
    conflicts: result.conflicts,
    filesChanged: result.files_changed,
  };
}

function fromWireRepoStatus(status: GitRepoStatus): RepoStatus {
  return {
    isClean: status.is_clean,
    pendingChanges: status.pending_changes,
    aheadBy: status.ahead_by,
    behindBy: status.behind_by,
    hasConflicts: status.has_conflicts,
    currentBranch: status.current_branch,
    modifiedFiles: status.modified_files,
    untrackedFiles: status.untracked_files,
    conflictFiles: status.conflict_files,
  };
}
