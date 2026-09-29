/**
 * `TsLibrary`: the demo's `LibraryService` (phase 5d-1), Core's library
 * rules in TypeScript over the demo's sql.js file, until phase 8.
 *
 * It follows `seaquel_workspace::library` and Core's `library.rs` step for
 * step, so the library parity fixtures replay through it with exactly
 * `changes.json`'s differences (`ts-library.test.ts`):
 *
 * - ids (`conn-`, `project-`, `label-`, `saved-`, `ver-` and a v4 uuid) and
 *   times are made here, never by the managers;
 * - every call checks its input before it reads, then reads and checks what
 *   the rows say (a missing parent, `NAME_TAKEN` by `nameKey`, labels on a
 *   connection), and only then writes, in one transaction;
 * - patches keep what they leave out and clear `null`s;
 * - a changed saved query text appends a keyframe of the previous text and
 *   prunes back to the nearest keyframe;
 * - removal deletes by project id too (a beta-era file has no foreign key
 *   there), and a label's id from every connection;
 * - the demo has no keychain: `SecretChanges` are refused like the web
 *   workspace refuses them (`NOT_SUPPORTED`);
 * - the pre-5a connection strings are dropped once, before the first call,
 *   as Core's data step `drop_legacy_built_connection_strings` does.
 *
 * Calls run one at a time. The change sequence has one epoch per instance
 * and counts writes.
 */
import type { SqliteDatabase } from "$lib/storage/sqlite-types";
import {
  isLegacyBuiltString,
  stripConnectionStringSecrets,
} from "$lib/utils/connection-string-rules";
import { DEFAULT_PROJECT_ID, DEFAULT_PROJECT_NAME } from "$lib/types";
import type { PersistedQueryParameter } from "$lib/types/generated/PersistedQueryParameter";
import type { SSHTunnelConfig } from "$lib/types/generated/SSHTunnelConfig";
import {
  LibraryCallError,
  type ChangeSeq,
  type ConnectionDraft,
  type ConnectionLabel,
  type ConnectionPatch,
  type LabelDraft,
  type LabelPatch,
  type LabelRemoved,
  type LibraryService,
  type ProjectDraft,
  type ProjectPatch,
  type ProjectRemoved,
  type SavedQueryDraft,
  type SavedQueryPatch,
  type SavedQueryUpdated,
  type SecretChanges,
  type Seqd,
  type WireConnection,
  type WireProject,
  type WireQueryVersion,
  type WireSavedQuery,
} from "./types";

// ── Fixed values (as `seaquel_workspace::library`) ──

const ENGINE_TYPES = ["postgres", "mysql", "mariadb", "sqlite", "mssql", "duckdb"];
const PREDEFINED_LABEL_IDS = ["local", "staging", "prod"];
const PARAMETER_TYPES = ["number", "boolean", "text", "date", "datetime"];
const QUERY_VERSION_LIMIT_KEY = "query_version_limit";
const DEFAULT_VERSION_LIMIT = 100;
const INVALID_ARGUMENT = "INVALID_ARGUMENT";

type Row = Record<string, unknown>;
type Statement = { sql: string; params?: unknown[] };

// ── Names ──

/**
 * The key names are compared by: trimmed, NFC-normalised and fully case
 * folded (`Straße` is `STRASSE`), normalised again after the fold. The fold
 * is upper then lower case, which folds `ß` to `ss` as Unicode's full
 * folding does.
 */
export function nameKey(name: string): string {
  return name.trim().normalize("NFC").toUpperCase().toLowerCase().normalize("NFC");
}

/** `name`, else the first free `"<name> (n)"` against the `taken` keys. */
export function freeName(name: string, taken: Set<string>): string {
  if (!taken.has(nameKey(name))) return name;
  for (let n = 2; n <= taken.size + 2; n++) {
    const candidate = `${name} (${n})`;
    if (!taken.has(nameKey(candidate))) return candidate;
  }
  return `${name} (${taken.size + 2})`;
}

interface IdName {
  id: string;
  name: string;
}

function findTaken(name: string, others: IdName[], except?: string): string | undefined {
  const key = nameKey(name);
  return others.find((o) => o.id !== except && nameKey(o.name) === key)?.id;
}

/** `name`, or `NAME_TAKEN`; with `rename`, the first free name instead. */
function resolveName(
  what: string,
  name: string,
  rows: IdName[],
  rename: boolean,
  except?: string,
): string {
  const others = rows.filter((r) => r.id !== except);
  if (rename) return freeName(name, new Set(others.map((r) => nameKey(r.name))));
  const taken = findTaken(name, others);
  if (taken !== undefined) {
    throw new LibraryCallError("NAME_TAKEN", `Another ${what} here already has this name.`, taken);
  }
  return name;
}

// ── Versions ──

export interface VersionMeta {
  id: string;
  version: number;
  keyframe: boolean;
}

/**
 * The ids to delete so the newest `keep` versions stay, with every older one
 * back to the nearest keyframe. `keep` 0 keeps everything.
 */
export function versionPrune(versions: VersionMeta[], keep: number): string[] {
  if (keep === 0 || versions.length <= keep) return [];
  const ordered = [...versions].sort((a, b) => a.version - b.version);
  let first = ordered.length - keep;
  while (first > 0 && !ordered[first].keyframe) first--;
  return ordered.slice(0, first).map((v) => v.id);
}

/** `query_version_limit` as `parseInt` read it: 100 when unset, unreadable or negative. */
export function parseVersionLimit(value: string | null | undefined): number {
  if (value === null || value === undefined) return DEFAULT_VERSION_LIMIT;
  const m = /^\s*([+-]?)(\d+)/.exec(value);
  if (!m) return DEFAULT_VERSION_LIMIT;
  if (m[1] === "-") return /^0+$/.test(m[2]) ? 0 : DEFAULT_VERSION_LIMIT;
  const n = Number(m[2]);
  return Number.isFinite(n) ? Math.min(n, 0xffffffff) : 0xffffffff;
}

