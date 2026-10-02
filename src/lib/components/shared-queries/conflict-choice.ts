import type { ConflictContent } from "$lib/types";

/**
 * What resolving a conflicted file with `side` writes: that side's text, or
 * `null` when that side deleted the file (a modify/delete conflict), which
 * deletes it instead of writing an empty file (phase 5e probe fix 5).
 */
export function conflictChoice(
  content: ConflictContent,
  side: "ours" | "theirs" | "base",
): string | null {
  if (side === "ours" && content.oursDeleted) return null;
  if (side === "theirs" && content.theirsDeleted) return null;
  return content[side] ?? "";
}
