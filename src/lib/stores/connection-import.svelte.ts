/**
 * The TablePlus and DBeaver import dialogs' state (phase 5e, Decision 47).
 * Core finds and reads the other tool's file and answers candidates marked
 * against the project's connections: `duplicateOf` for one the project
 * already has, `problem` for one that can't be imported. Nothing found and
 * a file that can't be read are said, never a silent no-op (bug 22).
 */
import { toast } from "svelte-sonner";
import { m } from "$lib/paraglide/messages.js";
import { errorToast } from "$lib/utils/toast";
import { extractErrorMessage } from "$lib/errors";
import { log } from "$lib/utils/logger";
import { errorCode } from "$lib/core/client";
import { getSettings } from "$lib/hooks/database/library/index";
import {
  getImports,
  type ImportCandidate,
  type ImportProblem,
  type ImportSource,
} from "$lib/hooks/database/shared/index";
import { onStoredChange, toastIfStorageFull } from "./settings-sync";

/** A candidate as the dialog lists it, with its tick. */
export type ListedCandidate = ImportCandidate & { selected: boolean };

const TOOL: Record<ImportSource, string> = { tableplus: "TablePlus", dbeaver: "DBeaver" };

export class ConnectionImportStore {
  isOpen = $state(false);
  isLoading = $state(false);
  candidates = $state<ListedCandidate[]>([]);
  /** The project the candidates were read against, and are imported into. */
  projectId = $state<string | null>(null);
  hasOfferedImport = $state(false);
  private initialized = false;

  constructor(readonly source: ImportSource) {
    // Another window offered the import: don't offer it here again.
    onStoredChange("importState", (ids) =>
      this.initialized && (ids === null || ids.includes(source)) ? this.read() : undefined,
    );
  }

  /** The other tool's name, as the dialog says it. */
  get tool(): string {
    return TOOL[this.source];
  }

  private async read(): Promise<void> {
    const { value } = await getSettings().getImportState(this.source);
    if (value) this.hasOfferedImport = value.hasOfferedImport;
  }

  /** Loads whether the import was offered before. */
  async initialize(): Promise<void> {
    if (this.initialized) return;
    this.initialized = true;
    try {
      await this.read();
    } catch (error) {
      void log.warn(`Failed to load the ${this.source} import state:`, error);
    }
  }

  /**
   * Read the other tool's connections against `projectId`'s and open the
   * dialog, or say that none were found or the file couldn't be read.
   */
  async checkAndShowDialog(projectId: string): Promise<void> {
    this.isLoading = true;
    try {
      const answer = await getImports().candidates(this.source, projectId);
      if (!answer.found) {
        toast.info(m.import_nothing_found({ tool: this.tool }));
        return;
      }
      if (answer.unreadable !== undefined) {
        errorToast(m.import_unreadable({ tool: this.tool, message: answer.unreadable }));
        return;
      }
      const candidates = answer.candidates ?? [];
      if (candidates.length === 0) {
        toast.info(m.import_nothing_found({ tool: this.tool }));
        return;
      }
      this.projectId = projectId;
      this.candidates = candidates.map((c) => ({ ...c, selected: selectable(c) }));
      this.isOpen = true;
    } catch (error) {
      void log.warn(`Reading the ${this.source} connections failed (${errorCode(error) ?? "?"})`);
      errorToast(m.import_unreadable({ tool: this.tool, message: extractErrorMessage(error) }));
    } finally {
      this.isLoading = false;
    }
  }

  /** Whether candidate `index` can be ticked (no duplicate, no problem). */
  canSelect(index: number): boolean {
    const c = this.candidates[index];
    return !!c && selectable(c);
  }

  toggleConnection(index: number): void {
    if (!this.canSelect(index)) return;
    this.candidates = this.candidates.map((c, i) =>
      i === index ? { ...c, selected: !c.selected } : c,
    );
  }

  selectAll(): void {
    this.candidates = this.candidates.map((c) => ({ ...c, selected: selectable(c) }));
  }

  deselectAll(): void {
    this.candidates = this.candidates.map((c) => ({ ...c, selected: false }));
  }

  /** The ticked candidates, as `importConnections` takes them. */
  selected(): { key: string; name: string }[] {
    return this.candidates.filter((c) => c.selected).map((c) => ({ key: c.key, name: c.name }));
  }

  /** Why candidate `c` can't be imported, or `null` when it can. */
  problemText(c: ImportCandidate): string | null {
    return c.problem ? problemText(c.problem) : null;
  }

  /** Close the dialog without importing; it isn't offered again. */
  async dismiss(): Promise<void> {
    this.isOpen = false;
    this.hasOfferedImport = true;
    await this.persist();
  }

  /** Close the dialog after an import. */
  async completeImport(): Promise<void> {
    this.isOpen = false;
    this.hasOfferedImport = true;
    await this.persist();
  }

  private async persist(): Promise<void> {
    try {
      await getSettings().saveImportState(
        this.source,
        this.hasOfferedImport,
        new Date().toISOString(),
      );
    } catch (error) {
      toastIfStorageFull(error);
      void log.warn(`Failed to persist the ${this.source} import state:`, error);
    }
  }
}

function selectable(c: ImportCandidate): boolean {
  return !c.duplicateOf && !c.problem;
}

function problemText(problem: ImportProblem): string {
  switch (problem) {
    case "invalidPort":
      return m.import_problem_invalid_port();
    case "invalidSshPort":
      return m.import_problem_invalid_ssh_port();
    case "noId":
      return m.import_problem_no_id();
    case "duplicateId":
      return m.import_problem_duplicate_id();
    case "noName":
      return m.import_problem_no_name();
  }
}

export const tablePlusImportStore = new ConnectionImportStore("tableplus");
export const dbeaverImportStore = new ConnectionImportStore("dbeaver");
