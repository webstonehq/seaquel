/**
 * The library's wire rows (`LibraryService`) as the view models hold them,
 * and the patches the view models send: only the fields that changed
 * (Decision 2), so another window's edit to another field survives.
 */
import type { DatabaseConnection, Project, Query, QueryVersion } from "$lib/types";
import { DEFAULT_PROJECT_ID } from "$lib/types";
import type {
  ConnectionPatch,
  SavedQueryPatch,
  WireConnection,
  WireProject,
  WireQueryVersion,
  WireSavedQuery,
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
