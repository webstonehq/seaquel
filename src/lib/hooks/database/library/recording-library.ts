/**
 * A `LibraryService` for tests: rows in memory, every call recorded as
 * sent, failures injectable per method, and a `seq` the test can move. It
 * keeps only the rules the GUI's tests lean on (ids, patches, `lastConnected`
 * on `connected`, label stripping, keyframe versions); Core holds the full
 * rules (the replays run them through the browser module).
 */
import type {
  ChatDraft,
  ChatMessageDraft,
  ChatMessages,
  ChatPatch,
  DashboardDraft,
  DashboardPatch,
  DashboardUpdated,
  WireChat,
  WireDashboard,
  WireDashboardVersion,
  WireDashboardVersionMeta,
  WireWorkflowMeta,
  ChangeSeq,
  ConnectionDraft,
  ConnectionLabel,
  ConnectionPatch,
  LabelDraft,
  LabelPatch,
  LibraryService,
  ProjectDraft,
  ProjectPatch,
  SavedQueryDraft,
  SavedQueryPatch,
  SecretChanges,
  Seqd,
  WireConnection,
  WireProject,
  WireQueryVersion,
  WireSavedQuery,
} from "./types";
import { LibraryCallError } from "./types";

/** A version as Core lists it: no snapshot (5d-2 Task 7). */
function versionMeta(v: WireDashboardVersion): WireDashboardVersionMeta {
  let widgetCount: number | null = null;
  try {
    const widgets = (JSON.parse(v.snapshot) as { widgets?: unknown } | null)?.widgets;
    if (Array.isArray(widgets)) widgetCount = widgets.length;
  } catch {
    // Not JSON: no count.
  }
  return {
    id: v.id,
    dashboardId: v.dashboardId,
    version: v.version,
    createdAt: v.createdAt,
    widgetCount,
    bytes: v.snapshot.length,
  };
}

type Method = keyof LibraryService;

export interface RecordedCall {
  method: Method;
  args: unknown[];
}

const NOW = "2030-01-01T00:00:00.000Z";

export class RecordingLibrary implements LibraryService {
  readonly calls: RecordedCall[] = [];
  readonly connections = new Map<string, WireConnection>();
  readonly projects = new Map<string, WireProject>();
  readonly savedQueries = new Map<string, WireSavedQuery>();
  readonly versions: WireQueryVersion[] = [];
  /** A method listed here rejects with its error (once, unless `sticky`). */
  readonly failures = new Map<Method, { error: Error; sticky?: boolean }>();
  epoch = "epoch-1";
  n = 0;
  private ids = 0;

  /** A call recorded as sent; `failures` answers it. */
  private record(method: Method, args: unknown[]): void {
    this.calls.push({ method, args: structuredClone(args) });
    const failure = this.failures.get(method);
    if (failure) {
      if (!failure.sticky) this.failures.delete(method);
      throw failure.error;
    }
  }

  /** The calls of `method`, their arguments. */
  callsOf(method: Method): unknown[][] {
    return this.calls.filter((c) => c.method === method).map((c) => c.args);
  }

  seq(): ChangeSeq {
    return { epoch: this.epoch, n: this.n };
  }

  private write(): ChangeSeq {
    this.n += 1;
    return this.seq();
  }

  private id(prefix: string): string {
    return `${prefix}${++this.ids}`;
  }

  // -------- Seeding (no call recorded) --------

  seedProject(id: string, name = id, extra: Partial<WireProject> = {}): WireProject {
    const row: WireProject = {
      id,
      name,
      createdAt: NOW,
      updatedAt: NOW,
      customLabels: [],
      ...extra,
    };
    this.projects.set(id, row);
    return row;
  }

  seedConnection(id: string, extra: Partial<WireConnection> = {}): WireConnection {
    const row: WireConnection = {
      id,
      projectId: "p1",
      name: id,
      type: "postgres",
      host: "localhost",
      port: 5432,
      databaseName: "app",
      username: "me",
      savePassword: false,
      saveSshPassword: false,
      saveSshKeyPassphrase: false,
      labelIds: [],
      ...extra,
    };
    this.connections.set(id, row);
    return row;
  }

  seedSavedQuery(id: string, extra: Partial<WireSavedQuery> = {}): WireSavedQuery {
    const row: WireSavedQuery = {
      id,
      name: id,
      query: "SELECT 1",
      projectId: "p1",
      createdAt: NOW,
      updatedAt: NOW,
      starred: false,
      shared: false,
      ...extra,
    };
    this.savedQueries.set(id, row);
    return row;
  }

