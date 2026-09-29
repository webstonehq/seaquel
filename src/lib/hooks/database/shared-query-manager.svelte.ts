import type { Query } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import { SEAQUEL_DIR, type SharedRepoManager } from "./shared-repo-manager.svelte.js";
import {
  serializeQueryFile,
  queryNameToFilename,
  isValidQueryPath,
} from "$lib/services/query-file-parser";
import { nameToFilename } from "$lib/services/config-file-parser";
import { writeTextFile, remove, mkdir, exists } from "@tauri-apps/plugin-fs";
import { join, dirname } from "@tauri-apps/api/path";

/**
 * File projection utility for shared queries.
 * Writes/deletes .sql files in the git repo as projections of Query objects.
 * Does NOT manage query state — that's handled by SavedQueryManager.
 */
export class SharedQueryManager {
  constructor(
    private state: DatabaseState,
    private repoManager: SharedRepoManager,
  ) {}

  /**
   * Get the queries base path for the active project.
   */
  private getQueriesBasePath(): string | null {
    const project = this.state.projects.find((p) => p.id === this.state.activeProjectId);
    if (!project) return null;
    const dirName = nameToFilename(project.name);
    return `${SEAQUEL_DIR}/projects/${dirName}/queries`;
  }

  /**
   * Extract the queries base path from an existing filePath.
   */
  private extractQueriesBase(filePath: string): string {
    const match = filePath.match(/^(\.seaquel\/projects\/[^/]+\/queries)\//);
    return match ? match[1] : "";
  }

  /**
   * Get the active repo and its path, or null if unavailable.
   */
  private getActiveRepo(): { repoId: string; repoPath: string } | null {
    const repoId = this.state.activeRepoId;
    if (!repoId) return null;
    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return null;
    return { repoId, repoPath: repo.path };
  }

  /**
   * Write a query as a .sql file in the git repo.
   */
  async writeQueryFile(query: Query): Promise<void> {
    const activeRepo = this.getActiveRepo();
    if (!activeRepo) return;

    const queriesBase = this.getQueriesBasePath();
    if (!queriesBase) return;

    const filename = queryNameToFilename(query.name);
    const folder = query.folder || "";
    const relPath = folder ? `${folder}/${filename}` : filename;
    const filePath = `${queriesBase}/${relPath}`;

    if (!isValidQueryPath(filePath)) {
      throw new Error("Invalid query file path");
    }

    const content = serializeQueryFile(query);
    const fullPath = await join(activeRepo.repoPath, filePath);
    const folderPath = await dirname(fullPath);

    if (!(await exists(folderPath))) {
      await mkdir(folderPath, { recursive: true });
    }

    await writeTextFile(fullPath, content);
    await this.repoManager.refreshRepoStatus(activeRepo.repoId);
  }

  /**
   * Delete the .sql file for a query from the git repo.
   */
  async deleteQueryFile(query: Query): Promise<void> {
    const activeRepo = this.getActiveRepo();
    if (!activeRepo) return;

    const queriesBase = this.getQueriesBasePath();
    if (!queriesBase) return;

    const filename = queryNameToFilename(query.name);
    const folder = query.folder || "";
    const relPath = folder ? `${folder}/${filename}` : filename;
    const filePath = `${queriesBase}/${relPath}`;

    const fullPath = await join(activeRepo.repoPath, filePath);
    try {
      await remove(fullPath);
    } catch {
      // File may not exist (already deleted externally)
    }

    await this.repoManager.refreshRepoStatus(activeRepo.repoId);
  }

  /**
   * Create a new folder in the repository's queries directory.
   */
  async createFolder(folderPath: string): Promise<boolean> {
    const activeRepo = this.getActiveRepo();
    if (!activeRepo) return false;

    const queriesBase = this.getQueriesBasePath();
    if (!queriesBase) return false;

    const repoRelPath = `${queriesBase}/${folderPath}`;
    const fullPath = await join(activeRepo.repoPath, repoRelPath);

    if (await exists(fullPath)) {
      return false;
    }

    await mkdir(fullPath, { recursive: true });

    const gitkeepPath = await join(fullPath, ".gitkeep");
    await writeTextFile(gitkeepPath, "");

    return true;
  }

  /**
   * Reconcile .sql files from the scan cache with SQLite queries.
   * - New .sql files → create Query with shared=true
   * - Missing .sql files for shared queries → set shared=false
   * - Updated .sql files → update query content
   * Returns the list of queries after reconciliation.
   */
  reconcileWithGitFiles(projectId: string, queries: Query[]): Query[] {
    const activeRepo = this.getActiveRepo();
    if (!activeRepo) return queries;

    const queriesBase = this.getQueriesBasePath();
    if (!queriesBase) return queries;

    // Get scanned queries from the scan cache, filtered to this project's directory
    const allScannedQueries = this.state.sharedQueriesByRepo[activeRepo.repoId] ?? [];
    const gitQueries = allScannedQueries.filter((q) => q.filePath?.startsWith(queriesBase + "/"));

    const result = [...queries];
    const matchedGitNames = new Set<string>();

    // Match existing shared queries to git files by name+folder
    for (const gitQuery of gitQueries) {
      const matchKey = `${gitQuery.folder || ""}/${gitQuery.name}`.toLowerCase();
      const existingIdx = result.findIndex(
        (q) => q.shared && `${q.folder || ""}/${q.name}`.toLowerCase() === matchKey,
      );

      if (existingIdx !== -1) {
        // Update content if git file is newer
        const existing = result[existingIdx];
        if (gitQuery.query !== existing.query) {
          result[existingIdx] = {
            ...existing,
            query: gitQuery.query,
            description: gitQuery.description,
            databaseType: gitQuery.databaseType,
            tags: gitQuery.tags,
            parameters: gitQuery.parameters,
            updatedAt: new Date(),
          };
        }
        matchedGitNames.add(matchKey);
      } else {
        // New .sql file → create a new shared Query
        const newQuery: Query = {
          id: `saved-${crypto.randomUUID()}`,
          name: gitQuery.name,
          query: gitQuery.query,
          projectId,
          createdAt: new Date(),
          updatedAt: new Date(),
          parameters: gitQuery.parameters,
          shared: true,
          description: gitQuery.description,
          databaseType: gitQuery.databaseType,
          tags: gitQuery.tags,
          folder: gitQuery.folder || undefined,
        };
        result.push(newQuery);
        matchedGitNames.add(matchKey);
      }
    }

    // Shared queries in SQLite with no matching .sql file → mark as unshared
    for (let i = 0; i < result.length; i++) {
      const q = result[i];
      if (q.shared) {
        const matchKey = `${q.folder || ""}/${q.name}`.toLowerCase();
        if (!matchedGitNames.has(matchKey)) {
          result[i] = { ...q, shared: false };
        }
      }
    }

    return result;
  }
}
