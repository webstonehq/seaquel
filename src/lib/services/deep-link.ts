/**
 * Deep link handling for seaquel:// URLs.
 * Format: seaquel://open?r=<github-url-to-resource>
 *
 * Users copy a file URL from the GitHub web interface and prepend seaquel://open?r=
 * to create a working deep link. The resource type is inferred from the file path.
 */

import type { useDatabase } from "$lib/hooks/database.svelte.js";
import { deepLinkDialogStore } from "$lib/stores/deep-link-dialog.svelte.js";
import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
import { m } from "$lib/paraglide/messages.js";
import { sharedProjectImportStore } from "$lib/stores/shared-project-import.svelte.js";
import { templatePath } from "$lib/components/sidebar/manage/share-link";

type DatabaseContext = ReturnType<typeof useDatabase>;

export type DeepLinkResourceType = "query" | "connection" | "dashboard" | "project";

export interface DeepLinkAction {
  type: DeepLinkResourceType;
  repoUrl: string;
  branch: string;
  filePath: string;
}

/**
 * Parse a GitHub blob/tree URL into its components.
 * Handles:
 *   HTTPS: https://github.com/{owner}/{repo}/blob/{branch}/{path}
 *   SSH:   git@github.com:{owner}/{repo}.git/blob/{branch}/{path}
 */
export function parseGitHubUrl(
  url: string,
): { repoUrl: string; branch: string; filePath: string } | null {
  // SSH format: git@github.com:owner/repo.git/blob/branch/path
  const sshMatch = url.match(
    /^([\w.-]+@([^:]+):([^/]+\/[^/]+?))(?:\.git)?\/(blob|tree)\/([^/]+)\/(.+?)\/?\s*$/,
  );
  if (sshMatch) {
    const [, repoUrl, , , , branch, filePath] = sshMatch;
    return { repoUrl, branch, filePath };
  }

  // HTTPS format: https://github.com/owner/repo/blob/branch/path
  try {
    const parsed = new URL(url);
    const match = parsed.pathname.match(/^\/([^/]+\/[^/]+)\/(blob|tree)\/([^/]+)\/(.+?)\/?\s*$/);
    if (!match) return null;

    const [, ownerRepo, , branch, filePath] = match;
    return {
      repoUrl: `${parsed.origin}/${ownerRepo}`,
      branch,
      filePath,
    };
  } catch {
    return null;
  }
}

/**
 * Infer the resource type from a file path within a .seaquel/ directory.
 */
export function inferResourceType(filePath: string): DeepLinkResourceType | null {
  if (/\/connections\/[^/]+\.ya?ml$/.test(filePath)) return "connection";
  if (/\/queries\/.*\.sql$/.test(filePath)) return "query";
  if (/\/dashboards\/[^/]+\.json$/.test(filePath)) return "dashboard";
  // Project: path like .seaquel/projects/<name> (directory, no file extension)
  if (/\/projects\/[^/]+\/?$/.test(filePath) && !/\.\w+$/.test(filePath)) return "project";
  return null;
}

/**
 * Parse a seaquel://open?r=<github-url> deep link into a typed action.
 */
export function parseDeepLink(url: string): DeepLinkAction | null {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "seaquel:") return null;

    const host = parsed.hostname || parsed.pathname.replace(/^\/\//, "");
    if (host !== "open") return null;

    const githubUrl = parsed.searchParams.get("r");
    if (!githubUrl) return null;

    const parts = parseGitHubUrl(githubUrl);
    if (!parts) return null;

    const type = inferResourceType(parts.filePath);
    if (!type) return null;

    return { type, ...parts };
  } catch {
    return null;
  }
}

/**
 * Build a seaquel://open?r=<github-url> deep link.
 */
export function buildDeepLinkUrl(githubRepoUrl: string, branch: string, filePath: string): string {
  // For projects (directories), use tree/; for files, use blob/
  const pathType = filePath.includes(".") ? "blob" : "tree";
  const githubUrl = `${githubRepoUrl.replace(/\/+$/, "")}/${pathType}/${branch}/${filePath}`;
  return `seaquel://open?r=${githubUrl}`;
}

