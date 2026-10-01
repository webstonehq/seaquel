import type { ConnectionLabel, DatabaseConnection } from "$lib/types";
import { PREDEFINED_LABELS } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import { patchConnection } from "./library/view.js";

/**
 * Manages connection labels and their operations.
 */
export class LabelManager {
  /** Labels are stored through the library. */
  constructor(private state: DatabaseState) {}

  /**
   * Get all available labels for a project (predefined + custom).
   */
  getLabelsForProject(projectId: string): ConnectionLabel[] {
    const project = this.state.projects.find((p) => p.id === projectId);
    return [...Object.values(PREDEFINED_LABELS), ...(project?.customLabels ?? [])];
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
  async addLabelToConnection(connectionId: string, labelId: string): Promise<void> {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) return;

    // Already added: nothing to store. Whether the label exists is Core's
    // check (`LABEL_NOT_FOUND` for one that is neither predefined nor the
    // project's own, Decision 7), not the page's.
    if (connection.labelIds.includes(labelId)) return;

    // Stored, then shown: a refusal throws (worded for the user) and changes nothing.
    await patchConnection(this.state, connectionId, {
      labelIds: [...connection.labelIds, labelId],
    });
  }

  /**
   * Remove a label from a connection.
   */
  async removeLabelFromConnection(connectionId: string, labelId: string): Promise<void> {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) return;
    if (!connection.labelIds.includes(labelId)) return;

    // Stored, then shown: a refusal throws (worded for the user) and changes nothing.
    await patchConnection(this.state, connectionId, {
      labelIds: connection.labelIds.filter((id) => id !== labelId),
    });
  }

  /**
   * Set all labels for a connection. Core refuses an id that is neither
   * predefined nor the project's own (`LABEL_NOT_FOUND`, Decision 7), where
   * this used to drop it silently.
   */
  async setConnectionLabels(connectionId: string, labelIds: string[]): Promise<void> {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) return;
    await patchConnection(this.state, connectionId, { labelIds });
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
