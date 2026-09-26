/**
 * The demo's `StorageClient`: the TypeScript repositories over sql.js
 * (`web-sqlite.ts`), persisted to `localStorage`. The demo has no Rust core,
 * so it keeps these until phase 8, the same way it keeps `duckdb.ts`.
 *
 * Desktop and web never load this module (`getStorage()` imports it lazily
 * in the demo only).
 */

import type { StorageClient } from "./client";
import {
  aiChatsRepo,
  appStateRepo,
  connectionOverridesRepo,
  connectionsRepo,
  dashboardVersionsRepo,
  dashboardsRepo,
  importStateRepo,
  licenseRepo,
  onboardingRepo,
  projectStateRepo,
  projectsRepo,
  queryHistoryRepo,
  queryVersionsRepo,
  savedQueriesRepo,
  sharedReposRepo,
  themeRepo,
  tutorialRepo,
  userCredentialsRepo,
  vaultStateRepo,
} from "./repository";
import { CURRENT_STORAGE_VERSION, initializeSchema } from "./schema";
import type { SqliteDatabase } from "./sqlite-types";

/** A repository's methods with the leading `db` argument bound. */
type Bound<R> = {
  [K in keyof R]: R[K] extends (db: SqliteDatabase, ...args: infer A) => infer Ret
    ? (...args: A) => Ret
    : never;
};

function bind<R extends object>(repo: R, db: SqliteDatabase): Bound<R> {
  const bound: Record<string, unknown> = {};
  for (const [name, fn] of Object.entries(repo)) {
    if (typeof fn === "function") {
      // Called with the repo as `this`: `projectsRepo.saveAll` uses `this.save`.
      bound[name] = (...args: unknown[]) =>
        (fn as (...a: unknown[]) => unknown).call(repo, db, ...args);
    }
  }
  return bound as Bound<R>;
}

/** Wraps the repositories around an open, bootstrapped database. */
export function createSqljsStorageClient(db: SqliteDatabase): StorageClient {
  return {
    projects: bind(projectsRepo, db),
    appState: bind(appStateRepo, db),
    connections: bind(connectionsRepo, db),
    connectionOverrides: bind(connectionOverridesRepo, db),
    projectState: bind(projectStateRepo, db),
    savedQueries: bind(savedQueriesRepo, db),
    queryVersions: bind(queryVersionsRepo, db),
    queryHistory: bind(queryHistoryRepo, db),
    sharedRepos: bind(sharedReposRepo, db),
    themes: bind(themeRepo, db),
    license: bind(licenseRepo, db),
    onboarding: bind(onboardingRepo, db),
    tutorial: bind(tutorialRepo, db),
    importState: bind(importStateRepo, db),
    dashboards: bind(dashboardsRepo, db),
    dashboardVersions: bind(dashboardVersionsRepo, db),
    aiChats: bind(aiChatsRepo, db),
    vaultState: bind(vaultStateRepo, db),
    userCredentials: bind(userCredentialsRepo, db),
  };
}

/** Creates the schema on a fresh database, or brings an existing one up to date. */
export async function bootstrapSqljsDatabase(db: SqliteDatabase): Promise<void> {
  await db.execute("PRAGMA foreign_keys=ON");
  const isFreshDb = await initializeSchema(db);
  if (isFreshDb) {
    await db.execute("INSERT INTO schema_version (version) VALUES (?)", [CURRENT_STORAGE_VERSION]);
  }
}

/** Opens the demo's database from `localStorage` and wraps it. */
export async function openSqljsStorageClient(): Promise<StorageClient> {
  const { WebSqliteProvider } = await import("./web-sqlite");
  const db = await new WebSqliteProvider().open("seaquel.db");
  await bootstrapSqljsDatabase(db);
  return createSqljsStorageClient(db);
}
