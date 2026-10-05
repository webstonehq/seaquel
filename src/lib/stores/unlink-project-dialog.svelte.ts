/**
 * The unlink dialog: which connections the repo brought, and whether
 * to remove them. The user's own connections always stay; cancelling
 * changes nothing.
 */

/** A connection as the dialog lists it. */
export interface UnlinkListed {
  id: string;
  name: string;
}

class UnlinkProjectDialogStore {
  isOpen = $state(false);
  projectName = $state("");
  imported = $state<UnlinkListed[]>([]);
  private resolve: ((choice: "remove" | "keep" | null) => void) | null = null;

  /** Ask about `imported`. Resolves to the choice, or `null` when cancelled. */
  prompt(
    projectName: string,
    imported: readonly UnlinkListed[],
  ): Promise<"remove" | "keep" | null> {
    this.resolve?.(null);
    this.projectName = projectName;
    this.imported = imported.map((c) => ({ id: c.id, name: c.name }));
    this.isOpen = true;
    return new Promise((resolve) => {
      this.resolve = resolve;
    });
  }

  answer(choice: "remove" | "keep" | null): void {
    const resolve = this.resolve;
    this.resolve = null;
    this.isOpen = false;
    resolve?.(choice);
  }
}

export const unlinkProjectDialogStore = new UnlinkProjectDialogStore();
