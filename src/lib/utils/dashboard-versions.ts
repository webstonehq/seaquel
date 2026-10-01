import type { Dashboard, DashboardSnapshot } from "$lib/types";
import { stripWidgetRuntimeState } from "$lib/hooks/database/dashboard-manager.svelte";

/**
 * Create a snapshot from a Dashboard's current state.
 * Strips runtime widget state (result, isLoading, error, lastRefreshed).
 */
export function createDashboardSnapshot(dashboard: Dashboard): DashboardSnapshot {
  return {
    name: dashboard.name,
    description: dashboard.description,
    widgets: dashboard.widgets.map(stripWidgetRuntimeState),
    viewport: dashboard.viewport,
    dateFilter: dashboard.dateFilter,
  };
}
