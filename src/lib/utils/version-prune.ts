/**
 * Version pruning, planned in TypeScript and run by storage.
 *
 * Query versions are diff-match-patch deltas, which count UTF-16 code units,
 * so the text of the oldest surviving version is resolved here, where the
 * diffs are made. Storage then only deletes and promotes, in one
 * transaction (`QueryVersionsPrune`, `DashboardVersionsPrune`).
 */

import type { PersistedDashboardVersion, PersistedQueryVersion } from "$lib/types";
import type { DashboardVersionsPrune } from "$lib/types/generated/DashboardVersionsPrune";
import type { QueryVersionsPrune } from "$lib/types/generated/QueryVersionsPrune";
import { resolveVersions } from "./query-versions";

/**
 * Keep the newest `keepCount` versions of a saved query. The oldest survivor
 * becomes a keyframe if it's a delta. `null` when there's nothing to do,
 * including `keepCount` 0, which keeps everything.
 */
export function planQueryVersionsPrune(
  savedQueryId: string,
  versions: PersistedQueryVersion[],
  keepCount: number,
): QueryVersionsPrune | null {
  if (versions.length <= keepCount) return null;
  const newestFirst = [...versions].sort((a, b) => b.version - a.version);
  const oldestSurvivor = newestFirst[keepCount - 1];
  if (oldestSurvivor === undefined) return null;

  const deleteIds = versions.filter((v) => v.version < oldestSurvivor.version).map((v) => v.id);
  let promote: QueryVersionsPrune["promote"];
  if (oldestSurvivor.snapshot === null) {
    // Resolve before deleting: the survivor's text depends on the versions
    // that are about to go.
    const resolved = resolveVersions(
      versions.map((v) => ({ ...v, createdAt: new Date(v.createdAt) })),
    ).find((r) => r.id === oldestSurvivor.id);
    if (resolved) promote = { id: oldestSurvivor.id, snapshot: resolved.query };
  }
  if (deleteIds.length === 0 && !promote) return null;
  return promote ? { savedQueryId, deleteIds, promote } : { savedQueryId, deleteIds };
}

/**
 * Keep the newest `keepCount` versions of a dashboard. Unlike query
 * versions, `keepCount` 0 deletes every version (the SQL this replaces used
 * `LIMIT 1 OFFSET keepCount`). `null` when there's nothing to delete.
 */
export function planDashboardVersionsPrune(
  dashboardId: string,
  versions: PersistedDashboardVersion[],
  keepCount: number,
): DashboardVersionsPrune | null {
  const newestFirst = [...versions].sort((a, b) => b.version - a.version);
  const threshold = newestFirst[Math.max(keepCount, 0)];
  if (threshold === undefined) return null;
  const deleteIds = versions.filter((v) => v.version <= threshold.version).map((v) => v.id);
  return deleteIds.length === 0 ? null : { dashboardId, deleteIds };
}