// ── Checks ──

function invalid(message: string): LibraryCallError {
  return new LibraryCallError(INVALID_ARGUMENT, message);
}

/** No NUL in any string `value` holds (a draft, a patch). */
function noNul(value: unknown): void {
  if (typeof value === "string") {
    if (value.includes("\u0000")) throw invalid("A value can't contain a NUL character.");
  } else if (Array.isArray(value)) {
    value.forEach(noNul);
  } else if (value && typeof value === "object") {
    Object.values(value).forEach(noNul);
  }
}

function checkName(name: unknown, what: string): void {
  if (typeof name !== "string") throw invalid(`The ${what} name must be text.`);
  noNul(name);
  if (name.trim() === "") throw invalid(`The ${what} name can't be empty.`);
}

function checkPort(port: unknown, what: string): void {
  const ok =
    typeof port === "number" &&
    Number.isFinite(port) &&
    Number.isInteger(port) &&
    port >= 0 &&
    port <= 65535;
  if (!ok) throw invalid(`The ${what} must be a whole number from 0 to 65535.`);
}

function checkType(ty: unknown): void {
  if (typeof ty !== "string" || !ENGINE_TYPES.includes(ty)) {
    throw invalid("The connection type must be postgres, mysql, mariadb, sqlite, mssql or duckdb.");
  }
}

function checkTunnel(t: SSHTunnelConfig | null | undefined): void {
  if (t) checkPort(t.port, "SSH port");
}

function isPredefinedLabel(id: string): boolean {
  return PREDEFINED_LABEL_IDS.includes(id);
}

function checkLabels(ids: string[], custom: ConnectionLabel[]): void {
  const known = new Set(custom.map((l) => l.id));
  if (ids.some((id) => !isPredefinedLabel(id) && !known.has(id))) {
    throw new LibraryCallError(
      "LABEL_NOT_FOUND",
      "A label on the connection isn't one of this project's labels.",
    );
  }
}

function checkColour(color: unknown): void {
  if (typeof color !== "string" || !/^#[0-9a-fA-F]{6}$/.test(color)) {
    throw invalid("A label colour must be written #rrggbb.");
  }
}

function checkCustomLabelId(id: string): void {
  noNul(id);
  if (isPredefinedLabel(id)) throw invalid("The predefined labels can't be changed or removed.");
}

function checkParameters(params: PersistedQueryParameter[]): void {
  const names = new Set<string>();
  for (const p of params) {
    if (!PARAMETER_TYPES.includes(p.type)) {
      throw invalid("A parameter's type must be number, boolean, text, date or datetime.");
    }
    if (names.has(p.name)) throw invalid("Two parameters of the query have the same name.");
    names.add(p.name);
  }
}

/** The demo has no keychain: any secret change is refused, as on web. */
function checkNoSecrets(secrets: SecretChanges | undefined): void {
  if (!secrets) return;
  if (secrets.db !== undefined || secrets.ssh !== undefined || secrets.sshKey !== undefined) {
    throw new LibraryCallError(
      "NOT_SUPPORTED",
      "Saving passwords with the connection isn't available here.",
    );
  }
}

// ── Stored values ──

/** A column as text; SQLite gives only strings, numbers, bytes or NULL. */
function optText(v: unknown): string | undefined {
  if (v === null || v === undefined) return undefined;
  if (typeof v === "string") return v;
  if (typeof v === "number" || typeof v === "bigint") return String(v);
  return new TextDecoder().decode(v as Uint8Array);
}

/** `optText`, with NULL as `""`. */
function text(v: unknown): string {
  return optText(v) ?? "";
}

const isOne = (v: unknown) => v === 1;

/** Stored JSON: absent when NULL, empty or not JSON; kept as its text. */
function jsonText(v: unknown): string | undefined {
  if (typeof v !== "string" || v === "") return undefined;
  try {
    JSON.parse(v);
    return v;
  } catch {
    return undefined;
  }
}

/** `value ? JSON.stringify(value) : null` on stored JSON text. */
function truthyJson(t: string | undefined): string | null {
  if (t === undefined) return null;
  const falsy =
    t === "null" || t === "false" || t === '""' || (/^[-+0-9.eE]+$/.test(t) && !Number(t));
  return falsy ? null : t;
}

/** An SSH tunnel as Rust serializes its struct: fixed key order, `keyPath` only when set. */
function tunnelJson(t: SSHTunnelConfig): string {
  const out: Record<string, unknown> = {
    enabled: t.enabled,
    host: t.host,
    port: t.port,
    username: t.username,
    authMethod: t.authMethod,
  };
  if (t.keyPath !== undefined && t.keyPath !== null) out.keyPath = t.keyPath;
  return JSON.stringify(out);
}

function parametersJson(params: PersistedQueryParameter[]): string {
  return JSON.stringify(
    params.map((p) => {
      const out: Record<string, unknown> = { name: p.name, type: p.type };
      if (p.defaultValue !== undefined && p.defaultValue !== null) {
        out.defaultValue = p.defaultValue;
      }
      if (p.description !== undefined && p.description !== null) out.description = p.description;
      return out;
    }),
  );
}

function dedup(ids: string[]): string[] {
  return [...new Set(ids)];
}

/** A connection as its columns hold it (JSON as text). */
interface ConnRow {
  id: string;
  project_id: string;
  name: string;
  type: string;
  host: string;
  port: number;
  database_name: string;
  username: string;
  ssl_mode: string | null;
  connection_string: string | null;
  last_connected: string | null;
  ssh_tunnel: string | null;
  save_password: boolean;
  save_ssh_password: boolean;
  save_ssh_key_passphrase: boolean;
  is_local_only: boolean;
  shared_connection_id: string | null;
  ai_share_schema: boolean | null;
  ai_share_data: boolean | null;
  active_ai_provider_id: string | null;
  active_ai_model: string | null;
  label_ids: string[];
}