/**
 * Normalize a Git URL to a canonical `host/org/repo` form for comparison.
 * Handles HTTPS, SSH (git@), and trailing .git / slashes.
 */
export function normalizeGitUrl(url: string): string {
  let normalized = url.trim();

  // Remove trailing slashes and .git suffix
  normalized = normalized.replace(/\/+$/, "").replace(/\.git$/, "");

  // SSH format: git@github.com:org/repo -> github.com/org/repo
  const sshMatch = normalized.match(/^[\w-]+@([^:]+):(.+)$/);
  if (sshMatch) {
    return `${sshMatch[1]}/${sshMatch[2]}`.toLowerCase();
  }

  // HTTPS format: https://github.com/org/repo -> github.com/org/repo
  try {
    const parsed = new URL(normalized);
    return `${parsed.host}${parsed.pathname}`.toLowerCase();
  } catch {
    return normalized.toLowerCase();
  }
}

/**
 * Handle a deep link URL. Dispatches to the appropriate handler based on resource type.
 *
 * Shared files are Core's (phase 5e): a query or dashboard opens the stored
 * row whose `sharedPath` is the link's path, once the project linked to its
 * directory has synced; a file not stored yet says so (bug 24). No "active
 * repo" is set: the repo is the link's, and the project is the one linked
 * to the file's directory (Task 1, M5).
 */
export async function handleDeepLink(url: string, db: DatabaseContext): Promise<void> {
  const action = parseDeepLink(url);
  if (!action) return;

  await db.whenReady();

  const repo = await resolveRepo(action, db);
  if (!repo) return;

  switch (action.type) {
    case "query":
    case "dashboard":
      await handleFileDeepLink(repo, action.type, action.filePath, db);
      break;
    case "connection":
      await handleConnectionDeepLink(repo, action.filePath, db);
      break;
    case "project":
      await handleProjectDeepLink(repo, action.filePath, db);
      break;
  }
}

type LinkedRepo = { id: string; path: string };

/**
 * Find the local repo matching the deep link, or prompt to clone it.
 */
async function resolveRepo(
  action: DeepLinkAction,
  db: DatabaseContext,
): Promise<LinkedRepo | null> {
  const normalizedActionUrl = normalizeGitUrl(action.repoUrl);

  const findRepo = () =>
    db.state.sharedRepos.find((repo) => normalizeGitUrl(repo.remoteUrl) === normalizedActionUrl);

  const repo = findRepo();
  if (repo) return repo;

  // Repo not cloned — show clone dialog
  const cloned = await deepLinkDialogStore.prompt(
    action.type,
    action.repoUrl,
    action.filePath,
    action.branch,
  );
  if (!cloned) return null;

  return findRepo() ?? null;
}

/** The project directory a repo path names (`.seaquel/projects/<dir>/…`). */
function projectDirOf(filePath: string): string | null {
  return /^\.seaquel\/projects\/([^/]+)/.exec(filePath)?.[1] ?? null;
}

/** The local project linked to `dir` of `repo` (Core's scan says), if any. */
async function linkedProjectFor(
  repo: LinkedRepo,
  dir: string,
  db: DatabaseContext,
): Promise<string | null> {
  const preview = await db.sharedRepos.scan(repo.path);
  const project = preview.projects.find((p) => p.dir === dir);
  return project?.linkedProjectIds.find((id) => db.state.projects.some((p) => p.id === id)) ?? null;
}

/** Show a linked project, synced with its files (activation syncs it). */
async function showSyncedProject(projectId: string, db: DatabaseContext): Promise<void> {
  if (db.state.activeProjectId === projectId) await db.sharedRepos.syncProject(projectId);
  else await db.projects.setActive(projectId);
}

