/**
 * `TsState`: the demo's side of the phase 5d-2 `library` additions
 * (Decisions 21, 23, 24 and 26): dashboards and their versions, saved
 * workflows, and AI chats and messages, over the demo's sql.js file.
 * `TsLibrary` holds one and runs each call through its queue, so the
 * demo's library calls stay one at a time and share one change sequence.
 * It lives apart only to keep `ts-library.ts` readable.
 *
 * It follows `crates/seaquel-core/src/state.rs`:
 * - Core-made ids (`dashboard-`, `dver-`, `workflow-` and a uuid; chats a
 *   bare uuid) and times; message ids are the GUI's;
 * - dashboard names clash within a project (`NAME_TAKEN`, by `nameKey`); a
 *   patch changes only its fields, `starred` alone keeps `updated_at`; with
 *   `captureVersion` a snapshot of the stored dashboard is appended as
 *   `MAX(version) + 1` and the versions are pruned by
 *   `dashboard_version_limit` (0 keeps all); a missing dashboard is
 *   `DASHBOARD_NOT_FOUND`, never re-inserted;
 * - a workflow's `id`, `projectId`, `createdAt` and `updatedAt` are set
 *   here and the rest is kept as sent;
 * - messages are upserted by id (a stored one keeps its place), an id of
 *   another chat is refused, and they read in `timestamp, rowid` order.
 *
 * No web limits (the demo has none).
 */
import type { SqliteDatabase } from "$lib/storage/sqlite-types";
import {
  CHAT_NOT_FOUND,
  CONNECTION_NOT_FOUND,
  DASHBOARD_NOT_FOUND,
  DASHBOARD_VERSION_NOT_FOUND,
  INVALID_ARGUMENT,
  LibraryCallError,
  PROJECT_NOT_FOUND,
  WORKFLOW_NOT_FOUND,
  type ChatDraft,
  type ChatMessageDraft,
  type ChatMessages,
  type ChatPatch,
  type DashboardDraft,
  type DashboardPatch,
  type DashboardUpdated,
  type WireChat,
  type WireDashboard,
  type WireDashboardVersion,
  type WireDashboardVersionMeta,
  type WireWorkflowMeta,
} from "./types";

type Row = Record<string, unknown>;
type Obj = Record<string, unknown>;
export type Statement = { sql: string; params?: unknown[] };

/** What `TsState` needs of `TsLibrary`. */
export interface TsStateHost {
  iso(): string;
  newId(prefix: string): string;
  nameKey(name: string): string;
  versionPrune(
    versions: { id: string; version: number; keyframe: boolean }[],
    keep: number,
  ): string[];
  parseVersionLimit(value: string | null | undefined): number;
}

const MESSAGE_ID = /^[A-Za-z0-9_-]{1,64}$/;

function invalid(message: string): LibraryCallError {
  return new LibraryCallError(INVALID_ARGUMENT, message);
}

function hasNul(value: unknown): boolean {
  return typeof value === "string" && value.includes("\u0000");
}

function checkName(name: unknown, what: string): void {
  if (typeof name !== "string" || name.trim() === "") throw invalid(`A ${what} needs a name.`);
  if (hasNul(name)) throw invalid("A value can't contain a NUL character.");
}

function text(v: unknown): string {
  return typeof v === "string" ? v : typeof v === "number" ? String(v) : "";
}

function optText(v: unknown): string | undefined {
  return typeof v === "string" ? v : undefined;
}

/** A JSON value as stored text; `null` when it isn't JSON-able. */
function jsonText(value: unknown): string {
  return JSON.stringify(value) ?? "null";
}

