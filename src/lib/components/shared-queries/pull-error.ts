import { errorCode } from "$lib/core/client";
import { m } from "$lib/paraglide/messages.js";

/**
 * The words seaquel-git's refused fast-forward starts with
 * (`PULL_REFUSED_LOCAL_CHANGES`): the pull would overwrite uncommitted
 * changes, so nothing was pulled (phase 5e bug 2). The rest is each path as
 * a JSON string, joined by `, `, then ` and N more` past ten, then ` first`.
 */
export const PULL_REFUSED_LOCAL_CHANGES = "Commit or discard your changes to ";

const REFUSAL = /^(.*?)(?: and (\d+) more)? first$/s;

/**
 * The files a refused fast-forward names, and how many more it left out;
 * `null` for any other failure, or a refusal whose list can't be read.
 */
export function refusedPaths(error: unknown): { paths: string[]; more: number } | null {
  if (errorCode(error) !== "PULL_ERROR") return null;
  const message = error instanceof Error ? error.message : String(error);
  const at = message.indexOf(PULL_REFUSED_LOCAL_CHANGES);
  if (at === -1) return null;
  const match = REFUSAL.exec(message.slice(at + PULL_REFUSED_LOCAL_CHANGES.length));
  if (!match) return null;
  let paths: unknown;
  try {
    paths = JSON.parse(`[${match[1]}]`);
  } catch {
    return null;
  }
  if (
    !Array.isArray(paths) ||
    paths.length === 0 ||
    !paths.every((p): p is string => typeof p === "string")
  ) {
    return null;
  }
  return { paths, more: match[2] ? Number(match[2]) : 0 };
}

/**
 * The toast for a failed pull. A fast-forward refused over uncommitted
 * changes says they're kept and which files to commit or discard first, in
 * the app's language; any other failure is `failed` with its message.
 */
export function pullFailureText(error: unknown, failed: (message: string) => string): string {
  const refused = refusedPaths(error);
  if (refused) {
    const files = refused.paths.join(", ");
    return refused.more > 0
      ? m.shared_pull_local_changes_more({ files, count: refused.more })
      : m.shared_pull_local_changes({ files });
  }
  return failed(error instanceof Error ? error.message : String(error));
}