/** A query or dashboard file: the stored row with that `sharedPath`, opened. */
async function handleFileDeepLink(
  repo: LinkedRepo,
  type: "query" | "dashboard",
  filePath: string,
  db: DatabaseContext,
): Promise<void> {
  const dir = projectDirOf(filePath);
  const projectId = dir ? await linkedProjectFor(repo, dir, db) : null;
  if (!projectId) {
    errorToast(m.deep_link_project_not_linked({ path: filePath }));
    return;
  }
  await showSyncedProject(projectId, db);

  if (type === "query") {
    const query = (db.state.queriesByProject[projectId] ?? []).find(
      (q) => q.sharedPath === filePath,
    );
    if (!query) {
      errorToast(m.deep_link_not_stored({ path: filePath }));
      return;
    }
    db.queryTabs.loadQuery(query.id, () => db.ui.setActiveView("query"));
    toast.success(m.deep_link_opened_query({ name: query.name }));
    return;
  }
  const dashboard = (db.state.dashboardsByProject[projectId] ?? []).find(
    (d) => d.sharedPath === filePath,
  );
  if (!dashboard) {
    errorToast(m.deep_link_not_stored({ path: filePath }));
    return;
  }
  db.dashboardTabs.add(dashboard.id, dashboard.name);
  toast.success(m.deep_link_opened_dashboard({ name: dashboard.name }));
}

/**
 * A connection template: in a project linked to its directory, the sync
 * imports it (Q23) and the connection whose template is the link's path
 * opens. In a directory no local project links, the import dialog asks
 * to import that project (Q32: the directory ticked); once imported, which
 * imports its templates (Decision 40), the connection opens. Cancelling
 * does nothing.
 */
async function handleConnectionDeepLink(
  repo: LinkedRepo,
  filePath: string,
  db: DatabaseContext,
): Promise<void> {
  const dir = projectDirOf(filePath);
  if (!dir) {
    errorToast(m.deep_link_not_stored({ path: filePath }));
    return;
  }
  const projectId = await linkedProjectFor(repo, dir, db);
  if (projectId) {
    await showSyncedProject(projectId, db);
    await openTemplateConnection(projectId, filePath, db);
    return;
  }
  const preview = await db.sharedRepos.scan(repo.path);
  const project = preview.projects.find((p) => p.dir === dir);
  if (!project) {
    errorToast(m.deep_link_project_not_found({ name: dir }));
    return;
  }
  sharedProjectImportStore.openWithResults(repo.path, [project], {
    onImported: async (ids) => {
      const imported = ids[0];
      if (imported) await openTemplateConnection(imported, filePath, db);
    },
  });
}

/** Open the connection of `projectId` whose template is `filePath`, or say it isn't there. */
async function openTemplateConnection(
  projectId: string,
  filePath: string,
  db: DatabaseContext,
): Promise<void> {
  const connection = db.state.connections.find(
    (c) => c.projectId === projectId && templatePath(c.sharedConnectionId) === filePath,
  );
  if (!connection) {
    errorToast(m.deep_link_not_stored({ path: filePath }));
    return;
  }
  await db.connectionTabs.open(connection);
}

async function handleProjectDeepLink(
  repo: LinkedRepo,
  filePath: string,
  db: DatabaseContext,
): Promise<void> {
  // Extract project dirName from path like .seaquel/projects/<dirName>
  const dir = filePath.replace(/\/$/, "").split("/").pop();
  if (!dir) {
    errorToast(m.deep_link_invalid_project());
    return;
  }
  const projectId = await linkedProjectFor(repo, dir, db);
  if (projectId) {
    await db.projects.setActive(projectId);
    const name = db.state.projects.find((p) => p.id === projectId)?.name ?? dir;
    toast.info(m.deep_link_project_already_imported({ name }));
    return;
  }
  await importProjectDir(repo, dir, db);
}

/** Import one project directory of `repo` (Core links it and imports its templates). */
async function importProjectDir(repo: LinkedRepo, dir: string, db: DatabaseContext): Promise<void> {
  const preview = await db.sharedRepos.scan(repo.path);
  const project = preview.projects.find((p) => p.dir === dir);
  if (!project) {
    errorToast(m.deep_link_project_not_found({ name: dir }));
    return;
  }
  try {
    const ids = await db.projects.importProjects(repo.path, [dir]);
    if (ids.length > 0) toast.success(m.deep_link_project_imported({ name: project.name }));
  } catch (error) {
    errorToast(error instanceof Error ? error.message : String(error));
  }
}
