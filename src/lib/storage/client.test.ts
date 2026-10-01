/**
 * The two `StorageClient`s against the frozen Task 2 fixtures
 * (`crates/seaquel-storage/tests/fixtures/repos`).
 *
 * - `RustStorageClient` runs against a fake transport that replays each
 *   case: it checks every request the client sends (method, params, and the
 *   byte layout Rust needs) and answers with the recorded result in its wire
 *   form. So this covers the client's encoding and its mapping to the app's
 *   types (`lastConnected` as a `Date`, workflows through `toStorable`).
 * - `SqljsStorageClient` (the demo) runs every case for real on sql.js, and
 *   its stored rows must match the recorded ones too.
 *
 * Both give the recorded results, so they agree. The `demoDiffers` cases
 * agree as well now that `web-sqlite.ts` keeps foreign keys on after each
 * export.
 */

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import initSqlJs from "sql.js";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { PersistedQueryHistoryItem } from "$lib/types";
import { fromStorable, toStorable } from "$lib/values";
import type { StorageClient } from "./client";
import {
  CoreCallError,
  RustStorageClient,
  callSecret,
  type CoreTransport,
  type StorageMethod,
} from "./rust-client";
import { bootstrapSqljsDatabase, createSqljsStorageClient } from "./sqljs-client";
import { CoreSettings } from "$lib/hooks/database/library/core-settings";
import {
  aiChatsRepo,
  appStateRepo,
  connectionOverridesRepo,
  connectionsRepo,
  dashboardVersionsRepo,
  dashboardsRepo,
  importStateRepo,
  onboardingRepo,
  projectStateRepo,
  projectsRepo,
  queryVersionsRepo,
  savedQueriesRepo,
  themeRepo,
  tutorialRepo,
} from "./repository";
import { WebSqliteDatabase } from "./web-sqlite";

// -------- Fixtures --------

interface CallStep {
  call: string;
  args: unknown[];
  result?: unknown;
  error?: { message: string; code?: string };
}
interface SqlStep {
  sql: string;
  params?: unknown[];
}
type Step = CallStep | SqlStep;
interface TableRows {
  columns: string[];
  rows: unknown[][];
  types: string[][];
}
interface Case {
  name: string;
  steps: Step[];
  rows?: Record<string, TableRows>;
  demoDiffers?: unknown;
}

const FIXTURES = join(process.cwd(), "crates/seaquel-storage/tests/fixtures/repos");
const cases: Case[] = readdirSync(FIXTURES)
  .filter((f) => f.endsWith(".json"))
  .sort()
  .flatMap((f) => (JSON.parse(readFileSync(join(FIXTURES, f), "utf8")) as { cases: Case[] }).cases);

const isCall = (s: Step): s is CallStep => "call" in s;

/**
 * Calls the frozen fixtures make that the app's clients no longer have.
 * `queryHistoryRepo.replaceAll` went in phase 5b (history is appended, never
 * replaced); Rust keeps the function for `repos.rs`. Here the Rust client
 * skips it, and the demo's database gets its rows by plain SQL, so the loads
 * and rows after it are still checked.
 */
const RETIRED = new Set(["queryHistoryRepo.replaceAll"]);

/**
 * The storage group's library methods, which phase 5d-1 retired: Core's
 * `library` group replaced them, and neither client has them any more. The
 * Rust client's replay checks that; the demo's replay still runs them as
 * recorded on the sql.js repositories (the frozen fixtures pin those), so
 * the rows the other calls read and write are the same.
 */
