/**
 * Closing every tab that belongs to a saved connection, in every project,
 * when the connection is gone: removed here, or in another window (phase 5d-1).
 * Schema, data, ERD, statistics, create-table and
 * DuckDB-extension tabs, and the connection's own connect/edit tab. The
 * tab managers work on the active project only, so this edits the state's
 * per-project records directly: the tab lists, the active ids, the tab
 * order and the pane layout.
 */
import type { DatabaseState } from "./state.svelte.js";

type TabRecords = {
  [K in keyof DatabaseState]: DatabaseState[K] extends Record<string, Array<{ id: string }>>
    ? K
    : never;
}[keyof DatabaseState];

type Tab = { id: string; connectionId?: string | null; dashboardId?: string };
type ActiveRecords = {
  [K in keyof DatabaseState]: DatabaseState[K] extends Record<string, string | null> ? K : never;
}[keyof DatabaseState];

/** Each tab kind tied to a connection, with its active-id record. */
const KINDS: Array<[TabRecords, ActiveRecords]> = [
  ["schemaTabsByProject", "activeSchemaTabIdByProject"],
  ["dataTabsByProject", "activeDataTabIdByProject"],
  ["erdTabsByProject", "activeErdTabIdByProject"],
  ["statisticsTabsByProject", "activeStatisticsTabIdByProject"],
  ["createTableTabsByProject", "activeCreateTableTabIdByProject"],
  ["extensionsDuckdbTabsByProject", "activeExtensionsDuckdbTabIdByProject"],
  ["connectionTabsByProject", "activeConnectionTabIdByProject"],
];

/**
 * Close connection `connectionId`'s tabs in every project. Returns the
 * projects whose tabs changed, whose state the caller saves.
 */
export function closeConnectionTabs(
  state: DatabaseState,
  connectionId: string,
  /**
   * The pane manager's `syncGlobalActiveState`, as `removePane` uses it: when
   * the active project's active pane shows another tab afterwards, the view
   * and its per-type active id follow it.
   */
  syncActive?: (tabId: string) => void,
): string[] {
  return closeTabs(state, KINDS, (t) => t.connectionId === connectionId, syncActive);
}

/**
 * Close the tabs showing dashboard `dashboardId` in every project (it was
 * deleted in another window, phase 5d-2). Returns the projects whose tabs
 * changed.
 */
export function closeDashboardTabs(
  state: DatabaseState,
  dashboardId: string,
  syncActive?: (tabId: string) => void,
): string[] {
  return closeTabs(
    state,
    [["dashboardTabsByProject", "activeDashboardTabIdByProject"]],
    (t) => t.dashboardId === dashboardId,
    syncActive,
  );
}

function closeTabs(
  state: DatabaseState,
  kinds: Array<[TabRecords, ActiveRecords]>,
  matches: (tab: Tab) => boolean,
  syncActive?: (tabId: string) => void,
): string[] {
  const touched = new Set<string>();
  const removed = new Set<string>();
  for (const [tabsKey, activeKey] of kinds) {
    const byProject = state[tabsKey] as unknown as Record<string, Tab[]>;
    let next: typeof byProject | null = null;
    for (const [projectId, tabs] of Object.entries(byProject)) {
      const gone = tabs.filter(matches);
      if (gone.length === 0) continue;
      next ??= { ...byProject };
      next[projectId] = tabs.filter((t) => !matches(t));
      for (const t of gone) removed.add(t.id);
      touched.add(projectId);
      const active = state[activeKey] as Record<string, string | null>;
      if (active[projectId] && gone.some((t) => t.id === active[projectId])) {
        (state[activeKey] as Record<string, string | null>) = {
          ...active,
          [projectId]: next[projectId][0]?.id ?? null,
        };
      }
    }
    if (next) (state[tabsKey] as unknown as typeof byProject) = next;
  }
  if (removed.size === 0) return [];

  let syncAfter: string | null = null;
  const order = { ...state.tabOrderByProject };
  const layouts = { ...state.paneLayoutByProject };
  for (const projectId of touched) {
    order[projectId] = (order[projectId] ?? []).filter((id) => !removed.has(id));
    const layout = layouts[projectId];
    if (!layout) continue;
    let panes = layout.panes.map((pane) => {
      const tabIds = pane.tabIds.filter((id) => !removed.has(id));
      const activeTabId =
        pane.activeTabId && removed.has(pane.activeTabId) ? (tabIds[0] ?? null) : pane.activeTabId;
      return { ...pane, tabIds, activeTabId };
    });
    // A pane left empty goes, unless it's the only one.
    if (panes.length > 1) {
      const kept = panes.filter((p) => p.tabIds.length > 0);
      panes = kept.length > 0 ? kept : [panes[0]];
    }
    const activePaneId = panes.some((p) => p.id === layout.activePaneId)
      ? layout.activePaneId
      : panes[0].id;
    layouts[projectId] = { panes, activePaneId };
    if (projectId === state.activeProjectId) {
      const before = layout.panes.find((p) => p.id === layout.activePaneId)?.activeTabId ?? null;
      const after = panes.find((p) => p.id === activePaneId)?.activeTabId ?? null;
      if (after && after !== before) syncAfter = after;
    }
  }
  state.tabOrderByProject = order;
  state.paneLayoutByProject = layouts;
  if (syncAfter) syncActive?.(syncAfter);
  return [...touched];
}
