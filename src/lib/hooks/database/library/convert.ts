/**
 * The library's wire rows (`LibraryService`) as the view models hold them,
 * and the patches the view models send: only the fields that changed
 * (Decision 2), so another window's edit to another field survives.
 */
import type {
  AIChat,
  AIMessage,
  Dashboard,
  DashboardSnapshot,
  DashboardVersion,
  DashboardWidget,
  DatabaseConnection,
  Project,
  Query,
  QueryVersion,
  ResolvedDashboardVersion,
} from "$lib/types";
import type { SavedWorkflowSummary } from "$lib/types/workflow";
import { DEFAULT_PROJECT_ID } from "$lib/types";
import { segmentsFromParts } from "../ai/events.js";
import { REPLY_CUT_NOTE, splitCutNote } from "../ai/reply.js";
import type {
  ConnectionPatch,
  SavedQueryPatch,
  WireConnection,
  WireProject,
  WireQueryVersion,
  WireSavedQuery,
  WireChat,
  WireDashboard,
  WireDashboardVersion,
  WireDashboardVersionMeta,
  WireWorkflowMeta,
  ChatMessages,
  ChatMessageDraft,
} from "./types";

/**
 * A stored connection as the page shows it. `keep` is the page's copy of
 * it, whose runtime fields (the Core connection, a password typed this
 * session, the tunnel port) survive.
 */
export function connectionFromWire(
  wire: WireConnection,
  keep?: DatabaseConnection,
): DatabaseConnection {
  // Rows saved before usernames were stored separately hold it in the URL.
  let username = wire.username ?? "";
  if (!username && wire.connectionString) {
    try {
      const s = wire.connectionString.replace("postgresql://", "postgres://");
      if (!s.startsWith("sqlite") && !s.startsWith("duckdb")) {
        const url = new URL(s);
        username = url.username ? decodeURIComponent(url.username) : "";
      }
    } catch {
      // Not a URL.
    }
  }
  return {
    id: wire.id,
    name: wire.name,
    type: wire.type,
    host: wire.host,
    port: wire.port,
    databaseName: wire.databaseName,
    username,
    password: keep?.password ?? "",
    sslMode: wire.sslMode,
    connectionString: wire.connectionString,
    lastConnected: wire.lastConnected ? new Date(wire.lastConnected) : undefined,
    // A stored JSON `null` loads as `null`.
    sshTunnel: wire.sshTunnel ?? undefined,
    savePassword: wire.savePassword,
    saveSshPassword: wire.saveSshPassword,
    saveSshKeyPassphrase: wire.saveSshKeyPassphrase,
    projectId: wire.projectId || DEFAULT_PROJECT_ID,
    labelIds: wire.labelIds ?? [],
    isLocalOnly: wire.isLocalOnly,
    sharedConnectionId: wire.sharedConnectionId,
    ...(wire.sharedOrigin ? { sharedOrigin: wire.sharedOrigin } : {}),
    aiShareSchema: wire.aiShareSchema,
    aiShareData: wire.aiShareData,
    activeAIProviderId: wire.activeAIProviderId,
    activeAIModel: wire.activeAIModel,
    ...(keep?.providerConnectionId ? { providerConnectionId: keep.providerConnectionId } : {}),
    ...(keep?.tunnelLocalPort !== undefined ? { tunnelLocalPort: keep.tunnelLocalPort } : {}),
  };
}

export function projectFromWire(wire: WireProject): Project {
  return {
    id: wire.id,
    name: wire.name,
    description: wire.description,
    createdAt: new Date(wire.createdAt),
    updatedAt: new Date(wire.updatedAt),
    customLabels: wire.customLabels,
    gitRepoPath: wire.gitRepoPath,
  };
}

export function savedQueryFromWire(wire: WireSavedQuery): Query {
  return {
    id: wire.id,
    name: wire.name,
    query: wire.query,
    projectId: wire.projectId,
    createdAt: new Date(wire.createdAt),
    updatedAt: new Date(wire.updatedAt),
    // Stored JSON `null` loads as `null`; the app's Query uses absent.
    parameters: wire.parameters ?? undefined,
    starred: wire.starred,
    shared: wire.shared ?? false,
    description: wire.description,
    databaseType: wire.databaseType,
    tags: wire.tags ?? undefined,
    folder: wire.folder,
    ...(wire.sharedPath ? { sharedPath: wire.sharedPath } : {}),
  };
}

export function queryVersionFromWire(wire: WireQueryVersion): QueryVersion {
  return {
    id: wire.id,
    queryId: wire.queryId,
    version: wire.version,
    snapshot: wire.snapshot,
    diff: wire.diff,
    createdAt: new Date(wire.createdAt),
  };
}

