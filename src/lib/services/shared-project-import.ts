/**
 * "Import from repo" (the app header and the getting-started tab): Core
 * scans the chosen folder's `.seaquel/projects/`; one project
 * not linked here yet is imported at once, several open the import dialog,
 * none is said. A directory a local project already links is never
 * imported again silently.
 */
import { toast } from "svelte-sonner";
import { m } from "$lib/paraglide/messages.js";
import { errorToast } from "$lib/utils/toast";
import { showErrorUnlessShown } from "$lib/errors";
import type { useDatabase } from "$lib/hooks/database.svelte.js";
import { sharedProjectImportStore } from "$lib/stores/shared-project-import.svelte.js";
import { skipReason } from "$lib/hooks/database/shared/notices";

type DatabaseContext = ReturnType<typeof useDatabase>;

export async function importSharedProjectsFrom(db: DatabaseContext, path: string): Promise<void> {
  const preview = await db.sharedRepos.scan(path);
  if (preview.conflicted) {
    errorToast(m.shared_error_repo_conflicted());
    return;
  }
  // A folder the scan didn't offer (a symlink) is named.
  for (const s of preview.skippedDirs ?? []) {
    toast.info(m.shared_import_skipped_dirs({ dirs: s.dir, reason: skipReason(s.why) }));
  }
  const projects = preview.projects;
  if (projects.length === 0) {
    toast.info(m.shared_import_none_found());
    return;
  }
  if (projects.every((p) => p.linkedProjectIds.length > 0)) {
    toast.info(m.shared_import_already_linked());
    return;
  }
  if (projects.length === 1) {
    await db.projects.importProjects(path, [projects[0].dir]);
    toast.success(m.shared_import_success({ count: 1 }));
    return;
  }
  sharedProjectImportStore.openWithResults(path, projects);
}

/**
 * The import dialog's Import: the ticked directories through Core, then
 * what the dialog was opened with (`onImported`), then the dialog closes.
 * Nothing when the dialog was cancelled. A refusal is said and the dialog
 * stays.
 */
export async function importSelectedProjects(db: DatabaseContext): Promise<void> {
  const store = sharedProjectImportStore;
  const path = store.folderPath;
  const dirs = store.discoveredProjects.filter((p) => p.selected).map((p) => p.dir);
  if (!store.isOpen || !path || dirs.length === 0) return;
  const onImported = store.onImported;
  store.isImporting = true;
  try {
    const ids = await db.projects.importProjects(path, dirs);
    if (ids.length > 0) toast.success(m.shared_import_success({ count: ids.length }));
    store.reset();
    await onImported?.(ids);
  } catch (error) {
    showErrorUnlessShown(error);
    store.isImporting = false;
  }
}
