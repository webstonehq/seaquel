# Connection Tab Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the connection dialog with a full connection tab that integrates into the existing tab system, providing more real estate for connection configuration.

**Architecture:** Add a new `ConnectionTab` type following the same `BaseTabManager` pattern as ERD/Statistics tabs. The connection wizard's two-step UI (method selection + details form) is reused as full-width tab content instead of a constrained dialog. All 10 entry points that opened the dialog are rewired to open a connection tab. The dialog and its stores are then deleted.

**Tech Stack:** Svelte 5, TypeScript, Tailwind CSS, bits-ui

---

### Task 1: Define ConnectionTab types

**Files:**
- Create: `src/lib/types/connection-tab.ts`
- Modify: `src/lib/types/index.ts`
- Modify: `src/lib/types/persisted.ts`
- Modify: `src/lib/types/project.ts`

**Step 1: Create the ConnectionTab type**

```typescript
// src/lib/types/connection-tab.ts

import type { DatabaseType, SSHAuthMethod } from "./database";

/**
 * Mode for connection tab behavior.
 * - wizard: New connection (shows method step first)
 * - reconnect: Reconnecting (shows details, may auto-connect)
 * - edit: Editing existing connection settings
 */
export type ConnectionTabMode = "wizard" | "reconnect" | "edit";

/**
 * Form data for the connection tab.
 */
export interface ConnectionFormData {
  name: string;
  type: DatabaseType;
  host: string;
  port: number;
  databaseName: string;
  username: string;
  password: string;
  sslMode: string;
  connectionString: string;
  sshEnabled: boolean;
  sshHost: string;
  sshPort: number;
  sshUsername: string;
  sshAuthMethod: SSHAuthMethod;
  sshPassword: string;
  sshKeyPath: string;
  sshKeyPassphrase: string;
  savePassword: boolean;
  saveSshPassword: boolean;
  saveSshKeyPassphrase: boolean;
}

/**
 * A tab for creating, editing, or reconnecting a database connection.
 */
export interface ConnectionTab {
  id: string;
  name: string;
  mode: ConnectionTabMode;
  /** Step in wizard mode: "method" or "details" */
  currentStep: "method" | "details";
  /** Form data for the connection */
  formData: ConnectionFormData;
  /** Connection ID being reconnected/edited (null for new) */
  connectionId: string | null;
  /** Loading states */
  isConnecting: boolean;
  isTesting: boolean;
  /** Error message */
  error: string | null;
  /** Whether keyring credentials have been loaded */
  credentialsLoaded: boolean;
}
```

**Step 2: Add persisted type**

Add to `src/lib/types/persisted.ts`:

```typescript
/**
 * Persisted connection tab state.
 * We don't persist form data (passwords etc.) - only the tab's existence.
 * On restore, connection tabs are discarded since they contain transient state.
 */
export interface PersistedConnectionTab {
  id: string;
  name: string;
}
```

Also update the `ActiveViewType` union:

```typescript
export type ActiveViewType =
  | "query"
  | "schema"
  | "explain"
  | "erd"
  | "statistics"
  | "canvas"
  | "visualize"
  | "connection";
```

**Step 3: Export from index.ts**

Add to `src/lib/types/index.ts`:

```typescript
// Connection tab types
export type { ConnectionTab, ConnectionTabMode, ConnectionFormData } from "./connection-tab";
```

And add `PersistedConnectionTab` to the persisted exports.

**Step 4: Update PersistedProjectState**

In `src/lib/types/project.ts`, add to `PersistedProjectState`:

```typescript
  /** Connection management tabs */
  connectionTabs?: import("./persisted").PersistedConnectionTab[];
  /** Currently active connection tab */
  activeConnectionTabId?: string | null;
```

---

### Task 2: Add ConnectionTab state to DatabaseState

**Files:**
- Modify: `src/lib/hooks/database/state.svelte.ts`

**Step 1: Add state properties**

