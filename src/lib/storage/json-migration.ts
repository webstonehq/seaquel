import { log } from "$lib/utils/logger";
import type { SqliteDatabase } from "./sqlite-types";
import type {
  PersistedProject,
  PersistedProjectState,
  PersistedSavedQuery,
  PersistedQueryHistoryItem,
  PersistedSharedQueryRepo,
} from "$lib/types";
import { DEFAULT_PROJECT_ID, DEFAULT_PROJECT_NAME } from "$lib/types";
import type { PersistedConnection } from "$lib/hooks/database/types";
import {
  projectsRepo,
  appStateRepo,
  connectionsRepo,
  projectStateRepo,
  savedQueriesRepo,
  queryHistoryRepo,
  sharedReposRepo,
  themeRepo,
  licenseRepo,
  onboardingRepo,
  tutorialRepo,
  importStateRepo,
} from "./repository";

/**
 * Migrates existing JSON store files to SQLite.
 * Called on first launch when schema_version table is empty AND legacy JSON files exist.
 *
 * This module uses the legacy storage provider (read-only) to load all JSON data,
 * then inserts everything into SQLite within repository calls.
 */
/**
 * `app_state` key recording that the legacy JSON import has run. The JSON files
 * stay on disk, so without this marker the import would repeat on every launch
 * whenever the connections table happened to be empty.
 */
export const JSON_MIGRATION_DONE_KEY = "json_migration_done";

