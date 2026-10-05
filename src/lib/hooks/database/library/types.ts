/**
 * The seam between the library's view models (`ConnectionManager`,
 * `ProjectManager`, `LabelManager`, `SavedQueryManager`) and whatever
 * stores the library (phase 5d-1), like 5c's `EditService`.
 *
 * `CoreLibrary` (every build; the demo's Core runs in the page since phase
 * 8): the `library` RPC group. Core checks the input, assigns ids and
 * times, writes in one transaction (and, on the desktop, the keychain in
 * the same call), and tells the user's other windows and tabs
 * (`storageChanged`).
 *
 * It takes and return the generated wire types: rows are the
 * `seaquel_types::storage` rows (`lastConnected` is text), and every result
 * carries the change `seq` it is at least as new as. A
 * refusal rejects with a `LibraryCallError` (`code`, and `takenBy` for
 * `NAME_TAKEN`). The GUI builds no ids, versions or names: an import's
 * draft sends `renameIfTaken` and Core picks the free name.
 */
import type { ChangeSeq } from "$lib/types/generated/ChangeSeq";
import type { ConnectionDraft } from "$lib/types/generated/ConnectionDraft";
import type { ConnectionLabel } from "$lib/types/generated/ConnectionLabel";
import type { ConnectionPatch } from "$lib/types/generated/ConnectionPatch";
import type { LabelDraft } from "$lib/types/generated/LabelDraft";
import type { LabelPatch } from "$lib/types/generated/LabelPatch";
import type { LabelRemoved } from "$lib/types/generated/LabelRemoved";
import type { PersistedConnection } from "$lib/types/generated/PersistedConnection";
import type { PersistedProject } from "$lib/types/generated/PersistedProject";
import type { PersistedQueryVersion } from "$lib/types/generated/PersistedQueryVersion";
import type { PersistedSavedQuery } from "$lib/types/generated/PersistedSavedQuery";
import type { ProjectDraft } from "$lib/types/generated/ProjectDraft";
import type { ProjectPatch } from "$lib/types/generated/ProjectPatch";
import type { ProjectRemoved } from "$lib/types/generated/ProjectRemoved";
import type { SavedQueryDraft } from "$lib/types/generated/SavedQueryDraft";
import type { SavedQueryPatch } from "$lib/types/generated/SavedQueryPatch";
import type { SavedQueryUpdated } from "$lib/types/generated/SavedQueryUpdated";
import type { SecretChanges } from "$lib/types/generated/SecretChanges";
import type { Seqd } from "$lib/types/generated/Seqd";
import type { StoredKind } from "$lib/types/generated/StoredKind";
import type { CopiedFrom } from "$lib/types/generated/CopiedFrom";
import type { AiProviderCreated } from "$lib/types/generated/AiProviderCreated";
import type { AiProviderDraft } from "$lib/types/generated/AiProviderDraft";
import type { AiProviderPatch } from "$lib/types/generated/AiProviderPatch";
import type { AiSettingsPatch } from "$lib/types/generated/AiSettingsPatch";
import type { ChatDraft } from "$lib/types/generated/ChatDraft";
import type { ChatMessageDraft } from "$lib/types/generated/ChatMessageDraft";
import type { ChatMessages } from "$lib/types/generated/ChatMessages";
import type { ChatPatch } from "$lib/types/generated/ChatPatch";
import type { DashboardDraft } from "$lib/types/generated/DashboardDraft";
import type { DashboardPatch } from "$lib/types/generated/DashboardPatch";
import type { DashboardUpdated } from "$lib/types/generated/DashboardUpdated";
import type { ImportSource } from "$lib/types/generated/ImportSource";
import type { ImportState } from "$lib/types/generated/ImportState";
import type { PersistedAIChat } from "$lib/types/generated/PersistedAIChat";
import type { PersistedDashboard } from "$lib/types/generated/PersistedDashboard";
import type { PersistedDashboardVersion } from "$lib/types/generated/PersistedDashboardVersion";
import type { PersistedDashboardVersionMeta } from "$lib/types/generated/PersistedDashboardVersionMeta";
import type { PersistedWorkflowMeta } from "$lib/types/generated/PersistedWorkflowMeta";
import type { SettingKey } from "$lib/types/generated/SettingKey";
import type { ThemeCreated } from "$lib/types/generated/ThemeCreated";
import type { Themes } from "$lib/types/generated/Themes";
import type { TutorialProgress } from "$lib/types/generated/TutorialProgress";
import type { WindowActive } from "$lib/types/generated/WindowActive";
import type { WindowStateSaved } from "$lib/types/generated/WindowStateSaved";
import type { PersistedProjectState } from "$lib/types/project";