const LIBRARY_RETIRED = new Set([
  "projectsRepo.loadAll",
  "projectsRepo.save",
  "projectsRepo.saveAll",
  "projectsRepo.remove",
  "connectionsRepo.loadAll",
  "connectionsRepo.save",
  "connectionsRepo.remove",
  "savedQueriesRepo.loadByProject",
  "savedQueriesRepo.saveAll",
  "savedQueriesRepo.removeByProject",
  "queryVersionsRepo.loadByQuery",
  "queryVersionsRepo.loadByProject",
  "queryVersionsRepo.insert",
  "queryVersionsRepo.pruneOldVersions",
  // Phase 5d-2 Task 6a: the project state is the `ui` group's now (a
  // window's view state, `UiService`), and left the storage client.
  "projectStateRepo.load",
  "projectStateRepo.save",
  "projectStateRepo.remove",
]);

/**
 * The repositories whose storage methods phase 5d-2 retired: the `library`,
 * `settings` and `ui` groups replaced them (Task 6b moved the GUI), and
 * connection overrides were retired (Q13). They are gone from both clients;
 * the demo's replay runs them through the repositories, which stay for the
 * frozen fixtures.
 */
const STATE_RETIRED_REPOS = new Set([
  "appState",
  "connectionOverrides",
  "themes",
  "onboarding",
  "tutorial",
  "importState",
  "dashboards",
  "dashboardVersions",
  "aiChats",
]);

/** What `replaceAll` did, as plain SQL, for the demo's replay. */
async function seedHistory(db: WebSqliteDatabase, step: CallStep): Promise<void> {
  const [connectionId, items] = step.args as [string, PersistedQueryHistoryItem[]];
  const cols = [
    "id",
    "query",
    "timestamp",
    "execution_time",
    "row_count",
    "connection_id",
    "favorite",
    "connection_labels_snapshot",
    "connection_name_snapshot",
  ];
  const statements = [
    {
      sql: "DELETE FROM query_history WHERE connection_id = ?",
      params: [connectionId] as unknown[],
    },
    ...items.map((h) => ({
      sql: `INSERT INTO query_history (${cols.join(", ")}) VALUES (${cols.map(() => "?").join(", ")})`,
      params: [
        h.id,
        h.query,
        h.timestamp,
        h.executionTime,
        h.rowCount,
        h.connectionId,
        h.favorite ? 1 : 0,
        h.connectionLabelsSnapshot == null ? null : JSON.stringify(h.connectionLabelsSnapshot),
        h.connectionNameSnapshot,
      ],
    })),
  ];
  const run = db.transaction(statements);
  if (step.error) await expect(run, step.call).rejects.toThrow();
  else await run;
}

/** Fixture repo name → `StorageClient` property and wire method prefix. */
const REPOS: Record<string, string> = {
  projectsRepo: "projects",
  appStateRepo: "appState",
  connectionsRepo: "connections",
  connectionOverridesRepo: "connectionOverrides",
  projectStateRepo: "projectState",
  savedQueriesRepo: "savedQueries",
  queryVersionsRepo: "queryVersions",
  queryHistoryRepo: "queryHistory",
  sharedReposRepo: "sharedRepos",
  themeRepo: "themes",
  licenseRepo: "license",
  onboardingRepo: "onboarding",
  tutorialRepo: "tutorial",
  importStateRepo: "importState",
  dashboardsRepo: "dashboards",
  dashboardVersionsRepo: "dashboardVersions",
  aiChatsRepo: "aiChats",
  vaultStateRepo: "vaultState",
  userCredentialsRepo: "userCredentials",
};

/** Each wire method's param names, in the order the fixture passes the args. */
const PARAMS: Partial<Record<StorageMethod, string[]>> = {
  queryHistoryLoadByConnection: ["connectionId"],
  queryHistoryRemoveByConnection: ["connectionId"],
  sharedReposSaveAll: ["repos", "activeRepoId"],
  licenseSave: ["data"],
  vaultStateSave: ["state"],
  userCredentialsLoad: ["scope", "key"],
  userCredentialsSave: ["credential"],
  userCredentialsRemove: ["scope", "key"],
  userCredentialsRemoveAllForKey: ["key"],
};