Add after the visualize tab state block (around line 71), before the saved canvases line:

```typescript
  // Connection tabs (for creating/editing connections)
  connectionTabsByProject = $state<Record<string, ConnectionTab[]>>({});
  activeConnectionTabIdByProject = $state<Record<string, string | null>>({});
```

Add the import of `ConnectionTab` to the imports at the top.

**Step 2: Add derived values**

Add after the visualize tab derived values section (after line 255):

```typescript
  // === CONNECTION TAB DERIVED VALUES ===

  // Derived: connection tabs for active project
  connectionTabs = $derived(
    this.activeProjectId ? (this.connectionTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active connection tab ID for active project
  activeConnectionTabId = $derived(
    this.activeProjectId
      ? (this.activeConnectionTabIdByProject[this.activeProjectId] ?? null)
      : null,
  );

  // Derived: active connection tab object
  activeConnectionTab = $derived(
    this.connectionTabs.find((t) => t.id === this.activeConnectionTabId) || null,
  );
```

**Step 3: Update the activeView type**

Change the `activeView` state declaration to include `"connection"`:

```typescript
  activeView = $state<
    "query" | "schema" | "explain" | "erd" | "statistics" | "canvas" | "visualize" | "connection"
  >("query");
```

---

### Task 3: Create ConnectionTabManager

**Files:**
- Create: `src/lib/hooks/database/connection-tabs.svelte.ts`

**Step 1: Implement the manager**

```typescript
import type { ConnectionTab, ConnectionFormData, ConnectionTabMode } from "$lib/types";
import type { DatabaseType, SSHAuthMethod } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";
import { getKeyringService } from "$lib/services/keyring";

/** Prefill data for opening a connection tab (matches old ConnectionDialogPrefill) */
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
}

const defaultFormData: ConnectionFormData = {
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
 * Manages connection tabs for creating, editing, and reconnecting connections.
 */
export class ConnectionTabManager extends BaseTabManager<ConnectionTab> {
  private setActiveView: (view: "connection") => void;

  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
    setActiveView: (view: "connection") => void,
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
   * Open a connection tab. This is the main entry point replacing connectionDialogStore.open().
   * - No prefill: new connection wizard
   * - With prefill + no mode: auto-determines reconnect vs wizard
   * - With prefill + mode: uses specified mode
   *
   * Returns the tab ID, or null if no active project.
   * For reconnect mode, returns a promise that resolves after auto-connect attempt.
   */
  async open(prefill?: ConnectionTabPrefill, mode?: ConnectionTabMode): Promise<string | null> {
    if (!this.state.activeProjectId) return null;

    const resolvedMode = mode ?? (prefill?.id ? "reconnect" : "wizard");

    // For edit/reconnect of an existing connection, check if a tab already exists
    if (prefill?.id) {
      const existing = this.getProjectTabs().find(
        (t) => t.connectionId === prefill.id && t.mode === resolvedMode,
      );
      if (existing) {
        this.setActiveTabId(existing.id);
        this.setActiveView("connection");
        return existing.id;
      }
    }

    const formData: ConnectionFormData = prefill
      ? {
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
        }
      : { ...defaultFormData };

    let tabName: string;
    if (resolvedMode === "edit") {
      tabName = `Edit: ${formData.name || "Connection"}`;
    } else if (resolvedMode === "reconnect") {
      tabName = `Reconnect: ${formData.name || "Connection"}`;
    } else {
      tabName = "New Connection";
    }

    const tab: ConnectionTab = {
      id: `connection-${Date.now()}`,
      name: tabName,
      mode: resolvedMode,
      currentStep: resolvedMode === "wizard" ? "method" : "details",
      formData,
      connectionId: prefill?.id || null,
      isConnecting: false,
      isTesting: false,
      error: null,
      credentialsLoaded: false,
    };

    this.appendTab(tab);
    this.setActiveView("connection");

    // Load credentials from keyring if reconnecting/editing
    if (prefill?.id) {
      await this.loadSavedCredentials(tab.id, prefill.id, prefill);
    } else {
      this.updateTab(tab.id, (t) => ({ ...t, credentialsLoaded: true }));
    }

    return tab.id;
  }

  /**
   * Add a new connection tab (simple form, used by BaseTabManager pattern).
   * For full control, use open() instead.
   */
  add(): string | null {
    // Fire and forget - open() is async but add() signature isn't
    void this.open();
    return null;
  }

  /**
   * Load saved credentials from the system keyring.
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
      const tab = this.getProjectTabs().find((t) => t.id === tabId);
      if (!tab) return;

      const updates: Partial<ConnectionFormData> = {};

      if (prefill.savePassword) {
        const savedPassword = await keyring.getDbPassword(connectionId);
        if (savedPassword) updates.password = savedPassword;
      }
      if (prefill.saveSshPassword) {
        const savedSshPassword = await keyring.getSshPassword(connectionId);
        if (savedSshPassword) updates.sshPassword = savedSshPassword;
      }
      if (prefill.saveSshKeyPassphrase) {
        const savedPassphrase = await keyring.getSshKeyPassphrase(connectionId);
        if (savedPassphrase) updates.sshKeyPassphrase = savedPassphrase;
      }

      this.updateTab(tabId, (t) => ({
        ...t,
        formData: { ...t.formData, ...updates },
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
   * Set the current wizard step.
   */
  setStep(tabId: string, step: "method" | "details"): void {
    this.updateTab(tabId, (t) => ({ ...t, currentStep: step }));
  }

  /**
   * Set error state.
   */
  setError(tabId: string, error: string | null): void {
    this.updateTab(tabId, (t) => ({ ...t, error }));
  }

  /**
   * Set connecting state.
   */
  setConnecting(tabId: string, isConnecting: boolean): void {
    this.updateTab(tabId, (t) => ({ ...t, isConnecting }));
  }

  /**
   * Set testing state.
   */
  setTesting(tabId: string, isTesting: boolean): void {
    this.updateTab(tabId, (t) => ({ ...t, isTesting }));
  }

  /**
   * Override remove to switch back to query view if no connection tabs remain.
   */
  override remove(id: string): void {
    super.remove(id);

    const remainingTabs = this.state.connectionTabsByProject[this.state.activeProjectId!] ?? [];
    if (remainingTabs.length === 0) {
      // Switch to query view or stay on whatever was active before
      // Don't force a view switch - let the tab ordering handle it
    }
  }
}
```