  // -------- Reads --------

  async listConnections(): Promise<Seqd<WireConnection[]>> {
    this.record("listConnections", []);
    return { value: [...this.connections.values()], seq: this.seq() };
  }

  async listProjects(): Promise<Seqd<WireProject[]>> {
    this.record("listProjects", []);
    return { value: [...this.projects.values()], seq: this.seq() };
  }

  async listSavedQueries(projectId: string): Promise<Seqd<WireSavedQuery[]>> {
    this.record("listSavedQueries", [projectId]);
    const value = [...this.savedQueries.values()].filter((q) => q.projectId === projectId);
    return { value, seq: this.seq() };
  }

  async listQueryVersions(projectId: string): Promise<Seqd<WireQueryVersion[]>> {
    this.record("listQueryVersions", [projectId]);
    const value = this.versions.filter(
      (v) => this.savedQueries.get(v.queryId)?.projectId === projectId,
    );
    return { value, seq: this.seq() };
  }

  // -------- Connections --------

  async createConnection(
    draft: ConnectionDraft,
    secrets?: SecretChanges,
  ): Promise<Seqd<WireConnection>> {
    this.record("createConnection", secrets === undefined ? [draft] : [draft, secrets]);
    const { connected, renameIfTaken: _rename, ...fields } = draft;
    const row: WireConnection = {
      ...fields,
      id: this.id("conn-"),
      savePassword: !!draft.savePassword,
      saveSshPassword: !!draft.saveSshPassword,
      saveSshKeyPassphrase: !!draft.saveSshKeyPassphrase,
      labelIds: draft.labelIds ?? [],
      ...(connected ? { lastConnected: NOW } : {}),
    };
    // Stored rows read local-only as `true` or absent.
    if (!row.isLocalOnly) delete row.isLocalOnly;
    this.connections.set(row.id, row);
    return { value: row, seq: this.write() };
  }

  async updateConnection(
    id: string,
    patch: ConnectionPatch,
    secrets?: SecretChanges,
  ): Promise<Seqd<WireConnection>> {
    this.record("updateConnection", secrets === undefined ? [id, patch] : [id, patch, secrets]);
    const before = this.connections.get(id);
    if (!before) throw new LibraryCallError("CONNECTION_NOT_FOUND", "Saved connection not found.");
    const { connected, ...fields } = patch;
    const row = { ...before } as Record<string, unknown>;
    for (const [key, value] of Object.entries(fields)) {
      if (value === null) delete row[key];
      else if (value !== undefined) row[key] = value;
    }
    if (connected) row.lastConnected = NOW;
    this.connections.set(id, row as WireConnection);
    return { value: row as WireConnection, seq: this.write() };
  }

  async removeConnection(id: string): Promise<Seqd<null>> {
    this.record("removeConnection", [id]);
    if (!this.connections.delete(id)) {
      throw new LibraryCallError("CONNECTION_NOT_FOUND", "Saved connection not found.");
    }
    return { value: null, seq: this.write() };
  }

  // -------- Projects --------

  async createProject(draft: ProjectDraft): Promise<Seqd<WireProject>> {
    this.record("createProject", [draft]);
    const row = this.seedProject(
      this.id("project-"),
      draft.name,
      draft.description ? { description: draft.description } : {},
    );
    return { value: row, seq: this.write() };
  }

  async ensureDefaultProject(): Promise<Seqd<WireProject[]>> {
    this.record("ensureDefaultProject", []);
    if (this.projects.size === 0) {
      this.seedProject("default-seaquel", "Seaquel");
      this.write();
    }
    return { value: [...this.projects.values()], seq: this.seq() };
  }

  async updateProject(id: string, patch: ProjectPatch): Promise<Seqd<WireProject>> {
    this.record("updateProject", [id, patch]);
    const before = this.projects.get(id);
    if (!before) throw new LibraryCallError("PROJECT_NOT_FOUND", "Project not found.");
    const row = { ...before } as Record<string, unknown>;
    for (const [key, value] of Object.entries(patch)) {
      if (value === null) delete row[key];
      else if (value !== undefined) row[key] = value;
    }
    this.projects.set(id, row as WireProject);
    return { value: row as WireProject, seq: this.write() };
  }