function isObject(v: unknown): v is Obj {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function dashboardFromRow(row: Row): WireDashboard {
  const d: WireDashboard = {
    id: text(row.id),
    projectId: text(row.project_id),
    name: text(row.name),
    viewport: text(row.viewport),
    widgets: text(row.widgets),
    dateFilter: typeof row.date_filter === "string" ? row.date_filter : null,
    starred: row.starred === 1,
    shared: row.shared === 1,
    createdAt: text(row.created_at),
    updatedAt: text(row.updated_at),
  };
  const description = optText(row.description);
  if (description !== undefined) d.description = description;
  return d;
}

function versionFromRow(row: Row): WireDashboardVersion {
  return {
    id: text(row.id),
    dashboardId: text(row.dashboard_id),
    version: Number(row.version),
    snapshot: text(row.snapshot),
    createdAt: text(row.created_at),
  };
}

/** Bytes of `s` as UTF-8, as Core counts a body's size. */
function utf8Bytes(s: string): number {
  return new TextEncoder().encode(s).length;
}

/**
 * A version without its snapshot (`dashboardVersionsList`, 5d-2 Task 7):
 * its widget count (`null` when the snapshot's `widgets` isn't a list)
 * and size, as Core answers them.
 */
function versionMeta(v: WireDashboardVersion): WireDashboardVersionMeta {
  let widgetCount: number | null = null;
  try {
    const widgets = (JSON.parse(v.snapshot) as Obj | null)?.widgets;
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
    bytes: utf8Bytes(v.snapshot),
  };
}

/**
 * A stored workflow row without its body (`workflowsList`, 5d-2 Task 7),
 * or `null` for a row no list shows (not JSON, or `null`), as Core reads
 * them.
 */
export function workflowMeta(id: string, projectId: string, data: string): WireWorkflowMeta | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(data);
  } catch {
    return null;
  }
  if (parsed === null) return null;
  const o = isObject(parsed) ? parsed : {};
  const str = (v: unknown) => (typeof v === "string" ? v : null);
  return {
    id,
    projectId,
    name: str(o.name) ?? "",
    createdAt: str(o.createdAt),
    updatedAt: str(o.updatedAt),
    bytes: utf8Bytes(data),
  };
}

/** Stored JSON text as a value to embed, or the text itself when it doesn't parse. */
function embed(stored: string): unknown {
  try {
    return JSON.parse(stored);
  } catch {
    return stored;
  }
}

/** A version's snapshot of a stored dashboard (`dashboard_snapshot`). */
function snapshotOf(d: WireDashboard): string {
  const snapshot: Obj = { name: d.name };
  if (d.description !== undefined) snapshot.description = d.description;
  snapshot.widgets = embed(d.widgets);
  snapshot.viewport = embed(d.viewport);
  snapshot.dateFilter = d.dateFilter ? embed(d.dateFilter) : null;
  return JSON.stringify(snapshot);
}

function chatFromRow(row: Row): WireChat {
  return {
    id: text(row.id),
    connectionId: text(row.connection_id),
    title: text(row.title),
    createdAt: text(row.created_at),
    updatedAt: text(row.updated_at),
  };
}

function messageFromRow(row: Row): ChatMessages["messages"][number] {
  const m: ChatMessages["messages"][number] = {
    id: text(row.id),
    chatId: text(row.chat_id),
    role: text(row.role) as "user" | "assistant",
    content: text(row.content),
    timestamp: text(row.timestamp),
  };
  const query = optText(row.query);
  if (query !== undefined) m.query = query;
  const dashboardId = optText(row.dashboard_id);
  if (dashboardId !== undefined) m.dashboardId = dashboardId;
  return m;
}

function bytes(s: string): number {
  return new TextEncoder().encode(s).length;
}

export class TsState {
  constructor(
    private readonly db: SqliteDatabase,
    private readonly host: TsStateHost,
  ) {}

  private async projectOrFail(projectId: string): Promise<void> {
    const rows = await this.db.query<Row>("SELECT id FROM projects WHERE id = ?", [projectId]);
    if (rows.length === 0) throw new LibraryCallError(PROJECT_NOT_FOUND, "Project not found.");
  }

  // -------- Dashboards --------

