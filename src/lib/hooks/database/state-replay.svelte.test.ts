/**
 * Phase 5d-2 Task 6b: the state fixtures replayed through the demo
 * (`crates/seaquel-workspace/tests/fixtures/state`, the README's
 * "TypeScript replay"). The case definitions and the harness are the
 * recorder's (`docs/plans/artifacts/2026-10-04-record-state-fixtures.test.ts.txt`),
 * with the managers and stores wired as `UseDatabase` wires them after
 * Task 6b: the library's, the `settings` group's and the `ui` group's
 * calls go to the demo's `TsLibrary`, `TsSettings` and `TsUi` on one
 * in-memory sql.js file, with the recorder's stubs, clock, uuid counter and
 * `TZ`.
 *
 * Each case runs its steps and, after every step, compares with the
 * recorded step (with `changes.json`'s expected steps in place):
 * - `outcome.ok` (the managers word their errors for the user; a create or
 *   rename that answers nothing counts as refused);
 * - the rows, as the Rust replay compares them: every dumped table whole,
 *   JSON columns parsed, `<id:n>` tokens bound to the ids made during the
 *   case (the same id everywhere in it), `<now>` any time of the pinned
 *   clock;
 * - `files` in order, and `view` (skipped where `changes.json` says `null`).
 *
 * Not compared (deviations, each named where it applies):
 * - `project_state` and `tabs`: the demo's `TsUi` writes no legacy mirror
 *   (6a, Decision 26: no older release reads the demo's file). The Rust
 *   replay pins the mirror.
 * - `secretCalls` and `secretStore`: the demo has no keychain.
 * - `web` cases run as the desktop does here, except onboarding (skipped on
 *   web, as the recorder's case).
 */
import initSqlJs from "sql.js";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { StorageClient } from "$lib/storage/client";
import type { SendAIMessageParams } from "$lib/services/ai";
import type { SchemaTable } from "$lib/types";
import type { SettingKey } from "./library/types";

// ---------------------------------------------------------------- mocks

const rec = vi.hoisted(() => {
  // Local days (the license nudge's) and times read in UTC, wherever this runs.
  process.env.TZ = "UTC";
  return {
    env: { web: false },
    storage: null as unknown,
    toasts: [] as { kind: string; message: string }[],
    secretCalls: [] as { method: string; key: string; value?: string }[],
    secrets: new Map<string, string>(),
    ai: null as null | ((p: unknown) => Promise<void>),
    /** What a workflow query node's run answers. */
    workflowRows: (() => []) as () => Record<string, unknown>[],
  };
});

vi.mock("$lib/storage", () => ({ getStorage: () => rec.storage }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => !rec.env.web,
  isWeb: () => rec.env.web,
  isDemo: () => false,
}));
vi.mock("$lib/storage/rust-client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/storage/rust-client")>()),
  callSecret: async (req: { method: string; params: { key: string; value?: string } }) => {
    const { key, value } = req.params;
    rec.secretCalls.push(
      value === undefined ? { method: req.method, key } : { method: req.method, key, value },
    );
    if (req.method === "set") {
      rec.secrets.set(key, value!);
      return null;
    }
    if (req.method === "delete") {
      rec.secrets.delete(key);
      return null;
    }
    return rec.secrets.get(key) ?? null;
  },
}));
vi.mock("$lib/engine", () => ({
  getEngineClient: () =>
    new Proxy(
      {},
      {
        get: (_t, key) =>
          key === "then" ? undefined : async () => (key === "schemaTables" ? [] : {}),
      },
    ),
  TsEngineClient: class {},
}));
vi.mock("$lib/stores/ssh-host-key-prompt.svelte", () => ({
  sshHostKeyPromptStore: { prompt: async () => true },
}));
vi.mock("$lib/services/ai", () => ({
  sendAIMessage: (p: unknown) => (rec.ai ? rec.ai(p) : Promise.resolve()),
}));
vi.mock("$lib/services/ai-mentions", () => ({ resolveMentions: (c: string) => c }));
vi.mock("mode-watcher", () => ({ mode: { current: "light" } }));
vi.mock("$lib/themes/apply", () => ({ applyTheme: () => {}, cacheThemeColors: () => {} }));
vi.mock("$lib/stores/license.svelte.js", () => ({ licenseStore: { status: "personal" } }));
vi.mock("$lib/stores/license.svelte", () => ({ licenseStore: { status: "personal" } }));
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

const { bootstrapSqljsDatabase, createSqljsStorageClient } =
  await import("$lib/storage/sqljs-client");
const { WebSqliteDatabase } = await import("$lib/storage/web-sqlite");
const { projectStateRepo } = await import("$lib/storage/repos/project-state-repo");
const { toStorable } = await import("$lib/values");
const { TsLibrary } = await import("./library/ts-library");
const { TsSettings } = await import("./library/ts-settings");
const { TsUi } = await import("./library/ts-ui");
const { getLibrary, setLibrary, setSettings } = await import("./library/index");
const { WindowStateManager } = await import("./window-state.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { ConnectionManager } = await import("./connection-manager.svelte.js");
const { PaneManager } = await import("./pane-manager.svelte.js");
const { TabOrderingManager } = await import("./tab-ordering.svelte.js");
const { QueryTabManager } = await import("./query-tabs.svelte.js");
const { SchemaTabManager } = await import("./schema-tabs.svelte.js");
const { ExplainTabManager } = await import("./explain-tabs.svelte.js");
const { ErdTabManager } = await import("./erd-tabs.svelte.js");
const { StatisticsTabManager } = await import("./statistics-tabs.svelte.js");
const { ExtensionsDuckdbTabManager } = await import("./extensions-duckdb-tabs.svelte.js");
const { WorkflowTabManager } = await import("./workflow-tabs.svelte.js");
const { DashboardTabManager } = await import("./dashboard-tabs.svelte.js");
const { DashboardManager } = await import("./dashboard-manager.svelte.js");
const { CreateTableTabManager } = await import("./create-table-tabs.svelte.js");
const { DataTabManager } = await import("./data-tabs.svelte.js");
const { StarterTabManager } = await import("./starter-tabs.svelte.js");
const { UIStateManager } = await import("./ui-state.svelte.js");
const { AIChatManager } = await import("./ai-chat-manager.svelte.js");
const { WorkflowState } = await import("./workflow-state.svelte.js");
const { WorkflowManager } = await import("./workflow-manager.svelte.js");

// ---------------------------------------------------------------- types

type Json = unknown;
type Row = Record<string, unknown>;
type Seed = Partial<Record<string, Row[]>>;
type Obj = Record<string, unknown>;

/** One Core call a GUI on Core sends (`group` is `library`, `settings` or `ui`). */
interface CoreCall {
  group: "library" | "settings" | "ui";
  method: string;
  params?: Json;
  /**
   * A create (or a versioned update): the id the TS made for the row, which
   * the replay binds to the id Core answers.
   */
  binds?: string;
  /**
   * `windowStateLoad`: what Core answers, by Decision 22's rules;
   * `chatMessagesList`: the message ids today's load returned, in order.
   */
  expect?: Json;
}

interface StorageEntry {
  page: string;
  call: string;
  args: Json[];
  result?: Json;
  error?: string;
}

/** A step's arguments as recorded, or a function of the ids earlier steps made. */
type Args = Obj | ((ids: string[]) => Obj);

interface Step {
  /** The TS call, as `manager.method`. */
  op: string;
  /** Its arguments, as recorded. */
  args?: Args;
  /** Core calls a GUI on Core sends for it that aren't derived from storage calls. */
  core?: CoreCall[] | ((ids: string[]) => CoreCall[]);
  /** The step's `core` replaces the calls derived from its storage calls. */
  coreOnly?: boolean;
  run: (t: Ctx, ids: string[]) => Promise<Json>;
}

interface RawStep {
  op: string;
  args?: Json;
  core: CoreCall[] | null;
  outcome: Json;
  storage: StorageEntry[];
  rows: Record<string, Row[]>;
  secretCalls: Json[];
  secretStore: Record<string, string>;
  files: Json[];
  toasts: Json[];
  view: Json;
}

interface RawCase {
  name: string;
  note?: string;
  file: string;
  seed: Seed;
  secrets?: Record<string, string>;
  target?: string;
  steps: RawStep[];
  /** Rows before step 0 (not written: the seed is them). */
  before: Record<string, Row[]>;
}

interface StepChange {
  outcome?: Json;
  rows?: Record<string, Row[]>;
  secretStore?: Record<string, string>;
  /** `null`: the step's `view` isn't compared; otherwise the view Core's rows give. */
  view?: Json;
  /** `windowStateLoad`'s answer, where it differs from the recording. */
  loaded?: Json;
}

interface Change {
  decision: string;
  why: string;
  expected: (raw: RawCase) => Record<number, StepChange> | Promise<Record<number, StepChange>>;
}

interface Case {
  name: string;
  note?: string;
  file?: "current" | "v2026.4.5-beta.1";
  seed?: Seed;
  secrets?: Record<string, string>;
  web?: boolean;
  /** How `view` is taken: a page's (default) or a store's. */
  view?: (t: Ctx) => Json;
  steps: Step[];
  change?: Change;
}

// ---------------------------------------------------------------- harness

const T0 = "2024-01-01T00:00:00.000Z";
const FIXED = new Date("2030-01-01T00:00:00.000Z");

let SQL: Awaited<ReturnType<typeof initSqlJs>>;
let uuidSeq = 0;

beforeAll(async () => {
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  });
  vi.spyOn(console, "error").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.spyOn(console, "log").mockImplementation(() => {});
  vi.spyOn(performance, "now").mockReturnValue(0);
  vi.spyOn(crypto, "randomUUID").mockImplementation(
    () =>
      `00000000-0000-4000-8000-${String(++uuidSeq).padStart(12, "0")}` as `${string}-${string}-${string}-${string}-${string}`,
  );
  SQL = await initSqlJs();
});

