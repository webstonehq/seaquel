/**
 * The link dialog (Q30): on a project's first link, which of its
 * connections are written to the repo as templates. Every connection is
 * ticked except the ones marked local-only; only ticked ones are exported
 * (Decision 53), the rest stay local.
 */

/** A connection as the dialog lists it. */
export interface LinkCandidate {
  id: string;
  name: string;
  isLocalOnly?: boolean;
}

/** The dialog's ticks over `connections`: all but the local-only ones at first. */
export function linkDialogSelection(connections: readonly LinkCandidate[]) {
  const ticked = new Set(connections.filter((c) => !c.isLocalOnly).map((c) => c.id));
  return {
    toggle(id: string): void {
      if (ticked.has(id)) ticked.delete(id);
      else ticked.add(id);
    },
    isTicked: (id: string) => ticked.has(id),
    /** The ticked ids, in the order the connections are listed. */
    ticked: () => connections.filter((c) => ticked.has(c.id)).map((c) => c.id),
  };
}

class LinkProjectDialogStore {
  isOpen = $state(false);
  projectName = $state("");
  connections = $state<LinkCandidate[]>([]);
  /** The ticked ids, in list order (what `linkProject`'s `share` gets). */
  ticked = $state<string[]>([]);
  private selection: ReturnType<typeof linkDialogSelection> | null = null;
  private resolve: ((share: string[] | null) => void) | null = null;

  /**
   * Ask which of `connections` to share. Resolves to the ticked ids, or
   * `null` when the dialog is cancelled (nothing is linked then).
   */
  prompt(projectName: string, connections: readonly LinkCandidate[]): Promise<string[] | null> {
    this.resolve?.(null);
    this.projectName = projectName;
    this.connections = [...connections];
    this.selection = linkDialogSelection(this.connections);
    this.ticked = this.selection.ticked();
    this.isOpen = true;
    return new Promise((resolve) => {
      this.resolve = resolve;
    });
  }

  toggle(id: string): void {
    if (!this.selection) return;
    this.selection.toggle(id);
    this.ticked = this.selection.ticked();
  }

  confirm(): void {
    this.finish(this.ticked);
  }

  cancel(): void {
    this.finish(null);
  }

  private finish(share: string[] | null): void {
    const resolve = this.resolve;
    this.resolve = null;
    this.selection = null;
    this.isOpen = false;
    resolve?.(share);
  }
}

export const linkProjectDialogStore = new LinkProjectDialogStore();