  async listDashboards(projectId: string): Promise<WireDashboard[]> {
    const rows = await this.db.query<Row>(
      "SELECT * FROM dashboards WHERE project_id = ? ORDER BY rowid",
      [projectId],
    );
    return rows.map(dashboardFromRow);
  }

  /** A project's versions without their snapshots (5d-2 Task 7). */
  async listDashboardVersions(projectId: string): Promise<WireDashboardVersionMeta[]> {
    const rows = await this.db.query<Row>(
      `SELECT dv.* FROM dashboard_versions dv JOIN dashboards d ON d.id = dv.dashboard_id
       WHERE d.project_id = ? ORDER BY dv.dashboard_id, dv.version ASC`,
      [projectId],
    );
    return rows.map((row) => versionMeta(versionFromRow(row)));
  }

  /** One version with its snapshot, only under its own dashboard. */
  async getDashboardVersion(dashboardId: string, versionId: string): Promise<WireDashboardVersion> {
    if (!(await this.getDashboard(dashboardId))) {
      throw new LibraryCallError(DASHBOARD_NOT_FOUND, "Dashboard not found.");
    }
    const [row] = await this.db.query<Row>(
      "SELECT * FROM dashboard_versions WHERE id = ? AND dashboard_id = ?",
      [versionId, dashboardId],
    );
    if (!row) {
      throw new LibraryCallError(DASHBOARD_VERSION_NOT_FOUND, "Dashboard version not found.");
    }
    return versionFromRow(row);
  }

  private async getDashboard(id: string): Promise<WireDashboard | null> {
    const [row] = await this.db.query<Row>("SELECT * FROM dashboards WHERE id = ?", [id]);
    return row ? dashboardFromRow(row) : null;
  }

  private async nameFree(projectId: string, name: string, except?: string): Promise<void> {
    const key = this.host.nameKey(name);
    const rows = await this.db.query<Row>(
      "SELECT id, name FROM dashboards WHERE project_id = ? ORDER BY rowid",
      [projectId],
    );
    const holder = rows.find((r) => r.id !== except && this.host.nameKey(text(r.name)) === key);
    if (holder) {
      throw new LibraryCallError(
        "NAME_TAKEN",
        "Another dashboard here already has this name.",
        text(holder.id),
      );
    }
  }

  /** `name`, or the first free `"<name> (n)"` among the project's dashboards. */
  private async freeName(projectId: string, name: string): Promise<string> {
    const rows = await this.db.query<Row>("SELECT name FROM dashboards WHERE project_id = ?", [
      projectId,
    ]);
    const taken = new Set(rows.map((r) => this.host.nameKey(text(r.name))));
    if (!taken.has(this.host.nameKey(name))) return name;
    for (let n = 2; ; n++) {
      const candidate = `${name} (${n})`;
      if (!taken.has(this.host.nameKey(candidate))) return candidate;
    }
  }

  private checkDashboardBody(widgets: unknown, viewport: unknown, dateFilter: unknown): void {
    if (widgets !== undefined && !Array.isArray(widgets)) {
      throw invalid("A dashboard's widgets are a list.");
    }
    if (viewport !== undefined && !isObject(viewport)) {
      throw invalid("A dashboard's viewport is a JSON object.");
    }
    if (dateFilter !== undefined && dateFilter !== null && !isObject(dateFilter)) {
      throw invalid("A dashboard's date filter is a JSON object.");
    }
  }