function connRowFrom(row: Row, labelIds: string[]): ConnRow {
  const flag = (v: unknown) => (v === null || v === undefined ? null : isOne(v));
  return {
    id: text(row.id),
    project_id: text(row.project_id),
    name: text(row.name),
    type: text(row.type),
    host: text(row.host),
    port: Number(row.port ?? 0),
    database_name: text(row.database_name),
    username: text(row.username),
    ssl_mode: optText(row.ssl_mode) ?? null,
    connection_string: optText(row.connection_string) ?? null,
    last_connected: optText(row.last_connected) || null,
    ssh_tunnel: jsonText(row.ssh_tunnel) ?? null,
    save_password: isOne(row.save_password),
    save_ssh_password: isOne(row.save_ssh_password),
    save_ssh_key_passphrase: isOne(row.save_ssh_key_passphrase),
    is_local_only: isOne(row.is_local_only),
    shared_connection_id: optText(row.shared_connection_id) ?? null,
    ai_share_schema: flag(row.ai_share_schema),
    ai_share_data: flag(row.ai_share_data),
    active_ai_provider_id: optText(row.active_ai_provider_id) ?? null,
    active_ai_model: optText(row.active_ai_model) ?? null,
    label_ids: labelIds,
  };
}

function connectionToWire(c: ConnRow): WireConnection {
  const w: Record<string, unknown> = {
    id: c.id,
    projectId: c.project_id,
    name: c.name,
    type: c.type,
    host: c.host,
    port: c.port,
    databaseName: c.database_name,
    username: c.username,
  };
  if (c.ssl_mode !== null) w.sslMode = c.ssl_mode;
  if (c.connection_string !== null) w.connectionString = c.connection_string;
  if (c.last_connected) w.lastConnected = c.last_connected;
  if (c.ssh_tunnel !== null) w.sshTunnel = JSON.parse(c.ssh_tunnel);
  w.savePassword = c.save_password;
  w.saveSshPassword = c.save_ssh_password;
  w.saveSshKeyPassphrase = c.save_ssh_key_passphrase;
  w.labelIds = c.label_ids;
  if (c.is_local_only) w.isLocalOnly = true;
  if (c.shared_connection_id !== null) w.sharedConnectionId = c.shared_connection_id;
  if (c.ai_share_schema !== null) w.aiShareSchema = c.ai_share_schema;
  if (c.ai_share_data !== null) w.aiShareData = c.ai_share_data;
  if (c.active_ai_provider_id !== null) w.activeAIProviderId = c.active_ai_provider_id;
  if (c.active_ai_model !== null) w.activeAIModel = c.active_ai_model;
  return w as WireConnection;
}

/** The statements that write `c` (insert or update) and replace its labels. */
function connectionWrites(c: ConnRow, insert: boolean): Statement[] {
  const stored = c.connection_string
    ? (stripConnectionStringSecrets(c.connection_string) ?? null)
    : null;
  const bit = (b: boolean) => (b ? 1 : 0);
  const optBit = (b: boolean | null) => (b === null ? null : bit(b));
  const values = [
    c.name,
    c.type,
    c.host,
    c.port,
    c.database_name,
    c.username,
    c.ssl_mode,
    stored,
    c.last_connected,
    truthyJson(c.ssh_tunnel ?? undefined),
    bit(c.save_password),
    bit(c.save_ssh_password),
    bit(c.save_ssh_key_passphrase),
    bit(c.is_local_only),
    c.shared_connection_id,
    optBit(c.ai_share_schema),
    optBit(c.ai_share_data),
    c.active_ai_provider_id,
    c.active_ai_model,
  ];
  const cols = [
    "name",
    "type",
    "host",
    "port",
    "database_name",
    "username",
    "ssl_mode",
    "connection_string",
    "last_connected",
    "ssh_tunnel",
    "save_password",
    "save_ssh_password",
    "save_ssh_key_passphrase",
    "is_local_only",
    "shared_connection_id",
    "ai_share_schema",
    "ai_share_data",
    "active_ai_provider_id",
    "active_ai_model",
  ];
  const write: Statement = insert
    ? {
        sql: `INSERT INTO connections (id, project_id, ${cols.join(", ")}) VALUES (${[
          "?",
          "?",
          ...cols.map(() => "?"),
        ].join(", ")})`,
        params: [c.id, c.project_id, ...values],
      }
    : {
        sql: `UPDATE connections SET ${cols.map((col) => `${col} = ?`).join(", ")} WHERE id = ?`,
        params: [...values, c.id],
      };
  return [
    write,
    { sql: "DELETE FROM connection_labels WHERE connection_id = ?", params: [c.id] },
    ...dedup(c.label_ids).map((label) => ({
      sql: "INSERT INTO connection_labels (connection_id, label_id) VALUES (?, ?)",
      params: [c.id, label],
    })),
  ];
}

interface SavedRow {
  id: string;
  project_id: string;
  name: string;
  query: string;
  parameters: string | null;
  starred: boolean;
  shared: boolean;
  description: string | null;
  database_type: string | null;
  tags: string | null;
  folder: string | null;
  created_at: string;
  updated_at: string;
}

function savedRowFrom(row: Row): SavedRow {
  return {
    id: text(row.id),
    project_id: text(row.project_id),
    name: text(row.name),
    query: text(row.query),
    parameters: jsonText(row.parameters) ?? null,
    starred: isOne(row.starred),
    shared: isOne(row.shared),
    description: optText(row.description) ?? null,
    database_type: optText(row.database_type) ?? null,
    tags: jsonText(row.tags) ?? null,
    folder: optText(row.folder) ?? null,
    created_at: text(row.created_at),
    updated_at: text(row.updated_at),
  };
}