function splitCall(call: string): {
  repo: string;
  method: string;
  wire: StorageMethod;
} {
  const [repoName, method] = call.split(".");
  const repo = REPOS[repoName];
  const wire = `${repo}${method[0].toUpperCase()}${method.slice(1)}` as StorageMethod;
  return { repo, method, wire };
}

/** Walks a JSON-ish value, replacing what `fn` returns non-undefined for. */
function mapDeep(v: unknown, fn: (v: unknown) => unknown): unknown {
  const replaced = fn(v);
  if (replaced !== undefined) return replaced;
  if (Array.isArray(v)) return v.map((x) => mapDeep(x, fn));
  if (v !== null && typeof v === "object" && Object.getPrototypeOf(v) === Object.prototype) {
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v)) {
      if (x !== undefined) out[k] = mapDeep(x, fn);
    }
    return out;
  }
  return v;
}

const isDateTag = (v: unknown): v is { $date: string | null } =>
  v !== null && typeof v === "object" && !Array.isArray(v) && "$date" in v;

/** Fixture encoding → the app's values (`Date`s, decoded workflow cells). */
function decodeFixture(v: unknown): unknown {
  return mapDeep(fromStorable(v), (x) =>
    isDateTag(x) ? new Date(x.$date ?? Number.NaN) : undefined,
  );
}

/** App values → fixture encoding, the way the recorder wrote them. */
function encodeFixture(v: unknown): unknown {
  const dated = mapDeep(v, (x) =>
    x instanceof Date
      ? { $date: Number.isNaN(x.getTime()) ? null : x.toISOString() }
      : typeof x === "string" && /^workflow-[0-9a-f-]{36}$/.test(x)
        ? "workflow-<random-uuid>"
        : undefined,
  );
  return toStorable(dated);
}

/** Fixture encoding → what crosses the wire: dates as their text. */
function toWire(v: unknown): unknown {
  // An Invalid Date loaded from text that doesn't parse; any such text will do.
  return mapDeep(v, (x) => (isDateTag(x) ? (x.$date ?? "not a date") : undefined));
}

// -------- The replaying transport --------

interface Sent {
  method: string;
  params?: unknown;
  raw: string;
}

function decodeBody(body: Uint8Array): Sent {
  const raw = new TextDecoder().decode(body);
  const outer = JSON.parse(raw) as { method: string; params: { method: string; params?: unknown } };
  expect(outer.method).toBe("storage");
  // `method` must come before `params`, at both levels.
  expect(raw.startsWith(`{"method":"storage","params":{"method":"${outer.params.method}"`)).toBe(
    true,
  );
  return { method: outer.params.method, params: outer.params.params, raw };
}

function reply(method: string, result: unknown): unknown {
  return { method: "storage", result: { method, result: result ?? null } };
}

/**
 * Replays one case. `begin` sets the step being run; the transport checks
 * each request against it and answers from the fixture. (The dashboard
 * versions' prune, which sent two requests, went with phase 5d-2.)
 */
function replayTransport(c: Case) {
  let step: CallStep | null = null;
  let answered = false;

  const transport: CoreTransport = async (body) => {
    expect(body).toBeInstanceOf(Uint8Array);
    const sent = decodeBody(body);
    if (!step) throw new Error("request outside a step");
    const { wire } = splitCall(step.call);

    expect(answered, `${c.name}: a second request for ${step.call}`).toBe(false);
    answered = true;
    expect(sent.method).toBe(wire);
    const names = PARAMS[wire];
    if (names) {
      const expected = Object.fromEntries(names.map((n, i) => [n, toWire(step!.args[i])]));
      expect(sent.params).toEqual(expected);
    } else {
      // No params: the key is left out, never `{}`.
      expect(sent.raw).toBe(`{"method":"storage","params":{"method":"${wire}"}}`);
    }
    if (step.error) {
      throw new CoreCallError({ code: "STORAGE_ERROR", message: step.error.message });
    }
    return reply(wire, toWire(step.result));
  };

  return {
    transport,
    begin(s: CallStep) {
      step = s;
      answered = false;
    },
  };
}