export type {
  ChangeSeq,
  ConnectionDraft,
  ConnectionLabel,
  ConnectionPatch,
  LabelDraft,
  LabelPatch,
  LabelRemoved,
  PersistedConnection as WireConnection,
  PersistedProject as WireProject,
  PersistedQueryVersion as WireQueryVersion,
  PersistedSavedQuery as WireSavedQuery,
  ProjectDraft,
  ProjectPatch,
  ProjectRemoved,
  SavedQueryDraft,
  SavedQueryPatch,
  SavedQueryUpdated,
  SecretChanges,
  Seqd,
  StoredKind,
  CopiedFrom,
  WindowActive,
  WindowStateSaved,
  AiProviderCreated,
  AiProviderDraft,
  AiProviderPatch,
  AiSettingsPatch,
  ChatDraft,
  ChatMessageDraft,
  ChatMessages,
  ChatPatch,
  DashboardDraft,
  DashboardPatch,
  DashboardUpdated,
  ImportState,
  PersistedAIChat as WireChat,
  PersistedDashboard as WireDashboard,
  PersistedDashboardVersion as WireDashboardVersion,
  PersistedDashboardVersionMeta as WireDashboardVersionMeta,
  PersistedWorkflowMeta as WireWorkflowMeta,
  SettingKey,
  ThemeCreated,
  Themes,
  TutorialProgress,
};

/** The import sources whose state is stored (`importStateGet`): Core's own. */
export type { ImportSource };

export interface LibraryService {
  // -------- Reads --------
  listConnections(): Promise<Seqd<PersistedConnection[]>>;
  listProjects(): Promise<Seqd<PersistedProject[]>>;
  listSavedQueries(projectId: string): Promise<Seqd<PersistedSavedQuery[]>>;
  /** Every version of the project's saved queries, oldest first per query. */
  listQueryVersions(projectId: string): Promise<Seqd<PersistedQueryVersion[]>>;

  // -------- Connections --------
  /** `secrets` only on desktop: web keeps its vault in the browser. */
  createConnection(
    draft: ConnectionDraft,
    secrets?: SecretChanges,
  ): Promise<Seqd<PersistedConnection>>;
  updateConnection(
    id: string,
    patch: ConnectionPatch,
    secrets?: SecretChanges,
  ): Promise<Seqd<PersistedConnection>>;
  removeConnection(id: string): Promise<Seqd<null>>;

  // -------- Projects --------
  createProject(draft: ProjectDraft): Promise<Seqd<PersistedProject>>;
  /** Makes the default project on a file with none; lists the projects. */
  ensureDefaultProject(): Promise<Seqd<PersistedProject[]>>;
  updateProject(id: string, patch: ProjectPatch): Promise<Seqd<PersistedProject>>;
  /** Refuses the last project (`LAST_PROJECT`); returns the connections it removed. */
  removeProject(id: string): Promise<Seqd<ProjectRemoved>>;

  // -------- Custom labels --------
  createLabel(projectId: string, label: LabelDraft): Promise<Seqd<ConnectionLabel>>;
  updateLabel(
    projectId: string,
    labelId: string,
    patch: LabelPatch,
  ): Promise<Seqd<ConnectionLabel>>;
  /** Also strips the label from every connection that had it (returned). */
  removeLabel(projectId: string, labelId: string): Promise<Seqd<LabelRemoved>>;

  // -------- Saved queries --------
  createSavedQuery(draft: SavedQueryDraft): Promise<Seqd<PersistedSavedQuery>>;
  /** A changed text appends a keyframe of the previous text and prunes. */
  updateSavedQuery(id: string, patch: SavedQueryPatch): Promise<Seqd<SavedQueryUpdated>>;
  removeSavedQuery(id: string): Promise<Seqd<null>>;