function savedQueryToWire(q: SavedRow): WireSavedQuery {
  const w: Record<string, unknown> = {
    id: q.id,
    name: q.name,
    query: q.query,
    projectId: q.project_id,
    createdAt: q.created_at,
    updatedAt: q.updated_at,
  };
  if (q.parameters !== null) w.parameters = JSON.parse(q.parameters);
  w.starred = q.starred;
  w.shared = q.shared;
  if (q.description !== null) w.description = q.description;
  if (q.database_type !== null) w.databaseType = q.database_type;
  if (q.tags !== null) w.tags = JSON.parse(q.tags);
  if (q.folder !== null) w.folder = q.folder;
  return w as WireSavedQuery;
}

function versionToWire(row: Row): WireQueryVersion {
  return {
    id: text(row.id),
    queryId: text(row.saved_query_id),
    version: Number(row.version ?? 0),
    snapshot: optText(row.snapshot) ?? null,
    diff: optText(row.diff) ?? null,
    createdAt: text(row.created_at),
  };
}

function projectToWire(row: Row, labels: ConnectionLabel[]): WireProject {
  const w: Record<string, unknown> = { id: text(row.id), name: text(row.name) };
  const description = optText(row.description);
  if (description !== undefined) w.description = description;
  w.createdAt = text(row.created_at);
  w.updatedAt = text(row.updated_at);
  w.customLabels = labels;
  const git = optText(row.git_repo_path);
  if (git !== undefined) w.gitRepoPath = git;
  return w as WireProject;
}

/** The user in a URL string, for a row whose `username` is empty (the data step). */
function usernameFromString(s: string): string {
  const t = s.replace("postgresql://", "postgres://");
  if (t.startsWith("sqlite") || t.startsWith("duckdb")) return "";
  try {
    const url = new URL(t);
    if (!url.username) return "";
    return decodeURIComponent(url.username);
  } catch {
    return "";
  }
}

// ── The library ──

export interface TsLibraryOptions {
  now?: () => Date;
  epoch?: string;
}

export class TsLibrary implements LibraryService {
  private readonly now: () => Date;
  private readonly epoch: string;
  private n = 0;
  private queue: Promise<unknown> = Promise.resolve();
  private upgraded = false;

  constructor(
    private readonly db: SqliteDatabase,
    options: TsLibraryOptions = {},
  ) {
    this.now = options.now ?? (() => new Date());
    this.epoch = options.epoch ?? crypto.randomUUID();
  }

  // -------- Plumbing --------

  /** Runs `fn` after every earlier call, with the data step done first. */
  private run<T>(fn: () => Promise<T>): Promise<T> {
    const next = this.queue.then(async () => {
      if (!this.upgraded) {
        await this.dropLegacyStrings();
        this.upgraded = true;
      }
      return fn();
    });
    this.queue = next.catch(() => {});
    return next;
  }

  private seq(): ChangeSeq {
    return { epoch: this.epoch, n: this.n };
  }

  /** Writes `statements` in one transaction and takes the next change number. */
  private async write(statements: Statement[]): Promise<ChangeSeq> {
    if (statements.length > 0) await this.db.transaction(statements);
    this.n += 1;
    return this.seq();
  }

  private iso(): string {
    return this.now().toISOString();
  }

  private newId(prefix: string): string {
    return `${prefix}${crypto.randomUUID()}`;
  }

  /** Core's `drop_legacy_built_connection_strings`. */
  private async dropLegacyStrings(): Promise<void> {
    const rows = await this.db.query<Row>(
      `SELECT rowid AS rid, type, host, port, database_name, username, ssl_mode, connection_string
       FROM connections WHERE typeof(connection_string) = 'text' AND connection_string <> ''`,
    );
    const statements: Statement[] = [];
    for (const row of rows) {
      const s = text(row.connection_string);
      const storedUser = text(row.username);
      const username = storedUser === "" ? usernameFromString(s) : storedUser;
      const legacy = isLegacyBuiltString(s, {
        type: text(row.type),
        host: text(row.host),
        port: Number(row.port ?? 0),
        databaseName: text(row.database_name),
        username,
        sslMode: optText(row.ssl_mode),
      });
      if (!legacy) continue;
      statements.push(
        username === storedUser
          ? {
              sql: "UPDATE connections SET connection_string = NULL WHERE rowid = ?",
              params: [row.rid],
            }
          : {
              sql: "UPDATE connections SET connection_string = NULL, username = ? WHERE rowid = ?",
              params: [username, row.rid],
            },
      );
    }
    if (statements.length > 0) await this.db.transaction(statements);
  }

  private async connectionLabels(id: string): Promise<string[]> {
    const rows = await this.db.query<Row>(
      "SELECT label_id FROM connection_labels WHERE connection_id = ?",
      [id],
    );
    return rows.map((r) => text(r.label_id));
  }

  private async getConnection(id: string): Promise<ConnRow | null> {
    const [row] = await this.db.query<Row>("SELECT * FROM connections WHERE id = ?", [id]);
    return row ? connRowFrom(row, await this.connectionLabels(id)) : null;
  }

  private async projectLabels(projectId: string): Promise<ConnectionLabel[]> {
    const rows = await this.db.query<Row>(
      "SELECT id, name, is_predefined, color FROM project_labels WHERE project_id = ?",
      [projectId],
    );
    return rows.map((l) => ({
      id: text(l.id),
      name: text(l.name),
      isPredefined: isOne(l.is_predefined),
      color: text(l.color),
    }));
  }

  private async getProject(id: string): Promise<WireProject | null> {
    const [row] = await this.db.query<Row>("SELECT * FROM projects WHERE id = ?", [id]);
    return row ? projectToWire(row, await this.projectLabels(id)) : null;
  }