// -------- Running a case through a client --------

/** A retired library call's repository isn't on the client any more. */
function expectRetired(client: StorageClient, step: CallStep): void {
  const { repo } = splitCall(step.call);
  expect(repo in client, step.call).toBe(false);
}

/** The demo's retired library repositories, bound to `db`, for the replay. */
function libraryRepos(db: WebSqliteDatabase): Record<string, unknown> {
  const bind = (repo: object) =>
    Object.fromEntries(
      Object.entries(repo).map(([name, fn]) => [
        name,
        (...args: unknown[]) => (fn as (...a: unknown[]) => unknown).call(repo, db, ...args),
      ]),
    );
  return {
    projects: bind(projectsRepo),
    connections: bind(connectionsRepo),
    savedQueries: bind(savedQueriesRepo),
    queryVersions: bind(queryVersionsRepo),
    projectState: bind(projectStateRepo),
    appState: bind(appStateRepo),
    connectionOverrides: bind(connectionOverridesRepo),
    themes: bind(themeRepo),
    onboarding: bind(onboardingRepo),
    tutorial: bind(tutorialRepo),
    importState: bind(importStateRepo),
    dashboards: bind(dashboardsRepo),
    dashboardVersions: bind(dashboardVersionsRepo),
    aiChats: bind(aiChatsRepo),
  };
}

async function runCall(client: object, step: CallStep): Promise<void> {
  const { repo, method } = splitCall(step.call);
  const fn = (
    (client as Record<string, unknown>)[repo] as Record<
      string,
      (...a: unknown[]) => Promise<unknown>
    >
  )[method];
  const args = step.args.map(decodeFixture);
  if (step.error) {
    await expect(fn(...args), step.call).rejects.toThrow();
    return;
  }
  const result = await fn(...args);
  if ("result" in step) {
    expect(encodeFixture(result), step.call).toEqual(step.result);
  } else {
    expect(result, step.call).toBeUndefined();
  }
}

async function tableRows(db: WebSqliteDatabase, table: string, columns: string[]) {
  const order = columns.map((_, i) => i + 1).join(", ");
  const cols = columns.map((c) => `"${c}"`).join(", ");
  const types = columns.map((c, i) => `typeof("${c}") AS "t${i}"`).join(", ");
  const rows = await db.query<Record<string, unknown>>(
    `SELECT ${cols}, ${types} FROM "${table}" ORDER BY ${order}`,
  );
  return {
    columns,
    rows: rows.map((r) => columns.map((c) => encodeFixture(r[c]))),
    types: rows.map((r) => columns.map((_, i) => r[`t${i}`])),
  };
}

// -------- Tests --------

let SQL: Awaited<ReturnType<typeof initSqlJs>>;
const savedTz = process.env.TZ;

beforeAll(async () => {
  // The fixtures were recorded in UTC: `new Date("2024-01-02 03:04:05")` reads local time.
  process.env.TZ = "UTC";
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  });
  SQL = await initSqlJs();
});

afterAll(() => {
  process.env.TZ = savedTz;
  vi.unstubAllGlobals();
});

function historyItem(id: string) {
  return {
    id,
    query: "SELECT 1",
    timestamp: "2026-01-02T00:00:00.000Z",
    executionTime: 1.5,
    rowCount: 1,
    connectionId: "demo-connection",
    favorite: false,
    connectionLabelsSnapshot: [],
    connectionNameSnapshot: "Demo Database",
  };
}

async function freshSqljs() {
  const db = new WebSqliteDatabase(new SQL.Database());
  await bootstrapSqljsDatabase(db);
  return { db, client: createSqljsStorageClient(db) };
}