---

### Task 4: Wire ConnectionTabManager into UseDatabase

**Files:**
- Modify: `src/lib/hooks/database.svelte.ts`
- Modify: `src/lib/hooks/database/tab-ordering.svelte.ts`
- Modify: `src/lib/hooks/database/ui-state.svelte.ts`

**Step 1: Update UIStateManager view type**

In `src/lib/hooks/database/ui-state.svelte.ts`, update the `setActiveView` method signature and the constructor's type:

```typescript
  setActiveView(
    view: "query" | "schema" | "explain" | "erd" | "statistics" | "canvas" | "visualize" | "connection",
  ) {
```

**Step 2: Update TabOrderingManager**

In `src/lib/hooks/database/tab-ordering.svelte.ts`:

1. Add `ConnectionTab` to imports
2. Add `"connection"` to the type union in the `ordered` getter
3. Add connection tabs to the `allTabsUnordered` array

The type union becomes:
```typescript
type: "query" | "schema" | "explain" | "erd" | "statistics" | "canvas" | "visualize" | "connection";
tab: QueryTab | SchemaTab | ExplainTab | ErdTab | StatisticsTab | CanvasTab | VisualizeTab | ConnectionTab;
```

Add to the body of `ordered`:
```typescript
const connectionTabs = this.state.connectionTabs || [];
// ...
for (const t of connectionTabs) {
  allTabsUnordered.push({ id: t.id, type: "connection", tab: t });
}
```