  private async projectOrFail(id: string): Promise<WireProject> {
    const project = await this.getProject(id);
    if (!project) throw new LibraryCallError("PROJECT_NOT_FOUND", "Project not found.");
    return project;
  }

  private async idNames(sql: string, params: unknown[]): Promise<IdName[]> {
    const rows = await this.db.query<Row>(sql, params);
    return rows.map((r) => ({ id: text(r.id), name: text(r.name) }));
  }

  private async getSavedQuery(id: string): Promise<SavedRow | null> {
    const [row] = await this.db.query<Row>("SELECT * FROM saved_queries WHERE id = ?", [id]);
    return row ? savedRowFrom(row) : null;
  }

  private namesInFolder(projectId: string, folder: string | null): Promise<IdName[]> {
    return this.idNames(
      `SELECT id, name FROM saved_queries
       WHERE project_id = ? AND COALESCE(folder, '') = COALESCE(?, '') ORDER BY rowid`,
      [projectId, folder],
    );
  }

  // -------- Reads --------

  listConnections(): Promise<Seqd<WireConnection[]>> {
    return this.run(async () => {
      const seq = this.seq();
      const rows = await this.db.query<Row>("SELECT * FROM connections");
      const value: WireConnection[] = [];
      for (const row of rows) {
        value.push(connectionToWire(connRowFrom(row, await this.connectionLabels(text(row.id)))));
      }
      return { value, seq };
    });
  }

  listProjects(): Promise<Seqd<WireProject[]>> {
    return this.run(async () => ({ value: await this.loadProjects(), seq: this.seq() }));
  }

  private async loadProjects(): Promise<WireProject[]> {
    const rows = await this.db.query<Row>("SELECT * FROM projects");
    const out: WireProject[] = [];
    for (const row of rows) out.push(projectToWire(row, await this.projectLabels(text(row.id))));
    return out;
  }

  listSavedQueries(projectId: string): Promise<Seqd<WireSavedQuery[]>> {
    return this.run(async () => {
      const seq = this.seq();
      const rows = await this.db.query<Row>("SELECT * FROM saved_queries WHERE project_id = ?", [
        projectId,
      ]);
      return { value: rows.map((r) => savedQueryToWire(savedRowFrom(r))), seq };
    });
  }

  listQueryVersions(projectId: string): Promise<Seqd<WireQueryVersion[]>> {
    return this.run(async () => {
      const seq = this.seq();
      const rows = await this.db.query<Row>(
        `SELECT qv.* FROM query_versions qv
         JOIN saved_queries sq ON sq.id = qv.saved_query_id
         WHERE sq.project_id = ?
         ORDER BY qv.saved_query_id, qv.version ASC`,
        [projectId],
      );
      return { value: rows.map(versionToWire), seq };
    });
  }

  // -------- Connections --------

  createConnection(draft: ConnectionDraft, secrets?: SecretChanges): Promise<Seqd<WireConnection>> {
    return this.run(async () => {
      noNul(draft);
      checkName(draft.name, "connection");
      checkType(draft.type);
      checkPort(draft.port, "port");
      checkTunnel(draft.sshTunnel);
      checkNoSecrets(secrets);
      const now = this.iso();
      const row: ConnRow = {
        id: this.newId("conn-"),
        project_id: draft.projectId,
        name: draft.name,
        type: draft.type,
        host: draft.host,
        port: draft.port,
        database_name: draft.databaseName,
        username: draft.username,
        ssl_mode: draft.sslMode ?? null,
        connection_string: draft.connectionString || null,
        last_connected: draft.connected ? now : null,
        ssh_tunnel: draft.sshTunnel ? tunnelJson(draft.sshTunnel) : null,
        save_password: !!draft.savePassword,
        save_ssh_password: !!draft.saveSshPassword,
        save_ssh_key_passphrase: !!draft.saveSshKeyPassphrase,
        is_local_only: draft.isLocalOnly === true,
        shared_connection_id: draft.sharedConnectionId ?? null,
        ai_share_schema: draft.aiShareSchema ?? null,
        ai_share_data: draft.aiShareData ?? null,
        active_ai_provider_id: draft.activeAIProviderId ?? null,
        active_ai_model: draft.activeAIModel ?? null,
        label_ids: dedup(draft.labelIds ?? []),
      };
      const project = await this.projectOrFail(row.project_id);
      checkLabels(row.label_ids, project.customLabels);
      const names = await this.idNames(
        "SELECT id, name FROM connections WHERE project_id = ? ORDER BY rowid",
        [row.project_id],
      );
      row.name = resolveName("connection", row.name, names, !!draft.renameIfTaken);
      const seq = await this.write(connectionWrites(row, true));
      const stored = await this.getConnection(row.id);
      return { value: connectionToWire(stored!), seq };
    });
  }

