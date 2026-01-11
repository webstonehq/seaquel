import type { ConnectionLabel, DatabaseConnection } from "$lib/types";
import { PREDEFINED_LABELS } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";

/**
 * Manages connection labels and their operations.
 */
export class LabelManager {
  constructor(
    private state: DatabaseState,
    private persistence: PersistenceManager
  ) {}

  /**
   * Get all available labels for a project (predefined + custom).
   */
  getLabelsForProject(projectId: string): ConnectionLabel[] {
    const project = this.state.projects.find((p) => p.id === projectId);
    return [
      ...Object.values(PREDEFINED_LABELS),
      ...(project?.customLabels ?? []),
    ];
  }

  /**
   * Get labels for a specific connection (resolve label IDs to label objects).
   */
  getConnectionLabels(connection: DatabaseConnection): ConnectionLabel[] {
    const allLabels = this.getLabelsForProject(connection.projectId);
    return connection.labelIds
      .map((id) => allLabels.find((l) => l.id === id))
      .filter((l): l is ConnectionLabel => l !== undefined);
  }

  /**
   * Get labels for a connection by ID.
   */
  getConnectionLabelsById(connectionId: string): ConnectionLabel[] {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) return [];
    return this.getConnectionLabels(connection);
  }

  /**
   * Create a snapshot of labels for a connection.
   * Used when saving to query history.
   */
  createLabelSnapshot(connection: DatabaseConnection): ConnectionLabel[] {
    return this.getConnectionLabels(connection).map((label) => ({ ...label }));
  }

  /**
   * Create a snapshot of labels for a connection by ID.
   */
  createLabelSnapshotById(connectionId: string): ConnectionLabel[] {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) return [];
    return this.createLabelSnapshot(connection);
  }

  /**
   * Add a label to a connection.
   */
  addLabelToConnection(connectionId: string, labelId: string): void {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) return;

    // Check if label exists and isn't already added
    const allLabels = this.getLabelsForProject(connection.projectId);
    if (!allLabels.some((l) => l.id === labelId)) return;
    if (connection.labelIds.includes(labelId)) return;

    // Update connection
    this.state.connections = this.state.connections.map((c) => {
      if (c.id !== connectionId) return c;
      return {
        ...c,
        labelIds: [...c.labelIds, labelId],
      };
    });

    // Persist connection
    const updatedConnection = this.state.connections.find((c) => c.id === connectionId);
    if (updatedConnection) {
      this.persistence.persistConnection(updatedConnection);
    }
  }

  /**
   * Remove a label from a connection.
   */
  removeLabelFromConnection(connectionId: string, labelId: string): void {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) return;
    if (!connection.labelIds.includes(labelId)) return;

    // Update connection
    this.state.connections = this.state.connections.map((c) => {
      if (c.id !== connectionId) return c;
      return {
        ...c,
        labelIds: c.labelIds.filter((id) => id !== labelId),
      };
    });

    // Persist connection
    const updatedConnection = this.state.connections.find((c) => c.id === connectionId);
    if (updatedConnection) {
      this.persistence.persistConnection(updatedConnection);
    }
  }

  /**
   * Set all labels for a connection.
   */
  setConnectionLabels(connectionId: string, labelIds: string[]): void {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) return;

    // Validate all label IDs exist
    const allLabels = this.getLabelsForProject(connection.projectId);
    const validLabelIds = labelIds.filter((id) => allLabels.some((l) => l.id === id));

    // Update connection
    this.state.connections = this.state.connections.map((c) => {
      if (c.id !== connectionId) return c;
      return {
        ...c,
        labelIds: validLabelIds,
      };
    });

    // Persist connection
    const updatedConnection = this.state.connections.find((c) => c.id === connectionId);
    if (updatedConnection) {
      this.persistence.persistConnection(updatedConnection);
    }
  }

  /**
   * Check if a connection has a specific label.
   */
  connectionHasLabel(connectionId: string, labelId: string): boolean {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) return false;
    return connection.labelIds.includes(labelId);
  }

  /**
   * Get the label object by ID for a specific project.
   */
  getLabelById(projectId: string, labelId: string): ConnectionLabel | undefined {
    const allLabels = this.getLabelsForProject(projectId);
    return allLabels.find((l) => l.id === labelId);
  }

  /**
   * Check if a label ID is a predefined label.
   */
  isPredefinedLabel(labelId: string): boolean {
    return Object.values(PREDEFINED_LABELS).some((l) => l.id === labelId);
  }
}