describe("fixtures", () => {
  it("loads every repository's cases", () => {
    expect(cases.length).toBe(76);
    const repos = new Set(
      cases.flatMap((c) => c.steps.filter(isCall).map((s) => s.call.split(".")[0])),
    );
    expect([...repos].sort()).toEqual(Object.keys(REPOS).sort());
  });
});

describe("RustStorageClient replays the fixtures", () => {
  for (const c of cases) {
    it(c.name, async () => {
      const replay = replayTransport(c);
      const client = new RustStorageClient(replay.transport);
      for (const step of c.steps) {
        if (!isCall(step)) continue; // raw SQL only set up rows for the recorder
        if (RETIRED.has(step.call)) continue;
        if (LIBRARY_RETIRED.has(step.call)) {
          expectRetired(client, step);
          continue;
        }
        if (STATE_RETIRED_REPOS.has(splitCall(step.call).repo)) {
          expectRetired(client, step);
          continue;
        }
        replay.begin(step);
        await runCall(client, step);
      }
    });
  }
});

describe("SqljsStorageClient (the demo) matches the fixtures", () => {
  for (const c of cases) {
    it(c.name, async () => {
      const { db, client } = await freshSqljs();
      const library = libraryRepos(db);
      for (const step of c.steps) {
        if (isCall(step) && RETIRED.has(step.call)) await seedHistory(db, step);
        else if (
          isCall(step) &&
          (LIBRARY_RETIRED.has(step.call) || STATE_RETIRED_REPOS.has(splitCall(step.call).repo))
        )
          await runCall(library, step);
        else if (isCall(step)) await runCall(client, step);
        else await db.execute(step.sql, step.params);
      }
      for (const [table, expected] of Object.entries(c.rows ?? {})) {
        expect(await tableRows(db, table, expected.columns), table).toEqual(expected);
      }
    });
  }

  it("keeps the demo connection's history and AI chats across a reload", async () => {
    const { db, client } = await freshSqljs();
    await projectsRepo.save(db, {
      id: "default-seaquel",
      name: "Seaquel",
      createdAt: "2026-01-01T00:00:00.000Z",
      updatedAt: "2026-01-01T00:00:00.000Z",
      customLabels: [],
    });
    // Without its row, the history and chat writes fail the foreign key.
    await expect(client.queryHistory.append(historyItem("h0"))).rejects.toThrow(/FOREIGN KEY/);
    // What `addDemoConnection` saves, twice, as two page loads would.
    for (let i = 0; i < 2; i++) {
      await connectionsRepo.save(db, {
        id: "demo-connection",
        projectId: "default-seaquel",
        name: "Demo Database",
        type: "duckdb",
        host: "browser",
        port: 0,
        databaseName: "demo",
        username: "",
        labelIds: ["prod"],
        lastConnected: new Date("2026-01-02T00:00:00.000Z"),
      });
    }
    await client.queryHistory.append(historyItem("h1"));
    await aiChatsRepo.saveChat(db, {
      id: "chat-1",
      connectionId: "demo-connection",
      title: "Chat",
      createdAt: "2026-01-02T00:00:00.000Z",
      updatedAt: "2026-01-02T00:00:00.000Z",
    });

    // Reload: open what the demo persisted to localStorage.
    const stored = localStorage.getItem("seaquel_db");
    expect(stored).toBeTruthy();
    const bytes = Uint8Array.from(atob(stored!), (ch) => ch.charCodeAt(0));
    const reloaded = new WebSqliteDatabase(new SQL.Database(bytes));
    await bootstrapSqljsDatabase(reloaded);
    const again = createSqljsStorageClient(reloaded);
    expect((await connectionsRepo.loadAll(reloaded)).map((c) => c.id)).toEqual(["demo-connection"]);
    expect((await again.queryHistory.loadByConnection("demo-connection")).map((h) => h.id)).toEqual(
      ["h1"],
    );
    expect(
      (await aiChatsRepo.loadByConnection(reloaded, "demo-connection")).map((c) => c.id),
    ).toEqual(["chat-1"]);
  });

  it("keeps foreign keys on after a write", async () => {
    const { db, client } = await freshSqljs();
    await client.license.save({}); // a write, so an export
    await expect(
      connectionsRepo.save(db, {
        id: "c",
        projectId: "missing",
        name: "c",
        type: "sqlite",
        host: "",
        port: 0,
        databaseName: "",
        username: "",
        labelIds: [],
      }),
    ).rejects.toThrow(/FOREIGN KEY/);
  });
});