  updateConnection(
    id: string,
    patch: ConnectionPatch,
    secrets?: SecretChanges,
  ): Promise<Seqd<WireConnection>> {
    return this.run(async () => {
      noNul(id);
      noNul(patch);
      if (patch.name !== undefined) checkName(patch.name, "connection");
      if (patch.type !== undefined) checkType(patch.type);
      if (patch.port !== undefined) checkPort(patch.port, "port");
      checkTunnel(patch.sshTunnel);
      checkNoSecrets(secrets);
      const now = this.iso();
      const before = await this.getConnection(id);
      if (!before) {
        throw new LibraryCallError("CONNECTION_NOT_FOUND", "Saved connection not found.");
      }
      const row: ConnRow = { ...before, label_ids: [...before.label_ids] };
      if (patch.name !== undefined) row.name = patch.name;
      if (patch.type !== undefined) row.type = patch.type;
      if (patch.host !== undefined) row.host = patch.host;
      if (patch.port !== undefined) row.port = patch.port;
      if (patch.databaseName !== undefined) row.database_name = patch.databaseName;
      if (patch.username !== undefined) row.username = patch.username;
      if (patch.sslMode !== undefined) row.ssl_mode = patch.sslMode;
      if (patch.connectionString !== undefined) {
        row.connection_string = patch.connectionString || null;
      }
      if (patch.sshTunnel !== undefined) {
        row.ssh_tunnel = patch.sshTunnel ? tunnelJson(patch.sshTunnel) : null;
      }
      if (patch.savePassword !== undefined) row.save_password = patch.savePassword;
      if (patch.saveSshPassword !== undefined) row.save_ssh_password = patch.saveSshPassword;
      if (patch.saveSshKeyPassphrase !== undefined) {
        row.save_ssh_key_passphrase = patch.saveSshKeyPassphrase;
      }
      if (patch.labelIds !== undefined) row.label_ids = dedup(patch.labelIds);
      if (patch.isLocalOnly !== undefined) row.is_local_only = patch.isLocalOnly;
      if (patch.aiShareSchema !== undefined) row.ai_share_schema = patch.aiShareSchema;
      if (patch.aiShareData !== undefined) row.ai_share_data = patch.aiShareData;
      if (patch.activeAIProviderId !== undefined) {
        row.active_ai_provider_id = patch.activeAIProviderId;
      }
      if (patch.activeAIModel !== undefined) row.active_ai_model = patch.activeAIModel;
      if (patch.connected) row.last_connected = now;

      if (patch.labelIds !== undefined) {
        checkLabels(row.label_ids, await this.projectLabels(row.project_id));
      }
      if (patch.name !== undefined && nameKey(row.name) !== nameKey(before.name)) {
        const names = await this.idNames(
          "SELECT id, name FROM connections WHERE project_id = ? ORDER BY rowid",
          [row.project_id],
        );
        resolveName("connection", row.name, names, false, id);
      }
      const seq = await this.write(connectionWrites(row, false));
      const stored = await this.getConnection(id);
      return { value: connectionToWire(stored!), seq };
    });
  }

  /**
   * The demo's own connection (`demo-connection`, one of the two fixed ids,
   * Q2): stored the first time with `draft`'s fields, then only marked
   * connected, so the user's edits to it (labels, AI model) stay. The
   * history and chats of the demo connection need its row.
   */
  putDemoConnection(id: string, draft: ConnectionDraft): Promise<Seqd<WireConnection>> {
    return this.run(async () => {
      const before = await this.getConnection(id);
      const now = this.iso();
      if (before) {
        const row: ConnRow = { ...before, label_ids: [...before.label_ids], last_connected: now };
        const seq = await this.write(connectionWrites(row, false));
        return { value: connectionToWire((await this.getConnection(id))!), seq };
      }
      await this.projectOrFail(draft.projectId);
      const row: ConnRow = {
        id,
        project_id: draft.projectId,
        name: draft.name,
        type: draft.type,
        host: draft.host,
        port: draft.port,
        database_name: draft.databaseName,
        username: draft.username,
        ssl_mode: null,
        connection_string: null,
        last_connected: now,
        ssh_tunnel: null,
        save_password: false,
        save_ssh_password: false,
        save_ssh_key_passphrase: false,
        is_local_only: false,
        shared_connection_id: null,
        ai_share_schema: null,
        ai_share_data: null,
        active_ai_provider_id: null,
        active_ai_model: null,
        label_ids: dedup(draft.labelIds ?? []),
      };
      const seq = await this.write(connectionWrites(row, true));
      return { value: connectionToWire((await this.getConnection(id))!), seq };
    });
  }

  removeConnection(id: string): Promise<Seqd<null>> {
    return this.run(async () => {
      noNul(id);
      if (!(await this.getConnection(id))) {
        throw new LibraryCallError("CONNECTION_NOT_FOUND", "Saved connection not found.");
      }
      const seq = await this.write([
        { sql: "DELETE FROM connections WHERE id = ?", params: [id] },
        {
          sql: "DELETE FROM user_credentials WHERE key = ? AND scope IN ('db', 'ssh', 'ssh-key')",
          params: [id],
        },
      ]);
      return { value: null, seq };
    });
  }

  // -------- Projects --------

  createProject(draft: ProjectDraft): Promise<Seqd<WireProject>> {
    return this.run(async () => {
      noNul(draft);
      checkName(draft.name, "project");
      const now = this.iso();
      const id = this.newId("project-");
      const names = await this.idNames("SELECT id, name FROM projects ORDER BY rowid", []);
      const name = resolveName("project", draft.name, names, !!draft.renameIfTaken);
      const seq = await this.write([
        {
          sql: `INSERT INTO projects (id, name, description, created_at, updated_at, git_repo_path)
                VALUES (?, ?, ?, ?, ?, NULL)`,
          params: [id, name, draft.description ?? null, now, now],
        },
      ]);
      return { value: (await this.getProject(id))!, seq };
    });
  }

  ensureDefaultProject(): Promise<Seqd<WireProject[]>> {
    return this.run(async () => {
      const [{ n }] = await this.db.query<{ n: number }>("SELECT COUNT(*) AS n FROM projects");
      let seq = this.seq();
      if (Number(n) === 0) {
        const now = this.iso();
        seq = await this.write([
          {
            sql: `INSERT INTO projects (id, name, description, created_at, updated_at, git_repo_path)
                  VALUES (?, ?, NULL, ?, ?, NULL) ON CONFLICT(id) DO NOTHING`,
            params: [DEFAULT_PROJECT_ID, DEFAULT_PROJECT_NAME, now, now],
          },
        ]);
      }
      return { value: await this.loadProjects(), seq };
    });
  }