  async removeProject(id: string): Promise<Seqd<{ connectionIds: string[] }>> {
    this.record("removeProject", [id]);
    if (!this.projects.has(id))
      throw new LibraryCallError("PROJECT_NOT_FOUND", "Project not found.");
    if (this.projects.size <= 1) {
      throw new LibraryCallError("LAST_PROJECT", "The last project can't be removed.");
    }
    this.projects.delete(id);
    const connectionIds = [...this.connections.values()]
      .filter((c) => c.projectId === id)
      .map((c) => c.id);
    for (const c of connectionIds) this.connections.delete(c);
    for (const q of this.savedQueries.values()) {
      if (q.projectId === id) this.savedQueries.delete(q.id);
    }
    return { value: { connectionIds }, seq: this.write() };
  }

  // -------- Labels --------

  async createLabel(projectId: string, label: LabelDraft): Promise<Seqd<ConnectionLabel>> {
    this.record("createLabel", [projectId, label]);
    const project = this.projects.get(projectId);
    if (!project) throw new LibraryCallError("PROJECT_NOT_FOUND", "Project not found.");
    const row: ConnectionLabel = { id: this.id("label-"), isPredefined: false, ...label };
    project.customLabels = [...project.customLabels, row];
    return { value: row, seq: this.write() };
  }

  async updateLabel(
    projectId: string,
    labelId: string,
    patch: LabelPatch,
  ): Promise<Seqd<ConnectionLabel>> {
    this.record("updateLabel", [projectId, labelId, patch]);
    const project = this.projects.get(projectId);
    const label = project?.customLabels.find((l) => l.id === labelId);
    if (!project || !label) throw new LibraryCallError("LABEL_NOT_FOUND", "Label not found.");
    const row = { ...label, ...patch };
    project.customLabels = project.customLabels.map((l) => (l.id === labelId ? row : l));
    return { value: row, seq: this.write() };
  }

  async removeLabel(
    projectId: string,
    labelId: string,
  ): Promise<Seqd<{ connectionIds: string[] }>> {
    this.record("removeLabel", [projectId, labelId]);
    const project = this.projects.get(projectId);
    if (!project?.customLabels.some((l) => l.id === labelId)) {
      throw new LibraryCallError("LABEL_NOT_FOUND", "Label not found.");
    }
    project.customLabels = project.customLabels.filter((l) => l.id !== labelId);
    const connectionIds: string[] = [];
    for (const c of this.connections.values()) {
      if (c.labelIds.includes(labelId)) {
        c.labelIds = c.labelIds.filter((id) => id !== labelId);
        connectionIds.push(c.id);
      }
    }
    return { value: { connectionIds }, seq: this.write() };
  }

  // -------- Saved queries --------

  async createSavedQuery(draft: SavedQueryDraft): Promise<Seqd<WireSavedQuery>> {
    this.record("createSavedQuery", [draft]);
    const row = this.seedSavedQuery(this.id("saved-"), {
      ...draft,
      starred: !!draft.starred,
      shared: !!draft.shared,
    });
    return { value: row, seq: this.write() };
  }

  async updateSavedQuery(
    id: string,
    patch: SavedQueryPatch,
  ): Promise<
    Seqd<{ query: WireSavedQuery; version: WireQueryVersion | null; prunedVersionIds: string[] }>
  > {
    this.record("updateSavedQuery", [id, patch]);
    const before = this.savedQueries.get(id);
    if (!before) throw new LibraryCallError("SAVED_QUERY_NOT_FOUND", "Saved query not found.");
    const row = { ...before } as Record<string, unknown>;
    for (const [key, value] of Object.entries(patch)) {
      if (value === null) delete row[key];
      else if (value !== undefined) row[key] = value;
    }
    this.savedQueries.set(id, row as WireSavedQuery);
    let version: WireQueryVersion | null = null;
    if (patch.query !== undefined && patch.query !== before.query) {
      const n = Math.max(0, ...this.versions.filter((v) => v.queryId === id).map((v) => v.version));
      version = {
        id: this.id("ver-"),
        queryId: id,
        version: n + 1,
        snapshot: before.query,
        diff: null,
        createdAt: NOW,
      };
      this.versions.push(version);
    }
    return {
      value: { query: row as WireSavedQuery, version, prunedVersionIds: [] },
      seq: this.write(),
    };
  }

