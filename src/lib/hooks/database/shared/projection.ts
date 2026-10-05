/**
 * What a library write did to its row's file in a shared project,
 * said to the user. Core publishes inside the library call and answers
 * `projection` on the `Seqd`; nothing is published for a row that isn't
 * shared or a project without a link, and the field is then absent.
 *
 * - `written`/`deleted`: the repo's status changed (the sync button's
 *   pending count), so it is read again.
 * - `failed` with `FILE_CHANGED`: a teammate changed the file since the last
 *   sync, so Core didn't overwrite it and synced the project instead.
 *   On an edit the repo's version is shown and the user's is in the history;
 *   on a removal or unshare the repo's version is kept and comes back. Either
 *   way the sync changed rows this page shows, and its events carry this
 *   page's origin (the feed skips them), so the project's rows are read again.
 * - any other `failed`: the row is stored and the file isn't; the next sync
 *   writes it (R6's "Saved. The shared file couldn't be written").
 */
import { toast } from "svelte-sonner";
import { m } from "$lib/paraglide/messages.js";
import { errorToast } from "$lib/utils/toast";
import { log } from "$lib/utils/logger";
import { FILE_CHANGED, type ProjectionOutcome } from "./types";

/** How the page follows up on a publish; set once by `UseDatabase`. */
export interface ProjectionHooks {
  /** A sync ran inside the call: read the project's rows again. */
  rowsChanged: (projectId: string) => Promise<void>;
  /** A file was written or deleted: read the repo's status again. */
  filesChanged: (projectId: string) => Promise<void>;
}

let hooks: ProjectionHooks | null = null;

/** Set (or, with `null`, clear) how the page follows up on a publish. */
export function setProjectionHooks(next: ProjectionHooks | null): void {
  hooks = next;
}

/**
 * Say what `answer.projection` reports for a write to a row of `projectId`.
 * `removal`: the write removed or unshared the row.
 */
export function reportProjection(
  answer: { projection?: ProjectionOutcome } | null | undefined,
  projectId: string | null | undefined,
  { removal = false }: { removal?: boolean } = {},
): void {
  const outcome = answer?.projection;
  if (!outcome) return;
  const follow = (fn: ((projectId: string) => Promise<void>) | undefined) => {
    if (!projectId || !fn) return;
    fn(projectId).catch((error: unknown) => {
      void log.warn("Following up on a shared file write failed:", error);
    });
  };
  if (outcome.status !== "failed") {
    follow(hooks?.filesChanged);
    return;
  }
  if (outcome.code === FILE_CHANGED) {
    toast.warning(removal ? m.shared_file_changed_removed() : m.shared_file_changed_edit());
    follow(hooks?.rowsChanged);
    return;
  }
  errorToast(m.shared_file_write_failed({ message: outcome.message ?? outcome.code ?? "" }));
}
