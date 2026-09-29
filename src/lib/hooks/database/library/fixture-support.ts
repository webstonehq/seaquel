/**
 * The library fixtures' replay rules, shared by the replay through
 * `TsLibrary`'s calls (`ts-library.test.ts`) and the one through the view
 * models (`library-replay.svelte.test.ts`): tokens and id binding, `<now>`,
 * the row comparison (library tables whole, cascade tables by id, no state
 * of a removed project), `changes.json` and the case files. Test support
 * only; nothing in the app imports it.
 */
import type initSqlJs from "sql.js";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bootstrapSqljsDatabase } from "$lib/storage/sqljs-client";
import { WebSqliteDatabase } from "$lib/storage/web-sqlite";

export type SqlJs = Awaited<ReturnType<typeof initSqlJs>>;

export type Json = unknown;
export type Row = Record<string, unknown>;
export type Obj = Record<string, Json>;

export const FIXTURES = join(process.cwd(), "crates/seaquel-workspace/tests/fixtures/library");
export const FILES = [
  "connections.json",
  "projects.json",
  "saved-queries.json",
  "imports.json",
  "legacy-strings.json",
];

/** The recorder's tables, in seed order, with their sort keys. */
export const TABLES: [string, string][] = [
  ["projects", "id"],
  ["project_labels", "project_id, id"],
  ["connections", "id"],
  ["connection_labels", "connection_id, label_id"],
  ["saved_queries", "id"],
  ["query_versions", "saved_query_id, version"],
  ["query_history", "id"],
  ["ai_chats", "id"],
  ["ai_messages", "id"],
  ["dashboards", "id"],
  ["dashboard_versions", "id"],
  ["saved_canvases", "id"],
  ["project_state", "project_id"],
  ["tabs", "project_id, id"],
  ["app_state", "key"],
];
export const LIBRARY_TABLES = [
  "projects",
  "project_labels",
  "connections",
  "connection_labels",
  "saved_queries",
  "query_versions",
];
export const CASCADE_TABLES = [
  "query_history",
  "ai_chats",
  "ai_messages",
  "dashboards",
  "dashboard_versions",
  "saved_canvases",
];
export const ID_PREFIXES = ["conn-", "project-", "label-", "saved-", "ver-"];
// ── Tokens ──

/** `<id:n>` (with the id prefix before it) and `<version:n>` tokens in `s`. */
export function tokensIn(s: string): [string, string][] {
  const out: [string, string][] = [];
  for (const marker of ["<id:", "<version:"]) {
    let from = 0;
    for (;;) {
      const start = s.indexOf(marker, from);
      if (start < 0) break;
      const close = s.indexOf(">", start);
      if (close < 0) break;
      const end = close + 1;
      if (marker === "<version:") {
        out.push([s.slice(start, end), "ver-"]);
      } else {
        const prefix = ID_PREFIXES.find((p) => s.slice(0, start).endsWith(p));
        if (prefix) out.push([s.slice(start - prefix.length, end), prefix]);
      }
      from = end;
    }
  }
  return out;
}

export function collectTokens(v: Json, out: [string, string][]): void {
  if (typeof v === "string") out.push(...tokensIn(v));
  else if (Array.isArray(v)) v.forEach((x) => collectTokens(x, out));
  else if (v && typeof v === "object") {
    for (const [k, x] of Object.entries(v)) {
      out.push(...tokensIn(k));
      collectTokens(x, out);
    }
  }
}

export function substitute(s: string, bound: Map<string, string>): string {
  let out = s;
  const tokens = [...bound.keys()].sort((a, b) => b.length - a.length);
  for (const t of tokens) if (out.includes(t)) out = out.split(t).join(bound.get(t)!);
  return out;
}

export function substituteValue(v: Json, bound: Map<string, string>): Json {
  if (typeof v === "string") return substitute(v, bound);
  if (Array.isArray(v)) return v.map((x) => substituteValue(x, bound));
  if (v && typeof v === "object") {
    return Object.fromEntries(
      Object.entries(v).map(([k, x]) => [substitute(k, bound), substituteValue(x, bound)]),
    );
  }
  return v;
}

export const UUID_V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
export const isIso = (s: string) => s.length === 24 && s.endsWith("Z") && s[10] === "T";

// ── Matching ──

export interface Ctx {
  bound: Map<string, string>;
  started: string;
}

