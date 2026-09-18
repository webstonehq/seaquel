/**
 * Feature flags for desktop / web / demo modes.
 * Controls which features are available in each environment.
 */

import { isDemo as checkIsDemo, isWeb as checkIsWeb } from "$lib/utils/environment";

/**
 * Feature flags interface.
 */
export interface FeatureFlags {
  /** Allow creating new database connections */
  newConnections: boolean;
  /**
   * Show SSH tunnel configuration options.
   * Desktop only — the runtime path (`$lib/services/ssh-tunnel`) invokes a
   * Tauri command; the web tenant container has no SSH stack of its own.
   */
  sshTunnels: boolean;
  /** Show MSSQL connection option */
  mssqlSupport: boolean;
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
  /** Show the AI assistant */
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
    fileExport: !demo && !web, // OS-save-dialog exports — Tauri-only
    appUpdater: !demo && !web, // Web tenant containers update via the platform, not the UI
    editConnections: !demo,
    aiAssistant: true, // Works in demo
    savedQueries: true, // Uses localStorage in demo
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

/**
 * Check if running in demo mode.
 */
export function isDemo(): boolean {
  return checkIsDemo();
}