  async removeSavedQuery(id: string): Promise<Seqd<null>> {
    this.record("removeSavedQuery", [id]);
    if (!this.savedQueries.delete(id)) {
      throw new LibraryCallError("SAVED_QUERY_NOT_FOUND", "Saved query not found.");
    }
    return { value: null, seq: this.write() };
  }

  // -------- Phase 5d-2 --------

  /** Each project's connection order. */
  readonly sidebars = new Map<string, string[]>();
  /** Each project's saved workflows, as stored. */
  readonly workflows = new Map<string, unknown[]>();

  async getProjectSidebar(projectId: string): Promise<Seqd<string[]>> {
    this.record("getProjectSidebar", [projectId]);
    return { value: [...(this.sidebars.get(projectId) ?? [])], seq: this.seq() };
  }

  async setProjectSidebar(projectId: string, connectionOrder: string[]): Promise<Seqd<string[]>> {
    this.record("setProjectSidebar", [projectId, connectionOrder]);
    if (!this.projects.has(projectId)) {
      throw new LibraryCallError("PROJECT_NOT_FOUND", "Project not found.");
    }
    this.sidebars.set(projectId, [...connectionOrder]);
    return { value: [...connectionOrder], seq: this.write() };
  }

  /** The project's workflows without their bodies, as Core lists them (5d-2 Task 7). */
  async listWorkflows(projectId: string): Promise<Seqd<WireWorkflowMeta[]>> {
    this.record("listWorkflows", [projectId]);
    const value = (this.workflows.get(projectId) ?? []).map((w) => {
      const o = w as Record<string, unknown>;
      const str = (v: unknown) => (typeof v === "string" ? v : null);
      return {
        id: String(o.id),
        projectId,
        name: str(o.name) ?? "",
        createdAt: str(o.createdAt),
        updatedAt: str(o.updatedAt),
        bytes: JSON.stringify(w).length,
      };
    });
    return { value, seq: this.seq() };
  }

  async getWorkflow(id: string): Promise<Seqd<unknown>> {
    this.record("getWorkflow", [id]);
    const at = this.workflowIndex(id);
    if (!at) throw new LibraryCallError("WORKFLOW_NOT_FOUND", "Saved workflow not found.");
    const [projectId, i] = at;
    return { value: (this.workflows.get(projectId) ?? [])[i], seq: this.seq() };
  }

  private workflowIndex(id: string): [string, number] | null {
    for (const [projectId, list] of this.workflows) {
      const i = list.findIndex((w) => (w as { id?: string }).id === id);
      if (i !== -1) return [projectId, i];
    }
    return null;
  }

  async createWorkflow(projectId: string, workflow: unknown): Promise<Seqd<unknown>> {
    this.record("createWorkflow", [projectId, workflow]);
    const stored = {
      id: this.id("workflow-"),
      ...(workflow as object),
      projectId,
      createdAt: NOW,
      updatedAt: NOW,
    };
    this.workflows.set(projectId, [...(this.workflows.get(projectId) ?? []), stored]);
    return { value: stored, seq: this.write() };
  }

  async updateWorkflow(id: string, workflow: unknown): Promise<Seqd<unknown>> {
    this.record("updateWorkflow", [id, workflow]);
    const at = this.workflowIndex(id);
    if (!at) throw new LibraryCallError("WORKFLOW_NOT_FOUND", "Saved workflow not found.");
    const [projectId, i] = at;
    const list = [...(this.workflows.get(projectId) ?? [])];
    const old = list[i] as { createdAt?: string };
    const stored = {
      id,
      ...(workflow as object),
      projectId,
      createdAt: old.createdAt,
      updatedAt: NOW,
    };
    list[i] = stored;
    this.workflows.set(projectId, list);
    return { value: stored, seq: this.write() };
  }

  /** Only the stored name and `updatedAt` change, as Core renames. */
  async renameWorkflow(id: string, name: string): Promise<Seqd<WireWorkflowMeta>> {
    this.record("renameWorkflow", [id, name]);
    const at = this.workflowIndex(id);
    if (!at) throw new LibraryCallError("WORKFLOW_NOT_FOUND", "Saved workflow not found.");
    const [projectId, i] = at;
    const list = [...(this.workflows.get(projectId) ?? [])];
    const stored: Record<string, unknown> = {
      ...(list[i] as Record<string, unknown>),
      name,
      updatedAt: NOW,
    };
    list[i] = stored;
    this.workflows.set(projectId, list);
    const createdAt = typeof stored.createdAt === "string" ? stored.createdAt : null;
    return {
      value: {
        id,
        projectId,
        name,
        createdAt,
        updatedAt: NOW,
        bytes: JSON.stringify(stored).length,
      },
      seq: this.write(),
    };
  }

