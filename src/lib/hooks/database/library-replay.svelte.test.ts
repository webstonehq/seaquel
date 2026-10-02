/**
 * The library fixtures replayed through the view models (phase 5d-1, the
 * fixtures README's "TypeScript replay"): every case's `op` with its `args`
 * runs through the real managers, wired as `UseDatabase` wires them, over
 * the demo's `TsLibrary` on an in-memory sql.js file, with the recorder's
 * stubs (providers, engine client) and clock.
 *
 * After every step it compares `outcome.ok`, what the window shows (`view`), and the rows as the
 * Rust replay does (the library tables whole, the cascade tables by id, no
 * state of a removed project), with `changes.json`'s expected steps in
 * place of the recorded ones. `view` isn't compared for a step whose
 * `changes.json` entry replaces rows. Secrets aren't compared: the demo has
 * no keychain, and nothing here sends one (this isn't desktop).
 *
 * Not replayed:
 * - `add/keychain-failure` step 0: it injects a keychain failure, and the
 *   demo has no keychain.
 * - `add/web-vault-cancelled`: a web case (its `library` is `null`).
 * - Phase 5e moved the shared projection and the imports to Core: the
 *   cases built on `RETIRED_OPS` (linking a git path, importing shared
 *   projects or templates, the TablePlus/DBeaver import) aren't replayed
 *   here (Core's `shared`/`imports` tests and the 5e fixtures pin them), and
 *   the recorded file calls (`files`) aren't compared: Core writes the files
 *   inside the library calls.
 *
 * The injected "Broken" import fails its `connectionCreate`, the demo's
 * equivalent of the recorder's failed storage save.
 */
import initSqlJs from "sql.js";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { StorageClient } from "$lib/storage/client";

const rec = vi.hoisted(() => ({
  storage: null as unknown,
  toasts: [] as { kind: string; message: string }[],
}));