describe("SqljsStorageClient (the demo) history", () => {
  async function withConnection() {
    const fresh = await freshSqljs();
    await projectsRepo.save(fresh.db, {
      id: "p",
      name: "P",
      createdAt: "2026-01-01T00:00:00.000Z",
      updatedAt: "2026-01-01T00:00:00.000Z",
      customLabels: [],
    });
    await connectionsRepo.save(fresh.db, {
      id: "demo-connection",
      projectId: "p",
      name: "Demo Database",
      type: "duckdb",
      host: "browser",
      port: 0,
      databaseName: "demo",
      username: "",
      labelIds: [],
    });
    return fresh;
  }
  const at = (n: number) => new Date(Date.UTC(2026, 0, 1, 0, 0, n)).toISOString();

  it("append over the cap keeps favourites", async () => {
    const { client } = await withConnection();
    // The two oldest are favourites.
    for (let n = 0; n < 505; n++) {
      await client.queryHistory.append({
        ...historyItem(`h${n}`),
        timestamp: at(n),
        favorite: n < 2,
      });
    }
    const ids = (await client.queryHistory.loadByConnection("demo-connection")).map((h) => h.id);
    expect(ids).toHaveLength(502);
    expect(ids.slice(0, 500)).toEqual(Array.from({ length: 500 }, (_, i) => `h${504 - i}`));
    expect(ids.slice(500).sort()).toEqual(["h0", "h1"]);
  }, 60_000);

  it("ranks equal timestamps by insertion, like the Rust cap", async () => {
    const { client } = await withConnection();
    for (let n = 0; n < 501; n++) {
      await client.queryHistory.append({ ...historyItem(`h${n}`), timestamp: at(0) });
    }
    const ids = new Set(
      (await client.queryHistory.loadByConnection("demo-connection")).map((h) => h.id),
    );
    expect(ids.size).toBe(500);
    expect(ids.has("h0")).toBe(false);
    expect(ids.has("h500")).toBe(true);
  }, 60_000);

  it("loads ties newest-appended first, the order the cap ranks by", async () => {
    const { client } = await withConnection();
    for (const id of ["a", "b", "c"]) {
      await client.queryHistory.append({ ...historyItem(id), timestamp: at(5) });
    }
    await client.queryHistory.append({ ...historyItem("older"), timestamp: at(1) });
    expect(
      (await client.queryHistory.loadByConnection("demo-connection")).map((h) => h.id),
    ).toEqual(["c", "b", "a", "older"]);
  });

  it("setFavorite sets and clears, and ignores an unknown id", async () => {
    const { client } = await withConnection();
    await client.queryHistory.append(historyItem("h1"));
    await client.queryHistory.setFavorite("h1", true);
    await client.queryHistory.setFavorite("h1", true);
    await client.queryHistory.setFavorite("nope", true);
    expect(
      (await client.queryHistory.loadByConnection("demo-connection")).map((h) => h.favorite),
    ).toEqual([true]);
    await client.queryHistory.setFavorite("h1", false);
    expect(
      (await client.queryHistory.loadByConnection("demo-connection")).map((h) => h.favorite),
    ).toEqual([false]);
  });

  it("has no whole-list replace", () => {
    const client = createSqljsStorageClient({} as never);
    expect("replaceAll" in client.queryHistory).toBe(false);
  });
});

// -------- The Rust client's plumbing --------

