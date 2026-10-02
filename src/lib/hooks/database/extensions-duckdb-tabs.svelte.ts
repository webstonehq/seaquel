import type { ActiveViewType } from "$lib/types/persisted";
import type { ExtensionsDuckdbTab, DuckDBExtension } from "$lib/types";
import { parseCommunityExtensionsHtml } from "$lib/utils/parse-community-extensions.js";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";
import type { ExtensionAction } from "$lib/types/generated/ExtensionAction";

/**
 * Runs an extensions tab action on the saved connection `connectionId` (the
 * tab's, never the active one): Core's `db.duckdbExtension`
 * (`EditService.duckdbExtension`), on desktop and in the demo's page.
 * `list` answers `duckdb_extensions()`'s rows.
 */
export type RunExtensionAction = (
  connectionId: string,
  action: ExtensionAction,
) => Promise<Record<string, unknown>[] | null>;

/**
 * Manages DuckDB Extensions tabs.
 * Tabs are organized per-project.
 */
export class ExtensionsDuckdbTabManager extends BaseTabManager<ExtensionsDuckdbTab> {
  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
    setActiveView: (view: ActiveViewType) => void,
    private runAction: RunExtensionAction,
  ) {
    super(state, tabOrdering, schedulePersistence, setActiveView);
  }

  protected get accessors(): TabStateAccessors<ExtensionsDuckdbTab> {
    return {
      getTabs: () => this.state.extensionsDuckdbTabsByProject,
      setTabs: (r) => (this.state.extensionsDuckdbTabsByProject = r),
      getActiveId: () => this.state.activeExtensionsDuckdbTabIdByProject,
      setActiveId: (r) => (this.state.activeExtensionsDuckdbTabIdByProject = r),
    };
  }

  /**
   * Add an extensions tab for the current DuckDB connection.
   * Returns the tab ID or null if not applicable.
   */
  async add(): Promise<string | null> {
    if (
      !this.state.activeProjectId ||
      !this.state.activeConnectionId ||
      !this.state.activeConnection ||
      this.state.activeConnection.type !== "duckdb"
    )
      return null;

    const tabs = this.getProjectTabs();

    // Reuse existing tab for the same connection
    const existingTab = tabs.find((t) => t.connectionId === this.state.activeConnectionId);
    if (existingTab) {
      this.setActive(existingTab.id);
      this.viewFallbackFn!("extensionsDuckdb");
      await this.refresh(existingTab.id);
      return existingTab.id;
    }

    const newTab: ExtensionsDuckdbTab = {
      id: `ext-duckdb-${crypto.randomUUID()}`,
      name: `Extensions: ${this.state.activeConnection.name}`,
      connectionId: this.state.activeConnectionId,
      isLoading: true,
    };

    this.appendTab(newTab);
    this.viewFallbackFn!("extensionsDuckdb");

    await Promise.all([this.loadExtensions(newTab.id), this.loadCommunityExtensions(newTab.id)]);

    return newTab.id;
  }

  /**
   * Refresh extensions data for a tab.
   */
  async refresh(tabId: string): Promise<void> {
    await Promise.all([this.loadExtensions(tabId), this.loadCommunityExtensions(tabId)]);
  }

  /**
   * Install a DuckDB extension.
   */
  async installExtension(tabId: string, extensionName: string): Promise<void> {
    await this.runExtensionAction(tabId, extensionName, "Installing", "install");
  }

  /**
   * Load an already-installed DuckDB extension.
   */
  async loadExtension(tabId: string, extensionName: string): Promise<void> {
    await this.runExtensionAction(tabId, extensionName, "Loading", "load");
  }

  /**
   * Update an installed DuckDB extension.
   */
  async updateExtension(tabId: string, extensionName: string): Promise<void> {
    await this.runExtensionAction(tabId, extensionName, "Updating", "update");
  }

  /**
   * Install and load a community extension.
   */
  async installCommunityExtension(tabId: string, extensionName: string): Promise<void> {
    await this.runExtensionAction(tabId, extensionName, "Installing", "installCommunity");
  }

  /**
   * Install and load a DuckDB extension.
   */
  async installAndLoadExtension(tabId: string, extensionName: string): Promise<void> {
    await this.runExtensionAction(tabId, extensionName, "Installing", "installAndLoad");
  }

  /** Core (and the demo) validate the name and build each statement. */
  private async runExtensionAction(
    tabId: string,
    extensionName: string,
    actionLabel: string,
    type: Exclude<ExtensionAction["type"], "list">,
  ): Promise<void> {
    const tab = this.getProjectTabs().find((t) => t.id === tabId);
    if (!tab) return;
    this.updateTab(tabId, (t) => ({
      ...t,
      actionInProgress: { ...t.actionInProgress, [extensionName]: actionLabel },
    }));

    try {
      await this.runAction(tab.connectionId, { type, name: extensionName });
      await this.loadExtensions(tabId);
    } catch (error) {
      this.updateTab(tabId, (t) => ({
        ...t,
        actionInProgress: { ...t.actionInProgress, [extensionName]: undefined } as Record<
          string,
          string
        >,
        error:
          error instanceof Error
            ? error.message
            : `Failed to ${actionLabel.toLowerCase()} ${extensionName}`,
      }));
    }
  }

  private async loadCommunityExtensions(tabId: string): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const tabs = this.getProjectTabs();
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab) return;

    // Skip if already loaded
    if (tab.communityExtensions && tab.communityExtensions.length > 0) return;

    this.updateTab(tabId, (t) => ({ ...t, isCommunityLoading: true, communityError: undefined }));

    try {
      const response = await fetch("https://duckdb.org/community_extensions/list_of_extensions");
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const html = await response.text();
      const communityExtensions = parseCommunityExtensionsHtml(html);

      this.updateTab(tabId, (t) => ({
        ...t,
        communityExtensions,
        isCommunityLoading: false,
      }));
    } catch (error) {
      this.updateTab(tabId, (t) => ({
        ...t,
        isCommunityLoading: false,
        communityError:
          error instanceof Error ? error.message : "Failed to load community extensions",
      }));
    }
  }

  /**
   * Force reload community extensions (clears cache).
   */
  async refreshCommunityExtensions(tabId: string): Promise<void> {
    this.updateTab(tabId, (t) => ({ ...t, communityExtensions: undefined }));
    await this.loadCommunityExtensions(tabId);
  }

  private async loadExtensions(tabId: string): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const tabs = this.getProjectTabs();
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab) return;

    this.updateTab(tabId, (t) => ({ ...t, isLoading: true, error: undefined }));

    try {
      const rows = (await this.runAction(tab.connectionId, { type: "list" })) ?? [];

      const extensions: DuckDBExtension[] = rows.map((row) => ({
        extension_name: String((row.extension_name as string) ?? ""),
        loaded: Boolean(row.loaded),
        installed: Boolean(row.installed),
        install_path: String((row.install_path as string) ?? ""),
        description: String((row.description as string) ?? ""),
        aliases: Array.isArray(row.aliases) ? row.aliases.map(String) : [],
        extension_version: String((row.extension_version as string) ?? ""),
        install_mode: String((row.install_mode as string) ?? ""),
        installed_from: String((row.installed_from as string) ?? ""),
      }));

      this.updateTab(tabId, (t) => ({
        ...t,
        extensions,
        isLoading: false,
        lastRefreshed: new Date(),
        actionInProgress: undefined,
      }));
    } catch (error) {
      this.updateTab(tabId, (t) => ({
        ...t,
        isLoading: false,
        error: error instanceof Error ? error.message : "Failed to load extensions",
      }));
    }
  }
}
