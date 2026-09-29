import type { ConnectionIdentity } from "$lib/services/connection-import";
import { getStorage } from "$lib/storage";
import type { TablePlusImportableConnection } from "$lib/types/tableplus";
import { discoverTablePlusConnections } from "$lib/services/tableplus-import";

class TablePlusImportStore {
  // Dialog state
  isOpen = $state(false);
  isLoading = $state(false);

  // Discovered connections
  connections = $state<TablePlusImportableConnection[]>([]);

  // Persistence tracking
  hasOfferedImport = $state(false);
  private initialized = false;

  /**
   * Initialize the store - loads persisted state
   */
  async initialize(): Promise<void> {
    if (this.initialized) return;
    this.initialized = true;

    // Load persisted state
    try {
      const persisted = await getStorage().importState.load("tableplus");

      if (persisted) {
        this.hasOfferedImport = persisted.hasOfferedImport;
      }
    } catch (error) {
      console.error("Failed to load TablePlus import state:", error);
    }
  }

  /**
   * Check for TablePlus connections and open dialog if found
   */
  /**
   * `existing`: the connections of the project they'd be imported into; one
   * already there (same type, host, port, database and user) is shown as a
   * duplicate.
   */
  async checkAndShowDialog(existing: readonly ConnectionIdentity[]): Promise<void> {
    this.isLoading = true;

    try {
      const importable = await discoverTablePlusConnections(existing);

      if (importable.length > 0) {
        this.connections = importable;
        this.isOpen = true;
      }
    } catch (error) {
      console.error("Failed to check TablePlus connections:", error);
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
  getSelectedConnections(): TablePlusImportableConnection[] {
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
      await getStorage().importState.save(
        "tableplus",
        this.hasOfferedImport,
        new Date().toISOString(),
      );
    } catch (error) {
      console.error("Failed to persist TablePlus import state:", error);
    }
  }
}

export const tablePlusImportStore = new TablePlusImportStore();
