import type { ConnectionTab, ConnectionFormData, ActiveViewType } from "$lib/types";
import type { DatabaseType, SSHAuthMethod } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";
import { getKeyringService } from "$lib/services/keyring";

/**
 * Prefill data for opening a connection tab, matching the old ConnectionDialogPrefill shape.
 */
export interface ConnectionTabPrefill {
  id?: string;
  name?: string;
  type?: DatabaseType;
  host?: string;
  port?: number;
  databaseName?: string;
  username?: string;
  password?: string;
  sslMode?: string;
  connectionString?: string;
  sshTunnel?: {
    enabled: boolean;
    host: string;
    port: number;
    username: string;
    authMethod: SSHAuthMethod;
    keyPath?: string;
  };
  savePassword?: boolean;
  saveSshPassword?: boolean;
  saveSshKeyPassphrase?: boolean;
  /** AI privacy overrides; undefined means the connection follows the global setting. */
  aiShareSchema?: boolean;
  aiShareData?: boolean;
}

export const defaultFormData: ConnectionFormData = {
  name: "",
  type: "postgres",
  host: "localhost",
  port: 5432,
  databaseName: "",
  username: "",
  password: "",
  sslMode: "disable",
  connectionString: "",
  sshEnabled: false,
  sshHost: "",
  sshPort: 22,
  sshUsername: "",
  sshAuthMethod: "password",
  sshPassword: "",
  sshKeyPath: "",
  sshKeyPassphrase: "",
  savePassword: true,
  saveSshPassword: true,
  saveSshKeyPassphrase: true,
};

/**
 * Manages connection tabs.
 * Tabs are organized per-project.
 */
export class ConnectionTabManager extends BaseTabManager<ConnectionTab> {
  private setActiveView: (view: ActiveViewType) => void;

