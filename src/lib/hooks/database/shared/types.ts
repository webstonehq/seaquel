/**
 * The seams between the GUI and Core's shared projection and imports
 * (phase 5e), like the library's `LibraryService`.
 *
 * - `SharedService`: the `shared` RPC group. Core owns the `.seaquel` tree:
 *   linking, unlinking and importing projects, the repo list, and the sync
 *   that reconciles a project's files with its rows in both directions. The
 *   page never builds a path, reads or writes a file, or pairs a file with a
 *   row; it shows what Core reports.
 * - `ImportsService`: the `imports` group. Core finds and reads TablePlus's
 *   and DBeaver's files, maps them to candidates, and imports the chosen
 *   ones in one transaction.
 *
 * Both are desktop only (`LocalFiles`): `CoreShared`/`CoreImports` there,
 * `NoShared`/`NoImports` on web and in the demo, which answer
 * `NOT_SUPPORTED` (the GUI hides the entry points there).
 */
import type { ImportCandidate } from "$lib/types/generated/ImportCandidate";
import type { ImportCandidates } from "$lib/types/generated/ImportCandidates";
import type { ImportedProjects } from "$lib/types/generated/ImportedProjects";
import type { ImportKeyOutcome } from "$lib/types/generated/ImportKeyOutcome";
import type { ImportOutcome } from "$lib/types/generated/ImportOutcome";
import type { ImportProblem } from "$lib/types/generated/ImportProblem";
import type { ImportSource } from "$lib/types/generated/ImportSource";
import type { PersistedSharedQueryRepo } from "$lib/types/generated/PersistedSharedQueryRepo";
import type { PreviewProject } from "$lib/types/generated/PreviewProject";
import type { PreviewTemplate } from "$lib/types/generated/PreviewTemplate";
import type { ProjectFailure } from "$lib/types/generated/ProjectFailure";
import type { ReplacedValues } from "$lib/types/generated/ReplacedValues";
import type { RepoPatch } from "$lib/types/generated/RepoPatch";
import type { RepoPreview } from "$lib/types/generated/RepoPreview";
import type { Seqd } from "$lib/types/generated/Seqd";
import type { SharedKind } from "$lib/types/generated/SharedKind";
import type { SkipReason } from "$lib/types/generated/SkipReason";
import type { SyncNotice } from "$lib/types/generated/SyncNotice";
import type { SyncReport } from "$lib/types/generated/SyncReport";
import type { UnlinkPreview } from "$lib/types/generated/UnlinkPreview";
import type { UnlinkReport } from "$lib/types/generated/UnlinkReport";

export type {
  ImportCandidate,
  ImportCandidates,
  ImportedProjects,
  ImportKeyOutcome,
  ImportOutcome,
  ImportProblem,
  ImportSource,
  PersistedSharedQueryRepo,
  PreviewProject,
  PreviewTemplate,
  ProjectFailure,
  ReplacedValues,
  RepoPatch,
  RepoPreview,
  Seqd,
  SharedKind,
  SkipReason,
  SyncNotice,
  SyncReport,
  UnlinkPreview,
  UnlinkReport,
};

/** What a library write did to its row's file, when anything. */
export type ProjectionOutcome = NonNullable<Seqd<unknown>["projection"]>;

/** Which projects a sync covers: one project, or every project linked to a repo. */
export type SyncTarget = { projectId: string } | { repoId: string };

/** Core's codes the shared GUI words itself. */
export const FILE_CHANGED = "FILE_CHANGED";
export const FILE_ERROR = "FILE_ERROR";
export const REPO_IN_USE = "REPO_IN_USE";
export const REPO_CONFLICTED = "REPO_CONFLICTED";
export const PROJECT_ALREADY_LINKED = "PROJECT_ALREADY_LINKED";
export const PROJECT_NOT_LINKED = "PROJECT_NOT_LINKED";
export const REPO_NOT_FOUND = "REPO_NOT_FOUND";
export const NOT_SUPPORTED = "NOT_SUPPORTED";

export interface SharedService {
  /** Every stored repo, as stored. */
  listRepos(): Promise<Seqd<PersistedSharedQueryRepo[]>>;
  /** Registers the repo at `path`, or answers the one already there. */
  registerRepo(
    path: string,
    init?: { name?: string; remoteUrl?: string },
  ): Promise<Seqd<PersistedSharedQueryRepo>>;
  /** Changes only the fields `patch` names; the rest stays byte for byte. */
  updateRepo(id: string, patch: RepoPatch): Promise<Seqd<PersistedSharedQueryRepo>>;
  /** `REPO_IN_USE` while a project links to it. */
  removeRepo(id: string): Promise<Seqd<null>>;
  /**
   * Links a project to the repo at `path` and syncs it. `share`: the link
   * dialog's ticked connections, exported as templates.
   */
  linkProject(projectId: string, path: string, share: string[]): Promise<Seqd<SyncReport>>;
  /**
   * Unlinks a project: the user's own connections stay, unlinked and
   * local-only; the ones the repo brought go when `removeImported`.
   */
  unlinkProject(projectId: string, removeImported: boolean): Promise<Seqd<UnlinkReport>>;
  /** The connections an unlink with `removeImported` would remove. Reads only. */
  unlinkPreview(projectId: string): Promise<UnlinkPreview>;
  /** What the repo at `path` holds, for the import dialog. Reads only. */
  scan(path: string): Promise<RepoPreview>;
  /** One project per directory, each linked and synced. */
  importProjects(path: string, dirs: string[]): Promise<Seqd<ImportedProjects>>;
  sync(target: SyncTarget): Promise<Seqd<SyncReport>>;
}

export interface ImportsService {
  /** What the other tool's file holds, marked against `projectId`'s connections. */
  candidates(source: ImportSource, projectId: string): Promise<ImportCandidates>;
  /** Imports the candidates `keys` names into `projectId`, read from the file again. */
  create(source: ImportSource, projectId: string, keys: string[]): Promise<Seqd<ImportOutcome>>;
}