afterAll(() => {
  setLibrary(null);
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/** Seed order, and what the dump reads (the 5d-2 tables; projects and connections are only seeded). */
const SEED_TABLES = [
  "projects",
  "connections",
  "connection_overrides",
  "project_state",
  "tabs",
  "saved_canvases",
  "dashboards",
  "dashboard_versions",
  "ai_chats",
  "ai_messages",
  "app_state",
  "theme_preferences",
  "user_themes",
  "onboarding_state",
  "tutorial_progress",
  "import_state",
];
const DUMP: [string, string][] = [
  ["connection_overrides", "shared_connection_id"],
  ["project_state", "project_id"],
  ["tabs", "project_id, id"],
  ["saved_canvases", "id"],
  ["dashboards", "id"],
  ["dashboard_versions", "dashboard_id, version"],
  ["ai_chats", "id"],
  ["ai_messages", "chat_id, id"],
  ["app_state", "key"],
  ["theme_preferences", "id"],
  ["user_themes", "id"],
  ["onboarding_state", "id"],
  ["tutorial_progress", "lesson_id, challenge_id"],
  ["import_state", "source"],
];

type Db = InstanceType<typeof WebSqliteDatabase>;

async function insertRows(db: Db, seed: Seed): Promise<void> {
  for (const table of SEED_TABLES) {
    for (const row of seed[table] ?? []) {
      const cols = Object.keys(row);
      await db.execute(
        `INSERT INTO ${table} (${cols.join(", ")}) VALUES (${cols.map(() => "?").join(", ")})`,
        cols.map((c) => row[c]),
      );
    }
  }
}

/**
 * A file that started on v2026.4.5-beta.1, as the baseline upgrade leaves
 * it: `dashboards.project_id` is a plain nullable column with no foreign key
 * (and so is `saved_queries.project_id`, which these cases don't touch).
 */
async function makeBetaEra(db: Db): Promise<void> {
  await db.execute("PRAGMA foreign_keys=OFF");
  await db.execute(`CREATE TABLE dashboards_beta (id TEXT PRIMARY KEY, name TEXT NOT NULL,
    viewport TEXT NOT NULL DEFAULT '{"x":0,"y":0,"zoom":1}', widgets TEXT NOT NULL DEFAULT '[]',
    date_filter TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, starred INTEGER DEFAULT 0,
    shared INTEGER NOT NULL DEFAULT 0, description TEXT, project_id TEXT)`);
  await db.execute("DROP TABLE dashboards");
  await db.execute("ALTER TABLE dashboards_beta RENAME TO dashboards");
  await db.execute("DROP INDEX IF EXISTS idx_dashboards_project");
  await db.execute("CREATE INDEX idx_dashboards_project ON dashboards(project_id)");
  await db.execute("PRAGMA foreign_keys=ON");
}

async function dump(db: Db): Promise<Record<string, Row[]>> {
  const out: Record<string, Row[]> = {};
  for (const [table, order] of DUMP) {
    const rows = await db.query<Row>(`SELECT * FROM ${table} ORDER BY ${order}`);
    if (rows.length) out[table] = rows;
  }
  return out;
}

/** A value as JSON holds it: bigint, bytes and decimals tagged, `undefined` as `null`. */
function plain(v: unknown): Json {
  if (v === undefined) return null;
  const text = JSON.stringify(toStorable(v));
  return text === undefined ? null : JSON.parse(text);
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Lets fire-and-forget writes finish and the 500 ms debounced saves fire. */
async function settle(): Promise<void> {
  for (let i = 0; i < 6; i++) {
    await new Promise((r) => setImmediate(r));
    await vi.advanceTimersByTimeAsync(600);
  }
}

// ---------------------------------------------------------------- Core calls derived from storage calls

/** The view state a window saves: today's project state minus what Decision 22 keeps elsewhere. */
function viewStateOf(s: Obj): Obj {
  const {
    savedWorkflows: _w,
    connectionOrder: _c,
    starredSharedQueryIds: _q,
    starredSharedDashboardIds: _d,
    ...rest
  } = s;
  return rest;
}

/** A workflow as a `workflowCreate`/`workflowUpdate` sends it: its storable JSON minus what Core sets. */
function workflowBody(w: Obj): Obj {
  const { id: _i, projectId: _p, createdAt: _c, updatedAt: _u, ...rest } = w;
  return rest;
}

/** Decision 22's window rows, as Core would hold them during the case. */
interface WindowRow {
  state: Json;
  rev: number;
  touched: number;
}

/** The case's record: calls in order, and Core's window rows. */
const run = {
  storage: [] as StorageEntry[],
  core: [] as CoreCall[],
  windows: new Map<string, WindowRow>(),
  /** Decision 22's `windows` rows: each window's active project and when it was last written. */
  windowMeta: new Map<string, { activeProjectId: string | null; touched: number }>(),
  /** `lastActiveProjectId` as Core would hold it. */
  lastActive: null as string | null,
  clock: 0,
};

/** A write to window `windowId` (a save, an activation, a copied load): it becomes the most recently used. */
function touchWindow(windowId: string, activeProjectId?: string): void {
  const meta = run.windowMeta.get(windowId) ?? { activeProjectId: null, touched: 0 };
  if (activeProjectId !== undefined) meta.activeProjectId = activeProjectId;
  meta.touched = ++run.clock;
  run.windowMeta.set(windowId, meta);
}

/**
 * `windowGet`'s answer (Decision 22): the window's own active project; for a
 * window with none, the most recently used window's; else `lastActiveProjectId`.
 */
function windowGet(windowId: string): Json {
  const own = run.windowMeta.get(windowId)?.activeProjectId;
  if (own) return { activeProjectId: own, from: "window" };
  const recent = [...run.windowMeta.entries()]
    .filter(([id, m]) => id !== windowId && m.activeProjectId)
    .sort(([, a], [, b]) => b.touched - a.touched)[0];
  if (recent) return { activeProjectId: recent[1].activeProjectId, from: "recent" };
  return { activeProjectId: run.lastActive, from: run.lastActive ? "lastActive" : null };
}

/** What a page on Core would remember between calls, to send only what changed. */
interface Baseline {
  connectionOrder: Map<string, string>;
  workflows: Map<string, Map<string, string>>;
  dashboards: Map<string, Obj>;
  pendingVersion: Map<string, string>;
  chats: Map<string, Obj>;
  messages: Map<string, Map<string, string>>;
  revs: Map<string, number>;
  onboarding: Obj;
  themePrefs: Obj | null;
  pendingPrefs: Obj | null;
  themes: Map<string, string>;
}

const ONBOARDING_DEFAULTS: Obj = {
  isFirstRun: true,
  userBackground: "none",
  hasCompletedWizard: false,
  showWizardHints: true,
  dismissedHints: [],
  learnEnabled: true,
};
const PREFS_DEFAULTS: Obj = { lightThemeId: "default-light", darkThemeId: "default-dark" };

/** A user theme as `userThemeCreate`/`userThemeUpdate` send it: without what Core sets. */
function themeBody(theme: Obj): Obj {
  const { id: _i, createdAt: _c, updatedAt: _u, ...rest } = theme;
  return rest;
}

function newBaseline(): Baseline {
  return {
    connectionOrder: new Map(),
    workflows: new Map(),
    dashboards: new Map(),
    pendingVersion: new Map(),
    chats: new Map(),
    messages: new Map(),
    revs: new Map(),
    onboarding: { ...ONBOARDING_DEFAULTS },
    themePrefs: null,
    pendingPrefs: null,
    themes: new Map(),
  };
}

const windowKey = (windowId: string, projectId: string) => `${windowId}\u0000${projectId}`;

/** `windowStateLoad`'s answer by Decision 22, given what today's load returned. */
function windowLoad(windowId: string, projectId: string, legacy: Obj | null, base: Baseline): Json {
  const own = run.windows.get(windowKey(windowId, projectId));
  let answer: { state: Json; rev: number; copiedFrom: string | null };
  if (own) {
    // A load writes nothing, so it doesn't make the row more recent.
    answer = { state: own.state, rev: own.rev, copiedFrom: null };
  } else {
    const others = [...run.windows.entries()]
      .filter(([k]) => k.endsWith(`\u0000${projectId}`))
      .sort(([, a], [, b]) => b.touched - a.touched);
    if (others.length > 0) {
      answer = { state: others[0][1].state, rev: 0, copiedFrom: "window" };
    } else if (legacy) {
      answer = { state: viewStateOf(legacy), rev: 0, copiedFrom: "legacy" };
    } else {
      answer = { state: null, rev: 0, copiedFrom: "empty" };
    }
    if (answer.state !== null) {
      // The copy is written as the window's row at once.
      run.windows.set(windowKey(windowId, projectId), {
        state: answer.state,
        rev: 0,
        touched: ++run.clock,
      });
      touchWindow(windowId);
    }
  }
  base.revs.set(projectId, answer.rev);
  return answer;
}

function dashboardParams(d: Obj): Obj {
  return {
    name: d.name,
    description: d.description ?? null,
    widgets: JSON.parse(d.widgets as string),
    viewport: JSON.parse(d.viewport as string),
    dateFilter: d.dateFilter ? JSON.parse(d.dateFilter as string) : null,
    starred: !!d.starred,
    shared: !!d.shared,
  };
}

/** The Core calls for one storage call, before it runs (saves) or after (loads). */
function derive(
  page: PageCtx,
  call: string,
  args: unknown[],
  result: unknown,
  phase: "before" | "after",
): void {
  const base = page.baseline;
  const push = (c: CoreCall) => run.core.push(c);
  if (phase === "after") {
    switch (call) {
      case "projectState.load": {
        const projectId = args[0] as string;
        const legacy = result ? (plain(result) as Obj) : null;
        push({
          group: "ui",
          method: "windowStateLoad",
          params: { windowId: page.windowId, projectId },
          expect: windowLoad(page.windowId, projectId, legacy, base),
        });
        base.connectionOrder.set(projectId, JSON.stringify(legacy?.connectionOrder ?? []));
        base.workflows.set(
          projectId,
          new Map(
            ((legacy?.savedWorkflows as Obj[]) ?? []).map((w) => [
              w.id as string,
              JSON.stringify(w),
            ]),
          ),
        );
        return;
      }
      case "appState.get":
        if (args[0] === "lastActiveProjectId") {
          push({
            group: "ui",
            method: "windowGet",
            params: { windowId: page.windowId },
            expect: windowGet(page.windowId),
          });
        }
        return;
      case "dashboards.loadByProject":
        push({
          group: "library",
          method: "dashboardsList",
          params: { projectId: args[0] },
          expect: {
            dashboards: (result as Obj[]).map((d) => ({ id: d.id, starred: !!d.starred })),
          },
        });
        for (const d of result as Obj[]) base.dashboards.set(d.id as string, plain(d) as Obj);
        return;
      case "dashboardVersions.loadByProject":
        push({
          group: "library",
          method: "dashboardVersionsList",
          params: { projectId: args[0] },
          expect: { ids: (result as Obj[]).map((v) => v.id) },
        });
        return;
      case "aiChats.loadByConnection":
        push({
          group: "library",
          method: "chatsList",
          params: { connectionId: args[0] },
          expect: { ids: (result as Obj[]).map((c) => c.id) },
        });
        for (const c of result as Obj[]) base.chats.set(c.id as string, plain(c) as Obj);
        return;
      case "onboarding.load":
        base.onboarding = { ...ONBOARDING_DEFAULTS, ...(plain(result) as Obj | null) };
        return;
      case "themes.loadPreferences":
        base.themePrefs = (plain(result) as Obj | null) ?? { ...PREFS_DEFAULTS };
        return;
      case "themes.loadUserThemes":
        base.themes = new Map(
          (plain(result) as Obj[]).map((t) => [t.id as string, JSON.stringify(t)]),
        );
        return;
      case "aiChats.loadMessages":
        push({
          group: "library",
          method: "chatMessagesList",
          params: { chatId: args[0] },
          expect: { ids: (result as Obj[]).map((m) => m.id) },
        });
        base.messages.set(
          args[0] as string,
          new Map((result as Obj[]).map((m) => [m.id as string, JSON.stringify(plain(m))])),
        );
        return;
    }
    return;
  }
  switch (call) {
    case "projectState.save": {
      const s = plain(args[0]) as Obj;
      const projectId = s.projectId as string;
      const order = JSON.stringify(s.connectionOrder ?? []);
      if (base.connectionOrder.get(projectId) !== order) {
        push({
          group: "library",
          method: "projectSidebarSet",
          params: { projectId, connectionOrder: s.connectionOrder ?? [] },
        });
        base.connectionOrder.set(projectId, order);
      }
      const before = base.workflows.get(projectId) ?? new Map<string, string>();
      const after = new Map<string, string>();
      for (const w of (s.savedWorkflows as Obj[]) ?? []) {
        const text = JSON.stringify(w);
        after.set(w.id as string, text);
        if (!before.has(w.id as string)) {
          push({
            group: "library",
            method: "workflowCreate",
            params: { workflow: { projectId, workflow: workflowBody(w) } },
            binds: w.id as string,
          });
        } else if (before.get(w.id as string) !== text) {
          push({
            group: "library",
            method: "workflowUpdate",
            params: { id: w.id, workflow: workflowBody(w) },
          });
        }
      }
      for (const id of before.keys()) {
        if (!after.has(id)) push({ group: "library", method: "workflowRemove", params: { id } });
      }
      base.workflows.set(projectId, after);
      const rev = (base.revs.get(projectId) ?? 0) + 1;
      base.revs.set(projectId, rev);
      const state = viewStateOf(s);
      push({
        group: "ui",
        method: "windowStateSave",
        params: { windowId: page.windowId, projectId, rev, state },
      });
      run.windows.set(windowKey(page.windowId, projectId), { state, rev, touched: ++run.clock });
      touchWindow(page.windowId);
      return;
    }
    case "appState.set":
      if (args[0] === "lastActiveProjectId") {
        if (args[1]) {
          push({
            group: "ui",
            method: "windowActivate",
            params: { windowId: page.windowId, projectId: args[1] },
          });
          touchWindow(page.windowId, args[1] as string);
          run.lastActive = args[1] as string;
        }
      } else if (args[0] !== "aiSettings") {
        // aiSettings is split into the calls its steps name (Decision 20).
        push({
          group: "settings",
          method: "settingSet",
          params: { key: args[0], value: args[1] ?? null },
        });
      }
      return;
    case "importState.save":
      push({
        group: "settings",
        method: "importStateSave",
        params: { source: args[0], hasOfferedImport: args[1], lastCheckTimestamp: args[2] ?? null },
      });
      return;
    case "tutorial.save":
      push({
        group: "settings",
        method: "tutorialSave",
        params: { lessonId: args[0], challengeId: args[1], state: args[2] ?? null },
      });
      return;
    case "tutorial.removeLesson":
      push({ group: "settings", method: "tutorialRemoveLesson", params: { lessonId: args[0] } });
      return;
    case "tutorial.removeAll":
      push({ group: "settings", method: "tutorialReset" });
      return;
    case "onboarding.save": {
      const next = plain(args[0]) as Obj;
      const patch: Obj = {};
      for (const [k, v] of Object.entries(next)) {
        if (JSON.stringify(v) !== JSON.stringify(base.onboarding[k])) patch[k] = v;
      }
      if (Object.keys(patch).length > 0) {
        push({ group: "settings", method: "onboardingPatch", params: { patch } });
      }
      base.onboarding = { ...base.onboarding, ...next };
      return;
    }
    case "themes.savePreferences":
      base.pendingPrefs = { lightThemeId: args[0], darkThemeId: args[1] };
      return;
    case "themes.saveUserThemes": {
      const next = new Map((plain(args[0]) as Obj[]).map((t) => [t.id as string, t]));
      const removed = [...base.themes.keys()].filter((id) => !next.has(id));
      for (const id of removed) {
        push({ group: "settings", method: "userThemeRemove", params: { id } });
      }
      for (const [id, theme] of next) {
        const was = base.themes.get(id);
        if (was === undefined) {
          push({
            group: "settings",
            method: "userThemeCreate",
            params: { theme: themeBody(theme) },
            binds: id,
          });
        } else if (was !== JSON.stringify(theme)) {
          push({
            group: "settings",
            method: "userThemeUpdate",
            params: { id, theme: themeBody(theme) },
          });
        }
      }
      const prefs = base.pendingPrefs ?? base.themePrefs ?? PREFS_DEFAULTS;
      const was = base.themePrefs ?? PREFS_DEFAULTS;
      // A removed theme's preference falls back inside userThemeRemove.
      const reset = (slot: "lightThemeId" | "darkThemeId", fallback: string) =>
        prefs[slot] === was[slot] ||
        (removed.includes(was[slot] as string) && prefs[slot] === fallback);
      if (!reset("lightThemeId", "default-light") || !reset("darkThemeId", "default-dark")) {
        push({ group: "settings", method: "themePreferencesSet", params: prefs });
      }
      base.themePrefs = prefs;
      base.pendingPrefs = null;
      base.themes = new Map([...next].map(([id, t]) => [id, JSON.stringify(t)]));
      return;
    }
    case "dashboards.save": {
      const d = plain(args[0]) as Obj;
      const id = d.id as string;
      const now = dashboardParams(d);
      const old = base.dashboards.get(id);
      const version = base.pendingVersion.get(id);
      base.pendingVersion.delete(id);
      if (!old) {
        const draft: Obj = {
          projectId: d.projectId,
          name: now.name,
          widgets: now.widgets,
          viewport: now.viewport,
        };
        if (now.description !== null) draft.description = now.description;
        if (now.dateFilter !== null) draft.dateFilter = now.dateFilter;
        push({
          group: "library",
          method: "dashboardCreate",
          params: { dashboard: draft },
          binds: id,
        });
      } else {
        const was = dashboardParams(old);
        const patch: Obj = {};
        for (const k of Object.keys(now)) {
          if (JSON.stringify(now[k]) !== JSON.stringify(was[k])) patch[k] = now[k];
        }
        if (version) patch.captureVersion = true;
        if (Object.keys(patch).length > 0) {
          push({
            group: "library",
            method: "dashboardUpdate",
            params: { id, patch },
            ...(version ? { binds: version } : {}),
          });
        }
      }
      base.dashboards.set(id, d);
      return;
    }
    case "dashboards.remove":
      // The page keeps what it last sent: a save racing the removal is an
      // update of the removed row, as a GUI on Core would send it.
      push({ group: "library", method: "dashboardRemove", params: { id: args[0] } });
      return;
    case "dashboardVersions.insert": {
      const v = args[0] as Obj;
      base.pendingVersion.set(v.dashboardId as string, v.id as string);
      return;
    }
    case "aiChats.saveChat": {
      const c = plain(args[0]) as Obj;
      const id = c.id as string;
      const old = base.chats.get(id);
      if (!old) {
        push({
          group: "library",
          method: "chatCreate",
          params: { chat: { connectionId: c.connectionId, title: c.title } },
          binds: id,
        });
      } else {
        const patch: Obj = {};
        if (old.title !== c.title) patch.title = c.title;
        if (old.updatedAt !== c.updatedAt) patch.touched = true;
        if (Object.keys(patch).length > 0) {
          push({ group: "library", method: "chatUpdate", params: { id, patch } });
        }
      }
      base.chats.set(id, c);
      return;
    }
    case "aiChats.replaceAllMessages": {
      const chatId = args[0] as string;
      const old = base.messages.get(chatId) ?? new Map<string, string>();
      const next = new Map<string, string>();
      const changed: Obj[] = [];
      for (const m of plain(args[1]) as Obj[]) {
        const text = JSON.stringify(m);
        next.set(m.id as string, text);
        if (old.get(m.id as string) !== text) {
          const draft: Obj = {
            id: m.id,
            role: m.role,
            content: m.content,
            timestamp: m.timestamp,
          };
          if (m.query != null) draft.query = m.query;
          if (m.dashboardId != null) draft.dashboardId = m.dashboardId;
          changed.push(draft);
        }
      }
      if (changed.length > 0) {
        push({
          group: "library",
          method: "chatMessagesPut",
          params: { chatId, messages: changed },
        });
      }
      base.messages.set(chatId, next);
      return;
    }
    case "aiChats.removeChat":
      push({ group: "library", method: "chatRemove", params: { id: args[0] } });
      base.chats.delete(args[0] as string);
      base.messages.delete(args[0] as string);
      return;
  }
}

/** A page's storage client: every call recorded, and its Core calls derived. */
function pageClient(base: StorageClient, page: PageCtx): StorageClient {
  const out: Record<string, Record<string, unknown>> = {};
  for (const [group, methods] of Object.entries(base as unknown as Record<string, Obj>)) {
    out[group] = {};
    for (const [name, fn] of Object.entries(methods)) {
      if (typeof fn !== "function") continue;
      const call = `${group}.${name}`;
      out[group][name] = async (...args: unknown[]) => {
        const entry: StorageEntry = { page: page.name, call, args: args.map(plain) };
        run.storage.push(entry);
        derive(page, call, args, undefined, "before");
        try {
          const result = await (fn as (...a: unknown[]) => Promise<unknown>)(...args);
          if (result !== undefined) entry.result = plain(result);
          derive(page, call, args, result, "after");
          if (call === "projectState.load") await listWorkflows(page, args[0] as string);
          return result;
        } catch (e) {
          entry.error = errorText(e);
          throw e;
        }
      };
    }
  }
  return out as unknown as StorageClient;
}

// ---------------------------------------------------------------- pages and stores

interface PageCtx {
  name: string;
  windowId: string;
  baseline: Baseline;
  db: Db;
}

/**
 * `workflowsList` (Decision 23), which a page on Core reads beside its view
 * state: every stored workflow of the project whose JSON parses, whether or
 * not the GUI can decode it, and whether or not the project has a
 * `project_state` row. A row that isn't JSON is skipped, never refused.
 */
async function listWorkflows(page: PageCtx, projectId: string): Promise<void> {
  const rows = await page.db.query<{ id: string; data: string }>(
    "SELECT id, data FROM saved_canvases WHERE project_id = ? ORDER BY id",
    [projectId],
  );
  const ids = rows
    .filter((r) => {
      try {
        JSON.parse(r.data);
        return true;
      } catch {
        return false;
      }
    })
    .map((r) => r.id);
  run.core.push({
    group: "library",
    method: "workflowsList",
    params: { projectId },
    expect: { ids },
  });
}

async function openPage(db: Db, name: string, windowId: string, load: boolean) {
  const ctx: PageCtx = { name, windowId, baseline: newBaseline(), db };
  const client = pageClient(createSqljsStorageClient(db), ctx);
  rec.storage = client;
  const state = new DatabaseState();
  const schedule = (projectId: string | null) => windowState.scheduleProject(projectId);
  const setActiveView = (
    view: Parameters<InstanceType<typeof UIStateManager>["setActiveView"]>[0],
  ) => ui.setActiveView(view);
  // This window's view state, under its own id (the demo's page is `demo`;
  // the cases' windows keep the recorder's ids so two share one file).
  const uiService = new TsUi(db, { origin: () => windowId });
  const windowState = new WindowStateManager(state, {
    ui: () => uiService,
    windowId: () => windowId,
  });
  const panes = new PaneManager(state, schedule);
  const tabs = new TabOrderingManager(state, schedule, panes);
  const restoration = new StateRestorationManager(state);
  const projects = new ProjectManager(state, windowState, restoration);
  const aiChats = new AIChatManager(
    state,
    (chatId) => restoration.loadAIChatMessages(chatId),
    (chatId) => ui.abortStreamFor(chatId),
  );
  const dashboardTabs = new DashboardTabManager(state, tabs, schedule, setActiveView);
  const dashboards = new DashboardManager(
    state,
    async () => [{ total: 1n, day: "2030-01-01" }],
    schedule,
  );
  dashboards.setSyncActive((tabId) => panes.syncGlobalActiveState(tabId));
  dashboardTabs.setOnClose((id) => dashboards.closeDashboard(id));
  const ui = new UIStateManager(
    state,
    schedule,
    async () => ({ rows: [], truncated: false }),
    aiChats,
    (chatId) => aiChats.persistMessages(chatId),
    dashboards,
    dashboardTabs,
  );
  const queryTabs = new QueryTabManager(state, tabs, schedule);
  const schemaTabs = new SchemaTabManager(state, tabs, schedule);
  const explainTabs = new ExplainTabManager(state, tabs, schedule, setActiveView);
  const erdTabs = new ErdTabManager(state, tabs, schedule, setActiveView);
  const statisticsTabs = new StatisticsTabManager(state, tabs, schedule, setActiveView);
  const extensionsDuckdbTabs = new ExtensionsDuckdbTabManager(
    state,
    tabs,
    schedule,
    setActiveView,
    async () => [] as never,
  );
  const workflowTabs = new WorkflowTabManager(state, tabs, schedule, setActiveView);
  const starterTabs = new StarterTabManager(state, tabs, schedule);
  const createTableTabs = new CreateTableTabManager(
    state,
    tabs,
    schedule,
    setActiveView,
    async () => {},
    () => ({}) as never,
    () => ({}) as never,
  );
  const dataTabs = new DataTabManager(
    state,
    tabs,
    schedule,
    setActiveView,
    {} as never,
    {} as never,
  );
  const workflowState = new WorkflowState();
  const workflow = new WorkflowManager(state, workflowState, async () => ({
    rows: rec.workflowRows(),
    truncated: false,
  }));
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
    tabs,
    { getForType: async () => provider } as never,
    async () => {},
    () => {},
    () => ui.resetAISessionState(),
  );
  projects.setStarterTabManager(starterTabs);
  projects.setConnectionManager(connections);
  /** `UseDatabase.flush`: the view state, then a chat still streaming. */
  const flush = async () => {
    await windowState.flush();
    const streaming = state.aiStreamingChatId;
    if (streaming) await aiChats.persistMessages(streaming);
  };
  if (load) {
    await projects.initialize();
    await connections.initializePersistedConnections();
  }
  return {
    ctx,
    client,
    state,
    windowState,
    flush,
    restoration,
    projects,
    connections,
    panes,
    tabs,
    queryTabs,
    schemaTabs,
    explainTabs,
    erdTabs,
    statisticsTabs,
    extensionsDuckdbTabs,
    workflowTabs,
    dashboardTabs,
    dashboards,
    createTableTabs,
    dataTabs,
    starterTabs,
    ui,
    aiChats,
    workflowState,
    workflow,
  };
}

type Page = Awaited<ReturnType<typeof openPage>>;

/** The stores, imported fresh per case. */
async function importStores() {
  return {
    ai: (await import("$lib/stores/ai-settings.svelte")).aiSettingsStore,
    theme: (await import("$lib/stores/theme.svelte")).themeStore,
    onboarding: (await import("$lib/stores/onboarding.svelte")).onboardingStore,
    tutorial: (await import("$lib/stores/tutorial-progress.svelte")).tutorialProgressStore,
    tableplus: (await import("$lib/stores/connection-import.svelte")).tablePlusImportStore,
    dbeaver: (await import("$lib/stores/connection-import.svelte")).dbeaverImportStore,
    editor: (await import("$lib/stores/editor-settings.svelte")).editorSettingsStore,
    pending: (await import("$lib/stores/pending-changes-settings.svelte"))
      .pendingChangesSettingsStore,
    update: (await import("$lib/stores/update.svelte")).updateStore,
    nudge: (await import("$lib/stores/license-nudge.svelte")).licenseNudgeStore,
    notice: (await import("$lib/stores/connection-secrets-notice.svelte")).connectionSecretsNotice,
  };
}

type Stores = Awaited<ReturnType<typeof importStores>>;

/** One case's world: the file, its pages and its stores. */
class Ctx {
  pages = new Map<string, Page>();
  current!: Page;
  private storesOf = new Map<string, Stores>();
  constructor(
    readonly db: Db,
    /** The demo's `settings` group on the case's file, shared by its windows. */
    readonly settings: InstanceType<typeof TsSettings>,
  ) {}

  get page(): Page {
    return this.current;
  }

  /** Opens a window (a page load) on the file; it becomes the current page. */
  async open(name = "main", windowId = "main", load = true): Promise<Page> {
    const page = await openPage(this.db, name, windowId, load);
    this.pages.set(name, page);
    this.current = page;
    return page;
  }

  /** Runs `fn` on another page (its storage client), then settles. */
  async on<T>(name: string, fn: (p: Page) => Promise<T> | T): Promise<T> {
    const prev = this.current;
    this.current = this.pages.get(name)!;
    rec.storage = this.current.client;
    try {
      const out = await fn(this.current);
      await settle();
      return out;
    } finally {
      this.current = prev;
      rec.storage = prev.client;
    }
  }

  /** The stores of window `name` (a fresh module graph per window). */
  async stores(name = "main"): Promise<Stores> {
    let s = this.storesOf.get(name);
    if (!s) {
      vi.resetModules();
      // The stores read and write through the window's fresh `settings` seam.
      (await import("./library/index")).setSettings(this.settings);
      s = await importStores();
      this.storesOf.set(name, s);
    }
    return s;
  }
}

// ---------------------------------------------------------------- views

/** Drops keys holding `null`, `[]` or `{}`, so a view lists only what's there. */
function compact(o: Obj): Obj {
  const out: Obj = {};
  for (const [k, v] of Object.entries(o)) {
    if (v === null || v === undefined) continue;
    if (Array.isArray(v) && v.length === 0) continue;
    if (typeof v === "object" && !Array.isArray(v) && Object.keys(v as Obj).length === 0) continue;
    out[k] = v;
  }
  return out;
}

function byKey<T>(o: Record<string, T>, f: (v: T, k: string) => Json): Obj {
  return compact(
    Object.fromEntries(
      Object.keys(o)
        .sort()
        .map((k) => [k, f(o[k], k)]),
    ),
  );
}

/**
 * How many nodes each listed workflow's body holds. The page lists saved
 * workflows without their bodies (5d-2 Task 7), so the view reads each one
 * (`workflowGet`) as opening it would, raw: a workflow that won't decode is
 * listed, and says so only when it's opened.
 */
async function workflowNodes(p: Page): Promise<Map<string, number>> {
  const out = new Map<string, number>();
  for (const list of Object.values(p.state.savedWorkflowsByProject)) {
    for (const w of list) {
      const { value } = await getLibrary().getWorkflow(w.id);
      const nodes = (value as { nodes?: unknown } | null)?.nodes;
      out.set(w.id, Array.isArray(nodes) ? nodes.length : 0);
    }
  }
  return out;
}

/** What a window shows: its projects' tabs and layout, and the documents it holds. */
async function pageView(p: Page): Promise<Json> {
  const nodes = await workflowNodes(p);
  const s = p.state;
  const ids = (list: { id: string }[] | undefined) => (list ?? []).map((t) => t.id);
  const project = (pid: string) =>
    compact({
      queryTabs: (s.queryTabsByProject[pid] ?? []).map((t) =>
        compact({ id: t.id, name: t.name, query: t.query, queryId: t.queryId }),
      ),
      schemaTabs: (s.schemaTabsByProject[pid] ?? []).map((t) => ({
        id: t.id,
        connectionId: t.connectionId,
        table: `${t.table.schema}.${t.table.name}`,
      })),
      explainTabs: (s.explainTabsByProject[pid] ?? []).map((t) => ({
        id: t.id,
        name: t.name,
        sourceQuery: t.sourceQuery,
      })),
      erdTabs: ids(s.erdTabsByProject[pid]),
      statisticsTabs: ids(s.statisticsTabsByProject[pid]),
      workflowTabs: ids(s.workflowTabsByProject[pid]),
      dashboardTabs: (s.dashboardTabsByProject[pid] ?? []).map((t) => ({
        id: t.id,
        dashboardId: t.dashboardId,
      })),
      createTableTabs: ids(s.createTableTabsByProject[pid]),
      dataTabs: (s.dataTabsByProject[pid] ?? []).map((t) => ({
        id: t.id,
        table: `${t.schemaName}.${t.tableName}`,
      })),
      starterTabs: ids(s.starterTabsByProject[pid]),
      extensionsDuckdbTabs: ids(s.extensionsDuckdbTabsByProject[pid]),
      tabOrder: s.tabOrderByProject[pid],
      paneLayout: s.paneLayoutByProject[pid]
        ? plain({
            panes: s.paneLayoutByProject[pid].panes.map((x) => ({
              id: x.id,
              tabIds: x.tabIds,
              activeTabId: x.activeTabId,
            })),
            activePaneId: s.paneLayoutByProject[pid].activePaneId,
          })
        : null,
      active: compact({
        query: s.activeQueryTabIdByProject[pid],
        schema: s.activeSchemaTabIdByProject[pid],
        explain: s.activeExplainTabIdByProject[pid],
        erd: s.activeErdTabIdByProject[pid],
        statistics: s.activeStatisticsTabIdByProject[pid],
        workflow: s.activeWorkflowTabIdByProject[pid],
        dashboard: s.activeDashboardTabIdByProject[pid],
        createTable: s.activeCreateTableTabIdByProject[pid],
        data: s.activeDataTabIdByProject[pid],
        starter: s.activeStarterTabIdByProject[pid],
        extensionsDuckdb: s.activeExtensionsDuckdbTabIdByProject[pid],
      }),
      activeView: s.activeViewByProject[pid],
      activeConnectionId: s.activeConnectionIdByProject[pid],
      connectionOrder: s.connectionOrderByProject[pid],
    });
  return compact({
    activeProjectId: s.activeProjectId,
    activeView: s.activeView,
    projects: byKey(s.queryTabsByProject, (_v, pid) => project(pid)),
    savedWorkflows: byKey(s.savedWorkflowsByProject, (ws) =>
      ws.map((w) => ({ id: w.id, name: w.name, nodes: nodes.get(w.id) ?? 0 })),
    ),
    dashboards: byKey(s.dashboardsByProject, (ds) =>
      ds.map((d) => ({
        id: d.id,
        name: d.name,
        starred: !!d.starred,
        shared: !!d.shared,
        widgets: d.widgets.map((w) => w.id),
      })),
    ),
    dashboardVersions: byKey(s.dashboardVersionsByProject, (vs) =>
      vs.map((v) => `${v.dashboardId}#${v.version}`),
    ),
    chats: byKey(s.aiChatsByConnection, (cs) => cs.map((c) => ({ id: c.id, title: c.title }))),
    activeChats: compact({ ...s.activeAIChatIdByConnection }),
    messages: byKey(s.aiMessagesByChat, (ms) =>
      ms.map((m) =>
        compact({
          id: m.id,
          role: m.role,
          content: m.content,
          pendingApproval: m.pendingApproval ? true : null,
          pendingModelSelection: m.pendingModelSelection ?? null,
        }),
      ),
    ),
    streamingChatId: s.aiStreamingChatId,
  });
}

const aiView = async (t: Ctx) => plain({ settings: (await t.stores()).ai.settings });
async function themeView(t: Ctx): Promise<Json> {
  const { theme } = await t.stores();
  return plain({
    preferences: theme.preferences,
    userThemes: theme.userThemes.map((x) => ({ id: x.id, name: x.name, isDark: x.isDark })),
  });
}
async function onboardingView(t: Ctx): Promise<Json> {
  const { onboarding: o } = await t.stores();
  return plain({
    isFirstRun: o.isFirstRun,
    userBackground: o.userBackground,
    hasCompletedWizard: o.hasCompletedWizard,
    showWizardHints: o.showWizardHints,
    dismissedHints: o.dismissedHints,
    learnEnabled: o.learnEnabled,
  });
}
async function tutorialView(t: Ctx): Promise<Json> {
  const { tutorial } = await t.stores();
  return plain({
    completed: byKey(tutorial.completedChallenges, (set) => [...set].sort()),
    states: byKey(tutorial.challengeStates, (m) => m),
  });
}
async function importView(t: Ctx): Promise<Json> {
  const s = await t.stores();
  return { tableplus: s.tableplus.hasOfferedImport, dbeaver: s.dbeaver.hasOfferedImport };
}
async function settingsView(t: Ctx): Promise<Json> {
  const s = await t.stores();
  return plain({
    editorKeybindingMode: s.editor.keybindingMode,
    pendingChangesEnabled: s.pending.enabled,
    skippedUpdateVersion: s.update.skippedVersion,
    licenseNudge: {
      queryCount: s.nudge.queryCount,
      activeDays: s.nudge.activeDays,
      lastActiveDay: s.nudge.lastActiveDay,
      answer: s.nudge.answer,
      snoozedUntil: s.nudge.snoozedUntil,
    },
  });
}

// ---------------------------------------------------------------- matching the recording

type ViewFn = (t: Ctx) => unknown;

const FIXTURES = join(process.cwd(), "crates/seaquel-workspace/tests/fixtures/state");
const ID = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
const NOW = "2030-01-01T\\d\\d:\\d\\d:\\d\\d\\.\\d{3}Z";

/**
 * The ids made during the case, bound to the recording's `<id:n>` tokens
 * the first time they meet (both ways: one token, one id).
 */
class Binding {
  readonly byToken = new Map<string, string>();
  readonly byId = new Map<string, string>();
  copy(): Binding {
    const b = new Binding();
    for (const [k, v] of this.byToken) b.byToken.set(k, v);
    for (const [k, v] of this.byId) b.byId.set(k, v);
    return b;
  }
  take(other: Binding): void {
    this.byToken.clear();
    this.byId.clear();
    for (const [k, v] of other.byToken) this.byToken.set(k, v);
    for (const [k, v] of other.byId) this.byId.set(k, v);
  }
}

const escape = (t: string) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** `e` (a recorded string with `<id:n>` and `<now>` tokens) against `a`, binding ids. */
function stringMatches(e: string, a: string, b: Binding): boolean {
  if (!e.includes("<id:") && !e.includes("<now>")) return e === a;
  const tokens: string[] = [];
  const pattern = e
    .split(/(<id:\d+>|<now>)/)
    .map((part) => {
      if (part === "<now>") return NOW;
      if (/^<id:\d+>$/.test(part)) {
        tokens.push(part);
        return `(${ID})`;
      }
      return escape(part);
    })
    .join("");
  const m = new RegExp(`^${pattern}$`).exec(a);
  if (!m) return false;
  for (const [i, token] of tokens.entries()) {
    const id = m[i + 1];
    const was = b.byToken.get(token);
    if (was !== undefined && was !== id) return false;
    const owner = b.byId.get(id);
    if (owner !== undefined && owner !== token) return false;
    b.byToken.set(token, id);
    b.byId.set(id, token);
  }
  return true;
}

/** Structural match of a recorded value against an actual one (object keys may hold tokens). */
function valueMatches(e: Json, a: Json, b: Binding): boolean {
  if (typeof e === "string") return typeof a === "string" && stringMatches(e, a, b);
  if (e === null || typeof e !== "object") return e === a;
  if (Array.isArray(e)) {
    return Array.isArray(a) && e.length === a.length && e.every((x, i) => valueMatches(x, a[i], b));
  }
  if (a === null || typeof a !== "object" || Array.isArray(a)) return false;
  const eo = e as Obj;
  const ao = a as Obj;
  const eKeys = Object.keys(eo);
  const aKeys = Object.keys(ao);
  if (eKeys.length !== aKeys.length) return false;
  const used = new Set<string>();
  for (const ek of eKeys) {
    let found = false;
    for (const ak of aKeys) {
      if (used.has(ak)) continue;
      const trial = b.copy();
      if (stringMatches(ek, ak, trial) && valueMatches(eo[ek], ao[ak], trial)) {
        b.take(trial);
        used.add(ak);
        found = true;
        break;
      }
    }
    if (!found) return false;
  }
  return true;
}

/** Columns holding JSON, compared parsed (the README's rule). */
const JSON_COLUMNS: Record<string, string[]> = {
  saved_canvases: ["data"],
  dashboards: ["widgets", "viewport", "date_filter"],
  dashboard_versions: ["snapshot"],
  user_themes: ["data"],
  onboarding_state: ["data"],
};

/** The AI settings record after the legacy provider cleanup (the README's rule). */
function cleanAi(value: Json): Json {
  if (!value || typeof value !== "object" || Array.isArray(value)) return value;
  const o = { ...(value as Obj) };
  if (Array.isArray(o.providers)) {
    o.providers = (o.providers as Json[]).map((p) => {
      if (!p || typeof p !== "object") return p;
      const { model: _m, provider, ...rest } = p as Obj;
      return { ...rest, type: rest.type ?? provider ?? "anthropic" };
    });
  }
  return o;
}

function parseCell(table: string, row: Row, column: string, value: Json): Json {
  const json =
    JSON_COLUMNS[table]?.includes(column) ||
    (table === "app_state" &&
      column === "value" &&
      (row.key === "license_nudge" || row.key === "aiSettings"));
  if (!json || typeof value !== "string") return value;
  let parsed: Json;
  try {
    parsed = JSON.parse(value);
  } catch {
    return value;
  }
  if (table === "dashboard_versions" && parsed && typeof parsed === "object") {
    const o = parsed as Obj;
    if (o.description === null) delete o.description;
  }
  return table === "app_state" && row.key === "aiSettings" ? cleanAi(parsed) : parsed;
}

function parsedRow(table: string, row: Row): Row {
  return Object.fromEntries(Object.entries(row).map(([k, v]) => [k, parseCell(table, row, k, v)]));
}

/** The recorded rows of a table against the actual ones, as a set (greedy binding). */
function tableMatches(table: string, expected: Row[], actual: Row[], b: Binding): boolean {
  if (expected.length !== actual.length) return false;
  const used = new Set<number>();
  for (const e of expected.map((r) => parsedRow(table, r))) {
    let found = false;
    for (const [i, a] of actual.entries()) {
      if (used.has(i)) continue;
      const trial = b.copy();
      if (valueMatches(e, parsedRow(table, a), trial)) {
        b.take(trial);
        used.add(i);
        found = true;
        break;
      }
    }
    if (!found) return false;
  }
  return true;
}

/** The tables the TS replay compares: `project_state` and `tabs` are the mirror (see the header). */
const COMPARED = DUMP.map(([t]) => t).filter((t) => t !== "project_state" && t !== "tabs");

interface Fixture {
  name: string;
  target?: string;
  steps: {
    op: string;
    outcome: { ok: boolean };
    rows: Record<string, Row[]>;
    files: Json[];
    view: Json;
  }[];
}

const loadFixtures = (file: string) =>
  JSON.parse(readFileSync(join(FIXTURES, file), "utf8")) as Fixture[];
const CHANGES = JSON.parse(readFileSync(join(FIXTURES, "changes.json"), "utf8")) as Record<
  string,
  { expected?: { steps?: Record<string, Obj> } }
>;

/**
 * Steps whose outcome the demo can't reproduce the recorded way, each for a
 * reason (field, then why). Everything else about the step is compared.
 */
/** The exemptions a replay used, so a stale one fails the test. */
const exemptSeen = new Set<string>();

const EXEMPT: Record<string, string> = {
  // The managers show a refused edit (a toast) instead of throwing it; the
  // rows (nothing written) are compared.
  "dashboards/delete-during-pending-edit#1 outcome":
    "The pan's DASHBOARD_NOT_FOUND is shown, not thrown; the dashboard stays removed.",
  "settings/editor-keybinding-mode#2 outcome":
    "A refused key binding is logged and kept for the session, not thrown; nothing is stored.",
};

/**
 * The demo's `TsSettings` refuses API keys (it has no keychain, as the web
 * workspace has none). The cases run as the desktop, where Core takes the
 * key with the provider; secrets aren't compared, so the key is dropped
 * here and the rest of the call goes through.
 */
function keyless(settings: InstanceType<typeof TsSettings>): InstanceType<typeof TsSettings> {
  return new Proxy(settings, {
    get(target, method: string) {
      const fn = (target as unknown as Record<string, (...a: unknown[]) => unknown>)[method];
      if (typeof fn !== "function") return fn;
      if (method === "createAiProvider") return (draft: unknown) => fn.call(target, draft);
      if (method === "updateAiProvider") {
        return (id: unknown, patch: unknown) => fn.call(target, id, patch);
      }
      return fn.bind(target);
    },
  });
}

/** Replays case `c` against its recording `fx`; the differences found, per step. */
async function replay(c: Case, fx: Fixture): Promise<string[]> {
  uuidSeq = 0;
  rec.env.web = !!c.web;
  rec.toasts.length = 0;
  rec.secretCalls.length = 0;
  rec.secrets = new Map(Object.entries(c.secrets ?? {}));
  rec.ai = null;
  rec.workflowRows = () => [];
  run.storage = [];
  run.core = [];
  run.windows = new Map();
  run.windowMeta = new Map();
  run.lastActive = null;
  run.clock = 0;
  vi.useFakeTimers({
    toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"],
    now: FIXED,
  });
  const failures: string[] = [];
  try {
    const db = new WebSqliteDatabase(new SQL.Database());
    await bootstrapSqljsDatabase(db);
    if (c.file === "v2026.4.5-beta.1") await makeBetaEra(db);
    const seed = c.seed ?? standard();
    await insertRows(db, seed);
    setLibrary(new TsLibrary(db));
    const settings = keyless(new TsSettings(db));
    setSettings(settings);
    // Store cases open no page: their storage calls go through this one.
    rec.storage = pageClient(createSqljsStorageClient(db), {
      name: "main",
      windowId: "main",
      baseline: newBaseline(),
      db,
    });
    const t = new Ctx(db, settings);
    const view: ViewFn =
      (c.view as ViewFn | undefined) ?? ((x) => (x.current ? pageView(x.current) : null));
    const b = new Binding();
    const ids: string[] = [];
    for (const [i, step] of c.steps.entries()) {
      const recorded = fx.steps[i];
      rec.toasts.length = 0;
      rec.secretCalls.length = 0;
      let ok: boolean;
      try {
        await step.run(t, ids);
        ok = true;
      } catch {
        ok = false;
      }
      await settle();
      const change = CHANGES[c.name]?.expected?.steps?.[String(i)];
      const why: string[] = [];
      const expectedOk = ((change?.outcome as Obj | undefined) ?? recorded.outcome).ok === true;
      for (const key of Object.keys(EXEMPT)) {
        if (key.startsWith(`${c.name}#${i} `)) exemptSeen.add(key);
      }
      if (ok !== expectedOk && !EXEMPT[`${c.name}#${i} outcome`]) {
        why.push(`outcome: expected ok=${expectedOk}, got ok=${ok}`);
      }
      // An entry's rows are the whole dump after the step (an absent table is empty).
      const rows = (change?.rows as Record<string, Row[]> | undefined) ?? recorded.rows;
      const actual = await dump(db);
      for (const table of COMPARED) {
        const e = rows[table] ?? [];
        const a = actual[table] ?? [];
        if (EXEMPT[`${c.name}#${i} rows:${table}`]) continue;
        if (!tableMatches(table, e, a, b)) {
          why.push(`${table}:\n  expected ${JSON.stringify(e)}\n  actual   ${JSON.stringify(a)}`);
        }
      }
      // `files` (a shared dashboard's file calls) aren't compared: Core
      // writes a shared row's file inside the library call since phase 5e.
      const expectedView = change && "view" in change ? change.view : recorded.view;
      if (expectedView !== null && !EXEMPT[`${c.name}#${i} view`]) {
        const shown = plain(await view(t));
        if (!valueMatches(expectedView, shown, b)) {
          why.push(
            `view:\n  expected ${JSON.stringify(expectedView)}\n  actual   ${JSON.stringify(shown)}`,
          );
        }
      }
      if (why.length) failures.push(`${c.name} step ${i} (${step.op}):\n${why.join("\n")}`);
    }
  } finally {
    rec.ai = null;
    vi.clearAllTimers();
    vi.useRealTimers();
  }
  return failures;
}

// ---------------------------------------------------------------- change helpers

/** The rows before step `i` (the seed's rows for step 0). */
function rowsBefore(raw: RawCase, i: number): Record<string, Row[]> {
  return i === 0 ? raw.before : raw.steps[i - 1].rows;
}

/** `rows` with `table` replaced (dropped when empty, as the dump does). */
function withTable(rows: Record<string, Row[]>, table: string, next: Row[]): Record<string, Row[]> {
  const out = { ...rows };
  if (next.length) out[table] = next;
  else delete out[table];
  return out;
}

/** A refusal at step `i`: nothing written. */
function refused(raw: RawCase, i: number, outcome: Obj): Record<number, StepChange> {
  return { [i]: { outcome: { ok: false, ...outcome }, rows: rowsBefore(raw, i) } };
}

/** The Core calls step `i` recorded. */
function coreOf(raw: RawCase, i: number): CoreCall[] {
  return raw.steps[i].core ?? [];
}

// ---------------------------------------------------------------- seeds

const P1: Row = {
  id: "p1",
  name: "Main",
  description: null,
  git_repo_path: null,
  created_at: T0,
  updated_at: T0,
};
const P2: Row = { ...P1, id: "p2", name: "Other" };

/** A stored connection row with defaults. */
function conn(over: Row): Row {
  return {
    id: "c1",
    project_id: "p1",
    name: "Local",
    type: "postgres",
    host: "localhost",
    port: 5432,
    database_name: "app",
    username: "me",
    ssl_mode: null,
    connection_string: null,
    last_connected: T0,
    ssh_tunnel: null,
    save_password: 0,
    save_ssh_password: 0,
    save_ssh_key_passphrase: 0,
    is_local_only: 1,
    shared_connection_id: null,
    ai_share_schema: null,
    ai_share_data: null,
    active_ai_provider_id: null,
    active_ai_model: null,
    ...over,
  };
}

/** `c1` (Postgres, with an AI model chosen) and `c2` (DuckDB), both in `p1`. */
const C1 = conn({ active_ai_provider_id: "prov-1", active_ai_model: "model-1" });
const C2 = conn({
  id: "c2",
  name: "Duck",
  type: "duckdb",
  host: "",
  port: 0,
  database_name: "/data/app.duckdb",
  username: "",
});

/** Projects `p1` "Main" and `p2` "Other", connections `c1` and `c2`, `p1` last active; plus `extra`. */
function standard(extra: Seed = {}): Seed {
  const base: Seed = {
    projects: [P1, P2],
    connections: [C1, C2],
    // Retired (Q13): no call reads or writes it, and every page case checks it survives.
    connection_overrides: [
      {
        shared_connection_id: "repo-1:prod",
        username: "me",
        host_override: "localhost",
        port_override: 15432,
        save_password: 1,
        save_ssh_password: 0,
        save_ssh_key_passphrase: 0,
      },
    ],
    app_state: [{ key: "lastActiveProjectId", value: "p1" }],
  };
  const out: Seed = {};
  for (const table of SEED_TABLES) {
    const rows = [...(base[table] ?? []), ...(extra[table] ?? [])];
    if (rows.length) out[table] = rows;
  }
  return out;
}

/** Only `extra`: the store cases need no projects. */
function bare(extra: Seed = {}): Seed {
  return extra;
}

const ORDERS = {
  schema: "public",
  name: "orders",
  type: "table",
  columns: [
    {
      name: "id",
      type: "integer",
      nullable: false,
      isPrimaryKey: true,
      isForeignKey: false,
    },
  ],
  indexes: [],
} as unknown as SchemaTable;

// ---------------------------------------------------------------- step helpers

function step(op: string, args: Step["args"], runFn: Step["run"], core?: Step["core"]): Step {
  return { op, ...(args !== undefined ? { args } : {}), run: runFn, ...(core ? { core } : {}) };
}

/** A window opening (a page load): projects, the active project's state, connections. */
function open(page = "main", windowId = "main"): Step {
  return step("page.open", { page, windowId }, async (t) => {
    await t.open(page, windowId);
    return null;
  });
}

function activate(connectionId: string, projectId = "p1"): Step {
  return step("connections.setActiveForProject", { connectionId, projectId }, async (t) =>
    t.page.connections.setActiveForProject(connectionId, projectId),
  );
}

function addQueryTab(name: string, query: string): Step {
  return step("queryTabs.add", { name, query }, async (t, ids) => {
    const id = t.page.queryTabs.add(name, query)!;
    ids.push(id);
    return id;
  });
}

function switchProject(id: string): Step {
  return step("projects.setActive", { id }, async (t) => t.page.projects.setActive(id));
}

// ---------------------------------------------------------------- cases: view state

/** Today's `project_state` and `tabs` rows for `p1`, with one tab of each stored type. */
const LEGACY_P1: Seed = {
  project_state: [
    {
      project_id: "p1",
      active_view: "query",
      active_connection_id: "c1",
      active_query_tab_id: "tab-q1",
      active_schema_tab_id: "tab-s1",
      active_explain_tab_id: null,
      active_erd_tab_id: null,
      active_statistics_tab_id: null,
      active_workflow_tab_id: null,
      active_visualize_tab_id: null,
      active_starter_tab_id: null,
      active_dashboard_tab_id: null,
      active_create_table_tab_id: null,
      active_data_tab_id: "tab-d1",
      tab_order: '["tab-q1","tab-s1","tab-d1","tab-e1"]',
      connection_order: '["c1","c2"]',
      starred_shared_query_ids: "[]",
      starred_shared_dashboard_ids: "[]",
      pane_layout: null,
    },
  ],
  tabs: [
    {
      id: "tab-q1",
      project_id: "p1",
      tab_type: "query",
      name: "Orders",
      query: "SELECT * FROM orders",
      saved_query_id: null,
    },
    {
      id: "tab-s1",
      project_id: "p1",
      tab_type: "schema",
      name: "orders",
      table_name: "orders",
      schema_name: "public",
      connection_id: "c1",
    },
    {
      id: "tab-d1",
      project_id: "p1",
      tab_type: "data",
      name: "orders",
      table_name: "orders",
      schema_name: "public",
      connection_id: "c1",
    },
    {
      id: "tab-e1",
      project_id: "p1",
      tab_type: "explain",
      name: "Explain: SELECT 1...",
      source_query: "SELECT 1",
    },
  ],
};

/** The mirror rows a save of `state` writes, as today's save would with repeated tab ids skipped. */
async function mirrorRows(raw: RawCase, i: number, state: Obj): Promise<Record<string, Row[]>> {
  const db = new WebSqliteDatabase(new SQL.Database());
  await bootstrapSqljsDatabase(db);
  await insertRows(db, { projects: raw.seed.projects, connections: raw.seed.connections });
  const before = rowsBefore(raw, i);
  await insertRows(db, before);
  const seen = new Set<string>();
  const firstOnly = <T extends { id: string }>(list: T[] | undefined) =>
    (list ?? []).filter((t) => (seen.has(t.id) ? false : (seen.add(t.id), true)));
  const s = { ...state } as Obj;
  for (const key of [
    "queryTabs",
    "schemaTabs",
    "explainTabs",
    "erdTabs",
    "statisticsTabs",
    "workflowTabs",
    "starterTabs",
    "dashboardTabs",
    "createTableTabs",
    "dataTabs",
  ]) {
    s[key] = firstOnly(s[key] as { id: string }[]);
  }
  const stored = await db.query<{ data: string }>(
    "SELECT data FROM saved_canvases WHERE project_id = ?",
    [s.projectId],
  );
  // The mirror keeps the stored connection order and never touches saved workflows.
  const order = await db.query<{ connection_order: string }>(
    "SELECT connection_order FROM project_state WHERE project_id = ?",
    [s.projectId],
  );
  await projectStateRepo.save(db, {
    ...s,
    connectionOrder: order.length ? JSON.parse(order[0].connection_order) : [],
    savedWorkflows: [],
  } as unknown as Parameters<typeof projectStateRepo.save>[1]);
  for (const r of stored) {
    await db.execute("INSERT INTO saved_canvases (id, project_id, data) VALUES (?, ?, ?)", [
      (JSON.parse(r.data) as Obj).id,
      s.projectId,
      r.data,
    ]);
  }
  const after = await dump(db);
  let out = withTable(before, "project_state", after.project_state ?? []);
  out = withTable(out, "tabs", after.tabs ?? []);
  return out;
}

const viewCases: Case[] = [
  {
    name: "view-state/first-load-empty",
    note: "A file with no project_state row: the first window's load answers nothing (copiedFrom empty) and the starter tabs are added and saved.",
    steps: [open()],
  },
  {
    name: "view-state/query-tabs",
    note: "Two query tabs, a text change, the first made active, then the app restarts (the main window again).",
    steps: [
      open(),
      addQueryTab("Orders", "SELECT * FROM orders"),
      step("queryTabs.add", { name: "Scratch" }, async (t, ids) => {
        const id = t.page.queryTabs.add("Scratch")!;
        ids.push(id);
        return id;
      }),
      step(
        "queryTabs.updateContent",
        (ids) => ({ id: ids[1], query: "SELECT 2" }),
        async (t, ids) => t.page.queryTabs.updateContent(ids[1], "SELECT 2"),
      ),
      step(
        "queryTabs.setActive",
        (ids) => ({ id: ids[0] }),
        async (t, ids) => t.page.queryTabs.setActive(ids[0]),
      ),
      step(
        "queryTabs.remove",
        (ids) => ({ id: ids[1] }),
        async (t, ids) => t.page.queryTabs.remove(ids[1]),
      ),
      open("restart", "main"),
    ],
  },
  {
    name: "view-state/schema-data-and-create-table-tabs",
    steps: [
      open(),
      activate("c1"),
      step("schemaTabs.add", { table: "public.orders" }, async (t) =>
        t.page.schemaTabs.add(ORDERS),
      ),
      step("dataTabs.addWithoutRefresh", { table: "public.orders" }, async (t) =>
        t.page.dataTabs.addWithoutRefresh(ORDERS),
      ),
      step("createTableTabs.add", { schemaName: "public" }, async (t) =>
        t.page.createTableTabs.add("public"),
      ),
      open("restart", "main"),
    ],
  },
  {
    name: "view-state/erd-statistics-workflow-tabs",
    steps: [
      open(),
      activate("c1"),
      step("erdTabs.add", undefined, async (t) => t.page.erdTabs.add()),
      step("statisticsTabs.add", undefined, async (t) => t.page.statisticsTabs.add()),
      step("workflowTabs.add", undefined, async (t) => t.page.workflowTabs.add()),
      open("restart", "main"),
    ],
  },
  {
    name: "view-state/explain-tab",
    note: "An explain tab as `ExplainTabManager.execute` opens one (the recorder appends it the same way; the run itself needs an engine).",
    steps: [
      open(),
      activate("c1"),
      step("explainTabs.execute", { sourceQuery: "SELECT 1" }, async (t, ids) => {
        const s = t.page.state;
        const id = `explain-${crypto.randomUUID()}`;
        ids.push(id);
        s.explainTabsByProject = {
          ...s.explainTabsByProject,
          p1: [
            ...(s.explainTabsByProject.p1 ?? []),
            { id, name: "Explain: SELECT 1...", sourceQuery: "SELECT 1", isExecuting: false },
          ],
        };
        s.activeExplainTabIdByProject = { ...s.activeExplainTabIdByProject, p1: id };
        t.page.tabs.add(id);
        t.page.ui.setActiveView("explain");
        return id;
      }),
      open("restart", "main"),
    ],
  },
  {
    name: "view-state/dashboard-tab",
    steps: [
      open(),
      step("dashboards.createDashboard", { name: "Sales" }, async (t, ids) => {
        const d = await t.page.dashboards.createDashboard("Sales");
        ids.push(d!.id);
        return d!.id;
      }),
      step(
        "dashboardTabs.add",
        (ids) => ({ dashboardId: ids[0], name: "Sales" }),
        async (t, ids) => t.page.dashboardTabs.add(ids[0], "Sales"),
      ),
      open("restart", "main"),
    ],
  },
  {
    name: "view-state/extensions-tab",
    note: "A DuckDB extensions tab: today's save and load drop it (its id stays in tabOrder). Core keeps it in the window's row and leaves it out of the mirror.",
    steps: [
      open(),
      activate("c2"),
      step("extensionsDuckdbTabs.add", undefined, async (t) => t.page.extensionsDuckdbTabs.add()),
      open("restart", "main"),
    ],
    change: {
      decision: "22",
      why: "Extensions tabs stay in the window's row (today the save drops them). The mirror rows are today's, so only the restarted window's load differs: windowStateLoad answers the saved state with the extensions tab (its `expect`), and the window shows it.",
      expected: (raw) => ({ [raw.steps.length - 1]: { view: null } }),
    },
  },
  {
    name: "view-state/pane-layout-one-pane",
    note: "One pane: the layout isn't saved (paneLayout absent), and the restart rebuilds it.",
    steps: [
      open(),
      addQueryTab("A", "SELECT 1"),
      step("panes.ensureLayout", undefined, async (t) => void t.page.panes.ensureLayout()),
      step("panes.setActivePane", undefined, async (t) =>
        t.page.panes.setActivePane(t.page.panes.ensureLayout().panes[0].id),
      ),
      open("restart", "main"),
    ],
  },
  {
    name: "view-state/pane-layout-split",
    steps: [
      open(),
      addQueryTab("A", "SELECT 1"),
      addQueryTab("B", "SELECT 2"),
      step(
        "panes.splitRight",
        (ids) => ({ tabId: ids[1] }),
        async (t, ids) => {
          const pane = t.page.panes.ensureLayout().panes[0];
          t.page.panes.splitRight(pane.id, ids[1], pane.id);
        },
      ),
      step("panes.setActivePane", { pane: 0 }, async (t) =>
        t.page.panes.setActivePane(t.page.panes.ensureLayout().panes[0].id),
      ),
      open("restart", "main"),
    ],
  },
  {
    name: "view-state/active-view-per-project",
    note: "Each project keeps the view it was left on (Task 1): p1 on a dashboard tab, p2 on its starter tabs.",
    steps: [
      open(),
      step("dashboardTabs.add", { name: "Board" }, async (t) =>
        t.page.dashboardTabs.add("dash-x", "Board"),
      ),
      switchProject("p2"),
      step("ui.setActiveView", { view: "query" }, async (t) => t.page.ui.setActiveView("query")),
      switchProject("p1"),
      open("restart", "main"),
    ],
  },
  {
    name: "view-state/active-connection",
    note: "The active connection is per window (Q14); the mirror writes the saving window's.",
    steps: [open(), activate("c2"), open("restart", "main")],
  },
  {
    name: "view-state/connection-order",
    note: "The connection order is the project's (projectSidebarSet), not the window's.",
    steps: [
      open(),
      step("connections.reorder", { projectId: "p1", orderedIds: ["c2", "c1"] }, async (t) =>
        t.page.connections.reorder("p1", ["c2", "c1"]),
      ),
      open("restart", "main"),
    ],
  },
  {
    name: "view-state/repeated-tab-id",
    note: "Two query tabs with one id: today's save fails on the tabs key and stores nothing. Core stores the window's state as sent and the mirror skips the repeat.",
    steps: [
      open(),
      addQueryTab("A", "SELECT 1"),
      step(
        "queryTabs.add",
        (ids) => ({ name: "A copy", query: "SELECT 2", id: ids[0] }),
        async (t, ids) => {
          const s = t.page.state;
          s.queryTabsByProject = {
            ...s.queryTabsByProject,
            p1: [
              ...(s.queryTabsByProject.p1 ?? []),
              { id: ids[0], name: "A copy", query: "SELECT 2", isExecuting: false },
            ],
          };
          t.page.windowState.scheduleProject("p1");
        },
      ),
    ],
    change: {
      decision: "22",
      why: "The legacy mirror skips a tab whose id repeats within the state instead of failing the save (the window's own row keeps the state as sent). Today the whole save fails on the tabs key and nothing is stored.",
      expected: async (raw) => {
        const i = 2;
        const save = coreOf(raw, i).find((c) => c.method === "windowStateSave")!;
        return { [i]: { rows: await mirrorRows(raw, i, (save.params as Obj).state as Obj) } };
      },
    },
  },
  {
    name: "view-state/save-before-load",
    note: "A save of a project this page hasn't loaded is refused in the GUI since Task 1: nothing is sent.",
    steps: [
      open(),
      step("persistence.persistProjectState", { projectId: "p2" }, async (t) =>
        t.page.windowState.saveNow("p2"),
      ),
    ],
  },
  {
    name: "view-state/first-load-from-legacy-rows",
    note: "The first window after the upgrade: no window rows, so windowStateLoad answers today's rows (copiedFrom legacy), which must equal today's load.",
    seed: standard(LEGACY_P1),
    steps: [open(), open("restart", "main")],
  },
  {
    name: "view-state/second-window",
    note: "A second window copies the most recently used window's state, then each keeps its own. Today both write the one stored state, so the restarted main window shows the second window's tabs.",
    seed: standard(LEGACY_P1),
    steps: [
      open(),
      addQueryTab("Main's", "SELECT 'main'"),
      open("second", "win-2"),
      addQueryTab("Second's", "SELECT 'second'"),
      open("restart", "main"),
    ],
    change: {
      decision: "22",
      why: "View state is per window (Q12 B): the restarted main window gets its own row back (its windowStateLoad `expect`), where today it loads the last save, the second window's.",
      expected: (raw) => ({ [raw.steps.length - 1]: { view: null } }),
    },
  },
  {
    name: "view-state/project-switch",
    note: "A switch saves the old project at once, activates the new one (lastActiveProjectId) and loads it (no row: starter tabs).",
    steps: [
      open(),
      addQueryTab("A", "SELECT 1"),
      switchProject("p2"),
      addQueryTab("B", "SELECT 2"),
      switchProject("p1"),
    ],
  },
];

// ---------------------------------------------------------------- cases: workflows

/** A query node run's rows: a bigint and bytes, which `toStorable` tags. */
const RUN_ROWS = () => [
  { id: 9007199254740993n, payload: new Uint8Array([1, 2, 255]), name: "a" },
  { id: 2n, payload: new Uint8Array([]), name: "b" },
];

function addQueryNode(sql: string): Step {
  return step("workflow.addQueryNode", { query: sql }, async (t, ids) => {
    const id = t.page.workflow.addQueryNode(sql, { x: 100, y: 100 });
    ids.push(id);
    return id;
  });
}

function saveWorkflow(name: string): Step {
  return step("workflow.saveWorkflow", { name }, async (t, ids) => {
    const w = await t.page.workflow.saveWorkflow(name);
    if (!w) throw new Error("not saved");
    ids.push(w.id);
    return { id: w.id, name: w.name };
  });
}

/** A saved workflow row whose chart node holds a copy of its source's rows (saved before 5d-2). */
function chartCopyRow(id: string, name: string): Row {
  const data = {
    id,
    name,
    projectId: "p1",
    nodes: [
      {
        id: "n-q",
        type: "queryNode",
        position: { x: 0, y: 0 },
        data: {
          type: "query",
          name: "Query",
          query: "SELECT 1",
          connectionId: "c1",
          isExecuting: false,
        },
      },
      {
        id: "n-r",
        type: "resultNode",
        position: { x: 370, y: 0 },
        data: {
          type: "result",
          sourceQueryNodeId: "n-q",
          columns: ["n"],
          rows: [[1], [2]],
          totalRows: 2,
        },
      },
      {
        id: "n-c",
        type: "chartNode",
        position: { x: 740, y: 0 },
        data: {
          type: "chart",
          sourceNodeId: "n-r",
          columns: ["n"],
          rows: [[1], [2]],
          chartConfig: { type: "bar", xAxis: "n", yAxis: ["n"] },
        },
      },
    ],
    edges: [
      {
        id: "n-q-n-r",
        source: "n-q",
        target: "n-r",
        sourceHandle: "output",
        targetHandle: "input",
      },
      {
        id: "n-r-n-c",
        source: "n-r",
        target: "n-c",
        sourceHandle: "output",
        targetHandle: "input",
      },
    ],
    viewport: { x: 0, y: 0, zoom: 1 },
    createdAt: T0,
    updatedAt: T0,
  };
  return { id, project_id: "p1", data: JSON.stringify(data) };
}

/** A stored workflow whose rows don't decode (`fromStorable` throws), so today's load drops it. */
const UNDECODABLE: Row = {
  id: "workflow-bad",
  project_id: "p1",
  data: JSON.stringify({
    id: "workflow-bad",
    name: "Bad",
    projectId: "p1",
    nodes: [
      {
        id: "n-r",
        type: "resultNode",
        position: { x: 0, y: 0 },
        data: {
          type: "result",
          sourceQueryNodeId: "x",
          columns: ["n"],
          rows: [[{ $sq: "bigint", v: "x" }]],
        },
      },
    ],
    edges: [],
    viewport: { x: 0, y: 0, zoom: 1 },
    createdAt: T0,
    updatedAt: T0,
  }),
};

/** A stored `project_state` row for `p1` with no tabs (saved workflows load only with one). */
const STATE_P1: Row = {
  project_id: "p1",
  active_view: "query",
  active_connection_id: null,
  active_query_tab_id: null,
  active_schema_tab_id: null,
  active_explain_tab_id: null,
  active_erd_tab_id: null,
  active_statistics_tab_id: null,
  active_workflow_tab_id: null,
  active_visualize_tab_id: null,
  active_starter_tab_id: null,
  active_dashboard_tab_id: null,
  active_create_table_tab_id: null,
  active_data_tab_id: null,
  tab_order: "[]",
  connection_order: '["c1","c2"]',
  starred_shared_query_ids: "[]",
  starred_shared_dashboard_ids: "[]",
  pane_layout: null,
};

/** Binary order, as SQLite's `ORDER BY` on text. */
const byId = (a: Row, b: Row) =>
  String(a.id) < String(b.id) ? -1 : String(a.id) > String(b.id) ? 1 : 0;

const workflowCases: Case[] = [
  {
    name: "workflows/create",
    steps: [open(), activate("c1"), addQueryNode("SELECT 1"), saveWorkflow("Flow")],
  },
  {
    name: "workflows/rename",
    steps: [
      open(),
      activate("c1"),
      addQueryNode("SELECT 1"),
      saveWorkflow("Flow"),
      step(
        "workflow.renameWorkflow",
        (ids) => ({ id: ids[1], name: "Renamed" }),
        async (t, ids) => t.page.workflow.renameWorkflow(ids[1], "Renamed"),
      ),
    ],
  },
  {
    name: "workflows/result-rows",
    note: "A query node run gives a result node with a bigint beyond 2^53 and bytes; the saved JSON holds them tagged (toStorable). Saving again updates it.",
    steps: [
      open(),
      activate("c1"),
      addQueryNode("SELECT * FROM t"),
      step(
        "workflow.executeQueryNode",
        (ids) => ({ id: ids[0] }),
        async (t, ids) => {
          rec.workflowRows = RUN_ROWS;
          await t.page.workflow.executeQueryNode(ids[0]);
        },
      ),
      saveWorkflow("With rows"),
      step(
        "workflow.updateNodeData",
        (ids) => ({ id: ids[0], name: "Renamed node" }),
        async (t, ids) => t.page.workflow.updateNodeData(ids[0], { name: "Renamed node" }),
      ),
      saveWorkflow("With rows"),
    ],
  },
  {
    name: "workflows/chart-on-result-node",
    note: "A chart fed by a result node in the same workflow is stored with rows: [] (Q16); the result node keeps its rows.",
    steps: [
      open(),
      activate("c1"),
      addQueryNode("SELECT * FROM t"),
      step(
        "workflow.executeQueryNode",
        (ids) => ({ id: ids[0] }),
        async (t, ids) => {
          rec.workflowRows = () => [{ n: 1 }, { n: 2 }];
          await t.page.workflow.executeQueryNode(ids[0]);
        },
      ),
      step("workflow.addChartNode", { source: "result node" }, async (t) => {
        const result = t.page.workflowState.nodes.find((n) => n.data.type === "result")!;
        return t.page.workflow.addChartNode(result.id, ["n"], [[1], [2]], undefined, {
          x: 800,
          y: 0,
        });
      }),
      saveWorkflow("Chart"),
    ],
  },
  {
    name: "workflows/pre-5d-2-chart-copies",
    note: "A workflow saved before 5d-2 whose chart holds its source's rows: it loads as it is, a rename keeps the copies (the project save sends them as held), and only its next saveWorkflow drops them.",
    seed: standard({
      project_state: [STATE_P1],
      saved_canvases: [chartCopyRow("workflow-old", "Old")],
    }),
    steps: [
      open(),
      activate("c1"),
      step("workflow.renameWorkflow", { id: "workflow-old", name: "Old renamed" }, async (t) =>
        t.page.workflow.renameWorkflow("workflow-old", "Old renamed"),
      ),
      step("workflow.loadWorkflow", { id: "workflow-old" }, async (t) =>
        t.page.workflow.loadWorkflow("workflow-old"),
      ),
      saveWorkflow("Old renamed"),
    ],
  },
  {
    name: "workflows/delete",
    steps: [
      open(),
      activate("c1"),
      addQueryNode("SELECT 1"),
      saveWorkflow("Flow"),
      step(
        "workflow.deleteWorkflow",
        (ids) => ({ id: ids[1] }),
        async (t, ids) => t.page.workflow.deleteWorkflow(ids[1]),
      ),
    ],
  },
  {
    name: "workflows/two-in-one-project",
    steps: [
      open(),
      activate("c1"),
      addQueryNode("SELECT 1"),
      saveWorkflow("First"),
      step("workflow.clearWorkflow", undefined, async (t) => t.page.workflow.clearWorkflow()),
      addQueryNode("SELECT 2"),
      saveWorkflow("Second"),
    ],
  },
  {
    name: "workflows/does-not-decode",
    note: "A stored workflow whose rows don't decode is dropped by today's load, and the next project save deletes it. Core never deletes it: workflows are saved one row at a time.",
    seed: standard({
      project_state: [STATE_P1],
      saved_canvases: [UNDECODABLE, chartCopyRow("workflow-good", "Good")],
    }),
    steps: [
      open(),
      activate("c1"),
      step("workflow.renameWorkflow", { id: "workflow-good", name: "Good renamed" }, async (t) =>
        t.page.workflow.renameWorkflow("workflow-good", "Good renamed"),
      ),
    ],
    change: {
      decision: "23",
      why: "Saved workflows leave the project state: no call deletes a workflow the page didn't load, so the one that doesn't decode stays (today the project save after the load deletes it).",
      expected: (raw) => {
        const out: Record<number, StepChange> = {};
        raw.steps.forEach((s, i) => {
          const rows = s.rows.saved_canvases ?? [];
          if (!rows.some((r) => r.id === "workflow-bad")) {
            out[i] = {
              rows: withTable(s.rows, "saved_canvases", [...rows, UNDECODABLE].sort(byId)),
            };
          }
        });
        return out;
      },
    },
  },
];

// ---------------------------------------------------------------- cases: dashboards

const W1 = {
  id: "widget-1",
  title: "Revenue",
  x: 0,
  y: 0,
  width: 400,
  height: 300,
  querySource: "custom",
  query: "SELECT sum(total) AS total FROM orders",
  widgetType: "kpi",
};
const W2 = {
  id: "widget-2",
  title: "Orders",
  x: 420,
  y: 0,
  width: 400,
  height: 300,
  querySource: "custom",
  query: "SELECT day, count(*) FROM orders GROUP BY day",
  widgetType: "chart",
};

function dashRow(over: Row = {}): Row {
  return {
    id: "dash-1",
    project_id: "p1",
    name: "Sales",
    viewport: '{"x":0,"y":0,"zoom":1}',
    widgets: JSON.stringify([W1]),
    date_filter: null,
    created_at: T0,
    updated_at: T0,
    starred: 0,
    shared: 0,
    description: null,
    ...over,
  };
}

/** A stored dashboard's snapshot as a version holds it (`createDashboardSnapshot`). */
function snapshotOf(row: Row): string {
  const snap: Obj = { name: row.name };
  if (row.description !== null && row.description !== undefined) snap.description = row.description;
  snap.widgets = JSON.parse(row.widgets as string);
  snap.viewport = JSON.parse(row.viewport as string);
  snap.dateFilter = row.date_filter ? JSON.parse(row.date_filter as string) : null;
  return JSON.stringify(snap);
}

function versionRow(n: number, dashboard: Row = dashRow()): Row {
  return {
    id: `dver-${n}`,
    dashboard_id: dashboard.id,
    version: n,
    snapshot: snapshotOf(dashboard),
    created_at: T0,
  };
}

function limits(dashboard?: string, query?: string): Row[] {
  const out: Row[] = [];
  if (dashboard !== undefined) out.push({ key: "dashboard_version_limit", value: dashboard });
  if (query !== undefined) out.push({ key: "query_version_limit", value: query });
  return out;
}

function renameDashboard(id: string, name: string): Step {
  return step("dashboards.renameDashboard", { id, name }, async (t) => {
    // A refusal is shown, and answers false.
    if (!(await t.page.dashboards.renameDashboard(id, name))) throw new Error("refused");
  });
}

function createDashboard(name: string): Step {
  return step("dashboards.createDashboard", { name }, async (t, ids) => {
    const d = await t.page.dashboards.createDashboard(name);
    // A refusal is shown, and answers null.
    if (!d) throw new Error("refused");
    ids.push(d.id);
    return d.id;
  });
}

/**
 * The versions Core stores when `limit` 0 keeps them all: every version the
 * TS inserted, numbered 1, 2, … in insertion order with Core's ids.
 */
function keepAllVersions(raw: RawCase): Record<number, StepChange> {
  const out: Record<number, StepChange> = {};
  const kept: Row[] = [...(raw.before.dashboard_versions ?? [])];
  raw.steps.forEach((s, i) => {
    for (const e of s.storage) {
      if (e.call !== "dashboardVersions.insert" || e.error) continue;
      const v = e.args[0] as Obj;
      const version = kept.filter((k) => k.dashboard_id === v.dashboardId).length + 1;
      kept.push({
        id: v.id,
        dashboard_id: v.dashboardId,
        version,
        snapshot: v.snapshot,
        created_at: v.createdAt,
      });
    }
    const rows = withTable(s.rows, "dashboard_versions", kept);
    // The page's version list is today's (pruned to nothing); a page on Core lists what Core kept.
    if (JSON.stringify(rows) !== JSON.stringify(s.rows)) out[i] = { rows, view: null };
  });
  return out;
}

const dashboardSeed = (extra: Seed = {}) =>
  standard({ ...extra, dashboards: [dashRow(), ...(extra.dashboards ?? [])] });

const dashboardCases: Case[] = [
  {
    name: "dashboards/create",
    steps: [open(), createDashboard("Sales")],
  },
  {
    name: "dashboards/rename",
    note: "A rename records a version of the state before it (captureVersion).",
    seed: dashboardSeed(),
    steps: [open(), renameDashboard("dash-1", "Revenue")],
  },
  {
    name: "dashboards/widgets",
    note: "Adding, updating and removing a widget each record a version.",
    seed: dashboardSeed(),
    steps: [
      open(),
      step("dashboards.addWidget", { dashboardId: "dash-1", widget: W2 }, async (t) =>
        t.page.dashboards.addWidget("dash-1", { ...W2 } as never),
      ),
      step(
        "dashboards.updateWidget",
        { dashboardId: "dash-1", widgetId: "widget-2", updates: { title: "Orders by day" } },
        async (t) =>
          t.page.dashboards.updateWidget("dash-1", "widget-2", { title: "Orders by day" }),
      ),
      step("dashboards.removeWidget", { dashboardId: "dash-1", widgetId: "widget-1" }, async (t) =>
        t.page.dashboards.removeWidget("dash-1", "widget-1"),
      ),
    ],
  },
  {
    name: "dashboards/move-resize-pan",
    note: "A move, a resize and a pan change widgets or viewport and record no version.",
    seed: dashboardSeed(),
    steps: [
      open(),
      step(
        "dashboards.moveWidget",
        { dashboardId: "dash-1", widgetId: "widget-1", x: 40, y: 60 },
        async (t) => t.page.dashboards.moveWidget("dash-1", "widget-1", { x: 40, y: 60 }),
      ),
      step(
        "dashboards.resizeWidget",
        { dashboardId: "dash-1", widgetId: "widget-1", width: 500, height: 320 },
        async (t) =>
          t.page.dashboards.resizeWidget("dash-1", "widget-1", { width: 500, height: 320 }),
      ),
      step(
        "dashboards.updateViewport",
        { dashboardId: "dash-1", viewport: { x: -120, y: 30, zoom: 0.75 } },
        async (t) => t.page.dashboards.updateViewport("dash-1", { x: -120, y: 30, zoom: 0.75 }),
      ),
    ],
  },
  {
    name: "dashboards/date-filter",
    note: "Setting and clearing the date filter each record a version (widgets then run; none is connected here).",
    seed: dashboardSeed(),
    steps: [
      open(),
      step(
        "dashboards.setDateFilter",
        { dashboardId: "dash-1", range: { start: "2030-01-01", end: "2030-01-31" } },
        async (t) =>
          t.page.dashboards.setDateFilter("dash-1", { start: "2030-01-01", end: "2030-01-31" }),
      ),
      step("dashboards.setDateFilter", { dashboardId: "dash-1", range: null }, async (t) =>
        t.page.dashboards.setDateFilter("dash-1", null),
      ),
    ],
  },
  {
    name: "dashboards/star-and-unstar",
    note: "The star alone leaves updated_at (Decision 21).",
    seed: dashboardSeed(),
    steps: [
      open(),
      step("dashboards.toggleDashboardStarred", { id: "dash-1" }, async (t) =>
        t.page.dashboards.toggleDashboardStarred("dash-1"),
      ),
      step("dashboards.toggleDashboardStarred", { id: "dash-1" }, async (t) =>
        t.page.dashboards.toggleDashboardStarred("dash-1"),
      ),
    ],
  },
  {
    name: "dashboards/share-and-unshare",
    note: "Sharing writes the git file first, unsharing deletes it first (files); both set updated_at.",
    seed: dashboardSeed(),
    steps: [
      open(),
      step("dashboards.shareDashboardById", { id: "dash-1" }, async (t) =>
        t.page.dashboards.shareDashboardById("dash-1"),
      ),
      step("dashboards.unshareDashboardById", { id: "dash-1" }, async (t) =>
        t.page.dashboards.unshareDashboardById("dash-1"),
      ),
    ],
  },
  {
    name: "dashboards/delete-with-versions",
    seed: dashboardSeed({ dashboard_versions: [versionRow(1), versionRow(2)] }),
    steps: [
      open(),
      step("dashboards.deleteDashboard", { id: "dash-1" }, async (t) =>
        t.page.dashboards.deleteDashboard("dash-1"),
      ),
    ],
  },
  {
    name: "dashboards/restore-version",
    note: "Restoring version 1 records the current state as a version, then takes version 1's name, description, widgets, viewport and date filter.",
    seed: standard({
      dashboards: [
        dashRow({ name: "Sales v2", description: "Now", widgets: JSON.stringify([W1, W2]) }),
      ],
      dashboard_versions: [
        versionRow(
          1,
          dashRow({
            description: "Then",
            date_filter: '{"start":"2029-01-01","end":"2029-12-31"}',
          }),
        ),
      ],
    }),
    steps: [
      open(),
      step("dashboards.restoreVersion", { dashboardId: "dash-1", version: 1 }, async (t) => {
        // The history lists no snapshots: the one restored is read (5d-2 Task 7).
        const [listed] = t.page.dashboards.getVersionsForDashboard("dash-1");
        const v = await t.page.dashboards.loadVersion("dash-1", listed.id);
        await t.page.dashboards.restoreVersion("dash-1", v!);
      }),
    ],
  },
  {
    name: "dashboards/prune-at-3",
    note: "dashboard_version_limit 3 (query_version_limit 50 is not used): five renames keep the newest three versions.",
    seed: dashboardSeed({ app_state: limits("3", "50") }),
    steps: [
      open(),
      renameDashboard("dash-1", "Sales 1"),
      renameDashboard("dash-1", "Sales 2"),
      renameDashboard("dash-1", "Sales 3"),
      renameDashboard("dash-1", "Sales 4"),
      renameDashboard("dash-1", "Sales 5"),
    ],
  },
  {
    name: "dashboards/prune-at-0",
    note: "dashboard_version_limit 0: today every version is deleted after it's stored, and each next one is version 1 again.",
    seed: dashboardSeed({ app_state: limits("0") }),
    steps: [
      open(),
      renameDashboard("dash-1", "Sales 1"),
      renameDashboard("dash-1", "Sales 2"),
      renameDashboard("dash-1", "Sales 3"),
    ],
    change: {
      decision: "21",
      why: "dashboard_version_limit 0 keeps every version, as query_version_limit does (Decision 20, 'Version limits and 0'); versions are numbered MAX(version) + 1 inside the transaction. Today 0 deletes every version and the next one is numbered 1 again.",
      expected: keepAllVersions,
    },
  },
  {
    name: "dashboards/query-limit-not-used",
    note: "query_version_limit 3 with no dashboard_version_limit: dashboards keep the default 100, so all five versions stay.",
    seed: dashboardSeed({ app_state: limits(undefined, "3") }),
    steps: [
      open(),
      renameDashboard("dash-1", "Sales 1"),
      renameDashboard("dash-1", "Sales 2"),
      renameDashboard("dash-1", "Sales 3"),
      renameDashboard("dash-1", "Sales 4"),
      renameDashboard("dash-1", "Sales 5"),
    ],
  },
  {
    name: "dashboards/delete-during-pending-edit",
    note: "A pan saved while the delete awaits its storage call: today the upsert lands after the delete and the dashboard comes back (in the file, not the page).",
    seed: dashboardSeed({ dashboard_versions: [versionRow(1)] }),
    steps: [
      open(),
      step(
        "dashboards.deleteDashboard+updateViewport",
        { id: "dash-1", viewport: { x: 5, y: 5, zoom: 1 } },
        async (t) => {
          const removing = t.page.dashboards.deleteDashboard("dash-1");
          const panning = t.page.dashboards.updateViewport("dash-1", { x: 5, y: 5, zoom: 1 });
          await Promise.all([removing, panning]);
        },
      ),
    ],
    change: {
      decision: "21",
      why: "An update of a missing dashboard is DASHBOARD_NOT_FOUND (call 1, the pan), so the removed dashboard stays removed. Today the pan's whole-row upsert re-inserts it.",
      expected: (raw) => {
        const i = 1;
        const rows = raw.steps[i].rows;
        return {
          [i]: {
            outcome: { ok: false, code: "DASHBOARD_NOT_FOUND", call: 1 },
            rows: withTable(
              rows,
              "dashboards",
              (rows.dashboards ?? []).filter((d) => d.id !== "dash-1"),
            ),
          },
        };
      },
    },
  },
  {
    name: "dashboards/create-name-taken",
    note: 'Today a second dashboard named " sales " is stored next to "Sales".',
    seed: dashboardSeed(),
    steps: [open(), createDashboard(" sales ")],
    change: {
      decision: "21",
      why: 'Dashboard names clash within a project (Q3): trimmed, NFC-normalised and case-folded, " sales " is "Sales" (dash-1).',
      expected: (raw) => ({
        ...refused(raw, 1, { code: "NAME_TAKEN", takenBy: "dash-1" }),
        1: { ...refused(raw, 1, { code: "NAME_TAKEN", takenBy: "dash-1" })[1], view: null },
      }),
    },
  },
  {
    name: "dashboards/rename-to-taken-name",
    note: "Today the rename is stored, with a version.",
    seed: dashboardSeed({ dashboards: [dashRow({ id: "dash-2", name: "Costs" })] }),
    steps: [open(), renameDashboard("dash-2", "SALES")],
    change: {
      decision: "21",
      why: "A rename to another dashboard's name is NAME_TAKEN (dash-1), and nothing is written: no version, no rename.",
      expected: (raw) => ({
        1: { ...refused(raw, 1, { code: "NAME_TAKEN", takenBy: "dash-1" })[1], view: null },
      }),
    },
  },
  {
    name: "dashboards/create-empty-name",
    note: "Today stored with an empty name.",
    steps: [open(), createDashboard("  ")],
    change: {
      decision: "1",
      why: "Names are trimmed and non-empty: INVALID_ARGUMENT, nothing stored.",
      expected: (raw) => ({
        1: { ...refused(raw, 1, { code: "INVALID_ARGUMENT" })[1], view: null },
      }),
    },
  },
  {
    name: "dashboards/existing-duplicates-stay",
    note: "Two stored dashboards already share a name: both load, and either can still be patched.",
    seed: dashboardSeed({ dashboards: [dashRow({ id: "dash-2" })] }),
    steps: [
      open(),
      step("dashboards.toggleDashboardStarred", { id: "dash-2" }, async (t) =>
        t.page.dashboards.toggleDashboardStarred("dash-2"),
      ),
      step(
        "dashboards.moveWidget",
        { dashboardId: "dash-2", widgetId: "widget-1", x: 10, y: 10 },
        async (t) => t.page.dashboards.moveWidget("dash-2", "widget-1", { x: 10, y: 10 }),
      ),
    ],
  },
  {
    name: "dashboards/two-tabs-version-numbers",
    note: "Two windows rename one dashboard. The second's version is numbered from its own list (1 again): the insert fails on the unique constraint (logged only), and its rename is saved over the first's.",
    seed: dashboardSeed(),
    steps: [
      open(),
      open("second", "win-2"),
      step(
        "dashboards.renameDashboard",
        { page: "main", id: "dash-1", name: "Revenue" },
        async (t) => t.on("main", (p) => p.dashboards.renameDashboard("dash-1", "Revenue")),
      ),
      step(
        "dashboards.renameDashboard",
        { page: "second", id: "dash-1", name: "Turnover" },
        async (t) => t.on("second", (p) => p.dashboards.renameDashboard("dash-1", "Turnover")),
      ),
    ],
    change: {
      decision: "21",
      why: "Core numbers a version MAX(version) + 1 inside the update's transaction, and snapshots the stored row: the second window's version is 2, a snapshot of the first window's rename. Today its insert collides with version 1 and fails.",
      expected: (raw) => {
        const i = 3;
        const before = rowsBefore(raw, i);
        const stored = (before.dashboards ?? []).find((d) => d.id === "dash-1")!;
        const insert = raw.steps[i].storage.find((e) => e.call === "dashboardVersions.insert")!;
        const versions = [
          ...(raw.steps[i].rows.dashboard_versions ?? []),
          {
            id: (insert.args[0] as Obj).id,
            dashboard_id: "dash-1",
            version: 2,
            snapshot: snapshotOf(stored),
            created_at: (insert.args[0] as Obj).createdAt,
          },
        ];
        return {
          [i]: { rows: withTable(raw.steps[i].rows, "dashboard_versions", versions), view: null },
        };
      },
    },
  },
  {
    name: "dashboards/beta-era",
    note: "A file that started on v2026.4.5-beta.1 (dashboards.project_id without a foreign key): a dashboard with a NULL project_id is never listed or touched; creating one works; removing one takes its versions (they cascade from the dashboard).",
    file: "v2026.4.5-beta.1",
    seed: dashboardSeed({
      dashboards: [dashRow({ id: "dash-orphan", project_id: null, name: "Orphan" })],
      dashboard_versions: [versionRow(1), versionRow(2)],
    }),
    steps: [
      open(),
      createDashboard("New"),
      step("dashboards.deleteDashboard", { id: "dash-1" }, async (t) =>
        t.page.dashboards.deleteDashboard("dash-1"),
      ),
    ],
  },
  {
    name: "dashboards/widget-run-state-stripped",
    note: "A widget's result (bigint cells), loading flag, error and refresh time are never stored.",
    seed: dashboardSeed(),
    steps: [
      open(),
      activate("c1"),
      step("dashboards.executeWidget", { dashboardId: "dash-1", widgetId: "widget-1" }, async (t) =>
        t.page.dashboards.executeWidget("dash-1", "widget-1"),
      ),
      step(
        "dashboards.moveWidget",
        { dashboardId: "dash-1", widgetId: "widget-1", x: 1, y: 2 },
        async (t) => t.page.dashboards.moveWidget("dash-1", "widget-1", { x: 1, y: 2 }),
      ),
    ],
  },
];

// ---------------------------------------------------------------- cases: chats

type Ai = (p: SendAIMessageParams) => Promise<void>;

/** A reply in two chunks, then the end of the turn. */
const say =
  (text: string, dashboardId?: string): Ai =>
  async (p) => {
    p.onChunk(text.slice(0, 4));
    if (dashboardId) p.onDashboardCreated?.(dashboardId);
    p.onChunk(text.slice(4));
    p.onDone();
  };
const failing: Ai = async (p) => p.onError("rate_limit");
const throwing: Ai = async () => {
  throw new TypeError("Failed to fetch");
};
const aborted = (p: SendAIMessageParams) =>
  new Promise<void>((resolve) => p.signal!.addEventListener("abort", () => resolve()));
/** A turn that streams a chunk, then waits until it's stopped. */
const hanging: Ai = async (p) => {
  p.onChunk("Partial");
  await aborted(p);
};
/** A turn that asks to run a query, then waits until it's stopped. */
const approving: Ai = async (p) => {
  p.onChunk("Checking");
  p.onApprovalRequired?.(
    "SELECT count(*) FROM orders",
    { id: "c1", name: "Local", type: "postgres" } as never,
    () => {},
    () => {},
  );
  await aborted(p);
};

function chatRow(over: Row = {}): Row {
  return {
    id: "chat-1",
    connection_id: "c1",
    title: "Orders",
    created_at: T0,
    updated_at: T0,
    ...over,
  };
}
function msgRow(id: string, role: string, content: string, over: Row = {}): Row {
  return {
    id,
    chat_id: "chat-1",
    role,
    content,
    timestamp: T0,
    query: null,
    dashboard_id: null,
    ...over,
  };
}

const CHAT_1 = {
  ai_chats: [chatRow()],
  ai_messages: [
    msgRow("m1", "user", "How many orders?", { timestamp: "2024-01-01T00:00:01.000Z" }),
    msgRow("m2", "assistant", "There are 42.", { timestamp: "2024-01-01T00:00:02.000Z" }),
  ],
};

function send(content: string, ai: Ai): Step {
  return step("ui.sendAIMessage", { content }, async (t) => {
    rec.ai = ai as (p: unknown) => Promise<void>;
    // Resolves once the turn is dispatched (the chat made first, through Core).
    await t.page.ui.sendAIMessage(content);
    return t.page.state.activeAIChatId;
  });
}

const cancel = step("ui.cancelAIStream", undefined, async (t) => t.page.ui.cancelAIStream());

const chatCases: Case[] = [
  {
    name: "chats/create",
    steps: [
      open(),
      activate("c1"),
      step("aiChats.createChat", undefined, async (t) => t.page.aiChats.createChat()),
    ],
  },
  {
    name: "chats/first-turn",
    note: "The first message makes the chat and titles it; the turn's end stores both messages and touches the chat.",
    steps: [
      open(),
      activate("c1"),
      send("How many orders are there?", say("There are 42 orders.")),
    ],
  },
  {
    name: "chats/second-turn",
    note: "A turn in a stored chat: only its two new messages change.",
    seed: standard(CHAT_1),
    steps: [open(), activate("c1"), send("And yesterday?", say("Seven yesterday.", "dash-9"))],
  },
  {
    name: "chats/error-turn",
    steps: [open(), activate("c1"), send("Hello?", failing)],
  },
  {
    name: "chats/stream-throws",
    note: "A failure the provider didn't handle ends the turn as an error, which saves it (Task 1).",
    steps: [open(), activate("c1"), send("Hello?", throwing)],
  },
  {
    name: "chats/stop",
    note: "Stop saves what the turn has so far.",
    steps: [open(), activate("c1"), send("Tell me everything", hanging), cancel],
  },
  {
    name: "chats/stop-with-approval-pending",
    note: "A turn stopped while a query waits for approval: the approval is cleared and the turn saved.",
    steps: [open(), activate("c1"), send("Count the orders", approving), cancel],
  },
  {
    name: "chats/delete",
    seed: standard({
      ai_chats: [...CHAT_1.ai_chats, chatRow({ id: "chat-2", title: "Other" })],
      ai_messages: CHAT_1.ai_messages,
    }),
    steps: [
      open(),
      activate("c1"),
      step("aiChats.deleteChat", { id: "chat-1" }, async (t) =>
        t.page.aiChats.deleteChat("chat-1"),
      ),
    ],
  },
  {
    name: "chats/delete-streaming",
    note: "Deleting the chat a turn is streaming into stops the turn first; nothing of it is saved after (Task 1).",
    steps: [
      open(),
      activate("c1"),
      send("Tell me everything", hanging),
      step("aiChats.deleteChat", { chat: "the streaming one" }, async (t) =>
        t.page.aiChats.deleteChat(t.page.state.aiStreamingChatId!),
      ),
    ],
  },
  {
    name: "chats/flush-saves-streaming",
    note: "Closing the page while a turn streams saves its messages so far (Task 1).",
    steps: [
      open(),
      activate("c1"),
      send("Tell me everything", hanging),
      step("persistence.flush", undefined, async (t) => t.page.flush()),
    ],
  },
  {
    name: "chats/no-model-chosen",
    note: "A connection with no AI model: the message waits for a model choice (pendingModelSelection), which is never stored; only the chat row is.",
    seed: {
      ...standard(),
      connections: [conn({}), C2],
    },
    steps: [open(), activate("c1"), send("Hello?", say("unused"))],
  },
  {
    name: "chats/switch-loads-messages",
    note: "The most recent chat's messages load at startup; another chat's load when it's opened.",
    seed: standard({
      ai_chats: [
        chatRow(),
        chatRow({ id: "chat-2", title: "Newer", updated_at: "2024-01-02T00:00:00.000Z" }),
      ],
      ai_messages: [...CHAT_1.ai_messages, msgRow("m3", "user", "Hi", { chat_id: "chat-2" })],
    }),
    steps: [
      open(),
      activate("c1"),
      step("aiChats.switchChat", { id: "chat-1" }, async (t) =>
        t.page.aiChats.switchChat("chat-1"),
      ),
    ],
  },
  {
    name: "chats/two-tabs-same-chat",
    note: "Two windows on one chat each finish a turn. The second replaces the chat's messages with its own list, which lacks the first window's turn.",
    seed: standard(CHAT_1),
    steps: [
      open(),
      activate("c1"),
      open("second", "win-2"),
      activate("c1"),
      step("ui.sendAIMessage", { page: "main", content: "Main's question" }, async (t) =>
        t.on("main", (p) => {
          rec.ai = say("Main's answer") as (p: unknown) => Promise<void>;
          return p.ui.sendAIMessage("Main's question");
        }),
      ),
      step("ui.sendAIMessage", { page: "second", content: "Second's question" }, async (t) =>
        t.on("second", (p) => {
          rec.ai = say("Second's answer") as (p: unknown) => Promise<void>;
          return p.ui.sendAIMessage("Second's question");
        }),
      ),
    ],
    change: {
      decision: "24",
      why: "chatMessagesPut upserts the messages it lists and deletes none: the first window's turn stays. Today the second window's replace deletes it.",
      expected: (raw) => {
        const i = 5;
        const before = rowsBefore(raw, i).ai_messages ?? [];
        const now = raw.steps[i].rows.ai_messages ?? [];
        const lost = before.filter((m) => !now.some((n) => n.id === m.id));
        return {
          [i]: {
            view: null,
            rows: withTable(
              raw.steps[i].rows,
              "ai_messages",
              [...now, ...lost].sort((a, b) =>
                `${String(a.chat_id)} ${String(a.id)}` < `${String(b.chat_id)} ${String(b.id)}`
                  ? -1
                  : 1,
              ),
            ),
          },
        };
      },
    },
  },
];

// ---------------------------------------------------------------- cases: settings

const settingSet = (key: string, value: string | null): CoreCall => ({
  group: "settings",
  method: "settingSet",
  params: { key, value },
});
const settingGet = (key: string): CoreCall => ({
  group: "settings",
  method: "settingGet",
  params: { key },
});

/** A value written as another writer (or a newer page) could send it: a `settingSet`. */
function write(key: string, value: string | null): Step {
  return step("storage.appState.set", { key, value }, async (t) =>
    t.settings.setSetting(key as SettingKey, value),
  );
}

/** A read of `key` as another caller could send it. */
function read(key: string): Step {
  return {
    ...step("storage.appState.get", { key }, async (t) => t.settings.getSetting(key as SettingKey)),
    core: [settingGet(key)],
    coreOnly: true,
  };
}

type SettingPlan = Record<number, { refuse: string } | { remove: string }>;

/**
 * What Core stores for a settings case: a refused step writes nothing, a
 * `null` deletes the row (today it stores NULL). Every step after the first
 * one that differs must be refused or a removal, so the chain stays exact.
 */
function settingChanges(raw: RawCase, plan: SettingPlan): Record<number, StepChange> {
  const out: Record<number, StepChange> = {};
  let prev = raw.before;
  let diverged = false;
  raw.steps.forEach((s, i) => {
    const p = plan[i];
    if (p && "refuse" in p) {
      out[i] = { outcome: { ok: false, code: p.refuse }, rows: prev, view: null };
      diverged = true;
    } else if (p && "remove" in p) {
      const rows = withTable(
        prev,
        "app_state",
        (prev.app_state ?? []).filter((r) => r.key !== p.remove),
      );
      out[i] = { rows };
      diverged = true;
    } else if (diverged) {
      throw new Error(`${raw.name}: step ${i} follows a changed step`);
    }
    prev = out[i]?.rows ?? s.rows;
  });
  return out;
}

const settingsCase = (
  name: string,
  steps: Step[],
  plan: SettingPlan,
  why: string,
  seed: Seed = bare(),
  note?: string,
): Case => ({
  name,
  ...(note ? { note } : {}),
  seed,
  view: settingsView,
  steps,
  change: { decision: "20", why, expected: (raw) => settingChanges(raw, plan) },
});

const NULL_DELETES =
  "settingSet with null deletes the row (today it stores NULL; both read as unset).";

const settingsCases: Case[] = [
  settingsCase(
    "settings/editor-keybinding-mode",
    [
      step("editorSettingsStore.load", undefined, async (t) => (await t.stores()).editor.load(), [
        settingGet("editorKeybindingMode"),
      ]),
      step("editorSettingsStore.setKeybindingMode", { value: "vim" }, async (t) =>
        (await t.stores()).editor.setKeybindingMode("vim"),
      ),
      step("editorSettingsStore.setKeybindingMode", { value: "hyper" }, async (t) =>
        (await t.stores()).editor.setKeybindingMode("hyper" as never),
      ),
      write("editorKeybindingMode", null),
    ],
    { 2: { refuse: "INVALID_ARGUMENT" }, 3: { remove: "editorKeybindingMode" } },
    `editorKeybindingMode is default, vim or emacs; "hyper" is INVALID_ARGUMENT. ${NULL_DELETES}`,
    bare({ app_state: [{ key: "editorKeybindingMode", value: "emacs" }] }),
  ),
  settingsCase(
    "settings/pending-changes-enabled",
    [
      step(
        "pendingChangesSettingsStore.load",
        undefined,
        async (t) => (await t.stores()).pending.load(),
        [settingGet("pending_changes_enabled")],
      ),
      step("pendingChangesSettingsStore.setEnabled", { value: false }, async (t) =>
        (await t.stores()).pending.setEnabled(false),
      ),
      write("pending_changes_enabled", "maybe"),
      write("pending_changes_enabled", null),
    ],
    { 2: { refuse: "INVALID_ARGUMENT" }, 3: { remove: "pending_changes_enabled" } },
    `pending_changes_enabled is "true" or "false". ${NULL_DELETES}`,
  ),
  settingsCase(
    "settings/skipped-update-version",
    [
      step(
        "updateStore.initialize",
        undefined,
        async (t) => (await t.stores()).update.initialize(),
        [settingGet("skippedUpdateVersion")],
      ),
      step("updateStore.skip", { version: "2030.1.2" }, async (t) => {
        const { update } = await t.stores();
        update.setUpdateAvailable({ version: "2030.1.2" } as never);
        await update.skip();
      }),
      write("skippedUpdateVersion", "2030.1.3\u0000"),
      write("skippedUpdateVersion", null),
    ],
    { 2: { refuse: "INVALID_ARGUMENT" }, 3: { remove: "skippedUpdateVersion" } },
    `No string may hold a NUL (Decision 1). ${NULL_DELETES}`,
  ),
  settingsCase(
    "settings/query-version-limit",
    [
      step("settings.saveQueryVersionLimit", { typed: 50 }, async (t) =>
        t.settings.setSetting("query_version_limit", String(clampLimit(50))),
      ),
      step("settings.saveQueryVersionLimit", { typed: 3 }, async (t) =>
        t.settings.setSetting("query_version_limit", String(clampLimit(3))),
      ),
      write("query_version_limit", "0"),
      write("query_version_limit", "100000"),
      write("query_version_limit", "abc"),
      write("query_version_limit", "100001"),
      write("query_version_limit", null),
    ],
    {
      4: { refuse: "INVALID_ARGUMENT" },
      5: { refuse: "INVALID_ARGUMENT" },
      6: { remove: "query_version_limit" },
    },
    `A version limit is a whole number from 0 to 100,000 as text: "0" and "100000" are accepted (keep all; the UI clamps to 10 since 5d-1), "abc" and "100001" are INVALID_ARGUMENT. ${NULL_DELETES}`,
    bare(),
    "The settings UI saves the typed number clamped to at least 10 (clampVersionLimit); a hand-set 0 still round-trips.",
  ),
  settingsCase(
    "settings/dashboard-version-limit",
    [
      step("settings.saveDashboardVersionLimit", { typed: 20 }, async (t) =>
        t.settings.setSetting("dashboard_version_limit", String(clampLimit(20))),
      ),
      write("dashboard_version_limit", "-1"),
      write("dashboard_version_limit", "1.5"),
      write("dashboard_version_limit", null),
    ],
    {
      1: { refuse: "INVALID_ARGUMENT" },
      2: { refuse: "INVALID_ARGUMENT" },
      3: { remove: "dashboard_version_limit" },
    },
    `A version limit is a whole number from 0 to 100,000: "-1" and "1.5" are INVALID_ARGUMENT. ${NULL_DELETES}`,
  ),
  settingsCase(
    "settings/license-nudge",
    [
      step(
        "licenseNudgeStore.initialize",
        undefined,
        async (t) => (await t.stores()).nudge.initialize(),
        [settingGet("license_nudge")],
      ),
      step("licenseNudgeStore.recordQuery", undefined, async (t) =>
        (await t.stores()).nudge.recordQuery(),
      ),
      step("licenseNudgeStore.respond", { answer: "work" }, async (t) =>
        (await t.stores()).nudge.respond("work"),
      ),
      write("license_nudge", "[1]"),
      write("license_nudge", "nope"),
      write("license_nudge", null),
    ],
    {
      3: { refuse: "INVALID_ARGUMENT" },
      4: { refuse: "INVALID_ARGUMENT" },
      5: { remove: "license_nudge" },
    },
    `license_nudge is a JSON object: "[1]" and "nope" are INVALID_ARGUMENT. ${NULL_DELETES}`,
    bare({
      app_state: [
        {
          key: "license_nudge",
          value:
            '{"queryCount":99,"activeDays":3,"lastActiveDay":"2029-12-31","answer":null,"snoozedUntil":null}',
        },
      ],
    }),
  ),
  settingsCase(
    "settings/last-active-project-id",
    [
      {
        ...write("lastActiveProjectId", "p2"),
        core: [settingSet("lastActiveProjectId", "p2")],
        coreOnly: true,
      },
    ],
    { 0: { refuse: "INVALID_ARGUMENT" } },
    "lastActiveProjectId is read-only in the settings group: windowActivate writes it (Decision 22).",
  ),
  settingsCase(
    "settings/connection-string-secrets-notice",
    [
      step("connectionSecretsNotice.dismiss", undefined, async (t) =>
        (await t.stores()).notice.dismiss(),
      ),
      write("connectionStringSecretsNotice", '["c1"]'),
    ],
    { 0: { remove: "connectionStringSecretsNotice" }, 1: { refuse: "INVALID_ARGUMENT" } },
    `connectionStringSecretsNotice is Core's (Decision 12a): the GUI may only clear it with null, which deletes the row; any other value is INVALID_ARGUMENT.`,
    bare({ app_state: [{ key: "connectionStringSecretsNotice", value: '["c1"]' }] }),
  ),
  settingsCase(
    "settings/refused-keys",
    [
      write("somethingElse", "1"),
      write("activeRepoId", "repo-1"),
      write("connectionStringSecretsVacuum", "1"),
      write("connectionStringSecretsUpgraded", "1"),
      write("connectionStringSecretsCheckpoint", "1"),
      {
        ...write("aiSettings", '{"enabled":false}'),
        core: [settingSet("aiSettings", '{"enabled":false}')],
        coreOnly: true,
      },
      read("somethingElse"),
      read("connectionStringSecretsCheckpoint"),
    ],
    {
      0: { refuse: "INVALID_ARGUMENT" },
      1: { refuse: "INVALID_ARGUMENT" },
      2: { refuse: "INVALID_ARGUMENT" },
      3: { refuse: "INVALID_ARGUMENT" },
      4: { refuse: "INVALID_ARGUMENT" },
      5: { refuse: "INVALID_ARGUMENT" },
      6: { refuse: "INVALID_ARGUMENT" },
      7: { refuse: "INVALID_ARGUMENT" },
    },
    "App-state keys are a closed set of typed settings: an unknown key, activeRepoId (the shared repos' own), Core's connectionStringSecrets{Vacuum,Upgraded,Checkpoint} keys and aiSettings (written only through the AI settings calls) are refused by settingSet, and an unknown key or one of Core's by settingGet, with INVALID_ARGUMENT naming the key.",
  ),
];

function clampLimit(n: number): number {
  return Math.max(10, Math.trunc(n));
}

// ---------------------------------------------------------------- cases: AI settings

const aiGet: CoreCall = { group: "settings", method: "aiSettingsGet" };
const AI_RECORD = {
  enabled: true,
  providers: [{ id: "prov-1", name: "Anthropic", type: "anthropic" }],
  shareSchemaGlobally: true,
  shareDataGlobally: false,
};
const aiSeed = (record: unknown = AI_RECORD) =>
  bare({
    app_state: [
      { key: "aiSettings", value: typeof record === "string" ? record : JSON.stringify(record) },
    ],
  });

const aiInit = (page = "main") =>
  step(
    "aiSettingsStore.initialize",
    page === "main" ? undefined : { page },
    async (t) => (await t.stores(page)).ai.initialize(),
    [aiGet],
  );

function addProvider(
  provider: { name: string; type: string; baseUrl?: string },
  apiKey?: string,
  page = "main",
): Step {
  return step(
    "aiSettingsStore.addProvider",
    { ...(page === "main" ? {} : { page }), provider, apiKey: apiKey ?? null },
    async (t, ids) => {
      // Core gives the provider its id.
      const id = await (await t.stores(page)).ai.addProvider(provider as never, apiKey);
      ids.push(id);
      return id;
    },
    (ids) => [
      {
        group: "settings",
        method: "aiProviderCreate",
        params: { provider, ...(apiKey ? { apiKey } : {}) },
        binds: ids[ids.length - 1],
      },
    ],
  );
}

function updateProvider(
  provider: { id: string; name: string; type: string; baseUrl?: string },
  patch: Obj,
  apiKey?: string,
): Step {
  return step(
    "aiSettingsStore.updateProvider",
    { provider, apiKey: apiKey ?? null },
    async (t) => (await t.stores()).ai.updateProvider(provider as never, apiKey),
    [
      {
        group: "settings",
        method: "aiProviderUpdate",
        params: {
          id: provider.id,
          patch,
          ...(apiKey === undefined ? {} : { apiKey: apiKey === "" ? null : apiKey }),
        },
      },
    ],
  );
}

const setEnabled = (enabled: boolean) =>
  step(
    "aiSettingsStore.setEnabled",
    { enabled },
    async (t) => (await t.stores()).ai.setEnabled(enabled),
    [{ group: "settings", method: "aiSettingsPatch", params: { patch: { enabled } } }],
  );

const aiCases: Case[] = [
  {
    name: "ai-settings/add-provider",
    seed: bare(),
    view: aiView,
    steps: [aiInit(), addProvider({ name: "Anthropic", type: "anthropic" })],
  },
  {
    name: "ai-settings/add-provider-with-key",
    note: "The API key goes to the keychain (ai-api-key:<id>) after the record today; Core writes it in the call, before the record (Decision 20).",
    seed: bare(),
    view: aiView,
    steps: [
      aiInit(),
      addProvider(
        { name: "Local", type: "openai-compatible", baseUrl: "http://localhost:11434/v1" },
        "sk-local",
      ),
    ],
  },
  {
    name: "ai-settings/update-provider",
    note: 'A rename with a new key, a type change clearing the key (""), and a rename leaving the key (no key sent).',
    seed: aiSeed(),
    secrets: { "ai-api-key:prov-1": "sk-old" },
    view: aiView,
    steps: [
      aiInit(),
      updateProvider(
        { id: "prov-1", name: "Claude", type: "anthropic" },
        { name: "Claude" },
        "sk-new",
      ),
      updateProvider(
        {
          id: "prov-1",
          name: "Claude",
          type: "openai-compatible",
          baseUrl: "http://gw.internal/v1",
        },
        { type: "openai-compatible", baseUrl: "http://gw.internal/v1" },
        "",
      ),
      updateProvider(
        {
          id: "prov-1",
          name: "Gateway",
          type: "openai-compatible",
          baseUrl: "http://gw.internal/v1",
        },
        { name: "Gateway" },
      ),
    ],
  },
  {
    name: "ai-settings/remove-provider",
    seed: aiSeed(),
    secrets: { "ai-api-key:prov-1": "sk-old" },
    view: aiView,
    steps: [
      aiInit(),
      step(
        "aiSettingsStore.deleteProvider",
        { id: "prov-1" },
        async (t) => (await t.stores()).ai.deleteProvider("prov-1"),
        [{ group: "settings", method: "aiProviderRemove", params: { id: "prov-1" } }],
      ),
    ],
  },
  {
    name: "ai-settings/privacy-flags",
    seed: aiSeed(),
    view: aiView,
    steps: [
      aiInit(),
      step(
        "aiSettingsStore.savePrivacySettings",
        { shareSchemaGlobally: false, shareDataGlobally: true },
        async (t) =>
          (await t.stores()).ai.savePrivacySettings({
            shareSchemaGlobally: false,
            shareDataGlobally: true,
          }),
        [
          {
            group: "settings",
            method: "aiSettingsPatch",
            params: { patch: { shareSchemaGlobally: false, shareDataGlobally: true } },
          },
        ],
      ),
    ],
  },
  {
    name: "ai-settings/change-before-load",
    note: "A change before the store loaded loads it first (ensureLoaded), so the stored providers survive.",
    seed: aiSeed(),
    view: aiView,
    steps: [{ ...setEnabled(false), core: [aiGet, ...(setEnabled(false).core as CoreCall[])] }],
  },
  {
    name: "ai-settings/unknown-fields-kept",
    note: "A newer release's fields, on the record and on a provider, survive a write.",
    seed: aiSeed({
      ...AI_RECORD,
      providers: [{ id: "prov-1", name: "Anthropic", type: "anthropic", region: "eu" }],
      futureField: { on: true },
    }),
    view: aiView,
    steps: [aiInit(), setEnabled(false)],
  },
  {
    name: "ai-settings/two-tabs-different-providers",
    note: "Two windows each add a provider. The second window's save writes its whole copy, which lacks the first window's provider.",
    seed: aiSeed(),
    view: aiView,
    steps: [
      aiInit(),
      aiInit("second"),
      addProvider({ name: "First", type: "anthropic" }),
      addProvider({ name: "Second", type: "anthropic" }, undefined, "second"),
    ],
    change: {
      decision: "20",
      why: "AI settings are split into targeted calls rewritten from the stored record inside the transaction, so both windows' providers stay. Today the second window's whole-record save drops the first's.",
      expected: (raw) => {
        const i = 3;
        const before = JSON.parse(
          (rowsBefore(raw, i).app_state ?? []).find((r) => r.key === "aiSettings")!.value as string,
        ) as { providers: Obj[] };
        const rows = raw.steps[i].rows;
        const now = JSON.parse(
          (rows.app_state ?? []).find((r) => r.key === "aiSettings")!.value as string,
        ) as { providers: Obj[] };
        const merged = {
          ...now,
          providers: [
            ...before.providers,
            ...now.providers.filter((p) => !before.providers.some((b) => b.id === p.id)),
          ],
        };
        return {
          [i]: {
            rows: withTable(
              rows,
              "app_state",
              (rows.app_state ?? []).map((r) =>
                r.key === "aiSettings" ? { ...r, value: JSON.stringify(merged) } : r,
              ),
            ),
          },
        };
      },
    },
  },
];

// ---------------------------------------------------------------- cases: themes

const COLORS = { background: "#ffffff", foreground: "#111111", primary: "#3355ff" };
function themeRow(id: string, over: Obj = {}): Row {
  const theme = {
    name: "Mine",
    isDark: false,
    colors: COLORS,
    id,
    isBuiltIn: false,
    createdAt: T0,
    updatedAt: T0,
    ...over,
  };
  return { id, data: JSON.stringify(theme) };
}

/**
 * What Core stores for the theme cases: a user theme call writes only
 * `user_themes`, and `theme_preferences` changes only through
 * `themePreferencesSet` or the fallback inside `userThemeRemove`. Today every
 * theme save also writes the preferences, so a file with no preferences row
 * gets one holding the defaults (which read the same as no row).
 */
function themePrefsChanges(
  raw: RawCase,
  extra?: (i: number, rows: Record<string, Row[]>) => Record<string, Row[]>,
): Record<number, StepChange> {
  const out: Record<number, StepChange> = {};
  let prev = raw.before.theme_preferences ?? [];
  raw.steps.forEach((s, i) => {
    const calls = s.core ?? [];
    const recorded = s.rows.theme_preferences ?? [];
    const removed = calls
      .filter((c) => c.method === "userThemeRemove")
      .map((c) => (c.params as Obj).id);
    const inUse = prev.some(
      (r) => removed.includes(r.light_theme_id) || removed.includes(r.dark_theme_id),
    );
    const writes = calls.some((c) => c.method === "themePreferencesSet") || inUse;
    const prefs = writes ? recorded : prev;
    let rows = withTable(s.rows, "theme_preferences", prefs);
    if (extra) rows = extra(i, rows);
    if (JSON.stringify(rows) !== JSON.stringify(s.rows)) out[i] = { rows };
    prev = prefs;
  });
  return out;
}

const THEME_PREFS_CHANGE: Change = {
  decision: "20",
  why: "A user theme call (userThemeCreate, userThemeUpdate, userThemeRemove of a theme not in use) writes only its user_themes row. Today every theme save also writes theme_preferences, which leaves a row holding the defaults on a file that had none; no row reads as the defaults.",
  expected: (raw) => themePrefsChanges(raw),
};

const themesGet: CoreCall = { group: "settings", method: "themesGet" };
const themeInit = step(
  "themeStore.initialize",
  undefined,
  async (t) => (await t.stores()).theme.initialize(),
  [themesGet],
);

function addTheme(name: string, isDark = false): Step {
  const input = { name, isDark, colors: COLORS };
  return step("themeStore.addTheme", input, async (t, ids) => {
    const theme = await (await t.stores()).theme.addTheme(input as never);
    if (!theme) throw new Error("not saved");
    ids.push(theme.id);
    return theme.id;
  });
}

const themeCases: Case[] = [
  {
    name: "themes/add",
    seed: bare(),
    view: themeView,
    change: THEME_PREFS_CHANGE,
    steps: [themeInit, addTheme("Mine")],
  },
  {
    name: "themes/update",
    seed: bare({ user_themes: [themeRow("theme-1")] }),
    view: themeView,
    change: THEME_PREFS_CHANGE,
    steps: [
      themeInit,
      step(
        "themeStore.updateTheme",
        { id: "theme-1", updates: { name: "Renamed", colors: { ...COLORS, primary: "#ff0000" } } },
        async (t) =>
          (await t.stores()).theme.updateTheme("theme-1", {
            name: "Renamed",
            colors: { ...COLORS, primary: "#ff0000" } as never,
          }),
      ),
    ],
  },
  {
    name: "themes/delete-not-in-use",
    seed: bare({ user_themes: [themeRow("theme-1"), themeRow("theme-2", { name: "Other" })] }),
    view: themeView,
    change: THEME_PREFS_CHANGE,
    steps: [
      themeInit,
      step("themeStore.deleteTheme", { id: "theme-2" }, async (t) =>
        (await t.stores()).theme.deleteTheme("theme-2"),
      ),
    ],
  },
  {
    name: "themes/delete-in-use",
    note: "Removing the light theme in use resets that preference to default-light, in the same call on Core.",
    seed: bare({
      theme_preferences: [{ id: 1, light_theme_id: "theme-1", dark_theme_id: "default-dark" }],
      user_themes: [themeRow("theme-1")],
    }),
    view: themeView,
    change: THEME_PREFS_CHANGE,
    steps: [
      themeInit,
      step("themeStore.deleteTheme", { id: "theme-1" }, async (t) =>
        (await t.stores()).theme.deleteTheme("theme-1"),
      ),
    ],
  },
  {
    name: "themes/preferences",
    seed: bare(),
    view: themeView,
    change: THEME_PREFS_CHANGE,
    steps: [
      themeInit,
      step("themeStore.setLightTheme", { id: "nord-light" }, async (t) =>
        (await t.stores()).theme.setLightTheme("nord-light"),
      ),
      step("themeStore.setDarkTheme", { id: "dracula" }, async (t) =>
        (await t.stores()).theme.setDarkTheme("dracula"),
      ),
    ],
  },
  {
    name: "themes/unknown-theme-ignored",
    note: "The store ignores a theme id it doesn't have (or of the other mode): nothing is sent.",
    seed: bare(),
    view: themeView,
    change: THEME_PREFS_CHANGE,
    steps: [
      themeInit,
      step("themeStore.setLightTheme", { id: "missing" }, async (t) =>
        (await t.stores()).theme.setLightTheme("missing"),
      ),
      step("themeStore.setLightTheme", { id: "dracula" }, async (t) =>
        (await t.stores()).theme.setLightTheme("dracula"),
      ),
    ],
  },
];

// ---------------------------------------------------------------- cases: onboarding, tutorial, import state

const onboardingInit = step(
  "onboardingStore.initialize",
  undefined,
  async (t) => (await t.stores()).onboarding.initialize(),
  [{ group: "settings", method: "onboardingGet" }],
);

const BUILDER_STATE = { tables: [{ name: "orders", alias: "o" }], filters: [] };

const miscCases: Case[] = [
  {
    name: "onboarding/wizard-and-hints",
    note: "No stored record: the store starts from its defaults. Each setter saves the whole record today; on Core each sends the fields it changed.",
    seed: bare(),
    view: onboardingView,
    steps: [
      onboardingInit,
      step("onboardingStore.completeWizard", undefined, async (t) =>
        (await t.stores()).onboarding.completeWizard(),
      ),
      step("onboardingStore.setBackground", { background: "dbeaver" }, async (t) =>
        (await t.stores()).onboarding.setBackground("dbeaver"),
      ),
      step("onboardingStore.dismissHint", { hintId: "sidebar" }, async (t) =>
        (await t.stores()).onboarding.dismissHint("sidebar"),
      ),
      step("onboardingStore.setShowWizardHints", { show: false }, async (t) =>
        (await t.stores()).onboarding.setShowWizardHints(false),
      ),
      step("onboardingStore.setLearnEnabled", { enabled: false }, async (t) =>
        (await t.stores()).onboarding.setLearnEnabled(false),
      ),
    ],
  },
  {
    name: "onboarding/stored-record",
    seed: bare({
      onboarding_state: [
        {
          id: 1,
          data: JSON.stringify({
            isFirstRun: false,
            userBackground: "datagrip",
            hasCompletedWizard: true,
            showWizardHints: true,
            dismissedHints: ["sidebar"],
            learnEnabled: false,
          }),
        },
      ],
    }),
    view: onboardingView,
    steps: [
      onboardingInit,
      step("onboardingStore.dismissHint", { hintId: "editor" }, async (t) =>
        (await t.stores()).onboarding.dismissHint("editor"),
      ),
    ],
  },
  {
    name: "onboarding/web-writes-nothing",
    note: "Onboarding is desktop-only: on web nothing is loaded and nothing is written, with no toast (Task 1).",
    web: true,
    seed: bare(),
    view: onboardingView,
    steps: [
      step("onboardingStore.completeWizard", undefined, async (t) =>
        (await t.stores()).onboarding.completeWizard(),
      ),
    ],
  },
  {
    name: "tutorial/progress",
    seed: bare(),
    view: tutorialView,
    steps: [
      step(
        "tutorialProgressStore.initialize",
        undefined,
        async (t) => (await t.stores()).tutorial.initialize(),
        [{ group: "settings", method: "tutorialList" }],
      ),
      step(
        "tutorialProgressStore.completeChallenge",
        { lessonId: "select-basics", challengeId: "c1" },
        async (t) => (await t.stores()).tutorial.completeChallenge("select-basics", "c1"),
      ),
      step(
        "tutorialProgressStore.saveChallengeState",
        { lessonId: "select-basics", challengeId: "c2", state: BUILDER_STATE },
        async (t) =>
          (await t.stores()).tutorial.saveChallengeState(
            "select-basics",
            "c2",
            BUILDER_STATE as never,
          ),
      ),
      step(
        "tutorialProgressStore.completeChallenge",
        { lessonId: "joins", challengeId: "c1", state: BUILDER_STATE },
        async (t) =>
          (await t.stores()).tutorial.completeChallenge("joins", "c1", BUILDER_STATE as never),
      ),
      step("tutorialProgressStore.resetLesson", { lessonId: "select-basics" }, async (t) =>
        (await t.stores()).tutorial.resetLesson("select-basics"),
      ),
      step("tutorialProgressStore.resetAll", undefined, async (t) =>
        (await t.stores()).tutorial.resetAll(),
      ),
    ],
  },
  {
    name: "tutorial/reset-lesson-with-no-progress",
    note: "Resetting a lesson with nothing stored sends nothing.",
    seed: bare(),
    view: tutorialView,
    steps: [
      step(
        "tutorialProgressStore.initialize",
        undefined,
        async (t) => (await t.stores()).tutorial.initialize(),
        [{ group: "settings", method: "tutorialList" }],
      ),
      step("tutorialProgressStore.resetLesson", { lessonId: "joins" }, async (t) =>
        (await t.stores()).tutorial.resetLesson("joins"),
      ),
    ],
  },
  {
    name: "import-state/tableplus-dismiss",
    seed: bare(),
    view: importView,
    steps: [
      step(
        "tablePlusImportStore.initialize",
        undefined,
        async (t) => (await t.stores()).tableplus.initialize(),
        [{ group: "settings", method: "importStateGet", params: { source: "tableplus" } }],
      ),
      step("tablePlusImportStore.dismiss", undefined, async (t) =>
        (await t.stores()).tableplus.dismiss(),
      ),
    ],
  },
  {
    name: "import-state/dbeaver-complete",
    seed: bare({
      import_state: [{ source: "dbeaver", has_offered_import: 0, last_check_timestamp: T0 }],
    }),
    view: importView,
    steps: [
      step(
        "dbeaverImportStore.initialize",
        undefined,
        async (t) => (await t.stores()).dbeaver.initialize(),
        [{ group: "settings", method: "importStateGet", params: { source: "dbeaver" } }],
      ),
      step("dbeaverImportStore.completeImport", undefined, async (t) =>
        (await t.stores()).dbeaver.completeImport(),
      ),
    ],
  },
];

// ---------------------------------------------------------------- cases: old data

const oldAi = (name: string, record: string, note: string, change?: Change): Case => ({
  name,
  note,
  seed: aiSeed(record),
  view: aiView,
  steps: [aiInit(), setEnabled(false)],
  ...(change ? { change } : {}),
});

const NOT_PARSED_ROW: Row = { id: "workflow-garbage", project_id: "p1", data: "{not json" };

const oldDataCases: Case[] = [
  oldAi(
    "old-data/ai-settings-legacy-provider-fields",
    JSON.stringify({
      enabled: true,
      providers: [
        {
          id: "prov-1",
          name: "Old",
          provider: "openai-compatible",
          model: "gpt-4",
          baseUrl: "http://x/v1",
        },
        { id: "prov-2", name: "Older", model: "claude" },
      ],
      shareSchemaGlobally: false,
    }),
    "Providers from before the AIProvider shape: model and provider are dropped on read, type = type ?? provider ?? anthropic. Missing record fields read as the defaults.",
  ),
  oldAi(
    "old-data/ai-settings-not-json",
    "{not json",
    "A record that isn't JSON reads as the defaults; the next write stores the defaults plus the change.",
  ),
  oldAi(
    "old-data/ai-settings-not-an-object",
    "42",
    "A record that is JSON but not an object reads as the defaults.",
  ),
  oldAi(
    "old-data/ai-settings-array",
    '[{"enabled":false}]',
    'A record that is a JSON array: today the store spreads it into the settings, so the next write stores its elements under "0", "1", ….',
    {
      decision: "20",
      why: "A record that isn't a JSON object reads as the defaults, and the next write stores the defaults plus the change. Today an array's elements are spread into the settings and written back under numeric keys.",
      expected: (raw) => {
        const i = 1;
        const rows = raw.steps[i].rows;
        const value = {
          enabled: false,
          providers: [],
          shareSchemaGlobally: true,
          shareDataGlobally: false,
        };
        return {
          0: { view: { settings: { ...value, enabled: true } } },
          [i]: {
            view: { settings: value },
            rows: withTable(
              rows,
              "app_state",
              (rows.app_state ?? []).map((r) =>
                r.key === "aiSettings" ? { ...r, value: JSON.stringify(value) } : r,
              ),
            ),
          },
        };
      },
    },
  ),
  oldAi(
    "old-data/ai-settings-providers-null",
    '{"enabled":false,"providers":[null],"shareDataGlobally":true}',
    "providers holding null: the whole record reads as the defaults (enabled and the sharing flags too), as the MCP server's reader does.",
  ),
  {
    name: "old-data/user-theme-does-not-parse",
    note: "A user_themes row that doesn't parse is skipped by the load; today the next save replaces every row, which deletes it.",
    seed: bare({ user_themes: [{ id: "theme-bad", data: "{not json" }, themeRow("theme-1")] }),
    view: themeView,
    steps: [themeInit, addTheme("New")],
    change: {
      decision: "20",
      why: "Themes are saved one row at a time (userThemeCreate), so the row that doesn't parse stays. Today saveUserThemes replaces every row and drops it. As in every theme case, a theme call doesn't write theme_preferences either (see themePrefsChanges).",
      expected: (raw) =>
        themePrefsChanges(raw, (i, rows) =>
          i === 1
            ? withTable(
                rows,
                "user_themes",
                [...(rows.user_themes ?? []), { id: "theme-bad", data: "{not json" }].sort(byId),
              )
            : rows,
        ),
    },
  },
  {
    name: "old-data/onboarding-does-not-parse",
    note: "An onboarding_state record that doesn't parse reads as the defaults; the next write stores the defaults plus the change (today's whole record).",
    seed: bare({ onboarding_state: [{ id: 1, data: "{not json" }] }),
    view: onboardingView,
    steps: [
      onboardingInit,
      step("onboardingStore.completeWizard", undefined, async (t) =>
        (await t.stores()).onboarding.completeWizard(),
      ),
    ],
  },
  {
    name: "old-data/tutorial-state-does-not-parse",
    note: "One row's state doesn't parse: the load keeps its completion and drops only that state (Task 1); resetting the lesson removes both rows.",
    seed: bare({
      tutorial_progress: [
        { lesson_id: "select-basics", challenge_id: "c1", state: JSON.stringify(BUILDER_STATE) },
        { lesson_id: "select-basics", challenge_id: "c2", state: "{not json" },
        { lesson_id: "joins", challenge_id: "c1", state: null },
      ],
    }),
    view: tutorialView,
    steps: [
      step(
        "tutorialProgressStore.initialize",
        undefined,
        async (t) => (await t.stores()).tutorial.initialize(),
        [{ group: "settings", method: "tutorialList" }],
      ),
      step("tutorialProgressStore.resetLesson", { lessonId: "select-basics" }, async (t) =>
        (await t.stores()).tutorial.resetLesson("select-basics"),
      ),
    ],
  },
  {
    name: "old-data/project-state-canvas-view",
    note: "active_view canvas, from before the canvas→workflow rename, next to a query tab and a workflow tab (a workflow tab's stored tab_type is canvas today too, so only active_view is old). The load keeps activeView canvas as it is, and the next save writes it back unchanged.",
    seed: standard({
      project_state: [
        {
          ...STATE_P1,
          active_view: "canvas",
          active_workflow_tab_id: "tab-w1",
          active_query_tab_id: "tab-q1",
          tab_order: '["tab-q1","tab-w1"]',
        },
      ],
      tabs: [
        { id: "tab-q1", project_id: "p1", tab_type: "query", name: "Orders", query: "SELECT 1" },
        {
          id: "tab-w1",
          project_id: "p1",
          tab_type: "canvas",
          name: "Canvas: Local",
          connection_id: "c1",
        },
      ],
    }),
    steps: [open(), addQueryTab("A", "SELECT 1")],
  },
  {
    name: "old-data/project-state-json-null-orders",
    note: "tab_order and connection_order holding JSON null (the columns are NOT NULL): both load as empty lists.",
    seed: standard({
      project_state: [{ ...STATE_P1, tab_order: "null", connection_order: "null" }],
    }),
    steps: [open(), addQueryTab("A", "SELECT 1")],
  },
  {
    name: "old-data/project-state-empty-pane-layout",
    note: "pane_layout holding an empty string (p1) and a layout with no panes (p2): both load as no layout.",
    seed: standard({
      project_state: [
        { ...STATE_P1, pane_layout: "" },
        { ...STATE_P1, project_id: "p2", pane_layout: '{"panes":[],"activePaneId":"pane-x"}' },
      ],
    }),
    steps: [
      open(),
      addQueryTab("A", "SELECT 1"),
      switchProject("p2"),
      addQueryTab("B", "SELECT 2"),
    ],
  },
  {
    name: "old-data/saved-canvases-bad-rows",
    note: "A saved workflow that isn't JSON and one that doesn't decode: the load skips both, and today the next project save deletes them.",
    seed: standard({
      project_state: [STATE_P1],
      saved_canvases: [NOT_PARSED_ROW, UNDECODABLE, chartCopyRow("workflow-good", "Good")],
    }),
    steps: [open(), addQueryTab("A", "SELECT 1")],
    change: {
      decision: "23",
      why: "Saved workflows leave the project state: nothing deletes a workflow the page couldn't read. Today the first project save after the load deletes both.",
      expected: (raw) => {
        const out: Record<number, StepChange> = {};
        raw.steps.forEach((s, i) => {
          const rows = s.rows.saved_canvases ?? [];
          const missing = [NOT_PARSED_ROW, UNDECODABLE].filter(
            (b) => !rows.some((r) => r.id === b.id),
          );
          if (missing.length) {
            out[i] = {
              rows: withTable(s.rows, "saved_canvases", [...rows, ...missing].sort(byId)),
            };
          }
        });
        return out;
      },
    },
  },
  {
    name: "old-data/dashboards-starred-null",
    note: "dashboards.starred is nullable on every file: NULL reads as not starred; starring stores 1.",
    seed: dashboardSeed({ dashboards: [dashRow({ id: "dash-2", name: "Costs", starred: null })] }),
    steps: [
      open(),
      step("dashboards.toggleDashboardStarred", { id: "dash-2" }, async (t) =>
        t.page.dashboards.toggleDashboardStarred("dash-2"),
      ),
    ],
  },
  {
    name: "old-data/ai-messages-equal-timestamps",
    note: "Messages with one timestamp, stored in an order their ids don't sort in: the load returns them in insertion order.",
    seed: standard({
      ai_chats: [chatRow()],
      ai_messages: [
        msgRow("m-z", "user", "First, stored first"),
        msgRow("m-a", "assistant", "Second, stored second"),
      ],
    }),
    steps: [open(), activate("c1")],
  },
  {
    name: "old-data/app-state-null-rows",
    note: "app_state rows holding NULL read as unset.",
    seed: bare({
      app_state: [
        { key: "editorKeybindingMode", value: null },
        { key: "pending_changes_enabled", value: null },
        { key: "skippedUpdateVersion", value: null },
        { key: "license_nudge", value: null },
        { key: "aiSettings", value: null },
      ],
    }),
    view: async (t) => ({ settings: await settingsView(t), ai: await aiView(t) }),
    steps: [
      step(
        "stores.load",
        undefined,
        async (t) => {
          const s = await t.stores();
          await s.editor.load();
          await s.pending.load();
          await s.update.initialize();
          await s.nudge.initialize();
          await s.ai.initialize();
        },
        [
          settingGet("editorKeybindingMode"),
          settingGet("pending_changes_enabled"),
          settingGet("skippedUpdateVersion"),
          settingGet("license_nudge"),
          aiGet,
        ],
      ),
    ],
  },
];

// ---------------------------------------------------------------- cases: more view state

/** A project_state row for `p1` with `tabs` rows of only one kind open. */
function onlyTab(tab: Row, active: Row): Seed {
  return {
    project_state: [{ ...STATE_P1, ...active, tab_order: JSON.stringify([tab.id]) }],
    tabs: [{ project_id: "p1", ...tab }],
  };
}

const moreViewCases: Case[] = [
  {
    name: "view-state/starter-tabs-closed",
    note: "The starter tabs closed beside a query tab stay closed after a restart.",
    steps: [
      open(),
      addQueryTab("A", "SELECT 1"),
      step("starterTabs.remove", { id: "getting-started" }, async (t) =>
        t.page.starterTabs.remove("getting-started"),
      ),
      step("starterTabs.remove", { id: "migration-tips" }, async (t) =>
        t.page.starterTabs.remove("migration-tips"),
      ),
      open("restart", "main"),
    ],
  },
  {
    name: "view-state/only-a-workflow-tab",
    note: "A project whose only open tab is a workflow tab gets no starter tabs, on the first load and after a restart (fixed with this task: the check reads every saved tab type).",
    seed: standard(
      onlyTab(
        { id: "tab-w1", tab_type: "canvas", name: "Workflows: Local", connection_id: "c1" },
        { active_view: "workflow", active_workflow_tab_id: "tab-w1" },
      ),
    ),
    steps: [open(), open("restart", "main")],
  },
  {
    name: "view-state/only-a-dashboard-tab",
    note: "A project whose only open tab is a dashboard tab gets no starter tabs (before the fix, the check read the dashboard tabs before they were restored).",
    seed: standard({
      ...onlyTab(
        { id: "tab-db1", tab_type: "dashboard", name: "Sales", source_query: "dash-1" },
        { active_view: "dashboard", active_dashboard_tab_id: "tab-db1" },
      ),
      dashboards: [dashRow()],
    }),
    steps: [open(), open("restart", "main")],
  },
  {
    name: "view-state/new-window-fallback",
    note: "Three windows. A new window's active project is the most recently used window's (windowGet), and its first load of a project copies the most recently written of the other windows' rows. Today the page takes lastActiveProjectId, which the second window wrote last, while the main window was used after it.",
    steps: [
      open(),
      switchProject("p2"),
      open("second", "win-2"),
      step("projects.setActive", { page: "second", id: "p1" }, async (t) =>
        t.on("second", (p) => p.projects.setActive("p1")),
      ),
      step("queryTabs.add", { page: "main", name: "Main's", query: "SELECT 'main'" }, async (t) =>
        t.on("main", (p) => p.queryTabs.add("Main's", "SELECT 'main'")),
      ),
      open("third", "win-3"),
    ],
    change: {
      decision: "22",
      why: "A new window's active project is the most recently used window's (the main window, on p2: its tab save is the latest write), where today it is lastActiveProjectId (p1, from the second window's switch). windowGet answers p2 (its `expect`); a GUI on Core then loads p2, the project windowGet returned; the replayed step-5 windowStateLoad (p1) is today's lastActiveProjectId path as recorded, so what the window shows isn't the recording.",
      expected: (raw) => ({ [raw.steps.length - 1]: { view: null } }),
    },
  },
  {
    name: "view-state/two-windows-active-connections",
    note: "Q14: each window has its own active connection; the legacy mirror takes the saving window's.",
    steps: [
      open(),
      activate("c1"),
      open("second", "win-2"),
      activate("c2"),
      step("queryTabs.add", { page: "main", name: "A", query: "SELECT 1" }, async (t) =>
        t.on("main", (p) => p.queryTabs.add("A", "SELECT 1")),
      ),
    ],
  },
];

const moreDashboardCases: Case[] = [
  {
    name: "dashboards/rename-to-own-name-other-case",
    note: "A rename to the dashboard's own name in another case isn't a clash.",
    seed: dashboardSeed(),
    steps: [open(), renameDashboard("dash-1", "SALES")],
  },
  {
    name: "dashboards/create-name-taken-nfd",
    note: 'Today "Cafe" + U+0301 is stored next to "Café".',
    seed: dashboardSeed({ dashboards: [dashRow({ id: "dash-2", name: "Café" })] }),
    steps: [open(), createDashboard("Café")],
    change: {
      decision: "21",
      why: 'Names compare NFC-normalised: "Cafe" + U+0301 is "Café" (dash-2): NAME_TAKEN, nothing written.',
      expected: (raw) => ({
        1: { ...refused(raw, 1, { code: "NAME_TAKEN", takenBy: "dash-2" })[1], view: null },
      }),
    },
  },
];

const moreChatCases: Case[] = [
  {
    name: "chats/retitle-stored-chat",
    note: "A stored chat with no messages yet: its first message titles it (chatUpdate with title and touched).",
    seed: standard({
      ai_chats: [
        chatRow({ id: "chat-new", title: "New Chat", updated_at: "2024-01-02T00:00:00.000Z" }),
      ],
    }),
    steps: [open(), activate("c1"), send("How many orders are there?", say("There are 42."))],
  },
];

const ONBOARDING_PARTIAL = { isFirstRun: false, dismissedHints: ["sidebar"] };

const moreOldDataCases: Case[] = [
  {
    name: "old-data/onboarding-missing-fields",
    note: "A stored onboarding record missing fields: today the store takes them as undefined and its next write leaves them out.",
    seed: bare({ onboarding_state: [{ id: 1, data: JSON.stringify(ONBOARDING_PARTIAL) }] }),
    view: onboardingView,
    steps: [
      onboardingInit,
      step("onboardingStore.completeWizard", undefined, async (t) =>
        (await t.stores()).onboarding.completeWizard(),
      ),
    ],
    change: {
      decision: "20",
      why: "A stored onboarding record reads over the store's six defaults (as a missing record does), and onboardingPatch merges into that, so the write keeps every field. Today the missing fields read as undefined and the next write drops them.",
      expected: (raw) => {
        const read = { ...ONBOARDING_DEFAULTS, ...ONBOARDING_PARTIAL };
        const after = { ...read, hasCompletedWizard: true, isFirstRun: false };
        const rows = raw.steps[1].rows;
        return {
          0: { view: read },
          1: {
            view: after,
            rows: withTable(rows, "onboarding_state", [{ id: 1, data: JSON.stringify(after) }]),
          },
        };
      },
    },
  },
];

// ---------------------------------------------------------------- run

const FILES: Record<string, Case[]> = {
  "view-state.json": [...viewCases, ...moreViewCases],
  "workflows.json": workflowCases,
  "dashboards.json": [...dashboardCases, ...moreDashboardCases],
  "chats.json": [...chatCases, ...moreChatCases],
  "settings.json": settingsCases,
  "ai-settings.json": aiCases,
  "themes.json": themeCases,
  "misc.json": miscCases,
  "old-data.json": [...oldDataCases, ...moreOldDataCases],
};

describe("the state fixtures through the demo", () => {
  it("the demo replays the state fixture cases", async () => {
    const failures: string[] = [];
    let cases = 0;
    let steps = 0;
    for (const [file, defined] of Object.entries(FILES)) {
      const recorded = new Map(loadFixtures(file).map((f) => [f.name, f]));
      expect([...recorded.keys()].sort(), file).toEqual(defined.map((c) => c.name).sort());
      for (const c of defined) {
        const fx = recorded.get(c.name)!;
        expect(
          fx.steps.map((s) => s.op),
          c.name,
        ).toEqual(c.steps.map((s) => s.op));
        cases++;
        steps += c.steps.length;
        failures.push(...(await replay(c, fx)));
      }
    }
    expect(
      failures,
      `${failures.length} of ${steps} steps differ:\n\n${failures.join("\n\n")}`,
    ).toEqual([]);
    expect([...exemptSeen].sort(), "every exemption names a replayed step").toEqual(
      Object.keys(EXEMPT).sort(),
    );
    expect(cases).toBe(112);
    expect(steps).toBe(377);
  }, 600_000);
});
