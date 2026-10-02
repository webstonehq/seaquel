/**
 * Dashboard types for building persistent visual dashboards from query data.
 * @module types/dashboard
 */

import type { ChartConfig } from "./chart";

/**
 * KPI widget configuration for displaying a single prominent number.
 */
export interface KpiConfig {
  label: string;
  valueColumn: string;
  format?: "number" | "percentage";
  prefix?: string;
  suffix?: string;
}

/**
 * Text widget configuration for displaying free-form text content.
 */
export interface TextConfig {
  content: string;
}

/**
 * A single widget within a dashboard.
 */
export interface DashboardWidget {
  id: string;
  title: string;
  description?: string;
  // Canvas position (pixel coordinates)
  x: number;
  y: number;
  width: number;
  height: number;
  // Data source
  querySource: "custom" | "saved";
  query: string;
  savedQueryId?: string;
  // Display
  widgetType: "chart" | "kpi" | "text";
  chartConfig?: ChartConfig;
  kpiConfig?: KpiConfig;
  textConfig?: TextConfig;
  // Auto-refresh
  autoRefreshSeconds?: number;
  // Runtime state (not persisted)
  result?: Record<string, unknown>[];
  isLoading?: boolean;
  error?: string;
  lastRefreshed?: Date;
}

/**
 * A dashboard containing multiple widgets.
 * SQLite is the source of truth. When shared=true, a .json file is maintained as a git projection.
 */
export interface Dashboard {
  id: string;
  name: string;
  projectId: string;
  widgets: DashboardWidget[];
  viewport: { x: number; y: number; zoom: number };
  dateFilter?: { start: string; end: string } | null;
  createdAt: Date;
  updatedAt: Date;
  starred?: boolean;
  /** Whether this dashboard is shared via git */
  shared: boolean;
  /** Optional description (used in shared .json file) */
  description?: string;
  /** A shared dashboard's file, relative to the repo (Core's link; read only). */
  sharedPath?: string;
}

/**
 * Dashboard tab in the tab system.
 */
export interface DashboardTab {
  id: string;
  name: string;
  dashboardId: string;
}

/**
 * Snapshot of a dashboard's versionable state (excludes id, projectId, timestamps, etc.).
 */
export interface DashboardSnapshot {
  name: string;
  description?: string;
  widgets: Omit<DashboardWidget, "result" | "isLoading" | "error" | "lastRefreshed">[];
  viewport: { x: number; y: number; zoom: number };
  dateFilter?: { start: string; end: string } | null;
}

/**
 * A dashboard version as the history lists it, without its snapshot
 * (`dashboardVersionsList`, phase 5d-2 Task 7): the snapshot is fetched
 * when a version is compared or restored (`DashboardManager.loadVersion`).
 */
export interface DashboardVersion {
  id: string;
  dashboardId: string;
  version: number;
  /** How many widgets the snapshot holds; `null` when it can't be told. */
  widgetCount: number | null;
  createdAt: Date;
}

/**
 * A resolved dashboard version with the parsed snapshot.
 */
export interface ResolvedDashboardVersion {
  id: string;
  dashboardId: string;
  version: number;
  dashboard: DashboardSnapshot;
  createdAt: Date;
}
