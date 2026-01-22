/**
 * Git service for interacting with shared query repositories.
 * Wraps Tauri commands for Git operations.
 */

import { invoke } from '@tauri-apps/api/core';
import type { GitCredentials, RepoStatus, SyncResult, ConflictContent } from '$lib/types';

/**
 * Clone a Git repository to a local path.
 */
export async function cloneRepo(
	url: string,
	path: string,
	credentials?: GitCredentials
): Promise<void> {
	await invoke('git_clone_repo', {
		url,
		path,
		credentials: credentials ? toRustCredentials(credentials) : null
	});
}

/**
 * Initialize a new Git repository at the given path.
 */
export async function initRepo(path: string): Promise<void> {
	await invoke('git_init_repo', { path });
}

/**
 * Pull changes from remote repository.
 */
export async function pullRepo(
	path: string,
	credentials?: GitCredentials
): Promise<SyncResult> {
	const result = await invoke<RustSyncResult>('git_pull_repo', {
		path,
		credentials: credentials ? toRustCredentials(credentials) : null
	});
	return fromRustSyncResult(result);
}

/**
 * Push local changes to remote repository.
 */
export async function pushRepo(
	path: string,
	credentials?: GitCredentials
): Promise<SyncResult> {
	const result = await invoke<RustSyncResult>('git_push_repo', {
		path,
		credentials: credentials ? toRustCredentials(credentials) : null
	});
	return fromRustSyncResult(result);
}

/**
 * Get the current status of a repository.
 */
export async function getRepoStatus(path: string): Promise<RepoStatus> {
	const result = await invoke<RustRepoStatus>('git_get_repo_status', { path });
	return fromRustRepoStatus(result);
}

/**
 * Commit all changes in the repository.
 */
export async function commitChanges(path: string, message: string): Promise<string> {
	return invoke<string>('git_commit_changes', { path, message });
}

/**
 * Stage a specific file.
 */
export async function stageFile(path: string, filePath: string): Promise<void> {
	await invoke('git_stage_file', { path, filePath });
}

/**
 * Discard changes to a specific file.
 */
export async function discardFile(path: string, filePath: string): Promise<void> {
	await invoke('git_discard_file', { path, filePath });
}

/**
 * Resolve a merge conflict by writing the resolved content.
 */
export async function resolveConflict(
	path: string,
	filePath: string,
	resolution: string
): Promise<void> {
	await invoke('git_resolve_conflict', { path, filePath, resolution });
}

/**
 * Get the conflict content for a file (base, ours, theirs).
 */
export async function getConflictContent(
	path: string,
	filePath: string
): Promise<ConflictContent> {
	return invoke<ConflictContent>('git_get_conflict_content', { path, filePath });
}

/**
 * Set or update the remote URL for the repository.
 */
export async function setRemote(path: string, url: string): Promise<void> {
	await invoke('git_set_remote', { path, url });
}

/**
 * Get the current remote URL, if any.
 */
export async function getRemoteUrl(path: string): Promise<string | null> {
	return invoke<string | null>('git_get_remote_url', { path });
}

// === Type conversion helpers (Rust uses snake_case, TS uses camelCase) ===

interface RustCredentials {
	username: string | null;
	password: string | null;
	ssh_key_path: string | null;
	ssh_passphrase: string | null;
}

interface RustSyncResult {
	success: boolean;
	message: string;
	conflicts: string[];
	files_changed: string[];
}

interface RustRepoStatus {
	is_clean: boolean;
	pending_changes: number;
	ahead_by: number;
	behind_by: number;
	has_conflicts: boolean;
	current_branch: string;
	modified_files: string[];
	untracked_files: string[];
}

function toRustCredentials(creds: GitCredentials): RustCredentials {
	return {
		username: creds.username ?? null,
		password: creds.password ?? null,
		ssh_key_path: creds.sshKeyPath ?? null,
		ssh_passphrase: creds.sshPassphrase ?? null
	};
}

function fromRustSyncResult(result: RustSyncResult): SyncResult {
	return {
		success: result.success,
		message: result.message,
		conflicts: result.conflicts,
		filesChanged: result.files_changed
	};
}

function fromRustRepoStatus(status: RustRepoStatus): RepoStatus {
	return {
		isClean: status.is_clean,
		pendingChanges: status.pending_changes,
		aheadBy: status.ahead_by,
		behindBy: status.behind_by,
		hasConflicts: status.has_conflicts,
		currentBranch: status.current_branch,
		modifiedFiles: status.modified_files,
		untrackedFiles: status.untracked_files
	};
}
