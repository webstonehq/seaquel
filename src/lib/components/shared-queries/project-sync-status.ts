import type { RepoSyncStatus } from "$lib/types";
import type { SkipReason } from "$lib/types/generated/SkipReason";

/** What a project's sync badge shows: its repo's status, or `skipped`. */
export type ProjectSyncStatus = RepoSyncStatus | "skipped";

/**
 * A project the last sync skipped whole (probe fix 7) shows `skipped`
 * whatever its repo's git status; any other shows the repo's.
 */
export function projectSyncStatus(
  repoStatus: RepoSyncStatus,
  skipped: SkipReason | undefined,
): ProjectSyncStatus {
  return skipped === "tooMany" || skipped === "tooLarge" ? "skipped" : repoStatus;
}