export function matches(e: Json, a: Json, cx: Ctx): boolean {
  if (typeof e === "string" && typeof a === "string") {
    if (e === "<now>") return isIso(a) && a >= cx.started;
    const sub = substitute(e, cx.bound);
    if (sub === a) return true;
    const unbound = tokensIn(sub);
    return (
      unbound.length === 1 &&
      unbound[0][0] === sub &&
      a.startsWith(unbound[0][1]) &&
      UUID_V4.test(a.slice(unbound[0][1].length))
    );
  }
  if (typeof e === "number" && typeof a === "number") return e === a;
  if (e === null || a === null) return e === a;
  if (typeof e === "boolean") return e === a;
  if (Array.isArray(e)) {
    return Array.isArray(a) && e.length === a.length && e.every((x, i) => matches(x, a[i], cx));
  }
  if (typeof e === "object" && typeof a === "object" && !Array.isArray(a)) {
    const eo = e as Obj;
    const ao = a as Obj;
    return (
      Object.keys(eo).length === Object.keys(ao).length &&
      Object.entries(eo).every(([k, x]) => k in ao && matches(x, ao[k], cx))
    );
  }
  return false;
}

export function orderOf(table: string): string {
  return TABLES.find(([t]) => t === table)![1];
}

export function sortKey(row: Row, order: string, bound: Map<string, string>): string {
  return order
    .split(",")
    .map((c) => {
      const v = row[c.trim()];
      if (typeof v === "string") return substitute(v, bound);
      if (typeof v === "number") return v.toFixed(6).padStart(20, "0");
      return JSON.stringify(v);
    })
    .join("\u0001");
}

export const cmp = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);

export function rowsMatch(table: string, expected: Row[], actual: Row[], cx: Ctx): boolean {
  if (expected.length !== actual.length) return false;
  const order = orderOf(table);
  const e = [...expected].sort((x, y) =>
    cmp(sortKey(x, order, cx.bound), sortKey(y, order, cx.bound)),
  );
  const a = [...actual].sort((x, y) =>
    cmp(sortKey(x, order, cx.bound), sortKey(y, order, cx.bound)),
  );
  return e.every((x, i) => matches(x, a[i], cx));
}

export function idsOf(rows: Row[]): Set<string> {
  return new Set(rows.flatMap((r) => (typeof r.id === "string" ? [r.id] : [])));
}

export type Tables = Record<string, Row[]>;

export interface Expected {
  outcome: Obj;
  rows: Obj;
}

export type Outcome = { ok: true; value: Json } | { ok: false; code: string; takenBy?: string };

export function compare(exp: Expected, outcome: Outcome, snap: Tables, cx: Ctx): string | null {
  const why: string[] = [];
  if (exp.outcome.ok === true) {
    if (!outcome.ok) why.push(`expected ok, got ${outcome.code}`);
  } else if (outcome.ok) {
    why.push(`expected a refusal ${JSON.stringify(exp.outcome)}, got ok`);
  } else {
    let code = exp.outcome.code as string | undefined;
    if (code === "SAVED_CONNECTION_NOT_FOUND") code = "CONNECTION_NOT_FOUND";
    if (code !== outcome.code) {
      why.push(`expected ${JSON.stringify(exp.outcome)}, got ${outcome.code}`);
    }
    const takenBy = exp.outcome.takenBy;
    if (typeof takenBy === "string" && outcome.takenBy !== substitute(takenBy, cx.bound)) {
      why.push(`takenBy ${takenBy} != ${outcome.takenBy}`);
    }
  }
  for (const table of LIBRARY_TABLES) {
    const e = (exp.rows[table] as Row[] | undefined) ?? [];
    const a = snap[table];
    if (!rowsMatch(table, e, a, cx)) {
      why.push(
        `${table}:\n  expected ${JSON.stringify(substituteValue(e, cx.bound))}\n  actual   ${JSON.stringify(a)}`,
      );
    }
  }
  for (const table of CASCADE_TABLES) {
    const e = new Set(
      [...idsOf((exp.rows[table] as Row[] | undefined) ?? [])].map((id) =>
        substitute(id, cx.bound),
      ),
    );
    const a = idsOf(snap[table]);
    if (e.size !== a.size || [...e].some((id) => !a.has(id))) {
      why.push(`${table} ids: expected ${[...e].join(",")}, actual ${[...a].join(",")}`);
    }
  }
  const projects = idsOf(snap.projects);
  for (const table of ["project_state", "tabs"]) {
    for (const row of snap[table]) {
      const p = typeof row.project_id === "string" ? row.project_id : "";
      if (!projects.has(p)) why.push(`${table} row of removed project ${p}`);
    }
  }
  return why.length ? why.join("\n") : null;
}

// ── Running a case ──

export type Db = InstanceType<typeof WebSqliteDatabase>;