  updateProject(id: string, patch: ProjectPatch): Promise<Seqd<WireProject>> {
    return this.run(async () => {
      noNul(id);
      noNul(patch);
      if (patch.name !== undefined) checkName(patch.name, "project");
      const now = this.iso();
      const row = await this.projectOrFail(id);
      const before = row.name;
      const name = patch.name ?? row.name;
      const description = patch.description !== undefined ? patch.description : row.description;
      const git = patch.gitRepoPath !== undefined ? patch.gitRepoPath : row.gitRepoPath;
      if (patch.name !== undefined && nameKey(name) !== nameKey(before)) {
        const names = await this.idNames("SELECT id, name FROM projects ORDER BY rowid", []);
        resolveName("project", name, names, false, id);
      }
      const seq = await this.write([
        {
          sql: "UPDATE projects SET name = ?, description = ?, updated_at = ?, git_repo_path = ? WHERE id = ?",
          params: [name, description ?? null, now, git ?? null, id],
        },
      ]);
      return { value: (await this.getProject(id))!, seq };
    });
  }

  removeProject(id: string): Promise<Seqd<ProjectRemoved>> {
    return this.run(async () => {
      noNul(id);
      await this.projectOrFail(id);
      const [{ n }] = await this.db.query<{ n: number }>("SELECT COUNT(*) AS n FROM projects");
      if (Number(n) <= 1) {
        throw new LibraryCallError("LAST_PROJECT", "The last project can't be removed.");
      }
      const connections = await this.db.query<Row>(
        "SELECT id FROM connections WHERE project_id = ? AND id IS NOT NULL ORDER BY rowid",
        [id],
      );
      const connectionIds = connections.map((r) => text(r.id));
      const seq = await this.write([
        // A beta-era file has no foreign key on these (Decision 9).
        { sql: "DELETE FROM saved_queries WHERE project_id = ?", params: [id] },
        { sql: "DELETE FROM dashboards WHERE project_id = ?", params: [id] },
        { sql: "DELETE FROM saved_canvases WHERE project_id = ?", params: [id] },
        { sql: "DELETE FROM projects WHERE id = ?", params: [id] },
        ...connectionIds.map((c) => ({
          sql: "DELETE FROM user_credentials WHERE key = ? AND scope IN ('db', 'ssh', 'ssh-key')",
          params: [c],
        })),
      ]);
      return { value: { connectionIds }, seq };
    });
  }

  // -------- Custom labels --------

  createLabel(projectId: string, label: LabelDraft): Promise<Seqd<ConnectionLabel>> {
    return this.run(async () => {
      noNul(projectId);
      noNul(label);
      checkName(label.name, "label");
      checkColour(label.color);
      const project = await this.projectOrFail(projectId);
      const name = resolveName("label", label.name, project.customLabels, false);
      const created: ConnectionLabel = {
        id: this.newId("label-"),
        name,
        isPredefined: false,
        color: label.color,
      };
      const seq = await this.write([
        {
          sql: `INSERT INTO project_labels (id, project_id, name, is_predefined, color)
                VALUES (?, ?, ?, 0, ?)`,
          params: [created.id, projectId, created.name, created.color],
        },
      ]);
      return { value: created, seq };
    });
  }

  updateLabel(
    projectId: string,
    labelId: string,
    patch: LabelPatch,
  ): Promise<Seqd<ConnectionLabel>> {
    return this.run(async () => {
      noNul(projectId);
      checkCustomLabelId(labelId);
      noNul(patch);
      if (patch.name !== undefined) checkName(patch.name, "label");
      if (patch.color !== undefined) checkColour(patch.color);
      const project = await this.projectOrFail(projectId);
      const found = project.customLabels.find((l) => l.id === labelId);
      if (!found) throw new LibraryCallError("LABEL_NOT_FOUND", "Label not found.");
      const label: ConnectionLabel = {
        ...found,
        name: patch.name ?? found.name,
        color: patch.color ?? found.color,
      };
      if (patch.name !== undefined && nameKey(label.name) !== nameKey(found.name)) {
        resolveName("label", label.name, project.customLabels, false, labelId);
      }
      const seq = await this.write([
        {
          sql: "UPDATE project_labels SET name = ?, color = ? WHERE id = ? AND project_id = ?",
          params: [label.name, label.color, labelId, projectId],
        },
      ]);
      return { value: label, seq };
    });
  }

  removeLabel(projectId: string, labelId: string): Promise<Seqd<LabelRemoved>> {
    return this.run(async () => {
      noNul(projectId);
      checkCustomLabelId(labelId);
      await this.projectOrFail(projectId);
      const [exists] = await this.db.query<Row>(
        "SELECT 1 AS one FROM project_labels WHERE id = ? AND project_id = ?",
        [labelId, projectId],
      );
      if (!exists) throw new LibraryCallError("LABEL_NOT_FOUND", "Label not found.");
      const had = await this.db.query<Row>(
        `SELECT c.id FROM connections c
         JOIN connection_labels l ON l.connection_id = c.id
         WHERE l.label_id = ? ORDER BY c.rowid`,
        [labelId],
      );
      const seq = await this.write([
        {
          sql: "DELETE FROM project_labels WHERE id = ? AND project_id = ?",
          params: [labelId, projectId],
        },
        { sql: "DELETE FROM connection_labels WHERE label_id = ?", params: [labelId] },
      ]);
      return { value: { connectionIds: had.map((r) => text(r.id)) }, seq };
    });
  }

  // -------- Saved queries --------