  // -------- Phase 5d-2 --------
  /** A project's connection order (shared by the project's windows). */
  getProjectSidebar(projectId: string): Promise<Seqd<string[]>>;
  /** Replace a project's connection order; answers the order stored. */
  setProjectSidebar(projectId: string, connectionOrder: string[]): Promise<Seqd<string[]>>;
  /**
   * A project's saved workflows without their bodies (id, name, times and size):
   * what the sidebar lists. Read here since the view
   * state no longer carries them.
   */
  listWorkflows(projectId: string): Promise<Seqd<PersistedWorkflowMeta[]>>;
  /**
   * One saved workflow as stored (`toStorable` JSON), for opening or
   * renaming it. A missing one is `WORKFLOW_NOT_FOUND`.
   */
  getWorkflow(id: string): Promise<Seqd<unknown>>;
  /**
   * Save a new workflow: `workflow` is today's stored
   * (`toStorable`) `SavedWorkflow` JSON without `id`, `projectId`,
   * `createdAt` and `updatedAt`, which Core sets. Answers the stored JSON.
   */
  createWorkflow(projectId: string, workflow: unknown): Promise<Seqd<unknown>>;
  /** Replace a saved workflow but its id, project and `createdAt`. */
  updateWorkflow(id: string, workflow: unknown): Promise<Seqd<unknown>>;
  removeWorkflow(id: string): Promise<Seqd<null>>;
  /**
   * Rename a saved workflow: Core changes only its stored name (and
   * `updatedAt`), so another window's save isn't undone. Answers it
   * without its body.
   */
  renameWorkflow(id: string, name: string): Promise<Seqd<PersistedWorkflowMeta>>;

  // -------- Dashboards --------
  listDashboards(projectId: string): Promise<Seqd<PersistedDashboard[]>>;
  /**
   * Every version of the project's dashboards without their snapshots,
   * oldest first per dashboard.
   */
  listDashboardVersions(projectId: string): Promise<Seqd<PersistedDashboardVersionMeta[]>>;
  /**
   * One version with its snapshot, for the diff and restore. A version
   * the dashboard doesn't have is `DASHBOARD_VERSION_NOT_FOUND`.
   */
  getDashboardVersion(
    dashboardId: string,
    versionId: string,
  ): Promise<Seqd<PersistedDashboardVersion>>;
  createDashboard(draft: DashboardDraft): Promise<Seqd<PersistedDashboard>>;
  /**
   * Only the patch's fields; with `captureVersion` a version of the stored
   * dashboard before the change is appended and the versions pruned.
   */
  updateDashboard(id: string, patch: DashboardPatch): Promise<Seqd<DashboardUpdated>>;
  removeDashboard(id: string): Promise<Seqd<null>>;

  // -------- AI chats --------
  /** A connection's chats, most recently updated first. */
  listChats(connectionId: string): Promise<Seqd<PersistedAIChat[]>>;
  /** A chat's messages in `timestamp, rowid` order, and its stored bytes. */
  listChatMessages(chatId: string): Promise<Seqd<ChatMessages>>;
  createChat(draft: ChatDraft): Promise<Seqd<PersistedAIChat>>;
  updateChat(id: string, patch: ChatPatch): Promise<Seqd<PersistedAIChat>>;
  removeChat(id: string): Promise<Seqd<null>>;
  /** Upsert the listed messages by their (GUI-made) ids; answers only those. */
  putChatMessages(chatId: string, messages: ChatMessageDraft[]): Promise<Seqd<ChatMessages>>;
  removeChatMessages(chatId: string, ids: string[]): Promise<Seqd<number>>;
}

/**
 * The `settings` group (phase 5d-2): the app-state settings
 * (a closed set of keys), the AI settings record and its providers (with
 * their API keys on the desktop), themes, onboarding, tutorial progress
 * and import state. Every write answers the record it wrote with its
 * `seq`; a record Core keeps as JSON (`aiSettings`, onboarding) crosses as
 * a parsed value.
 *
 * `CoreSettings` (every build): the `settings` RPC group, writes on the
 * storage client's write queue.
 */