  async createDashboard(
    draft: DashboardDraft,
  ): Promise<{ row: WireDashboard; writes: Statement[] }> {
    checkName(draft.name, "dashboard");
    if (hasNul(draft.projectId) || hasNul(draft.description)) {
      throw invalid("A value can't contain a NUL character.");
    }
    this.checkDashboardBody(draft.widgets, draft.viewport, draft.dateFilter);
    await this.projectOrFail(draft.projectId);
    const name = draft.renameIfTaken
      ? await this.freeName(draft.projectId, draft.name)
      : (await this.nameFree(draft.projectId, draft.name), draft.name);
    const now = this.host.iso();
    const row: WireDashboard = {
      id: this.host.newId("dashboard-"),
      projectId: draft.projectId,
      name,
      viewport: jsonText(draft.viewport),
      widgets: jsonText(draft.widgets),
      dateFilter: draft.dateFilter === undefined ? null : jsonText(draft.dateFilter),
      starred: false,
      shared: !!draft.shared,
      createdAt: now,
      updatedAt: now,
    };
    if (draft.description !== undefined) row.description = draft.description;
    return {
      row,
      writes: [
        {
          sql: `INSERT INTO dashboards (id, project_id, name, viewport, widgets, date_filter,
                  starred, shared, description, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?)`,
          params: [
            row.id,
            row.projectId,
            row.name,
            row.viewport,
            row.widgets,
            row.dateFilter,
            row.shared ? 1 : 0,
            row.description ?? null,
            now,
            now,
          ],
        },
      ],
    };
  }

  async updateDashboard(
    id: string,
    patch: DashboardPatch,
  ): Promise<{ value: DashboardUpdated; writes: Statement[] }> {
    if (patch.name !== undefined) checkName(patch.name, "dashboard");
    if (hasNul(patch.description)) throw invalid("A value can't contain a NUL character.");
    this.checkDashboardBody(patch.widgets, patch.viewport, patch.dateFilter);
    const row = await this.getDashboard(id);
    if (!row) throw new LibraryCallError(DASHBOARD_NOT_FOUND, "Dashboard not found.");
    const now = this.host.iso();
    const snapshot = patch.captureVersion ? snapshotOf(row) : null;
    const next: WireDashboard = { ...row };
    let renamed = false;
    if (patch.name !== undefined) {
      renamed = this.host.nameKey(patch.name) !== this.host.nameKey(row.name);
      next.name = patch.name;
    }
    if (patch.description === null) delete next.description;
    else if (patch.description !== undefined) next.description = patch.description;
    if (patch.widgets !== undefined) next.widgets = jsonText(patch.widgets);
    if (patch.viewport !== undefined) next.viewport = jsonText(patch.viewport);
    if (patch.dateFilter === null) next.dateFilter = null;
    else if (patch.dateFilter !== undefined) next.dateFilter = jsonText(patch.dateFilter);
    if (patch.starred !== undefined) next.starred = patch.starred;
    if (patch.shared !== undefined) next.shared = patch.shared;
    const onlyStarred =
      patch.starred !== undefined &&
      patch.name === undefined &&
      patch.description === undefined &&
      patch.widgets === undefined &&
      patch.viewport === undefined &&
      patch.dateFilter === undefined &&
      patch.shared === undefined;
    if (!onlyStarred) next.updatedAt = now;
    if (renamed) await this.nameFree(next.projectId, next.name, id);
    const writes: Statement[] = [
      {
        sql: `UPDATE dashboards SET name = ?, viewport = ?, widgets = ?, date_filter = ?,
                starred = ?, shared = ?, description = ?, updated_at = ? WHERE id = ?`,
        params: [
          next.name,
          next.viewport,
          next.widgets,
          next.dateFilter ?? null,
          next.starred ? 1 : 0,
          next.shared ? 1 : 0,
          next.description ?? null,
          next.updatedAt,
          id,
        ],
      },
    ];
    let version: WireDashboardVersionMeta | null = null;
    let prunedVersionIds: string[] = [];
    if (snapshot !== null) {
      const stored = await this.db.query<Row>(
        "SELECT id, version FROM dashboard_versions WHERE dashboard_id = ? ORDER BY version",
        [id],
      );
      const highest = stored.reduce((max, r) => Math.max(max, Math.floor(Number(r.version))), 0);
      // Answered without its snapshot, as Core answers it (5d-2 Task 7).
      version = versionMeta({
        id: this.host.newId("dver-"),
        dashboardId: id,
        version: highest + 1,
        snapshot,
        createdAt: now,
      });
      writes.push({
        sql: `INSERT INTO dashboard_versions (id, dashboard_id, version, snapshot, created_at)
              VALUES (?, ?, ?, ?, ?)`,
        params: [version.id, id, version.version, snapshot, now],
      });
      const [limit] = await this.db.query<Row>(
        "SELECT value FROM app_state WHERE key = 'dashboard_version_limit'",
      );
      const keep = this.host.parseVersionLimit(
        typeof limit?.value === "string" ? limit.value : null,
      );
      const metas = [
        ...stored.map((r) => ({ id: text(r.id), version: Number(r.version), keyframe: true })),
        { id: version.id, version: version.version, keyframe: true },
      ];
      prunedVersionIds = this.host.versionPrune(metas, keep);
      for (const pruned of prunedVersionIds) {
        writes.push({ sql: "DELETE FROM dashboard_versions WHERE id = ?", params: [pruned] });
      }
    }
    return { value: { dashboard: next, version, prunedVersionIds }, writes };
  }