  // Tracks the view that was active before the connection tab was opened,
  // so we can restore it when the last connection tab is closed.
  private previousView: ActiveViewType | null = null;

  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
    setActiveView: (view: ActiveViewType) => void,
  ) {
    super(state, tabOrdering, schedulePersistence);
    this.setActiveView = setActiveView;
  }

  protected get accessors(): TabStateAccessors<ConnectionTab> {
    return {
      getTabs: () => this.state.connectionTabsByProject,
      setTabs: (r) => (this.state.connectionTabsByProject = r),
      getActiveId: () => this.state.activeConnectionTabIdByProject,
      setActiveId: (r) => (this.state.activeConnectionTabIdByProject = r),
    };
  }

  /**
   * Open a connection tab. Main entry point.
   * If prefill is provided, populates form data from an existing connection.
   */
  async open(
    prefill?: ConnectionTabPrefill,
    mode?: "wizard" | "reconnect" | "edit",
  ): Promise<string | null> {
    if (!this.state.activeProjectId) return null;

    const resolvedMode = mode ?? (prefill?.id ? "reconnect" : "wizard");

    // Build form data from prefill or defaults
    let formData: ConnectionFormData;
    let connectionId: string | null = null;

    if (prefill) {
      formData = {
        name: prefill.name || "",
        type: (prefill.type as DatabaseType) || "postgres",
        host: prefill.host || "localhost",
        port: prefill.port || 5432,
        databaseName: prefill.databaseName || "",
        username: prefill.username || "",
        password: prefill.password || "",
        sslMode: prefill.sslMode || "disable",
        connectionString: prefill.connectionString || "",
        sshEnabled: prefill.sshTunnel?.enabled || false,
        sshHost: prefill.sshTunnel?.host || "",
        sshPort: prefill.sshTunnel?.port || 22,
        sshUsername: prefill.sshTunnel?.username || "",
        sshAuthMethod: prefill.sshTunnel?.authMethod || "password",
        sshPassword: "",
        sshKeyPath: prefill.sshTunnel?.keyPath || "",
        sshKeyPassphrase: "",
        savePassword: prefill.savePassword ?? true,
        saveSshPassword: prefill.saveSshPassword ?? true,
        saveSshKeyPassphrase: prefill.saveSshKeyPassphrase ?? true,
        aiShareSchema: prefill.aiShareSchema,
        aiShareData: prefill.aiShareData,
      };
      connectionId = prefill.id || null;
    } else {
      formData = { ...defaultFormData };
    }

    const currentStep = resolvedMode === "wizard" ? "method" : "details";

    const newTab: ConnectionTab = {
      id: `connection-${crypto.randomUUID()}`,
      name: prefill?.name ? `Connect: ${prefill.name}` : "New Connection",
      mode: resolvedMode,
      currentStep,
      formData,
      connectionId,
      isConnecting: false,
      isTesting: false,
      error: null,
      credentialsLoaded: false,
    };

    this.appendTab(newTab);
    if (this.previousView === null) {
      this.previousView = this.state.activeView;
    }
    this.setActiveView("connection");

    // Load saved credentials asynchronously
    if (connectionId && prefill) {
      await this.loadSavedCredentials(newTab.id, connectionId, prefill);
    } else {
      this.updateTab(newTab.id, (t) => ({ ...t, credentialsLoaded: true }));
    }

    return newTab.id;
  }

  /**
   * Add a connection tab (BaseTabManager compatibility).
   * For connection tabs, prefer using open() instead.
   */
  add(): string | null {
    if (!this.state.activeProjectId) return null;

    const newTab: ConnectionTab = {
      id: `connection-${crypto.randomUUID()}`,
      name: "New Connection",
      mode: "wizard",
      currentStep: "method",
      formData: { ...defaultFormData },
      connectionId: null,
      isConnecting: false,
      isTesting: false,
      error: null,
      credentialsLoaded: true,
    };

    this.appendTab(newTab);
    if (this.previousView === null) {
      this.previousView = this.state.activeView;
    }
    this.setActiveView("connection");

    return newTab.id;
  }

  /**
   * Load saved credentials from keyring into the tab's form data.
   */
  private async loadSavedCredentials(
    tabId: string,
    connectionId: string,
    prefill: ConnectionTabPrefill,
  ): Promise<void> {
    const keyring = getKeyringService();
    if (!keyring.isAvailable()) {
      this.updateTab(tabId, (t) => ({ ...t, credentialsLoaded: true }));
      return;
    }

    try {
      const tabs = this.getProjectTabs();
      const tab = tabs.find((t) => t.id === tabId);
      if (!tab) return;

      let password = tab.formData.password;
      let sshPassword = tab.formData.sshPassword;
      let sshKeyPassphrase = tab.formData.sshKeyPassphrase;

      if (prefill.savePassword) {
        const savedPassword = await keyring.getDbPassword(connectionId);
        if (savedPassword) {
          password = savedPassword;
        }
      }
      if (prefill.saveSshPassword) {
        const savedSshPassword = await keyring.getSshPassword(connectionId);
        if (savedSshPassword) {
          sshPassword = savedSshPassword;
        }
      }
      if (prefill.saveSshKeyPassphrase) {
        const savedPassphrase = await keyring.getSshKeyPassphrase(connectionId);
        if (savedPassphrase) {
          sshKeyPassphrase = savedPassphrase;
        }
      }

      this.updateTab(tabId, (t) => ({
        ...t,
        formData: {
          ...t.formData,
          password,
          sshPassword,
          sshKeyPassphrase,
        },
        credentialsLoaded: true,
      }));
    } catch (error) {
      console.warn("Failed to load credentials from keyring:", error);
      this.updateTab(tabId, (t) => ({ ...t, credentialsLoaded: true }));
    }
  }

  /**
   * Update form data for a connection tab.
   */
  updateFormData(tabId: string, updates: Partial<ConnectionFormData>): void {
    this.updateTab(tabId, (t) => ({
      ...t,
      formData: { ...t.formData, ...updates },
    }));
  }

  /**
   * Set the current step for a connection tab.
   */
  setStep(tabId: string, step: "method" | "details"): void {
    this.updateTab(tabId, (t) => ({ ...t, currentStep: step }));
  }

  /**
   * Set the error message for a connection tab.
   */
  setError(tabId: string, error: string | null): void {
    this.updateTab(tabId, (t) => ({ ...t, error }));
  }

  /**
   * Set the connecting state for a connection tab.
   */
  setConnecting(tabId: string, isConnecting: boolean): void {
    this.updateTab(tabId, (t) => ({ ...t, isConnecting }));
  }

  /**
   * Set the testing state for a connection tab.
   */
  setTesting(tabId: string, isTesting: boolean): void {
    this.updateTab(tabId, (t) => ({ ...t, isTesting }));
  }

  /**
   * Remove a connection tab by ID.
   */
  override remove(id: string): void {
    super.remove(id);

    // If no more connection tabs, restore the view that was active before the connection tab was opened
    const remainingTabs = this.state.connectionTabsByProject[this.state.activeProjectId!] ?? [];
    if (remainingTabs.length === 0) {
      const restoreView = this.previousView ?? "query";
      this.previousView = null;
      this.setActiveView(restoreView);
    }
  }
}