export interface SettingsService {
  /** A setting's stored value; `null` for none. */
  getSetting(key: SettingKey): Promise<Seqd<string | null>>;
  /** Checked for the key; `null` deletes it. */
  setSetting(key: SettingKey, value: string | null): Promise<Seqd<string | null>>;

  /** The AI settings record, legacy fields cleaned and defaults filled. */
  getAiSettings(): Promise<Seqd<unknown>>;
  patchAiSettings(patch: AiSettingsPatch): Promise<Seqd<unknown>>;
  /** `apiKey` only on the desktop, where Core writes the keychain in the call. */
  createAiProvider(draft: AiProviderDraft, apiKey?: string): Promise<Seqd<AiProviderCreated>>;
  /** `apiKey` (desktop): left out keeps it, `null` deletes it, a string sets it. */
  updateAiProvider(
    id: string,
    patch: AiProviderPatch,
    apiKey?: string | null,
  ): Promise<Seqd<unknown>>;
  /** Also deletes its API key (the desktop keychain, or the web vault's rows). */
  removeAiProvider(id: string): Promise<Seqd<unknown>>;
  /**
   * Desktop: whether the keychain holds the provider's key (one read; the
   * page never reads the key). Web: `NOT_SUPPORTED` (the vault knows).
   */
  aiProviderHasKey(id: string): Promise<Seqd<boolean>>;

  getThemes(): Promise<Seqd<Themes>>;
  setThemePreferences(lightThemeId: string, darkThemeId: string): Promise<Seqd<Themes>>;
  /** `theme` without `id`, `createdAt` and `updatedAt`, which Core sets. */
  createUserTheme(theme: unknown): Promise<Seqd<ThemeCreated>>;
  updateUserTheme(id: string, theme: unknown): Promise<Seqd<Themes>>;
  /** A preference that named it goes back to its default in the same call. */
  removeUserTheme(id: string): Promise<Seqd<Themes>>;

  /** The onboarding record: the six defaults with the stored fields over them. */
  getOnboarding(): Promise<Seqd<unknown>>;
  /** Merges the named top-level fields into the stored record. */
  patchOnboarding(patch: Record<string, unknown>): Promise<Seqd<unknown>>;

  listTutorial(): Promise<Seqd<TutorialProgress[]>>;
  saveTutorial(
    lessonId: string,
    challengeId: string,
    state: string | null,
  ): Promise<Seqd<TutorialProgress[]>>;
  removeTutorialLesson(lessonId: string): Promise<Seqd<TutorialProgress[]>>;
  resetTutorial(): Promise<Seqd<TutorialProgress[]>>;

  getImportState(source: ImportSource): Promise<Seqd<ImportState | null>>;
  saveImportState(
    source: ImportSource,
    hasOfferedImport: boolean,
    lastCheckTimestamp: string | null,
  ): Promise<Seqd<ImportState>>;
}

/**
 * One window's view of a project: its open tabs with their
 * text, pane layout and active ids. Today's `PersistedProjectState` minus
 * what isn't the window's: the saved workflows, the
 * connection order (shared, `setProjectSidebar`) and the legacy starred
 * lists. Core stores it as sent.
 */
export type ViewState = Omit<
  PersistedProjectState,
  "savedWorkflows" | "connectionOrder" | "starredSharedQueryIds" | "starredSharedDashboardIds"
>;

/** What `windowStateLoad` answers, with the state typed. */
export interface ViewStateLoaded {
  /** `null` when there was nothing to copy (`copiedFrom: "empty"`). */
  state: ViewState | null;
  /** The stored `rev`: the page's next save sends one more. */
  rev: number;
  /** Where a first load took the state from; `null` for the window's own row. */
  copiedFrom: CopiedFrom | null;
}

/**
 * The `ui` group (phase 5d-2): one window's view state per
 * project, and its active project. `windowId` must be the caller's origin
 * (its window id); Core refuses any other (`INVALID_ARGUMENT`).
 *
 * `CoreUi` (every build): the `ui` RPC group, writes on the storage
 * client's write queue.
 */