  async removeDashboard(id: string): Promise<Statement[]> {
    if (!(await this.getDashboard(id))) {
      throw new LibraryCallError(DASHBOARD_NOT_FOUND, "Dashboard not found.");
    }
    return [
      // Versions by id too: a beta-era file has no foreign key to cascade.
      { sql: "DELETE FROM dashboard_versions WHERE dashboard_id = ?", params: [id] },
      { sql: "DELETE FROM dashboards WHERE id = ?", params: [id] },
    ];
  }

  // -------- Saved workflows --------

  /** The stored JSON (`workflow_json`): `id` first, the body, then Core's fields. */
  private workflowJson(
    body: unknown,
    id: string,
    projectId: string,
    createdAt: string,
    updatedAt: string,
  ): Obj {
    if (!isObject(body)) throw invalid("A workflow is a JSON object.");
    // Core's rule (5d-2 Task 7): the name is text.
    if (body.name === undefined || body.name === null) throw invalid("A workflow needs a name.");
    if (typeof body.name !== "string") throw invalid("A workflow's name is text.");
    if (hasNul(body.name)) throw invalid("A value can't contain a NUL character.");
    const out: Obj = { id };
    for (const [k, v] of Object.entries(body)) {
      if (!["id", "projectId", "createdAt", "updatedAt"].includes(k)) out[k] = v;
    }
    out.projectId = projectId;
    out.createdAt = createdAt;
    out.updatedAt = updatedAt;
    return out;
  }

  async createWorkflow(
    projectId: string,
    workflow: unknown,
  ): Promise<{ value: Obj; writes: Statement[] }> {
    if (hasNul(projectId)) throw invalid("A value can't contain a NUL character.");
    const now = this.host.iso();
    const id = this.host.newId("workflow-");
    const value = this.workflowJson(workflow, id, projectId, now, now);
    await this.projectOrFail(projectId);
    return {
      value,
      writes: [
        {
          sql: "INSERT INTO saved_canvases (id, project_id, data) VALUES (?, ?, ?)",
          params: [id, projectId, JSON.stringify(value)],
        },
      ],
    };
  }

  async updateWorkflow(
    id: string,
    workflow: unknown,
  ): Promise<{ value: Obj; writes: Statement[] }> {
    if (!isObject(workflow)) throw invalid("A workflow is a JSON object.");
    const [row] = await this.db.query<Row>(
      "SELECT project_id, data FROM saved_canvases WHERE id = ?",
      [id],
    );
    if (!row) throw new LibraryCallError(WORKFLOW_NOT_FOUND, "Saved workflow not found.");
    const now = this.host.iso();
    let created: unknown;
    try {
      created = (JSON.parse(text(row.data)) as Obj | null)?.createdAt;
    } catch {
      created = undefined;
    }
    const value = this.workflowJson(
      workflow,
      id,
      text(row.project_id),
      typeof created === "string" ? created : now,
      now,
    );
    return {
      value,
      writes: [
        {
          sql: "UPDATE saved_canvases SET data = ? WHERE id = ?",
          params: [JSON.stringify(value), id],
        },
      ],
    };
  }

