import type { PreviewProject } from "$lib/hooks/database/shared/index";

/**
 * A project directory Core's scan found, with its tick. One a local project
 * already links (`linkedProjectIds`) is shown as such, unticked, and can't be
 * ticked: importing it again would make a second project ("Team (2)")
 * silently.
 */
export type ImportableSharedProject = PreviewProject & {
  selected: boolean;
  alreadyLinked: boolean;
};

export interface OpenOptions {
  /** Run with the new projects' ids once they're imported (a deep link opens its connection). */
  onImported?: (projectIds: string[]) => Promise<void>;
}

class SharedProjectImportStore {
  isOpen = $state(false);
  isImporting = $state(false);
  folderPath = $state<string | null>(null);
  discoveredProjects = $state<ImportableSharedProject[]>([]);
  /** What runs after the import (cleared by `reset`, so a cancel runs nothing). */
  onImported: OpenOptions["onImported"] = undefined;

  openWithResults(
    path: string,
    projects: readonly PreviewProject[],
    options: OpenOptions = {},
  ): void {
    this.folderPath = path;
    this.discoveredProjects = projects.map((p) => {
      const alreadyLinked = p.linkedProjectIds.length > 0;
      return { ...p, alreadyLinked, selected: !alreadyLinked };
    });
    this.onImported = options.onImported;
    this.isOpen = true;
  }

  toggleProject(index: number): void {
    const project = this.discoveredProjects[index];
    if (!project || project.alreadyLinked) return;
    this.discoveredProjects = this.discoveredProjects.map((p, i) =>
      i === index ? { ...p, selected: !p.selected } : p,
    );
  }

  selectAll(): void {
    this.discoveredProjects = this.discoveredProjects.map((p) => ({
      ...p,
      selected: !p.alreadyLinked,
    }));
  }

  deselectAll(): void {
    this.discoveredProjects = this.discoveredProjects.map((p) => ({ ...p, selected: false }));
  }

  reset(): void {
    this.isOpen = false;
    this.isImporting = false;
    this.folderPath = null;
    this.discoveredProjects = [];
    this.onImported = undefined;
  }
}

export const sharedProjectImportStore = new SharedProjectImportStore();