function recordingTransport(respond: (method: string, params: unknown) => unknown = () => null): {
  transport: CoreTransport;
  sent: Sent[];
} {
  const sent: Sent[] = [];
  const transport: CoreTransport = async (body) => {
    const s = decodeBody(body);
    sent.push(s);
    return reply(s.method, await respond(s.method, s.params));
  };
  return { transport, sent };
}

describe("RustStorageClient", () => {
  it("sends history appends and favourites as targeted writes, in order", async () => {
    const { transport, sent } = recordingTransport();
    const client = new RustStorageClient(transport);
    const item = historyItem("h1");
    await Promise.all([
      client.queryHistory.append(item),
      client.queryHistory.setFavorite("h1", true),
    ]);
    expect(sent.map((s) => [s.method, s.params])).toEqual([
      ["queryHistoryAppend", { item }],
      ["queryHistorySetFavorite", { id: "h1", favorite: true }],
    ]);
    expect("replaceAll" in client.queryHistory).toBe(false);
  });

  it("lands writes in the order they were issued", async () => {
    const order: string[] = [];
    let releaseFirst!: () => void;
    const firstHeld = new Promise<void>((r) => (releaseFirst = r));
    const { transport } = recordingTransport(async (method, params) => {
      const key = (params as { key: string }).key;
      order.push(`start ${key}`);
      if (key === "a") await firstHeld;
      order.push(`end ${key}`);
      return null;
    });
    const client = new RustStorageClient(transport);
    const a = client.userCredentials.remove("db", "a");
    const b = client.userCredentials.remove("db", "b");
    await Promise.resolve();
    releaseFirst();
    await Promise.all([a, b]);
    expect(order).toEqual(["start a", "end a", "start b", "end b"]);
  });

  it("keeps the queue going after a failed write", async () => {
    let fail = true;
    const transport: CoreTransport = async (body) => {
      const s = decodeBody(body);
      if (fail) {
        fail = false;
        throw { code: "STORAGE_ERROR", message: "disk full" };
      }
      return reply(s.method, null);
    };
    const client = new RustStorageClient(transport);
    const a = client.userCredentials.remove("db", "a");
    const b = client.userCredentials.remove("db", "b");
    await expect(a).rejects.toMatchObject({ code: "STORAGE_ERROR" });
    await expect(b).resolves.toBeUndefined();
  });

  it("doesn't queue reads behind writes", async () => {
    let releaseWrite!: () => void;
    const writeHeld = new Promise<void>((r) => (releaseWrite = r));
    const { transport, sent } = recordingTransport(async (method) => {
      if (method === "userCredentialsRemove") await writeHeld;
      return method === "vaultStateLoad" ? null : null;
    });
    const client = new RustStorageClient(transport);
    const write = client.userCredentials.remove("db", "k");
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    await expect(client.vaultState.load()).resolves.toBeNull();
    expect(sent.map((s) => s.method)).toEqual(["userCredentialsRemove", "vaultStateLoad"]);
    releaseWrite();
    await write;
  });

  it("turns an RpcError into a CoreCallError with its code", async () => {
    const client = new RustStorageClient(async () => {
      throw { code: "LEGACY_STORAGE", message: "upgrade through 2026.9 first" };
    });
    const error = await client.vaultState.load().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreCallError);
    expect(error).toMatchObject({
      code: "LEGACY_STORAGE",
      message: "LEGACY_STORAGE: upgrade through 2026.9 first",
    });
  });

  it("refuses a response for another method", async () => {
    const client = new RustStorageClient(async () => reply("vaultStateSave", null));
    await expect(client.vaultState.load()).rejects.toMatchObject({ code: "PROTOCOL_ERROR" });
  });

  it("reads a setting through settings.settingGet (CoreSettings)", async () => {
    const sent: string[] = [];
    const client = new RustStorageClient(async (body) => {
      sent.push(new TextDecoder().decode(body));
      return {
        method: "settings",
        result: { method: "settingGet", result: { value: "7", seq: { epoch: "e", n: 1 } } },
      };
    });
    const settings = new CoreSettings(() => client);
    await expect(settings.getSetting("query_version_limit")).resolves.toMatchObject({
      value: "7",
    });
    expect(sent).toEqual([
      '{"method":"settings","params":{"method":"settingGet","params":{"key":"query_version_limit"}}}',
    ]);
  });

  it("rejects a settingGet read with the blocking storage codes (the storage gate)", async () => {
    for (const code of ["LEGACY_STORAGE", "STORAGE_CORRUPT"]) {
      const client = new RustStorageClient(async () => {
        throw { code, message: "blocked" };
      });
      const settings = new CoreSettings(() => client);
      const error = await settings.getSetting("query_version_limit").catch((e: unknown) => e);
      expect(error).toBeInstanceOf(CoreCallError);
      expect(error).toMatchObject({ code });
    }
  });

  it("the retired storage methods are gone from the client", () => {
    const client = new RustStorageClient(async () => {
      throw new Error("nothing may be sent");
    });
    for (const repo of STATE_RETIRED_REPOS) expect(repo in client, repo).toBe(false);
  });

  it("sends settings and ui calls as their groups, writes through the queue", async () => {
    const sent: string[] = [];
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    const client = new RustStorageClient(async (body) => {
      const raw = new TextDecoder().decode(body);
      sent.push(raw);
      const req = JSON.parse(raw) as { method: string; params: { method: string } };
      if (req.params.method === "settingSet") await held;
      return {
        method: req.method,
        result: { method: req.params.method, result: { value: null, seq: { epoch: "e", n: 2 } } },
      };
    });
    const write = client.settings("settingSet", { key: "editorKeybindingMode", value: "vim" });
    const queued = client.ui("windowStateSave", {
      windowId: "w",
      projectId: "p",
      rev: 1,
      state: {},
    });
    await vi.waitFor(() => expect(sent).toHaveLength(1));
    // A read doesn't wait for the queue; the queued save does.
    await client.ui("windowGet", { windowId: "w" });
    expect(
      sent.map((s) => (JSON.parse(s) as { params: { method: string } }).params.method),
    ).toEqual(["settingSet", "windowGet"]);
    release();
    await Promise.all([write, queued]);
    expect(sent[0]).toBe(
      '{"method":"settings","params":{"method":"settingSet","params":{"key":"editorKeybindingMode","value":"vim"}}}',
    );
    expect(sent[2]).toBe(
      '{"method":"ui","params":{"method":"windowStateSave","params":{"windowId":"w","projectId":"p","rev":1,"state":{}}}}',
    );
  });
});

describe("callSecret", () => {
  function secretTransport(result: unknown) {
    const sent: string[] = [];
    const transport: CoreTransport = async (body) => {
      const raw = new TextDecoder().decode(body);
      sent.push(raw);
      const req = JSON.parse(raw) as { params: { method: string } };
      return { method: "secret", result: { method: req.params.method, result } };
    };
    return { transport, sent };
  }

  it("sends a two-level request and returns the value", async () => {
    const { transport, sent } = secretTransport("hunter2");
    await expect(callSecret({ method: "get", params: { key: "db:c1" } }, transport)).resolves.toBe(
      "hunter2",
    );
    expect(sent).toEqual([
      '{"method":"secret","params":{"method":"get","params":{"key":"db:c1"}}}',
    ]);
  });

  it("never puts a response in a protocol error", async () => {
    const transport: CoreTransport = async () => ({
      method: "secret",
      result: { method: "set", result: "hunter2" },
    });
    const error = await callSecret({ method: "get", params: { key: "db:c1" } }, transport).catch(
      (e: unknown) => e as Error,
    );
    expect(error).toMatchObject({ code: "PROTOCOL_ERROR" });
    expect((error as Error).message).not.toContain("hunter2");
  });
});