**Step 3: Update UseDatabase**

In `src/lib/hooks/database.svelte.ts`:

1. Import `ConnectionTabManager`
2. Add `readonly connectionTabs: ConnectionTabManager;` to the class
3. Instantiate in constructor:

```typescript
this.connectionTabs = new ConnectionTabManager(
  this.state,
  this.tabs,
  scheduleProjectPersistence,
  setActiveView,
);
```

4. Update the `setActiveView` callback type to include `"connection"`.

---

### Task 5: Add persistence support

**Files:**
- Modify: `src/lib/hooks/database/persistence-manager.svelte.ts`
- Modify: `src/lib/hooks/database/project-manager.svelte.ts`

**Step 1: Add serializer to PersistenceManager**

Connection tabs contain transient state (passwords, loading states) so we only persist minimal info. On restore, we **don't** restore connection tabs since they contain ephemeral wizard state.

Add to `PersistenceManager`:

```typescript
  serializeConnectionTabs(_projectId: string): PersistedConnectionTab[] {
    // Connection tabs are transient - don't persist form data
    // We could persist their existence but it's better to just discard them on restart
    return [];
  }
```

Add to `persistProjectState`:
```typescript
  connectionTabs: [],  // Connection tabs are not persisted
  activeConnectionTabId: null,
```

Import `PersistedConnectionTab` in the imports.

**Step 2: Update project-manager restoration**

In `project-manager.svelte.ts`, add empty initialization for connection tabs in `loadProjectState`:

In the "no persisted state" block:
```typescript
  this.state.connectionTabsByProject[projectId] = [];
  this.state.activeConnectionTabIdByProject[projectId] = null;
```