export interface UiService {
  /** The window's active project, else the most recent window's, else `lastActiveProjectId`. */
  windowGet(windowId: string): Promise<Seqd<WindowActive>>;
  /** Sets the window's active project (and `lastActiveProjectId`). */
  windowActivate(windowId: string, projectId: string): Promise<Seqd<null>>;
  /**
   * The window's view state of a project. A window with none gets a copy of
   * the project's most recently used window's, else today's rows, else
   * nothing; a copy is stored as the window's own at once.
   */
  windowStateLoad(windowId: string, projectId: string): Promise<Seqd<ViewStateLoaded>>;
  /** Stored only when `rev` is higher than the stored one; else `stale` with the stored `rev`. */
  windowStateSave(
    windowId: string,
    projectId: string,
    rev: number,
    state: ViewState,
  ): Promise<Seqd<WindowStateSaved>>;
  /**
   * The same save as one `keepalive` request outside the write queue, for a
   * page that is going away (web). False when nothing was sent.
   */
  windowStateSaveKeepalive(
    windowId: string,
    projectId: string,
    rev: number,
    state: ViewState,
  ): boolean;
}

// -------- Error codes the GUI words --------

/** Another row of the scope has the name (after trimming and case folding). */
export const NAME_TAKEN = "NAME_TAKEN";
/** The last project can't be removed. */
export const LAST_PROJECT = "LAST_PROJECT";
export const PROJECT_NOT_FOUND = "PROJECT_NOT_FOUND";
export const SAVED_QUERY_NOT_FOUND = "SAVED_QUERY_NOT_FOUND";
export const LABEL_NOT_FOUND = "LABEL_NOT_FOUND";
/** A saved connection that isn't there (the wire code of `SAVED_CONNECTION_NOT_FOUND`). */
export const CONNECTION_NOT_FOUND = "CONNECTION_NOT_FOUND";
export const INVALID_ARGUMENT = "INVALID_ARGUMENT";
export const DASHBOARD_NOT_FOUND = "DASHBOARD_NOT_FOUND";
/** `dashboardVersionGet` naming a version its dashboard doesn't have. */
export const DASHBOARD_VERSION_NOT_FOUND = "DASHBOARD_VERSION_NOT_FOUND";
export const WORKFLOW_NOT_FOUND = "WORKFLOW_NOT_FOUND";
export const CHAT_NOT_FOUND = "CHAT_NOT_FOUND";
export const THEME_NOT_FOUND = "THEME_NOT_FOUND";
export const AI_PROVIDER_NOT_FOUND = "AI_PROVIDER_NOT_FOUND";
/** The user's metadata file reached its size cap (web); nothing of the call was written. */
export const STORAGE_FULL = "STORAGE_FULL";

/**
 * A refused or failed library call: `"CODE: message"`, with the code, and
 * for `NAME_TAKEN` the id of the row that has the name.
 */
export class LibraryCallError extends Error {
  readonly code: string;
  readonly takenBy?: string;
  constructor(code: string, message: string, takenBy?: string) {
    super(`${code}: ${message}`);
    this.name = "LibraryCallError";
    this.code = code;
    if (takenBy !== undefined) this.takenBy = takenBy;
  }
}

/** The per-row key the change feed and the `seq` rule use. */
export type RowKind =
  | "connection"
  | "project"
  | "projectSidebar"
  | "savedQuery"
  | "queryVersion"
  | "dashboard"
  | "dashboardVersion"
  | "workflow"
  | "chat"
  | "chatMessages"
  /** One stored chat message, applied from a turn's `done`/`error` (phase 6). */
  | "aiMessage"
  | "setting"
  | "aiSettings"
  | "theme"
  | "onboarding"
  | "tutorial"
  | "importState";
export function rowKey(kind: RowKind, id: string): string {
  return `${kind}:${id}`;
}

/** For `NAME_TAKEN`: the id of the row that has the name, if the error says. */
export function takenByOf(error: unknown): string | undefined {
  if (typeof error === "object" && error !== null && "takenBy" in error) {
    const { takenBy } = error as { takenBy?: unknown };
    if (typeof takenBy === "string") return takenBy;
  }
  return undefined;
}