export async function migrateJsonToSqlite(db: SqliteDatabase): Promise<boolean> {
  try {
    // Try loading the legacy storage module
    const { loadStore } = await import("./legacy");

    // Check if any legacy data exists by trying to load projects and connections
    let hasLegacyData = false;

    // === Projects ===
    let projectIds = new Set<string>();
    try {
      const projectsStore = await loadStore("projects.json", {
        autoSave: false,
        defaults: { projects: [] },
      });
      const projects = (await projectsStore.get("projects")) as PersistedProject[] | null;
      if (projects && projects.length > 0) {
        hasLegacyData = true;
        await projectsRepo.saveAll(db, projects);
        for (const p of projects) projectIds.add(p.id);
      }
    } catch {
      // No projects file
    }

    // === Connections ===
    let connectionIds: string[] = [];
    try {
      const connStore = await loadStore("database_connections.json", {
        autoSave: false,
        defaults: { connections: [] },
      });
      const connections = (await connStore.get("connections")) as PersistedConnection[] | null;
      if (connections && connections.length > 0) {
        hasLegacyData = true;

        // Ensure a default project exists for connections that have no projectId
        // or reference a project that wasn't in the projects.json file
        const needsDefaultProject = connections.some(
          (c) => !c.projectId || !projectIds.has(c.projectId),
        );
        if (needsDefaultProject && !projectIds.has(DEFAULT_PROJECT_ID)) {
          const now = new Date().toISOString();
          await projectsRepo.save(db, {
            id: DEFAULT_PROJECT_ID,
            name: DEFAULT_PROJECT_NAME,
            createdAt: now,
            updatedAt: now,
            customLabels: [],
          });
          projectIds.add(DEFAULT_PROJECT_ID);
        }

        for (const conn of connections) {
          // Default projectId if missing or referencing a non-existent project
          const safeConn = {
            ...conn,
            projectId:
              conn.projectId && projectIds.has(conn.projectId)
                ? conn.projectId
                : DEFAULT_PROJECT_ID,
            labelIds: conn.labelIds ?? [],
          };
          await connectionsRepo.save(db, safeConn);
          connectionIds.push(conn.id);
        }
      }
    } catch (error) {
      void log.error("Failed to migrate connections:", error);
    }

    if (!hasLegacyData) {
      // No legacy data to migrate
      return false;
    }

    void log.info("Migrating JSON data to SQLite...");

    // === App State ===
    try {
      const appStore = await loadStore("app_state.json", { autoSave: false, defaults: {} });
      const lastActiveProjectId = (await appStore.get("lastActiveProjectId")) as string | null;
      if (lastActiveProjectId) {
        await appStateRepo.set(db, "lastActiveProjectId", lastActiveProjectId);
      }
      const storageVersion = (await appStore.get("storageVersion")) as number | null;
      if (storageVersion) {
        await db.execute("INSERT INTO schema_version (version) VALUES (?)", [storageVersion]);
      }
    } catch {
      // No app state file
    }

    // === Project States (dynamic filenames) ===
    // Load all project IDs we know about, then try their state files
    try {
      const projects = await projectsRepo.loadAll(db);
      for (const project of projects) {
        try {
          const stateStore = await loadStore(`project_state_${project.id}.json`, {
            autoSave: false,
            defaults: { state: null },
          });
          const state = (await stateStore.get("state")) as PersistedProjectState | null;
          if (state) {
            await projectStateRepo.save(db, state);
          }
        } catch {
          // No state file for this project
        }
      }
    } catch {
      // Error loading projects
    }

    // === Connection Data (dynamic filenames) ===
    // Accumulate saved queries per project so that saveAll (which deletes missing IDs)
    // doesn't discard queries from earlier connections in the same project.
    const queriesByProject = new Map<string, PersistedSavedQuery[]>();

    for (const connectionId of connectionIds) {
      try {
        const dataStore = await loadStore(`connection_data_${connectionId}.json`, {
          autoSave: false,
          defaults: {},
        });
        const savedQueries = (await dataStore.get("savedQueries")) as PersistedSavedQuery[] | null;
        const queryHistory = (await dataStore.get("queryHistory")) as
          | PersistedQueryHistoryItem[]
          | null;

        if (savedQueries && savedQueries.length > 0) {
          // Resolve projectId from connection for migration
          const connRows = await db.query<{ project_id: string }>(
            "SELECT project_id FROM connections WHERE id = ?",
            [connectionId],
          );
          const projectId = connRows.length > 0 ? connRows[0].project_id : DEFAULT_PROJECT_ID;
          const migratedQueries: PersistedSavedQuery[] = savedQueries.map((q) => ({
            ...q,
            projectId,
          }));
          const existing = queriesByProject.get(projectId) ?? [];
          existing.push(...migratedQueries);
          queriesByProject.set(projectId, existing);
        }
        if (queryHistory && queryHistory.length > 0) {
          await queryHistoryRepo.replaceAll(db, connectionId, queryHistory);
        }
      } catch {
        // No data file for this connection
      }
    }

    // Save all accumulated queries per project in one batch
    for (const [projectId, queries] of queriesByProject) {
      await savedQueriesRepo.saveAll(db, projectId, queries);
    }

    // === Shared Repos ===
    try {
      const reposStore = await loadStore("shared_repos.json", {
        autoSave: false,
        defaults: { repos: [], activeRepoId: null },
      });
      const repos = (await reposStore.get("repos")) as PersistedSharedQueryRepo[] | null;
      const activeRepoId = (await reposStore.get("activeRepoId")) as string | null;
      if (repos && repos.length > 0) {
        await sharedReposRepo.saveAll(db, repos, activeRepoId);
      }
    } catch {
      // No shared repos file
    }

    // === Themes ===
    try {
      const themeStore = await loadStore("themes.json", {
        autoSave: false,
        defaults: { preferences: null, userThemes: [] },
      });
      const prefs = (await themeStore.get("preferences")) as {
        lightThemeId: string;
        darkThemeId: string;
      } | null;
      const userThemes = (await themeStore.get("userThemes")) as unknown[] | null;

      if (prefs) {
        await themeRepo.savePreferences(db, prefs.lightThemeId, prefs.darkThemeId);
      }
      if (userThemes && userThemes.length > 0) {
        await themeRepo.saveUserThemes(db, userThemes);
      }
    } catch {
      // No themes file
    }

    // === License ===
    try {
      const { load } = await import("@tauri-apps/plugin-store");
      const licStore = await load("license.json", { autoSave: false, defaults: { state: null } });
      const licState = await licStore.get("state");
      if (licState) {
        await licenseRepo.save(db, licState);
      }
    } catch {
      // No license file or not in Tauri
    }

    // === Onboarding ===
    try {
      const { load } = await import("@tauri-apps/plugin-store");
      const onbStore = await load("onboarding_state.json", {
        autoSave: false,
        defaults: { state: null },
      });
      const onbState = await onbStore.get("state");
      if (onbState) {
        await onboardingRepo.save(db, onbState);
      }
    } catch {
      // No onboarding file or not in Tauri
    }

    // === Tutorial Progress ===
    try {
      const { load } = await import("@tauri-apps/plugin-store");
      const tutStore = await load("tutorial_progress.json", {
        autoSave: false,
        defaults: { state: null },
      });
      const tutState = (await tutStore.get("state")) as {
        completedChallenges: Record<string, string[]>;
        challengeStates: Record<string, Record<string, unknown>>;
      } | null;
      if (tutState?.completedChallenges) {
        for (const [lessonId, challengeIds] of Object.entries(tutState.completedChallenges)) {
          for (const challengeId of challengeIds) {
            const state = tutState.challengeStates?.[lessonId]?.[challengeId];
            await tutorialRepo.save(
              db,
              lessonId,
              challengeId,
              state ? JSON.stringify(state) : null,
            );
          }
        }
      }
    } catch {
      // No tutorial file or not in Tauri
    }

    // === Import States (TablePlus, DBeaver) ===
    for (const source of ["tableplus", "dbeaver"] as const) {
      const filename = `${source === "tableplus" ? "tableplus" : "dbeaver"}_import_state.json`;
      try {
        const { load } = await import("@tauri-apps/plugin-store");
        const impStore = await load(filename, { autoSave: false, defaults: { state: null } });
        const impState = (await impStore.get("state")) as {
          hasOfferedImport: boolean;
          lastCheckTimestamp?: string;
        } | null;
        if (impState) {
          await importStateRepo.save(
            db,
            source,
            impState.hasOfferedImport,
            impState.lastCheckTimestamp ?? null,
          );
        }
      } catch {
        // No import state file or not in Tauri
      }
    }

    void log.info("JSON to SQLite migration completed successfully");
    return true;
  } catch (error) {
    void log.error("Failed to migrate JSON to SQLite:", error);
    return false;
  }
}