  createSavedQuery(draft: SavedQueryDraft): Promise<Seqd<WireSavedQuery>> {
    return this.run(async () => {
      noNul(draft);
      checkName(draft.name, "saved query");
      if (draft.parameters) checkParameters(draft.parameters);
      const now = this.iso();
      const row: SavedRow = {
        id: this.newId("saved-"),
        project_id: draft.projectId,
        name: draft.name,
        query: draft.query,
        parameters: draft.parameters ? parametersJson(draft.parameters) : null,
        starred: !!draft.starred,
        shared: !!draft.shared,
        description: draft.description ?? null,
        database_type: draft.databaseType ?? null,
        tags: draft.tags ? JSON.stringify(draft.tags) : null,
        folder: draft.folder ?? null,
        created_at: now,
        updated_at: now,
      };
      await this.projectOrFail(row.project_id);
      const names = await this.namesInFolder(row.project_id, row.folder);
      resolveName("saved query", row.name, names, false);
      const seq = await this.write([
        {
          sql: `INSERT INTO saved_queries (id, project_id, name, query, parameters, starred, shared,
                  description, database_type, tags, folder, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          params: [
            row.id,
            row.project_id,
            row.name,
            row.query,
            row.parameters,
            row.starred ? 1 : 0,
            row.shared ? 1 : 0,
            row.description,
            row.database_type,
            row.tags,
            row.folder,
            row.created_at,
            row.updated_at,
          ],
        },
      ]);
      return { value: savedQueryToWire((await this.getSavedQuery(row.id))!), seq };
    });
  }

  updateSavedQuery(id: string, patch: SavedQueryPatch): Promise<Seqd<SavedQueryUpdated>> {
    return this.run(async () => {
      noNul(id);
      noNul(patch);
      if (patch.name !== undefined) checkName(patch.name, "saved query");
      if (patch.parameters) checkParameters(patch.parameters);
      const now = this.iso();
      const before = await this.getSavedQuery(id);
      if (!before) throw new LibraryCallError("SAVED_QUERY_NOT_FOUND", "Saved query not found.");
      const row: SavedRow = { ...before };
      let renamed = false;
      let previousText: string | null = null;
      if (patch.name !== undefined) {
        renamed ||= nameKey(patch.name) !== nameKey(row.name);
        row.name = patch.name;
      }
      if (patch.query !== undefined && patch.query !== row.query) {
        previousText = row.query;
        row.query = patch.query;
      }
      if (patch.parameters !== undefined) {
        row.parameters = patch.parameters ? parametersJson(patch.parameters) : null;
      }
      if (patch.description !== undefined) row.description = patch.description;
      if (patch.databaseType !== undefined) row.database_type = patch.databaseType;
      if (patch.tags !== undefined) row.tags = patch.tags ? JSON.stringify(patch.tags) : null;
      if (patch.folder !== undefined) {
        renamed ||= (patch.folder ?? "") !== (row.folder ?? "");
        row.folder = patch.folder;
      }
      if (patch.starred !== undefined) row.starred = patch.starred;
      if (patch.shared !== undefined) row.shared = patch.shared;
      const onlyStarred =
        patch.name === undefined &&
        patch.query === undefined &&
        patch.parameters === undefined &&
        patch.description === undefined &&
        patch.databaseType === undefined &&
        patch.tags === undefined &&
        patch.folder === undefined &&
        patch.shared === undefined;
      if (!onlyStarred) row.updated_at = now;
      if (renamed) {
        const names = await this.namesInFolder(row.project_id, row.folder);
        resolveName("saved query", row.name, names, false, id);
      }

      const statements: Statement[] = [
        {
          sql: `UPDATE saved_queries SET name = ?, query = ?, parameters = ?, starred = ?, shared = ?,
                  description = ?, database_type = ?, tags = ?, folder = ?, updated_at = ? WHERE id = ?`,
          params: [
            row.name,
            row.query,
            row.parameters,
            row.starred ? 1 : 0,
            row.shared ? 1 : 0,
            row.description,
            row.database_type,
            row.tags,
            row.folder,
            row.updated_at,
            id,
          ],
        },
      ];
      let version: WireQueryVersion | null = null;
      let prunedVersionIds: string[] = [];
      if (previousText !== null) {
        const [{ highest }] = await this.db.query<{ highest: number | null }>(
          "SELECT MAX(version) AS highest FROM query_versions WHERE saved_query_id = ?",
          [id],
        );
        const number = highest === null ? 1 : Math.floor(Number(highest)) + 1;
        version = {
          id: this.newId("ver-"),
          queryId: id,
          version: number,
          snapshot: previousText,
          diff: null,
          createdAt: now,
        };
        statements.push({
          sql: `INSERT INTO query_versions (id, saved_query_id, version, snapshot, diff, created_at)
                VALUES (?, ?, ?, ?, NULL, ?)`,
          params: [version.id, id, number, previousText, now],
        });
        const [limit] = await this.db.query<Row>("SELECT value FROM app_state WHERE key = ?", [
          QUERY_VERSION_LIMIT_KEY,
        ]);
        const keep = parseVersionLimit(limit ? optText(limit.value) : undefined);
        const metas = await this.db.query<Row>(
          `SELECT id, version, snapshot IS NOT NULL AS keyframe FROM query_versions
           WHERE saved_query_id = ? ORDER BY version ASC`,
          [id],
        );
        const all: VersionMeta[] = [
          ...metas.map((m) => ({
            id: text(m.id),
            version: Number(m.version ?? 0),
            keyframe: isOne(m.keyframe),
          })),
          { id: version.id, version: number, keyframe: true },
        ];
        prunedVersionIds = versionPrune(all, keep);
        for (const pruned of prunedVersionIds) {
          statements.push({
            sql: "DELETE FROM query_versions WHERE saved_query_id = ? AND id = ?",
            params: [id, pruned],
          });
        }
      }
      const seq = await this.write(statements);
      const stored = await this.getSavedQuery(id);
      return {
        value: { query: savedQueryToWire(stored!), version, prunedVersionIds },
        seq,
      };
    });
  }

  removeSavedQuery(id: string): Promise<Seqd<null>> {
    return this.run(async () => {
      noNul(id);
      if (!(await this.getSavedQuery(id))) {
        throw new LibraryCallError("SAVED_QUERY_NOT_FOUND", "Saved query not found.");
      }
      const seq = await this.write([
        { sql: "DELETE FROM saved_queries WHERE id = ?", params: [id] },
      ]);
      return { value: null, seq };
    });
  }
}