  async removeWorkflow(id: string): Promise<Seqd<null>> {
    this.record("removeWorkflow", [id]);
    const at = this.workflowIndex(id);
    if (!at) throw new LibraryCallError("WORKFLOW_NOT_FOUND", "Saved workflow not found.");
    const [projectId, i] = at;
    this.workflows.set(
      projectId,
      (this.workflows.get(projectId) ?? []).filter((_, j) => j !== i),
    );
    return { value: null, seq: this.write() };
  }

  // Dashboards and chats: the rows only (no names, versions' prune or budget).
  readonly dashboards = new Map<string, WireDashboard>();
  readonly dashboardVersions: WireDashboardVersion[] = [];
  readonly chats = new Map<string, WireChat>();
  readonly messages = new Map<string, ChatMessages["messages"]>();

  async listDashboards(projectId: string): Promise<Seqd<WireDashboard[]>> {
    this.record("listDashboards", [projectId]);
    const value = [...this.dashboards.values()].filter((d) => d.projectId === projectId);
    return { value, seq: this.seq() };
  }

  /** The project's versions without their snapshots, as Core lists them (5d-2 Task 7). */
  async listDashboardVersions(projectId: string): Promise<Seqd<WireDashboardVersionMeta[]>> {
    this.record("listDashboardVersions", [projectId]);
    const ids = new Set(
      [...this.dashboards.values()].filter((d) => d.projectId === projectId).map((d) => d.id),
    );
    return {
      value: this.dashboardVersions.filter((v) => ids.has(v.dashboardId)).map(versionMeta),
      seq: this.seq(),
    };
  }

  async getDashboardVersion(
    dashboardId: string,
    versionId: string,
  ): Promise<Seqd<WireDashboardVersion>> {
    this.record("getDashboardVersion", [dashboardId, versionId]);
    if (!this.dashboards.has(dashboardId)) {
      throw new LibraryCallError("DASHBOARD_NOT_FOUND", "Dashboard not found.");
    }
    const v = this.dashboardVersions.find(
      (x) => x.id === versionId && x.dashboardId === dashboardId,
    );
    if (!v) {
      throw new LibraryCallError("DASHBOARD_VERSION_NOT_FOUND", "Dashboard version not found.");
    }
    return { value: { ...v }, seq: this.seq() };
  }

  async createDashboard(draft: DashboardDraft): Promise<Seqd<WireDashboard>> {
    this.record("createDashboard", [draft]);
    const row: WireDashboard = {
      id: this.id("dashboard-"),
      projectId: draft.projectId,
      name: draft.name,
      viewport: JSON.stringify(draft.viewport),
      widgets: JSON.stringify(draft.widgets),
      dateFilter: draft.dateFilter === undefined ? null : JSON.stringify(draft.dateFilter),
      starred: false,
      shared: !!draft.shared,
      createdAt: NOW,
      updatedAt: NOW,
    };
    if (draft.description !== undefined) row.description = draft.description;
    this.dashboards.set(row.id, row);
    return { value: row, seq: this.write() };
  }

  async updateDashboard(id: string, patch: DashboardPatch): Promise<Seqd<DashboardUpdated>> {
    this.record("updateDashboard", [id, patch]);
    const row = this.dashboards.get(id);
    if (!row) throw new LibraryCallError("DASHBOARD_NOT_FOUND", "Dashboard not found.");
    const next: WireDashboard = { ...row, updatedAt: NOW };
    if (patch.name !== undefined) next.name = patch.name;
    if (patch.description === null) delete next.description;
    else if (patch.description !== undefined) next.description = patch.description;
    if (patch.widgets !== undefined) next.widgets = JSON.stringify(patch.widgets);
    if (patch.viewport !== undefined) next.viewport = JSON.stringify(patch.viewport);
    if (patch.dateFilter !== undefined) {
      next.dateFilter = patch.dateFilter === null ? null : JSON.stringify(patch.dateFilter);
    }
    if (patch.starred !== undefined) next.starred = patch.starred;
    if (patch.shared !== undefined) next.shared = patch.shared;
    this.dashboards.set(id, next);
    let version: WireDashboardVersionMeta | null = null;
    if (patch.captureVersion) {
      const n = this.dashboardVersions.filter((v) => v.dashboardId === id).length + 1;
      const whole: WireDashboardVersion = {
        id: this.id("dver-"),
        dashboardId: id,
        version: n,
        snapshot: JSON.stringify({
          name: row.name,
          description: row.description,
          widgets: JSON.parse(row.widgets) as unknown,
          viewport: JSON.parse(row.viewport) as unknown,
          dateFilter: row.dateFilter ? (JSON.parse(row.dateFilter) as unknown) : null,
        }),
        createdAt: NOW,
      };
      this.dashboardVersions.push(whole);
      version = versionMeta(whole);
    }
    return { value: { dashboard: next, version, prunedVersionIds: [] }, seq: this.write() };
  }