vi.mock("$lib/storage", () => ({ getStorage: () => rec.storage }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => false,
  isWeb: () => false,
  isDemo: () => false,
}));
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({ isAvailable: () => false, isUnlocked: () => false }),
}));
vi.mock("$lib/engine", () => ({
  getEngineClient: () => ({ schemaTables: async () => [] }),
  TsEngineClient: class {},
}));
vi.mock("$lib/stores/ssh-host-key-prompt.svelte", () => ({
  sshHostKeyPromptStore: { prompt: async () => true },
}));
vi.mock("@tauri-apps/api/path", () => ({ join: async (...p: string[]) => p.join("/") }));
vi.mock("svelte-sonner", () => {
  const push = (kind: string) => (message: unknown) => {
    rec.toasts.push({ kind, message: String(message) });
  };
  return {
    toast: {
      info: push("info"),
      success: push("success"),
      warning: push("warning"),
      error: push("error"),
    },
  };
});
vi.mock("$lib/utils/toast", () => ({
  errorToast: (message: unknown) => {
    rec.toasts.push({ kind: "error", message: String(message) });
  },
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { createSqljsStorageClient } = await import("$lib/storage/sqljs-client");
const { WindowStateManager } = await import("./window-state.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { SavedQueryManager } = await import("./saved-queries.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { LabelManager } = await import("./label-manager.svelte.js");
const { ConnectionManager } = await import("./connection-manager.svelte.js");
const { TsLibrary } = await import("./library/ts-library");
const { setLibrary } = await import("./library/index");
const {
  FILES,
  LIBRARY_TABLES,
  bindStep,
  changeEntry,
  compare,
  expectedRows,
  load,
  loadChanges,
  matches,
  openCaseDb,
  snapshot,
  substituteValue,
} = await import("./library/fixture-support");

type Json = unknown;
type Obj = Record<string, Json>;
type Db = Awaited<ReturnType<typeof openCaseDb>>;
type Outcome = { ok: true; value: Json } | { ok: false; code: string; takenBy?: string };

/**
 * Steps the demo can't replay (see the header), and one the shared switch
 * no longer makes: `local-only/toggle-off-and-on#1` clicked the switch of an
 * unlinked connection a second time, which flipped it back to local-only.
 * Since the Task 7 review (I2) the switch shows "shared" only for a
 * connection with a template link, so a click on an unlinked one always
 * shares it (`isLocalOnly: false`). The Rust replays send the recorded
 * `connectionUpdate` and still cover the stored write.
 */
const SKIPPED = new Set(["add/keychain-failure#0", "local-only/toggle-off-and-on#1"]);

/** Steps replayed once the `RETIRED_OPS` cases are left out (145 before phase 5e). */
const STEPS_REPLAYED = 133;

/** The ops whose work moved to Core in phase 5e (see the header). */
const RETIRED_OPS = new Set([
  "projects.setGitRepoPath",
  "projects.importFromGitRepo",
  "projects.importSharedConnections",
  "projects.importSingleSharedConnection",
  "connections.importConnections",
]);

/**
 * Differences Task 6 makes that `changes.json` doesn't list, each for one
 * step and one field; everything else about the step is still compared.
 * - `project/remove-last#0` `outcome`: `ProjectManager.remove` keeps its
 *   contract and answers `false` for the last project (checked before any
 *   call), where Core refuses with `LAST_PROJECT`; nothing is written.
 * - `saved-query/prune-at-0#0` `view`: the recording shows today's bug
 *   ("`query_version_limit` 0 keeps only the first version": the page's
 *   prune dropped every version from memory). The page now shows the
 *   version Core stored, `q1#1`.
 */
const EXEMPT = new Set(["project/remove-last#0 outcome", "saved-query/prune-at-0#0 view"]);

/**
 * The page shows a connection's labels as Core stores and lists them (by
 * label id), not in the order they were added; `view` compares each
 * connection's `labelIds` as a set.
 */
function sortedLabels(v: Json): Json {
  const o = v as { connections?: { labelIds: string[] }[] };
  if (!o || !Array.isArray(o.connections)) return v;
  return {
    ...o,
    connections: o.connections.map((c) => ({ ...c, labelIds: [...c.labelIds].sort() })),
  };
}

const FIXED = new Date("2030-01-01T00:00:00.000Z");
let SQL: Awaited<ReturnType<typeof initSqlJs>>;

beforeAll(async () => {
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  });
  vi.spyOn(console, "error").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  SQL = await initSqlJs();
});

afterAll(() => {
  setLibrary(null);
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------- the page

/** One window: the managers over the case's database, wired as `UseDatabase` wires them. */
async function openPage(load: boolean) {
  const state = new DatabaseState();
  // The view state isn't part of the library's fixtures: off here.
  const windowState = new WindowStateManager(state, { enabled: false });
  const restoration = new StateRestorationManager(state);
  const projects = new ProjectManager(state, windowState, restoration);
  const labels = new LabelManager(state);
  const savedQueries = new SavedQueryManager(state, (id) => windowState.scheduleProject(id));
  let coreSeq = 0;
  const provider = {
    connect: async () => `core-${++coreSeq}`,
    disconnect: async () => {},
    test: async () => {},
  };
  const connections = new ConnectionManager(
    state,
    windowState,
    restoration,
    {} as never,
    { getForType: async () => provider } as never,
    async () => {},
    () => {},
  );
  projects.setConnectionManager(connections);
  if (load) {
    await projects.initialize();
    await connections.initializePersistedConnections();
  }
  return { state, projects, labels, savedQueries, connections };
}

type Page = Awaited<ReturnType<typeof openPage>>;

/** Lets writes finish and the 500 ms project save fire, as the recorder did. */
async function settle(): Promise<void> {
  for (let i = 0; i < 6; i++) {
    await new Promise((r) => setImmediate(r));
    await vi.advanceTimersByTimeAsync(600);
  }
}

/** What the window shows: the recorder's `view()`. */
function view(t: Page): Json {
  const s = t.state;
  const nonEmpty = <T>(o: Record<string, T[]>) =>
    Object.fromEntries(
      Object.entries(o)
        .filter(([, v]) => v.length > 0)
        .sort(([a], [b]) => a.localeCompare(b)),
    );
  return {
    activeProjectId: s.activeProjectId,
    projects: s.projects.map((p) => ({
      id: p.id,
      name: p.name,
      description: p.description ?? null,
      gitRepoPath: p.gitRepoPath ?? null,
      customLabels: p.customLabels.map((l) => ({ id: l.id, name: l.name, color: l.color })),
    })),
    connections: s.connections.map((c) => ({
      id: c.id,
      projectId: c.projectId,
      name: c.name,
      labelIds: c.labelIds,
      connected: !!c.providerConnectionId,
    })),
    connectionOrder: nonEmpty(s.connectionOrderByProject),
    savedQueries: nonEmpty(
      Object.fromEntries(
        Object.entries(s.queriesByProject).map(([p, qs]) => [
          p,
          qs.map((q) => ({ id: q.id, name: q.name, starred: !!q.starred, shared: !!q.shared })),
        ]),
      ),
    ),
    versions: nonEmpty(
      Object.fromEntries(
        Object.entries(s.queryVersionsByProject).map(([p, vs]) => [
          p,
          vs.map((v) => `${v.queryId}#${v.version}`),
        ]),
      ),
    ),
  };
}

// ---------------------------------------------------------------- the ops

/** `null` in `args` stands for `undefined` in the TS call (JSON has none). */
function undef<T>(v: T): T {
  if (v === null) return undefined as T;
  if (Array.isArray(v)) return v.map(undef) as T;
  if (v && typeof v === "object") {
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, undef(x)])) as T;
  }
  return v;
}

/** A query tab `t1` in `p1`, linked to `queryId` if given (the recorder's `tabWith`). */
function tabWith(t: Page, tabId: string, queryId?: string) {
  t.state.queryTabsByProject = {
    ...t.state.queryTabsByProject,
    p1: [
      ...(t.state.queryTabsByProject.p1 ?? []),
      { id: tabId, name: "Tab", query: "", ...(queryId ? { queryId } : {}) } as never,
    ],
  };
}

interface Ctx {
  case: Obj;
  step: Obj;
  /** Ids the case's earlier steps made (`<created>`). */
  created: string[];
  db: Db;
}

/** Runs one recorded `op` with its `args` through the page, as the recorder's step did. */
async function runOp(t: Page, op: string, rawArgs: Json, cx: Ctx): Promise<Json> {
  const a = undef((rawArgs ?? {}) as Obj);
  const id = (x: Json) => (x === "<created>" ? cx.created[0] : (x as string));
  switch (op) {
    case "connections.add": {
      const made = await t.connections.add(a as never);
      cx.created.push(made);
      return made;
    }
    case "connections.update": {
      let input = a.input as Obj;
      // The recorder passed these keys as `undefined`, which JSON dropped.
      if (cx.case.name === "update/ai-flags-cleared") {
        input = { ...input, aiShareSchema: undefined, aiShareData: undefined };
      }
      await t.connections.update(id(a.id), input as never);
      return null;
    }
    case "connections.reconnect":
      return await t.connections.reconnect(id(a.id), a.input as never);
    case "connections.autoReconnect":
      return await t.connections.autoReconnect(id(a.id));
    case "connections.remove":
      await t.connections.remove(id(a.id));
      return null;
    case "connections.toggleLocalOnly":
      await t.connections.toggleLocalOnly(a.connectionId as string);
      return null;
    case "connections.initializePersistedConnections":
      await t.connections.initializePersistedConnections();
      return null;
    case "labels.addLabelToConnection":
      await t.labels.addLabelToConnection(a.connectionId as string, a.labelId as string);
      return null;
    case "labels.removeLabelFromConnection":
      await t.labels.removeLabelFromConnection(a.connectionId as string, a.labelId as string);
      return null;
    case "labels.setConnectionLabels":
      await t.labels.setConnectionLabels(a.connectionId as string, a.labelIds as string[]);
      return null;
    case "setConnectionAIModel":
      // `UseDatabase.setConnectionAIModel`, which isn't built here.
      await t.connections.patch(a.connectionId as string, {
        activeAIProviderId: a.providerId as string,
        activeAIModel: a.model as string,
      });
      return null;
    case "projects.initialize":
      await t.projects.initialize();
      return t.state.projects.map((p) => p.id);
    case "projects.add": {
      const p = await t.projects.add(a.name as string, a.description as string | undefined);
      cx.created.push(p.id);
      return p.id;
    }
    case "projects.update": {
      // `{description: null}` recorded `update("p1", {description: undefined})`:
      // the key is there, its value undefined.
      const updates = (rawArgs as Obj).updates as Obj;
      await t.projects.update(a.id as string, undef({ ...updates }) as never);
      return null;
    }
    case "projects.remove":
      return await t.projects.remove(a.id as string);
    case "projects.addCustomLabel": {
      const l = await t.projects.addCustomLabel(a.projectId as string, a.label as never);
      cx.created.push(l.id);
      return l.id;
    }
    case "projects.updateCustomLabel":
      await t.projects.updateCustomLabel(
        a.projectId as string,
        a.labelId as string,
        a.updates as never,
      );
      return null;
    case "projects.removeCustomLabel":
      await t.projects.removeCustomLabel(a.projectId as string, a.labelId as string);
      return null;
    case "savedQueries.saveQuery": {
      if (Array.isArray(a.saves)) {
        tabWith(t, "t1");
        const saves = a.saves as Obj[];
        let first: string | null = null;
        for (const s of saves) {
          const made = await t.savedQueries.saveQuery(
            s.name as string,
            s.query as string,
            s.tabId as string,
          );
          first ??= made;
        }
        cx.created.push(first!);
        return first;
      }
      const tabId = a.tabId as string | undefined;
      if (tabId && !(t.state.queryTabsByProject.p1 ?? []).some((x) => x.id === tabId)) {
        // A step recorded as an update saved from a tab linked to `q1`.
        const library = cx.step.library as Obj;
        tabWith(t, tabId, library?.method === "savedQueryUpdate" ? "q1" : undefined);
      }
      const made = await t.savedQueries.saveQuery(
        a.name as string,
        a.query as string,
        tabId,
        a.parameters as never,
      );
      if (made && !cx.created.includes(made)) cx.created.push(made);
      return made;
    }
    case "savedQueries.saveQuery (two windows)": {
      const b = await openPage(true);
      const ab = a as { a: Obj; b: Obj };
      const first = await t.savedQueries.saveQuery(ab.a.name as string, ab.a.query as string);
      await settle();
      const second = await b.savedQueries.saveQuery(ab.b.name as string, ab.b.query as string);
      cx.created.push(first!, second!);
      return [first, second];
    }
    case "savedQueries.deleteQuery":
      await t.savedQueries.deleteQuery(a.id as string);
      return null;
    case "savedQueries.renameQuery":
      await t.savedQueries.renameQuery(a.id as string, a.name as string);
      return null;
    case "savedQueries.toggleQueryStarred":
      await t.savedQueries.toggleQueryStarred(a.id as string);
      return null;
    case "savedQueries.shareQuery":
      await t.savedQueries.shareQuery(a.id as string);
      return null;
    case "savedQueries.unshareQuery":
      await t.savedQueries.unshareQuery(a.id as string);
      return null;
    default:
      throw new Error(`no replay for op ${op}`);
  }
}

/** The ids a step made, from the rows it left (library ids the case didn't have before). */
function madeIds(before: Set<string>, rows: Record<string, Record<string, unknown>[]>): string[] {
  const out: string[] = [];
  for (const table of LIBRARY_TABLES) {
    for (const row of rows[table] ?? []) {
      for (const v of Object.values(row)) {
        if (typeof v === "string" && /^(conn|project|label|saved|ver)-[0-9a-f-]{36}$/.test(v)) {
          if (!before.has(v) && !out.includes(v)) out.push(v);
        }
      }
    }
  }
  return out;
}

/**
 * `view` against the recorded one. Its keys can hold ids (a project's
 * connection order), which `matches` doesn't bind, so they're substituted
 * first; an unbound token still matches any id of its prefix.
 */
function viewMatches(expected: Json, actual: Json, bound: Map<string, string>): boolean {
  return matches(sortedLabels(substituteValue(expected, bound)), sortedLabels(actual), {
    bound,
    started: FIXED.toISOString(),
  });
}

// ---------------------------------------------------------------- the replay

describe("the library fixtures through the view models", () => {
  it("the demo library replays the fixture cases", async () => {
    const changes = loadChanges();
    const failures: string[] = [];
    const exemptSeen = new Set<string>();
    let cases = 0;
    let steps = 0;
    for (const file of FILES) {
      for (const c of load(file)) {
        if (c.target === "web") continue; // add/web-vault-cancelled: see the header
        if ((c.steps as Obj[]).some((s) => RETIRED_OPS.has(s.op as string))) continue;
        cases++;
        const name = c.name as string;
        vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"], now: FIXED });
        try {
          const db = await openCaseDb(SQL, c);
          const lib = new TsLibrary(db);
          setLibrary(lib);
          rec.storage = createSqljsStorageClient(db) as StorageClient;
          const steps0 = c.steps as Obj[];
          const load0 = steps0[0]?.op !== "projects.initialize";
          const t = await openPage(load0);
          await settle();
          rec.toasts.length = 0;
          const bound = new Map<string, string>();
          const cx: Ctx = { case: c, step: {}, created: [], db };
          const started = FIXED.toISOString();
          let known = new Set(madeIds(new Set(), await snapshot(db)));
          for (const [i, step] of steps0.entries()) {
            if (SKIPPED.has(`${name}#${i}`)) continue;
            steps++;
            cx.step = step;
            let outcome: Outcome;
            try {
              const value = await runOp(t, step.op as string, step.args, cx);
              outcome = { ok: true, value };
            } catch (e) {
              const err = e as { code?: string | null; takenBy?: string };
              outcome = { ok: false, code: err.code ?? "ERROR", takenBy: err.takenBy };
            }
            await settle();
            const snap = await snapshot(db);
            const fresh = madeIds(known, snap);
            // Ids the step's calls returned but whose rows are gone (a
            // create then remove) still count as made.
            for (const made of cx.created) {
              if (!known.has(made) && !fresh.includes(made)) fresh.push(made);
            }
            const entry = changeEntry(changes, name, i);
            const expOutcome = (entry?.outcome as Obj | undefined) ?? (step.outcome as Obj);
            const expected = {
              outcome: expOutcome,
              rows: expectedRows(step.rows as Obj, entry),
            };
            const shown = view(t);
            // Rows only: the outcome is compared by `ok` below (the
            // managers' errors are worded for the user).
            const mirror = outcome.ok
              ? { ok: true }
              : { ok: false, code: outcome.code, ...(outcome.takenBy ? {} : {}) };
            const rowsWhyFor = (b: Map<string, string>) =>
              compare({ outcome: mirror, rows: expected.rows }, outcome, snap, {
                bound: b,
                started,
              });
            const fits = (b: Map<string, string>) =>
              rowsWhyFor(b) === null && (!!entry?.rows || viewMatches(step.view, shown, b));
            bindStep(bound, [expected.outcome, expected.rows, step.view], fresh, fits);
            const why: string[] = [];
            // `outcome.ok` only: the managers' errors are worded for the user.
            const exempt = (field: string) => EXEMPT.has(`${name}#${i} ${field}`);
            if ((expOutcome.ok === true) !== outcome.ok && !exempt("outcome")) {
              why.push(
                `outcome: expected ${JSON.stringify(expOutcome)}, got ${JSON.stringify(outcome)}`,
              );
            }
            const rowsWhy = rowsWhyFor(bound);
            if (rowsWhy) why.push(rowsWhy);
            if (!entry?.rows && !exempt("view") && !viewMatches(step.view, shown, bound)) {
              why.push(
                `view: expected ${JSON.stringify(step.view)}\n  actual   ${JSON.stringify(shown)}`,
              );
            }
            for (const e of EXEMPT) if (e.startsWith(`${name}#${i} `)) exemptSeen.add(e);
            if (why.length)
              failures.push(`${name} step ${i} (${String(step.op)}):\n${why.join("\n")}`);
            rec.toasts.length = 0;
            known = new Set([...known, ...fresh]);
          }
        } finally {
          vi.clearAllTimers();
          vi.useRealTimers();
          setLibrary(null);
        }
      }
    }
    expect(
      failures,
      `${failures.length} of ${steps} steps differ:\n\n${failures.join("\n\n")}`,
    ).toEqual([]);
    expect([...exemptSeen].sort(), "every exemption names a replayed step").toEqual(
      [...EXEMPT].sort(),
    );
    // 115 before phase 5e; the 11 cases built on `RETIRED_OPS` moved to Core.
    expect(cases).toBeGreaterThanOrEqual(104);
    expect(steps).toBeGreaterThanOrEqual(STEPS_REPLAYED);
  }, 300_000);
});
