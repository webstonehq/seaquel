import type { Dashboard, DashboardWidget } from "$lib/types";
import type { PersistedDashboard } from "$lib/storage/repos/dashboards-repo";

/**
 * Strip runtime-only state from a dashboard widget before persistence or serialization.
 * `result` holds query rows (which may contain bigint cells), so it must never be saved.
 */
export function stripWidgetRuntimeState(
  widget: DashboardWidget,
): Omit<DashboardWidget, "result" | "isLoading" | "error" | "lastRefreshed"> {
  const { result: _r, isLoading: _l, error: _e, lastRefreshed: _lr, ...rest } = widget;
  return rest;
}

/** The row saved for a dashboard: runtime widget state stripped, JSON columns stringified. */
export function toPersistedDashboard(dashboard: Dashboard): PersistedDashboard {
  return {
    id: dashboard.id,
    projectId: dashboard.projectId,
    name: dashboard.name,
    viewport: JSON.stringify(dashboard.viewport),
    widgets: JSON.stringify(dashboard.widgets.map(stripWidgetRuntimeState)),
    dateFilter: dashboard.dateFilter ? JSON.stringify(dashboard.dateFilter) : null,
    starred: dashboard.starred,
    shared: dashboard.shared,
    description: dashboard.description,
    createdAt: dashboard.createdAt.toISOString(),
    updatedAt: dashboard.updatedAt.toISOString(),
  };
}
