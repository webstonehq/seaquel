/**
 * A `LibraryService` for tests: rows in memory, every call recorded as
 * sent, failures injectable per method, and a `seq` the test can move. It
 * keeps only the rules the GUI's tests lean on (ids, patches, `lastConnected`
 * on `connected`, label stripping, keyframe versions); `TsLibrary` holds
 * Core's full rules.
 */
import type {
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
}