  async removeDashboard(id: string): Promise<Seqd<null>> {
    this.record("removeDashboard", [id]);
    if (!this.dashboards.delete(id)) {
      throw new LibraryCallError("DASHBOARD_NOT_FOUND", "Dashboard not found.");
    }
    return { value: null, seq: this.write() };
  }

  async listChats(connectionId: string): Promise<Seqd<WireChat[]>> {
    this.record("listChats", [connectionId]);
    const value = [...this.chats.values()]
      .filter((c) => c.connectionId === connectionId)
      .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
    return { value, seq: this.seq() };
  }

  async listChatMessages(chatId: string): Promise<Seqd<ChatMessages>> {
    this.record("listChatMessages", [chatId]);
    const messages = [...(this.messages.get(chatId) ?? [])];
    return {
      value: { messages, storedBytes: this.bytesOf(chatId), full: this.full.has(chatId) },
      seq: this.seq(),
    };
  }

  /** Chats Core answers as full (`ChatMessages.full`: the web's budget). */
  readonly full = new Set<string>();

  /** A chat's stored content bytes (as Core counts them). */
  bytesOf(chatId: string): number {
    return (this.messages.get(chatId) ?? []).reduce(
      (n, m) => n + new TextEncoder().encode(m.content).length,
      0,
    );
  }

  async createChat(draft: ChatDraft): Promise<Seqd<WireChat>> {
    this.record("createChat", [draft]);
    const row: WireChat = {
      id: this.id("chat-"),
      connectionId: draft.connectionId,
      title: draft.title,
      createdAt: NOW,
      updatedAt: NOW,
    };
    this.chats.set(row.id, row);
    return { value: row, seq: this.write() };
  }

  async updateChat(id: string, patch: ChatPatch): Promise<Seqd<WireChat>> {
    this.record("updateChat", [id, patch]);
    const row = this.chats.get(id);
    if (!row) throw new LibraryCallError("CHAT_NOT_FOUND", "Chat not found.");
    const next = { ...row, ...(patch.title !== undefined ? { title: patch.title } : {}) };
    this.chats.set(id, next);
    return { value: next, seq: this.write() };
  }

  async removeChat(id: string): Promise<Seqd<null>> {
    this.record("removeChat", [id]);
    if (!this.chats.delete(id)) throw new LibraryCallError("CHAT_NOT_FOUND", "Chat not found.");
    this.messages.delete(id);
    return { value: null, seq: this.write() };
  }

  async putChatMessages(chatId: string, messages: ChatMessageDraft[]): Promise<Seqd<ChatMessages>> {
    this.record("putChatMessages", [chatId, messages]);
    if (!this.chats.has(chatId)) throw new LibraryCallError("CHAT_NOT_FOUND", "Chat not found.");
    const list = [...(this.messages.get(chatId) ?? [])];
    const rows = messages.map((m) => ({ ...m, chatId }));
    for (const row of rows) {
      const i = list.findIndex((m) => m.id === row.id);
      if (i === -1) list.push(row);
      else list[i] = row;
    }
    this.messages.set(chatId, list);
    return {
      value: { messages: rows, storedBytes: this.bytesOf(chatId), full: this.full.has(chatId) },
      seq: this.write(),
    };
  }

  async removeChatMessages(chatId: string, ids: string[]): Promise<Seqd<number>> {
    this.record("removeChatMessages", [chatId, ids]);
    const list = this.messages.get(chatId) ?? [];
    const kept = list.filter((m) => !ids.includes(m.id));
    this.messages.set(chatId, kept);
    return { value: list.length - kept.length, seq: this.write() };
  }
}
