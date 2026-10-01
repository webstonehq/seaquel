import type { ConnectionIdentity } from "$lib/services/connection-import";
import { getSettings } from "$lib/hooks/database/library/index";
import { onStoredChange, toastIfStorageFull } from "./settings-sync";
import type { ImportableConnection } from "$lib/types/dbeaver";
import { discoverDbeaverConnections } from "$lib/services/dbeaver-import";

export class DbeaverImportStore {
  // Dialog state
  isOpen = $state(false);
  isLoading = $state(false);

  // Discovered connections
  connections = $state<ImportableConnection[]>([]);

  // Persistence tracking
  hasOfferedImport = $state(false);
  private initialized = false;

  constructor() {
    // Another window offered the import: don't offer it here again.
    onStoredChange("importState", (ids) =>
      this.initialized && (ids === null || ids.includes("dbeaver")) ? this.read() : undefined,
    );
  }

  private async read(): Promise<void> {
    const { value } = await getSettings().getImportState("dbeaver");
    if (value) this.hasOfferedImport = value.hasOfferedImport;
  }

  /**
   * Initialize the store - loads persisted state
   */
  async initialize(): Promise<void> {
    if (this.initialized) return;
    this.initialized = true;

    // Load persisted state
    try {
      await this.read();
    } catch (error) {
      console.error("Failed to load DBeaver import state:", error);
    }
  }

  /**
   * Check for DBeaver connections and open dialog if found
   * Called when user clicks on DBeaver card
   */
  /**
   * `existing`: the connections of the project they'd be imported into; one
   * already there (same type, host, port, database and user) is shown as a
   * duplicate.
   */
  async checkAndShowDialog(existing: readonly ConnectionIdentity[]): Promise<void> {
    this.isLoading = true;

    try {
      const importable = await discoverDbeaverConnections(existing);

      if (importable.length > 0) {
        this.connections = importable;
        this.isOpen = true;
      }
    } catch (error) {
      console.error("Failed to check DBeaver connections:", error);
    } finally {
      this.isLoading = false;
    }
  }

  /**
   * Toggle selection state for a connection
   */
  toggleConnection(index: number): void {
    if (this.connections[index] && !this.connections[index].isDuplicate) {
      this.connections[index].selected = !this.connections[index].selected;
      // Trigger reactivity
      this.connections = [...this.connections];
    }
  }

  /**
   * Select all non-duplicate connections
   */
  selectAll(): void {
    this.connections = this.connections.map((c) => ({
      ...c,
      selected: !c.isDuplicate,
    }));
  }

  /**
   * Deselect all connections
   */
  deselectAll(): void {
    this.connections = this.connections.map((c) => ({
      ...c,
      selected: false,
    }));
  }

  /**
   * Get all selected connections
   */
  getSelectedConnections(): ImportableConnection[] {
    return this.connections.filter((c) => c.selected);
  }

  /**
   * Dismiss the dialog without importing
   */
  async dismiss(): Promise<void> {
    this.isOpen = false;
    this.hasOfferedImport = true;
    await this.persist();
  }

  /**
   * Complete the import and close dialog
   */
  async completeImport(): Promise<void> {
    this.isOpen = false;
    this.hasOfferedImport = true;
    await this.persist();
  }

  /**
   * Persist state to store
   */
  private async persist(): Promise<void> {
    try {
      await getSettings().saveImportState(
        "dbeaver",
        this.hasOfferedImport,
        new Date().toISOString(),
      );
    } catch (error) {
      toastIfStorageFull(error);
      console.error("Failed to persist DBeaver import state:", error);
    }
  }
}

export const dbeaverImportStore = new DbeaverImportStore();