/** What a connection form (`ConnectionInput`) edits. */
export interface ConnectionFields {
  name: string;
  type: DatabaseConnection["type"];
  host?: string;
  port?: number;
  databaseName?: string;
  username?: string;
  sslMode?: string;
  connectionString?: string;
  sshTunnel?: DatabaseConnection["sshTunnel"];
  savePassword?: boolean;
  saveSshPassword?: boolean;
  saveSshKeyPassphrase?: boolean;
  aiShareSchema?: boolean;
  aiShareData?: boolean;
}

/** A text field as stored: `""` and absent are one (no SSL mode, no string). */
const opt = (s: string | undefined) => (s ? s : undefined);
const sameTunnel = (a: ConnectionFields["sshTunnel"], b: ConnectionFields["sshTunnel"]) =>
  JSON.stringify(a?.enabled ? a : null) === JSON.stringify(b?.enabled ? b : null);

/**
 * The patch that turns `before` into `after`: only the fields that differ.
 * The AI flags are compared only when `after` carries their keys (a form
 * that has them sends `undefined` for "follow the global setting", which
 * clears); a caller that leaves them out keeps them.
 */
export function connectionPatch(
  before: ConnectionFields,
  after: ConnectionFields,
): ConnectionPatch {
  const patch: ConnectionPatch = {};
  if (after.name !== before.name) patch.name = after.name;
  if (after.type !== before.type) patch.type = after.type;
  if ((after.host ?? "") !== (before.host ?? "")) patch.host = after.host ?? "";
  if ((after.port ?? 0) !== (before.port ?? 0)) patch.port = after.port ?? 0;
  if ((after.databaseName ?? "") !== (before.databaseName ?? "")) {
    patch.databaseName = after.databaseName ?? "";
  }
  if ((after.username ?? "") !== (before.username ?? "")) patch.username = after.username ?? "";
  if (opt(after.sslMode) !== opt(before.sslMode)) patch.sslMode = opt(after.sslMode) ?? null;
  if (opt(after.connectionString) !== opt(before.connectionString)) {
    patch.connectionString = opt(after.connectionString) ?? null;
  }
  if (!sameTunnel(after.sshTunnel, before.sshTunnel)) {
    patch.sshTunnel = after.sshTunnel?.enabled ? after.sshTunnel : null;
  }
  for (const flag of ["savePassword", "saveSshPassword", "saveSshKeyPassphrase"] as const) {
    if (after[flag] !== undefined && !!after[flag] !== !!before[flag]) patch[flag] = !!after[flag];
  }
  for (const flag of ["aiShareSchema", "aiShareData"] as const) {
    if (flag in after && after[flag] !== before[flag]) patch[flag] = after[flag] ?? null;
  }
  return patch;
}

/** What the save dialog and the managers change on a saved query. */
export type SavedQueryFields = Pick<
  Query,
  "name" | "query" | "parameters" | "description" | "databaseType" | "tags" | "folder"
> &
  Partial<Pick<Query, "starred" | "shared">>;

const sameJson = (a: unknown, b: unknown) =>
  JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

/** The patch that turns `before` into `after`: only the fields that differ. */
export function savedQueryPatch(
  before: SavedQueryFields,
  after: SavedQueryFields,
): SavedQueryPatch {
  const patch: SavedQueryPatch = {};
  if (after.name !== before.name) patch.name = after.name;
  if (after.query !== before.query) patch.query = after.query;
  if (!sameJson(after.parameters, before.parameters)) patch.parameters = after.parameters ?? null;
  if ((after.description ?? "") !== (before.description ?? "")) {
    patch.description = after.description || null;
  }
  if ((after.databaseType ?? "") !== (before.databaseType ?? "")) {
    patch.databaseType = after.databaseType || null;
  }
  if (!sameJson(after.tags, before.tags)) patch.tags = after.tags ?? null;
  if ((after.folder ?? "") !== (before.folder ?? "")) patch.folder = after.folder || null;
  if (after.starred !== undefined && !!after.starred !== !!before.starred) {
    patch.starred = !!after.starred;
  }
  if (after.shared !== undefined && !!after.shared !== !!before.shared) {
    patch.shared = !!after.shared;
  }
  return patch;
}

/** Whether a patch changes anything. */
export function isEmptyPatch(patch: object): boolean {
  return Object.keys(patch).length === 0;
}

// -------- Phase 5d-2 --------