In the restoration block (no need to restore connection tabs - they're transient):
```typescript
  // Connection tabs are transient and not restored
  this.state.connectionTabsByProject[projectId] = [];
  this.state.activeConnectionTabIdByProject[projectId] = null;
```

---

### Task 6: Create the ConnectionTab view component

**Files:**
- Create: `src/lib/components/connection-tab-view.svelte`

This component replaces the dialog content. It reuses `WizardStepMethod` and `WizardStepDetails` but renders them inline as full tab content instead of inside a dialog.

**Step 1: Create the component**

```svelte
<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import { m } from "$lib/paraglide/messages.js";
  import { useDatabase } from "$lib/hooks/database.svelte.js";
  import { onboardingStore } from "$lib/stores/onboarding.svelte.js";
  import { toast } from "svelte-sonner";
  import { extractErrorMessage } from "$lib/errors/types";
  import ArrowLeftIcon from "@lucide/svelte/icons/arrow-left";
  import type { ConnectionTab } from "$lib/types";
  import { databaseTypes } from "$lib/stores/connection-wizard.svelte.js";

  import WizardStepMethod from "./connection-wizard/wizard-step-method.svelte";
  import WizardStepDetails from "./connection-wizard/wizard-step-details.svelte";

  interface Props {
    tab: ConnectionTab;
  }

  let { tab }: Props = $props();

  const db = useDatabase();

  // Auto-connect when credentials are loaded in reconnect mode
  $effect(() => {
    if (tab.mode === "reconnect" && tab.credentialsLoaded && !tab.isConnecting && !autoConnectAttempted) {
      autoConnectAttempted = true;
      if (hasAllCredentials(tab)) {
        handleAutoConnect();
      }
    }
  });

  let autoConnectAttempted = $state(false);

  const selectedDbType = $derived(
    databaseTypes.find((t) => t.value === tab.formData.type)
  );

  const isReconnecting = $derived(tab.connectionId !== null && tab.mode !== "edit");
  const isEditing = $derived(tab.mode === "edit");
  const showBack = $derived(tab.currentStep === "details" && !isReconnecting && !isEditing);

  const canProceed = $derived(() => {
    if (tab.currentStep === "method") return true;
    if (tab.currentStep === "details") {
      return tab.formData.databaseName.trim().length > 0 && tab.formData.name.trim().length > 0;
    }
    return false;
  });

  function hasAllCredentials(t: ConnectionTab): boolean {
    if (!t.formData.name.trim()) return false;
    if (!t.formData.databaseName.trim()) return false;
    const isFileBasedDb = t.formData.type === "sqlite" || t.formData.type === "duckdb";
    if (!isFileBasedDb && !t.formData.host.trim()) return false;
    if (!isFileBasedDb && !t.formData.password) return false;
    if (t.formData.sshEnabled) {
      if (!t.formData.sshHost.trim()) return false;
      if (!t.formData.sshUsername.trim()) return false;
      if (t.formData.sshAuthMethod === "password" && !t.formData.sshPassword) return false;
      if (t.formData.sshAuthMethod === "key" && !t.formData.sshKeyPath) return false;
    }
    return true;
  }

  function getConnectionData() {
    // Build connection string from form data (reuse logic from connection-wizard store)
    const data = tab.formData;
    let connString = data.connectionString;

    if (!connString || connString.split(":").length !== 3) {
      connString = buildConnectionString(data);
    }

    const { getKeyringService } = $lib_keyring();
    const keyring = getKeyringService();
    const keychainAvailable = keyring.isAvailable();

    return {
      name: data.name,
      type: data.type,
      host: data.host,
      port: data.port,
      databaseName: data.databaseName,
      username: data.username,
      password: data.password,
      sslMode: data.sslMode,
      connectionString: connString,
      sshTunnel: data.sshEnabled
        ? {
            enabled: true,
            host: data.sshHost,
            port: data.sshPort,
            username: data.sshUsername,
            authMethod: data.sshAuthMethod,
            keyPath: data.sshKeyPath || undefined,
          }
        : undefined,
      sshPassword: data.sshPassword,
      sshKeyPath: data.sshKeyPath,
      sshKeyPassphrase: data.sshKeyPassphrase,
      savePassword: keychainAvailable ? data.savePassword : false,
      saveSshPassword: keychainAvailable ? data.saveSshPassword : false,
      saveSshKeyPassphrase: keychainAvailable ? data.saveSshKeyPassphrase : false,
    };
  }

  // NOTE: This is a simplified sketch. The actual implementation should
  // import and reuse buildConnectionString and getConnectionData from
  // the connection-wizard store or extract them into a shared utility.
  // The full logic is already in connectionWizardStore.buildConnectionString()
  // and connectionWizardStore.getConnectionData().

  const handleAutoConnect = async () => {
    db.connectionTabs.setConnecting(tab.id, true);
    try {
      const connectionData = getConnectionData();
      if (tab.connectionId) {
        await db.connections.reconnect(tab.connectionId, connectionData);
      }
      onboardingStore.completeWizard();
      if (db.state.activeSchema.length === 0) {
        toast.warning(m.wizard_connect_empty());
      } else {
        toast.success(m.wizard_connect_success());
      }
      db.connectionTabs.remove(tab.id);
    } catch (error) {
      db.connectionTabs.setConnecting(tab.id, false);
      db.connectionTabs.setError(tab.id, extractErrorMessage(error));
    }
  };

  const handleTestConnection = async () => {
    db.connectionTabs.setError(tab.id, null);
    // ... validation then test (same as wizard)
    db.connectionTabs.setTesting(tab.id, true);
    try {
      const connectionData = getConnectionData();
      await db.connections.test(connectionData);
      toast.success(m.wizard_test_success());
    } catch (error) {
      db.connectionTabs.setError(tab.id, extractErrorMessage(error));
    } finally {
      db.connectionTabs.setTesting(tab.id, false);
    }
  };

  const handleConnect = async () => {
    db.connectionTabs.setError(tab.id, null);
    // Validate...
    db.connectionTabs.setConnecting(tab.id, true);
    try {
      const connectionData = getConnectionData();
      if (tab.mode === "edit" && tab.connectionId) {
        await db.connections.update(tab.connectionId, connectionData);
        toast.success(m.wizard_edit_success());
      } else if (tab.connectionId) {
        await db.connections.reconnect(tab.connectionId, connectionData);
        onboardingStore.completeWizard();
        if (db.state.activeSchema.length === 0) {
          toast.warning(m.wizard_connect_empty());
        } else {
          toast.success(m.wizard_connect_success());
        }
      } else {
        await db.connections.add(connectionData);
        onboardingStore.completeWizard();
        if (db.state.activeSchema.length === 0) {
          toast.warning(m.wizard_connect_empty());
        } else {
          toast.success(m.wizard_connect_success());
        }
      }
      db.connectionTabs.remove(tab.id);
    } catch (error) {
      db.connectionTabs.setError(tab.id, extractErrorMessage(error));
    } finally {
      db.connectionTabs.setConnecting(tab.id, false);
    }
  };

  const handleParse = (connStr: string): boolean => {
    // Reuse parsing logic from connectionWizardStore
    // This should be extracted to a shared utility
    return parseConnectionString(tab.id, connStr);
  };

  const handleSelectType = (type: DatabaseType) => {
    const dbType = databaseTypes.find((t) => t.value === type);
    db.connectionTabs.updateFormData(tab.id, {
      type,
      port: dbType?.defaultPort ?? tab.formData.port,
    });
    db.connectionTabs.setStep(tab.id, "details");
  };
</script>

<div class="flex-1 flex flex-col min-h-0">
  <div class="flex-1 overflow-y-auto">
    <div class="max-w-lg mx-auto py-8 px-4">
      <h2 class="text-lg font-semibold mb-6">
        {#if isEditing}
          {m.wizard_dialog_title_edit()}
        {:else if isReconnecting}
          {m.connection_dialog_title_reconnect()}
        {:else}
          {m.wizard_dialog_title()}
        {/if}
      </h2>

      <div class="min-h-[300px]">
        {#if tab.currentStep === "method"}
          <WizardStepMethod
            bind:formData={tab.formData}
            onParse={handleParse}
            onSelectType={handleSelectType}
            onContinue={() => db.connectionTabs.setStep(tab.id, "details")}
            error={tab.error}
          />
        {:else if tab.currentStep === "details"}
          <WizardStepDetails
            bind:formData={tab.formData}
            {selectedDbType}
            {isReconnecting}
            {isEditing}
            isTesting={tab.isTesting}
            onTest={handleTestConnection}
            error={tab.error}
          />
        {/if}
      </div>

      {#if tab.currentStep === "details"}
        <div class="flex justify-between gap-2 mt-6 pt-4 border-t">
          <div>
            {#if showBack}
              <Button
                variant="ghost"
                onclick={() => db.connectionTabs.setStep(tab.id, "method")}
                disabled={tab.isConnecting}
              >
                <ArrowLeftIcon class="size-4 me-2" />
                {m.wizard_back()}
              </Button>
            {/if}
          </div>
          <div class="flex gap-2">
            <Button
              onclick={handleConnect}
              disabled={!canProceed() || tab.isConnecting || tab.isTesting}
            >
              {#if tab.isConnecting}
                {m.connection_dialog_button_connecting()}
              {:else if isEditing}
                {m.wizard_save()}
              {:else if isReconnecting}
                {m.connection_dialog_button_reconnect()}
              {:else}
                {m.wizard_connect()}
              {/if}
            </Button>
          </div>
        </div>
      {/if}
    </div>
  </div>
</div>
```

**Important implementation note:** The `getConnectionData()`, `buildConnectionString()`, and `parseConnectionString()` logic currently lives in `connectionWizardStore`. These should be extracted into a shared utility (e.g., `src/lib/utils/connection-string.ts`) or the relevant methods should be made into standalone exported functions that the component can import directly. Do NOT duplicate the logic.

---

### Task 7: Update header-tabs.svelte

**Files:**
- Modify: `src/lib/components/header-tabs.svelte`

**Step 1: Add connection tab type support**

1. Import `PlugIcon` (or `CableIcon`) from lucide for the connection tab icon, and import `ConnectionTab` type
2. Add `"connection"` to all type unions throughout the file
3. Add `handleConnectionTabClick` handler
4. Add the connection tab rendering block (after the visualize block)
5. Add connection tab to `closeTabDirect`, `closeTab`, `closeCurrentTab`
6. Update `currentTabIndex` to handle `"connection"` type
7. Update `switchToTab` to handle `"connection"` type

The connection tab should also appear in the tab bar **even when there's no active connection** (since you might be creating a new one). This requires a change: the `{:else if db.state.activeConnection}` gate on line 377 needs to also check for connection tabs. If there are connection tabs but no active connection, we should still show the regular tab bar (or at minimum show the connection tabs).

**Key change:** The tab bar rendering condition should be:
```svelte
{:else if db.state.activeConnection || db.state.connectionTabs.length > 0}
```

---

### Task 8: Update the main page view

**Files:**
- Modify: `src/routes/manage/+page.svelte`

**Step 1: Add connection tab view**

1. Import `ConnectionTabView` component
2. Add the view branch for connection tabs, both in the "active connection" content area and in a new section that handles connection tabs without an active connection:

```svelte
{:else if activeTabType === "connection"}
  {#if db.state.activeConnectionTab}
    <ConnectionTabView tab={db.state.activeConnectionTab} />
  {/if}
```

The connection tab view should be shown **regardless of active connection state** since it's used to create new connections. This means the rendering logic needs adjustment:

```svelte
{#if db.state.connectionsLoading || db.state.projectsLoading}
  <!-- Loading state -->
{:else if activeTabType === "connection" && db.state.activeConnectionTab}
  <!-- Connection tab takes priority - shown with or without active connection -->
  <div class="flex-1 min-h-0 flex flex-col">
    <ConnectionTabView tab={db.state.activeConnectionTab} />
  </div>
{:else if db.state.activeConnection}
  <!-- Regular content area -->
  ...
{:else}
  <!-- Starter tab area -->
  ...
{/if}
```

---

### Task 9: Rewire all entry points

**Files:**
- Modify: `src/lib/components/sidebar-manage.svelte`
- Modify: `src/lib/components/starter-tabs/getting-started-content.svelte`
- Modify: `src/lib/components/empty-states/connections-grid.svelte`
- Modify: `src/lib/components/empty-states/connection-card.svelte`
- Modify: `src/lib/components/query-editor/query-toolbar.svelte`
- Modify: `src/lib/components/command-palette.svelte`
- Modify: `src/lib/components/empty-states/welcome-screen.svelte`
- Modify: `src/lib/components/starter-tabs/no-tabs-empty-state.svelte`

**Step 1: Replace all `connectionDialogStore.open()` calls**

In every file listed above:
1. Remove the import of `connectionDialogStore`
2. Add `useDatabase` import (if not already present) or use the existing `db` reference
3. Replace `connectionDialogStore.open()` with `db.connectionTabs.open()`
4. Replace `connectionDialogStore.open(prefill)` with `db.connectionTabs.open(prefill)`
5. Replace `connectionDialogStore.open(prefill, "edit")` with `db.connectionTabs.open(prefill, "edit")`

The `ConnectionTabPrefill` interface matches the old `ConnectionDialogPrefill` shape, so the call sites should work with minimal changes.

**Detailed changes per file:**

**sidebar-manage.svelte:**
- Line ~182: `connectionDialogStore.open()` → `db.connectionTabs.open()`
- Lines ~123-137 (reconnect): `connectionDialogStore.open({...})` → `db.connectionTabs.open({...})`
- Line ~273 (edit): `connectionDialogStore.open(connection, "edit")` → `db.connectionTabs.open(connection, "edit")`

**getting-started-content.svelte:**
- Line ~76: `connectionDialogStore.open()` → `db.connectionTabs.open()`

**connections-grid.svelte:**
- Line ~33: `connectionDialogStore.open()` → `db.connectionTabs.open()`

**connection-card.svelte:**
- Lines ~40-54 (reconnect): `connectionDialogStore.open({...})` → `db.connectionTabs.open({...})`
- Lines ~60-77 (edit): `connectionDialogStore.open(connection, "edit")` → `db.connectionTabs.open(connection, "edit")`

**query-toolbar.svelte:**
- Lines ~78-92: `connectionDialogStore.open({...})` → `db.connectionTabs.open({...})`

**command-palette.svelte:**
- Lines ~178-192: `connectionDialogStore.open({...})` → `db.connectionTabs.open({...})`

**welcome-screen.svelte:**
- Line ~54: `connectionDialogStore.open()` → `db.connectionTabs.open()`

**no-tabs-empty-state.svelte:**
- Line ~24: `connectionDialogStore.open()` → `db.connectionTabs.open()`

---

### Task 10: Remove the old dialog code

**Files:**
- Delete: `src/lib/stores/connection-dialog.svelte.ts`
- Delete: `src/lib/stores/connection-wizard.svelte.ts` (keep only `databaseTypes` and `DatabaseTypeConfig` — extract these to a shared location first)
- Modify: `src/lib/components/connection-wizard/connection-wizard.svelte` (delete or repurpose)
- Remove dialog usage from any layout/root component that renders `<ConnectionWizard />`

**Step 1: Extract shared constants**

Before deleting `connection-wizard.svelte.ts`, extract `databaseTypes` and `DatabaseTypeConfig` to a shared location (e.g., `src/lib/utils/database-types.ts` or keep them in the store file but rename it). These are needed by the wizard step components and the connection tab manager.

**Step 2: Find and remove the ConnectionWizard component mount**

Search for where `<ConnectionWizard />` is rendered (likely in a layout file or the manage page). Remove it.

```bash
grep -r "ConnectionWizard\|connection-wizard.svelte" src/lib/components/ src/routes/
```

**Step 3: Delete old files**

- Delete `src/lib/stores/connection-dialog.svelte.ts`
- Delete or gut `src/lib/stores/connection-wizard.svelte.ts` (keep only the extracted parts)
- Delete `src/lib/components/connection-wizard/connection-wizard.svelte` (the dialog wrapper — keep the step components)

**Step 4: Clean up any remaining imports**

Search the codebase for any remaining references to:
- `connectionDialogStore`
- `connectionWizardStore`
- `ConnectionWizard` (the component)
- `connection-dialog.svelte`

And remove them.

---

### Task 11: Verify and fix type checking

**Step 1: Run type check**

```bash
npm run check
```

Fix any TypeScript errors. Common issues will be:
- View type unions missing `"connection"` in places we missed
- Import paths for moved types
- The `WizardStepMethod` and `WizardStepDetails` components may use `bind:formData` which needs the formData to be writable from the parent — since we now store formData in the tab state, we need to either:
  a. Use `bind:formData` and sync changes back to the tab manager, or
  b. Pass individual props and use callbacks for updates

The simpler approach is (a): let the child components bind to a local copy derived from the tab, then sync on change. Or, since the tab's `formData` is part of `$state`, binding should work if we pass a reference.

**Step 2: Test the flows**

Verify these scenarios work:
1. Click "+" in sidebar connections → opens new connection tab
2. Click disconnected connection → opens reconnect tab, auto-connects if credentials saved
3. Right-click connection → Edit → opens edit tab
4. Welcome screen "Add Connection" → opens new connection tab
5. Command palette → select disconnected connection → opens reconnect tab
6. Multiple connection tabs can exist simultaneously
7. Connection tabs appear in tab bar, support drag-drop reordering
8. Closing a connection tab works
9. After successful connect, the connection tab is automatically closed
