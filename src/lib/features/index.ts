/**
 * Feature flags for desktop / web / demo modes.
 * Controls which features are available in each environment.
 */

import type { DatabaseType } from "$lib/types";
import { isDemo as checkIsDemo, isWeb as checkIsWeb } from "$lib/utils/environment";

/**
 * Feature flags interface.
 */
export interface FeatureFlags {
  /** Allow creating new database connections */
  newConnections: boolean;
  /**
   * Show SSH tunnel configuration options.
   * Desktop only: Core opens the tunnel as part of `db.connect`/`db.test`.
   * The web server's connect policy refuses any connection with a tunnel.
   */
  sshTunnels: boolean;
  /** Show MSSQL connection option */
  mssqlSupport: boolean;
  /**
   * Offer SQLite connections. Off on web: a SQLite "connection string" is a
   * path on the server, and the server holds every user's data and its own
   * auth database. `seaquel-server` doesn't register the engine either
   * (Decision 11b), so this only keeps the UI from offering it.
   */
  sqliteSupport: boolean;
  /**
   * Offer DuckDB connections. Off on web for the same reason as SQLite, and
   * because DuckDB's `read_*` and `COPY TO` reach any file on the server.
   * The demo is DuckDB-WASM, and web's tutorial runs on DuckDB-WASM too.
   */
  duckdbSupport: boolean;
  /**
   * Allow saving query results / diagrams to disk via the OS save dialog.
   * Desktop only — uses `@tauri-apps/plugin-dialog` + `@tauri-apps/plugin-fs`.
   * Browser-based exports (CSV/JSON of query results via a `<a download>`
   * blob in `command-palette.svelte`) are not gated by this flag — those
   * work in web mode and stay enabled there.
   */
  fileExport: boolean;
  /** Show app updater UI */
  appUpdater: boolean;
  /** Allow editing connection settings */
  editConnections: boolean;
  /** Offer the AI assistant (off in the demo: it can keep no API key, Q7 A) */
  aiAssistant: boolean;
  /** Allow saving queries */
  savedQueries: boolean;
  /** Show connection type selector */
  connectionTypeSelector: boolean;
  /**
   * Allow sharing projects/queries/dashboards via a git repo.
   * Desktop only for now — the web tenant container has no user-accessible
   * filesystem and server-side git is a follow-up phase.
   */
  sharedProjects: boolean;
}

/**
 * Get feature flags for the current environment.
 */
export function getFeatures(): FeatureFlags {
  const demo = checkIsDemo();
  const web = checkIsWeb();

  return {
    newConnections: !demo,
    sshTunnels: !demo && !web, // Tauri-only — no SSH stack in the web container
    mssqlSupport: !demo,
    sqliteSupport: !web, // Server-side files: off on web (Decision 11b)
    duckdbSupport: !web, // Server-side files: off on web (Decision 11b)
    fileExport: !demo && !web, // OS-save-dialog exports — Tauri-only
    appUpdater: !demo && !web, // Web tenant containers update via the platform, not the UI
    editConnections: !demo,
    aiAssistant: !demo, // No key can be kept or sent in the demo (Q7 A; phase 6)
    savedQueries: true,
    connectionTypeSelector: !demo,
    sharedProjects: !demo && !web, // Desktop only until server-side git lands
  };
}

/**
 * Check if a specific feature is enabled.
 */
export function isFeatureEnabled(feature: keyof FeatureFlags): boolean {
  return getFeatures()[feature];
}

/** The flag that gates each database type; types without one are always offered. */
const DATABASE_TYPE_FLAGS: Partial<Record<DatabaseType, keyof FeatureFlags>> = {
  sqlite: "sqliteSupport",
  duckdb: "duckdbSupport",
  mssql: "mssqlSupport",
};

const DATABASE_TYPE_LABELS: Partial<Record<DatabaseType, string>> = {
  sqlite: "SQLite",
  duckdb: "DuckDB",
  mssql: "SQL Server",
};

/**
 * Whether this build can connect to `type`. The wizard offers only these,
 * the connection-string parser refuses the others, and `ConnectionManager`
 * refuses to connect one (a connection saved on desktop, or synced from it,
 * can still show up in a web workspace).
 */
export function isDatabaseTypeAvailable(
  type: DatabaseType,
  features: FeatureFlags = getFeatures(),
): boolean {
  const flag = DATABASE_TYPE_FLAGS[type];
  return flag ? features[flag] : true;
}

/** The error for a database type this build doesn't offer. */
export function databaseTypeUnavailableMessage(type: DatabaseType): string {
  const label = DATABASE_TYPE_LABELS[type] ?? type;
  return checkIsWeb()
    ? `${label} connections aren't available in the web app, because they would open files on the server. Use the desktop app for ${label}.`
    : `${label} connections aren't available in this build.`;
}

/**
 * Throw {@link databaseTypeUnavailableMessage} if this build can't connect to
 * `type`. Call before anything reaches a provider.
 */
export function assertDatabaseTypeAvailable(type: DatabaseType): void {
  if (!isDatabaseTypeAvailable(type)) {
    throw new Error(databaseTypeUnavailableMessage(type));
  }
}

/**
 * Check if running in demo mode.
 */
export function isDemo(): boolean {
  return checkIsDemo();
}