/** Stored JSON text, parsed; `fallback` when it doesn't parse. */
function parsed<T>(text: string | null | undefined, fallback: T): T {
  if (!text) return fallback;
  try {
    return JSON.parse(text) as T;
  } catch {
    return fallback;
  }
}

/**
 * A stored dashboard as the page shows it. `keep`: the page's copy, whose
 * widgets' run state (their rows, loading, error) survives for widgets
 * with the same id.
 */
export function dashboardFromWire(wire: WireDashboard, keep?: Dashboard): Dashboard {
  const stored = parsed<DashboardWidget[]>(wire.widgets, []);
  const runtime = new Map((keep?.widgets ?? []).map((w) => [w.id, w]));
  const widgets = (Array.isArray(stored) ? stored : []).map((w) => {
    const live = runtime.get(w.id);
    return live
      ? {
          ...w,
          result: live.result,
          isLoading: live.isLoading,
          error: live.error,
          lastRefreshed: live.lastRefreshed,
        }
      : w;
  });
  return {
    id: wire.id,
    name: wire.name,
    projectId: wire.projectId,
    widgets,
    viewport: parsed(wire.viewport, { x: 0, y: 0, zoom: 1 }),
    dateFilter: parsed(wire.dateFilter, null),
    shared: wire.shared ?? false,
    starred: wire.starred ?? false,
    description: wire.description,
    createdAt: new Date(wire.createdAt),
    updatedAt: new Date(wire.updatedAt),
    ...(wire.sharedPath ? { sharedPath: wire.sharedPath } : {}),
  };
}

/** A listed version (no snapshot) as the history shows it. */
export function dashboardVersionFromWire(wire: WireDashboardVersionMeta): DashboardVersion {
  return {
    id: wire.id,
    dashboardId: wire.dashboardId,
    version: wire.version,
    widgetCount: wire.widgetCount,
    createdAt: new Date(wire.createdAt),
  };
}

/**
 * A version fetched whole (`dashboardVersionGet`) with its snapshot
 * parsed, for the diff and restore; `null` when the snapshot isn't JSON.
 */
export function resolvedDashboardVersionFromWire(
  wire: WireDashboardVersion,
): ResolvedDashboardVersion | null {
  let dashboard: DashboardSnapshot;
  try {
    dashboard = JSON.parse(wire.snapshot) as DashboardSnapshot;
  } catch {
    return null;
  }
  if (!dashboard || typeof dashboard !== "object") return null;
  return {
    id: wire.id,
    dashboardId: wire.dashboardId,
    version: wire.version,
    dashboard,
    createdAt: new Date(wire.createdAt),
  };
}

/** A saved workflow as the sidebar lists it (`workflowsList`, no body). */
export function workflowSummaryFromWire(wire: WireWorkflowMeta): SavedWorkflowSummary {
  return {
    id: wire.id,
    projectId: wire.projectId,
    name: wire.name,
    createdAt: wire.createdAt,
    updatedAt: wire.updatedAt,
  };
}

export function chatFromWire(wire: WireChat): AIChat {
  return {
    id: wire.id,
    connectionId: wire.connectionId,
    title: wire.title,
    createdAt: new Date(wire.createdAt),
    updatedAt: new Date(wire.updatedAt),
  };
}

export function messageFromWire(wire: ChatMessages["messages"][number]): AIMessage {
  // A reply Core cut for its size (probe F2) ends with its note: shown as
  // the page's own wording instead.
  const { content, cut } =
    wire.role === "assistant" ? splitCutNote(wire.content) : { content: wire.content, cut: false };
  const message: AIMessage = {
    id: wire.id,
    chatId: wire.chatId,
    role: wire.role,
    content,
    timestamp: new Date(wire.timestamp),
    query: wire.query,
    dashboardId: wire.dashboardId,
  };
  // Q7: a reply's stored tool calls (`parts`, Decision 23) as its lines.
  const segments = segmentsFromParts(wire.parts);
  if (segments) message.segments = segments;
  if (cut) message.cut = true;
  return message;
}

/**
 * A message as `chatMessagesPut` sends it. The page's time is sent as ISO
 * text (a time that doesn't read as one as it was loaded).
 */
export function messageDraft(message: AIMessage, stored?: string): ChatMessageDraft {
  const time = message.timestamp;
  const draft: ChatMessageDraft = {
    id: message.id,
    role: message.role,
    content: message.cut ? message.content + REPLY_CUT_NOTE : message.content,
    timestamp: Number.isNaN(time.getTime()) ? (stored ?? "") : time.toISOString(),
  };
  if (message.query !== undefined) draft.query = message.query;
  if (message.dashboardId !== undefined) draft.dashboardId = message.dashboardId;
  return draft;
}