/** The recorder's beta-era file: no foreign key on the two `project_id` columns. */
export async function makeBetaEra(db: Db): Promise<void> {
  await db.execute("PRAGMA foreign_keys=OFF");
  const rebuild: [string, string, string][] = [
    [
      "saved_queries",
      `CREATE TABLE saved_queries_beta (id TEXT PRIMARY KEY, name TEXT NOT NULL, query TEXT NOT NULL,
        parameters TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
        starred INTEGER NOT NULL DEFAULT 0, shared INTEGER NOT NULL DEFAULT 0, description TEXT,
        database_type TEXT, tags TEXT, folder TEXT, project_id TEXT)`,
      "CREATE INDEX idx_saved_queries_project ON saved_queries(project_id)",
    ],
    [
      "dashboards",
      `CREATE TABLE dashboards_beta (id TEXT PRIMARY KEY, name TEXT NOT NULL,
        viewport TEXT NOT NULL DEFAULT '{"x":0,"y":0,"zoom":1}', widgets TEXT NOT NULL DEFAULT '[]',
        date_filter TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, starred INTEGER DEFAULT 0,
        shared INTEGER NOT NULL DEFAULT 0, description TEXT, project_id TEXT)`,
      "CREATE INDEX idx_dashboards_project ON dashboards(project_id)",
    ],
  ];
  for (const [table, create, index] of rebuild) {
    await db.execute(create);
    await db.execute(`DROP TABLE ${table}`);
    await db.execute(`ALTER TABLE ${table}_beta RENAME TO ${table}`);
    await db.execute(`DROP INDEX IF EXISTS idx_${table}_project`);
    await db.execute(index);
  }
  await db.execute("PRAGMA foreign_keys=ON");
}

export async function openCaseDb(SQL: SqlJs, c: Obj): Promise<Db> {
  const db = new WebSqliteDatabase(new SQL.Database());
  await bootstrapSqljsDatabase(db);
  if (c.file === "v2026.4.5-beta.1") await makeBetaEra(db);
  const seed = (c.seed ?? {}) as Record<string, Row[]>;
  for (const [table] of TABLES) {
    for (const row of seed[table] ?? []) {
      const cols = Object.keys(row);
      await db.execute(
        `INSERT INTO ${table} (${cols.join(", ")}) VALUES (${cols.map(() => "?").join(", ")})`,
        cols.map((k) => {
          const v = row[k];
          if (typeof v === "boolean") return v ? 1 : 0;
          if (v !== null && typeof v === "object") return JSON.stringify(v);
          return v;
        }),
      );
    }
  }
  return db;
}

export async function snapshot(db: Db): Promise<Tables> {
  const out: Tables = {};
  for (const [table, order] of TABLES) {
    out[table] = await db.query<Row>(`SELECT * FROM ${table} ORDER BY ${order}`);
  }
  return out;
}

export function permutations(n: number, k: number): number[][] {
  if (k === 0) return [[]];
  const out: number[][] = [];
  for (const rest of permutations(n, k - 1)) {
    for (let i = 0; i < n; i++) if (!rest.includes(i)) out.push([...rest, i]);
  }
  return out.slice(0, 5040);
}

export function bindStep(
  bound: Map<string, string>,
  expected: Json[],
  fresh: string[],
  fits: (b: Map<string, string>) => boolean,
): void {
  const all: [string, string][] = [];
  expected.forEach((v) => collectTokens(v, all));
  const seen = new Set<string>();
  const tokens = all.filter(([t]) => !bound.has(t) && !seen.has(t) && seen.add(t));
  const taken = new Set(bound.values());
  const free = fresh.filter((id) => !taken.has(id));
  let choices: [string, string][][] = [[]];
  for (const prefix of ID_PREFIXES) {
    const ts = tokens.filter(([, p]) => p === prefix).map(([t]) => t);
    const ids = free.filter((id) => id.startsWith(prefix));
    if (ts.length === 0 || ids.length < ts.length) continue;
    const next: [string, string][][] = [];
    for (const perm of permutations(ids.length, ts.length)) {
      for (const base of choices) {
        next.push([...base, ...ts.map((t, i) => [t, ids[perm[i]]] as [string, string])]);
      }
    }
    choices = next;
  }
  for (const c of choices) {
    const trial = new Map([...bound, ...c]);
    if (fits(trial)) {
      for (const [k, v] of c) bound.set(k, v);
      return;
    }
  }
  if (choices[0]) for (const [k, v] of choices[0]) bound.set(k, v);
}

export function load(file: string): Obj[] {
  return JSON.parse(readFileSync(join(FIXTURES, file), "utf8")) as Obj[];
}

export function expectedRows(recorded: Obj, entry: Obj | undefined): Obj {
  const rows = { ...recorded };
  const tables = entry?.rows as Obj | undefined;
  if (tables) Object.assign(rows, tables);
  return rows;
}

export function loadChanges(): Record<string, Obj> {
  return JSON.parse(readFileSync(join(FIXTURES, "changes.json"), "utf8")) as Record<string, Obj>;
}

/** The changes.json entry for step `i` of case `name`, if any. */
export function changeEntry(
  changes: Record<string, Obj>,
  name: string,
  i: number,
): Obj | undefined {
  return ((changes[name]?.expected as Obj | undefined)?.steps as Record<string, Obj> | undefined)?.[
    String(i)
  ];
}