  /**
   * Core's `workflowRename`: only the stored JSON's `name` and `updatedAt`
   * change, on the row as stored now, so another save isn't undone.
   */
  async renameWorkflow(
    id: string,
    name: string,
  ): Promise<{ value: WireWorkflowMeta; writes: Statement[] }> {
    checkName(name, "workflow");
    const [row] = await this.db.query<Row>(
      "SELECT project_id, data FROM saved_canvases WHERE id = ?",
      [id],
    );
    let stored: unknown;
    try {
      stored = row ? JSON.parse(text(row.data)) : undefined;
    } catch {
      stored = undefined;
    }
    if (!isObject(stored)) {
      throw new LibraryCallError(WORKFLOW_NOT_FOUND, "Saved workflow not found.");
    }
    const now = this.host.iso();
    const next: Obj = { ...stored, name, updatedAt: now };
    const data = JSON.stringify(next);
    const value = workflowMeta(id, text(row!.project_id), data)!;
    return {
      value,
      writes: [{ sql: "UPDATE saved_canvases SET data = ? WHERE id = ?", params: [data, id] }],
    };
  }

  async removeWorkflow(id: string): Promise<Statement[]> {
    const [row] = await this.db.query<Row>("SELECT id FROM saved_canvases WHERE id = ?", [id]);
    if (!row) throw new LibraryCallError(WORKFLOW_NOT_FOUND, "Saved workflow not found.");
    return [{ sql: "DELETE FROM saved_canvases WHERE id = ?", params: [id] }];
  }

  // -------- AI chats --------

  async listChats(connectionId: string): Promise<WireChat[]> {
    const rows = await this.db.query<Row>(
      "SELECT * FROM ai_chats WHERE connection_id = ? ORDER BY updated_at DESC",
      [connectionId],
    );
    return rows.map(chatFromRow);
  }

  private async storedBytes(chatId: string): Promise<number> {
    const rows = await this.db.query<Row>("SELECT content FROM ai_messages WHERE chat_id = ?", [
      chatId,
    ]);
    return rows.reduce((sum, r) => sum + bytes(text(r.content)), 0);
  }

  async listChatMessages(chatId: string): Promise<ChatMessages> {
    const rows = await this.db.query<Row>(
      "SELECT * FROM ai_messages WHERE chat_id = ? ORDER BY timestamp ASC, rowid ASC",
      [chatId],
    );
    // The demo has no limits: a chat is never full.
    return {
      messages: rows.map(messageFromRow),
      storedBytes: await this.storedBytes(chatId),
      full: false,
    };
  }

  private async getChat(id: string): Promise<WireChat | null> {
    const [row] = await this.db.query<Row>("SELECT * FROM ai_chats WHERE id = ?", [id]);
    return row ? chatFromRow(row) : null;
  }

  async createChat(draft: ChatDraft): Promise<{ row: WireChat; writes: Statement[] }> {
    if (typeof draft.title !== "string" || hasNul(draft.title) || hasNul(draft.connectionId)) {
      throw invalid("A value can't contain a NUL character.");
    }
    const [conn] = await this.db.query<Row>("SELECT id FROM connections WHERE id = ?", [
      draft.connectionId,
    ]);
    if (!conn) throw new LibraryCallError(CONNECTION_NOT_FOUND, "Connection not found.");
    const now = this.host.iso();
    const row: WireChat = {
      id: crypto.randomUUID(),
      connectionId: draft.connectionId,
      title: draft.title,
      createdAt: now,
      updatedAt: now,
    };
    return {
      row,
      writes: [
        {
          sql: `INSERT INTO ai_chats (id, connection_id, title, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?)`,
          params: [row.id, row.connectionId, row.title, now, now],
        },
      ],
    };
  }

  async updateChat(id: string, patch: ChatPatch): Promise<{ row: WireChat; writes: Statement[] }> {
    if (hasNul(patch.title)) throw invalid("A value can't contain a NUL character.");
    const row = await this.getChat(id);
    if (!row) throw new LibraryCallError(CHAT_NOT_FOUND, "Chat not found.");
    const next = { ...row };
    if (patch.title !== undefined) next.title = patch.title;
    if (patch.touched) next.updatedAt = this.host.iso();
    return {
      row: next,
      writes: [
        {
          sql: "UPDATE ai_chats SET title = ?, updated_at = ? WHERE id = ?",
          params: [next.title, next.updatedAt, id],
        },
      ],
    };
  }

  async removeChat(id: string): Promise<Statement[]> {
    if (!(await this.getChat(id))) throw new LibraryCallError(CHAT_NOT_FOUND, "Chat not found.");
    return [
      { sql: "DELETE FROM ai_messages WHERE chat_id = ?", params: [id] },
      { sql: "DELETE FROM ai_chats WHERE id = ?", params: [id] },
    ];
  }

  async putChatMessages(
    chatId: string,
    messages: ChatMessageDraft[],
  ): Promise<{ value: ChatMessages; writes: Statement[] }> {
    for (const m of messages) {
      if (!MESSAGE_ID.test(m.id)) {
        throw invalid("A message id is 1 to 64 letters, digits, '-' or '_'.");
      }
      if (m.role !== "user" && m.role !== "assistant") {
        throw invalid("A message's role is user or assistant.");
      }
      if ([m.content, m.timestamp, m.query, m.dashboardId].some(hasNul)) {
        throw invalid("A value can't contain a NUL character.");
      }
    }
    if (!(await this.getChat(chatId)))
      throw new LibraryCallError(CHAT_NOT_FOUND, "Chat not found.");
    for (const m of messages) {
      const [owner] = await this.db.query<Row>("SELECT chat_id FROM ai_messages WHERE id = ?", [
        m.id,
      ]);
      if (owner && owner.chat_id !== chatId) {
        throw invalid("A message id belongs to another chat.");
      }
    }
    const rows = messages.map((m) => {
      const row: ChatMessages["messages"][number] = {
        id: m.id,
        chatId,
        role: m.role,
        content: m.content,
        timestamp: m.timestamp,
      };
      if (m.query !== undefined) row.query = m.query;
      if (m.dashboardId !== undefined) row.dashboardId = m.dashboardId;
      return row;
    });
    const writes = rows.map((m) => ({
      sql: `INSERT INTO ai_messages (id, chat_id, role, content, timestamp, query, dashboard_id)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET chat_id = excluded.chat_id, role = excluded.role,
              content = excluded.content, timestamp = excluded.timestamp,
              query = excluded.query, dashboard_id = excluded.dashboard_id`,
      params: [
        m.id,
        chatId,
        m.role,
        m.content,
        m.timestamp,
        m.query ?? null,
        m.dashboardId ?? null,
      ],
    }));
    return { value: { messages: rows, storedBytes: 0, full: false }, writes };
  }

  /** The chat's stored bytes after a write. */
  chatBytes(chatId: string): Promise<number> {
    return this.storedBytes(chatId);
  }

  async removeChatMessages(
    chatId: string,
    ids: string[],
  ): Promise<{ count: number; writes: Statement[] }> {
    if (!(await this.getChat(chatId)))
      throw new LibraryCallError(CHAT_NOT_FOUND, "Chat not found.");
    let count = 0;
    for (const id of ids) {
      const [row] = await this.db.query<Row>(
        "SELECT id FROM ai_messages WHERE chat_id = ? AND id = ?",
        [chatId, id],
      );
      if (row) count++;
    }
    return {
      count,
      writes: ids.map((id) => ({
        sql: "DELETE FROM ai_messages WHERE chat_id = ? AND id = ?",
        params: [chatId, id],
      })),
    };
  }
}
