/**
 * `TsLibrary` (the demo's library, phase 5d-1) against the library parity
 * fixtures (`crates/seaquel-workspace/tests/fixtures/library`), replayed the
 * way the Rust replay does (`crates/seaquel-core/tests/library.rs`): each
 * step's `library` calls in order, then the library tables compared whole
 * and the cascade tables by id, with `changes.json`'s expected steps in
 * place of the recorded ones. Ids are bound to the ids the calls made, by
 * prefix; `<now>` is any time from the case.
 *
 * What differs from the Rust replay, because the demo has no keychain:
 * - `secretStore` isn't compared, and a call's `secrets` are left out (the
 *   demo GUI never sends them; `TsLibrary` refuses them like the web
 *   workspace). Rows hold no secret, so they still compare.
 * - `add/keychain-failure` (a keychain failure injected into the step) isn't
 *   replayed: there is no keychain to fail.
 *
 * Plus a few of `TsLibrary`'s own rules.
 */
import initSqlJs from "sql.js";
import { beforeAll, describe, expect, it } from "vitest";
import { bootstrapSqljsDatabase } from "$lib/storage/sqljs-client";
import { WebSqliteDatabase } from "$lib/storage/web-sqlite";
import { TsLibrary, freeName, nameKey, versionPrune } from "./ts-library";

import {
  type Json,
  type Obj,
  FILES,
  LIBRARY_TABLES,
  CASCADE_TABLES,
  ID_PREFIXES,
  collectTokens,
  substituteValue,
  UUID_V4,
  type Expected,
  type Outcome,
  compare,
  type Db,
  openCaseDb,
  snapshot,
  bindStep,
  load,
  expectedRows,
  loadChanges,
} from "./fixture-support";

/** Steps the demo can't replay (see the header). */
const SKIPPED = new Set(["add/keychain-failure#0"]);

let SQL: Awaited<ReturnType<typeof initSqlJs>>;

beforeAll(async () => {
  SQL = await initSqlJs();
});

async function openCase(c: Obj): Promise<{ db: Db; lib: TsLibrary }> {
  const db = await openCaseDb(SQL, c);
  return { db, lib: new TsLibrary(db) };
}

function param<T>(p: Obj, key: string): T {
  return p[key] as T;
}

async function callLibrary(lib: TsLibrary, method: string, p: Obj): Promise<Json> {
  switch (method) {
    case "connectionsList":
      return lib.listConnections();
    case "connectionCreate":
      return lib.createConnection(param(p, "connection"));
    case "connectionUpdate":
      return lib.updateConnection(param(p, "id"), param(p, "patch"));
    case "connectionRemove":
      return lib.removeConnection(param(p, "id"));
    case "projectCreate":
      return lib.createProject(param(p, "project"));
    case "projectEnsureDefault":
      return lib.ensureDefaultProject();
    case "projectUpdate":
      return lib.updateProject(param(p, "id"), param(p, "patch"));
    case "projectRemove":
      return lib.removeProject(param(p, "id"));
    case "labelCreate":
      return lib.createLabel(param(p, "projectId"), param(p, "label"));
    case "labelUpdate":
      return lib.updateLabel(param(p, "projectId"), param(p, "labelId"), param(p, "patch"));
    case "labelRemove":
      return lib.removeLabel(param(p, "projectId"), param(p, "labelId"));
    case "savedQueryCreate":
      return lib.createSavedQuery(param(p, "query"));
    case "savedQueryUpdate":
      return lib.updateSavedQuery(param(p, "id"), param(p, "patch"));
    case "savedQueryRemove":
      return lib.removeSavedQuery(param(p, "id"));
    default:
      throw new Error(`unknown library method ${method}`);
  }
}

class Replay {
  bound = new Map<string, string>();
  constructor(readonly lib: TsLibrary) {}

  async call(call: Obj, fresh: string[]): Promise<Outcome> {
    const method = call.method as string;
    const raw = (call.params ?? {}) as Obj;
    const tokens: [string, string][] = [];
    collectTokens(raw, tokens);
    for (const [token, prefix] of tokens) {
      if (this.bound.has(token)) continue;
      const taken = new Set(this.bound.values());
      const id = fresh.find((x) => x.startsWith(prefix) && !taken.has(x));
      if (!id) throw new Error(`${token} in ${method} names no id made earlier`);
      this.bound.set(token, id);
    }
    const p = substituteValue(raw, this.bound) as Obj;
    // The demo GUI never sends secrets (no keychain): see the header.
    delete p.secrets;
    let value: Json;
    try {
      value = await callLibrary(this.lib, method, p);
    } catch (e) {
      const err = e as { code?: string; takenBy?: string; message?: string };
      if (typeof err.code !== "string") throw e;
      return { ok: false, code: err.code, takenBy: err.takenBy };
    }
    const v = (value as { value: Obj }).value;
    const made: string[] = [];
    if (["connectionCreate", "projectCreate", "labelCreate", "savedQueryCreate"].includes(method)) {
      made.push(v.id as string);
    } else if (method === "savedQueryUpdate") {
      const version = v.version as Obj | null;
      if (version) made.push(version.id as string);
    }
    for (const id of made) {
      const prefix = ID_PREFIXES.find((x) => id.startsWith(x));
      expect(
        prefix && UUID_V4.test(id.slice(prefix.length)),
        `${id} is a prefix and a v4 uuid`,
      ).toBe(true);
      fresh.push(id);
    }
    return { ok: true, value };
  }
}

describe("the library fixtures", () => {
  it("replay through TsLibrary with exactly changes.json's differences", async () => {
    const changes = loadChanges();
    const failures: string[] = [];
    const listedSeen = new Set<string>();
    let cases = 0;
    let steps = 0;
    for (const file of FILES) {
      for (const c of load(file)) {
        cases++;
        const name = c.name as string;
        const started = new Date().toISOString();
        const { db, lib } = await openCase(c);
        const r = new Replay(lib);
        for (const [i, step] of (c.steps as Obj[]).entries()) {
          if (step.library === null) continue;
          const entry = (
            (changes[name]?.expected as Obj | undefined)?.steps as Record<string, Obj> | undefined
          )?.[String(i)];
          if (SKIPPED.has(`${name}#${i}`)) {
            if (entry) listedSeen.add(`${name}#${i}`);
            continue;
          }
          steps++;
          if (entry) listedSeen.add(`${name}#${i}`);
          const calls = (Array.isArray(step.library) ? step.library : [step.library]) as Obj[];
          const fresh: string[] = [];
          let outcome: Outcome = { ok: true, value: null };
          for (const call of calls) {
            outcome = await r.call(call, fresh);
            if (!outcome.ok) break;
          }
          const snap = await snapshot(db);
          const recorded: Expected = {
            outcome: step.outcome as Obj,
            rows: step.rows as Obj,
          };
          const expected: Expected = {
            outcome: (entry?.outcome as Obj | undefined) ?? (step.outcome as Obj),
            rows: expectedRows(step.rows as Obj, entry),
          };
          bindStep(
            r.bound,
            [expected.outcome, expected.rows],
            fresh,
            (b) => compare(expected, outcome, snap, { bound: b, started }) === null,
          );
          const unbound: [string, string][] = [];
          collectTokens(expected.outcome, unbound);
          for (const table of [...LIBRARY_TABLES, ...CASCADE_TABLES]) {
            collectTokens(expected.rows[table], unbound);
          }
          const missing = [
            ...new Set(
              unbound.map(([t]) => t).filter((t) => t.includes("<id:") && !r.bound.has(t)),
            ),
          ];
          if (missing.length) {
            failures.push(
              `${name} step ${i}: ${missing.join(", ")} bound to no id the library made`,
            );
          }
          const cx = { bound: r.bound, started };
          const why = compare(expected, outcome, snap, cx);
          if (why) {
            failures.push(`${name} step ${i}${entry ? " (listed)" : ""}:\n${why}`);
          } else if (entry && compare(recorded, outcome, snap, cx) === null) {
            failures.push(`${name} step ${i} is listed in changes.json but matches the recording`);
          }
        }
      }
    }
    for (const [name, entry] of Object.entries(changes)) {
      if (name === "*") continue;
      for (const i of Object.keys(((entry.expected as Obj).steps ?? {}) as Obj)) {
        if (!listedSeen.has(`${name}#${i}`)) {
          failures.push(`changes.json lists ${name} step ${i}, which wasn't replayed`);
        }
      }
    }
    expect(
      failures,
      `${failures.length} of ${steps} steps differ:\n\n${failures.join("\n\n")}`,
    ).toEqual([]);
    expect(cases).toBeGreaterThanOrEqual(116);
    expect(steps).toBeGreaterThanOrEqual(140);
  }, 120_000);
});

describe("TsLibrary's own rules", () => {
  async function fresh() {
    const db = new WebSqliteDatabase(new SQL.Database());
    await bootstrapSqljsDatabase(db);
    const lib = new TsLibrary(db);
    const { value } = await lib.ensureDefaultProject();
    return { db, lib, projectId: value[0].id };
  }

  it("folds names fully and normalises them", () => {
    expect(nameKey("STRASSE")).toBe(nameKey("Straße"));
    expect(nameKey("Café")).toBe(nameKey("Café"));
    expect(nameKey(" ärger db ")).toBe(nameKey("Ärger DB"));
    expect(nameKey("﻿x　")).toBe(nameKey("X"));
    expect(nameKey("a")).not.toBe(nameKey("b"));
  });

  it("picks the first free name", () => {
    const taken = new Set(["local", "local (2)"].map(nameKey));
    expect(freeName("Local", taken)).toBe("Local (3)");
    expect(freeName("Other", taken)).toBe("Other");
  });

  it("prunes back to the nearest keyframe", () => {
    const v = (version: number, keyframe: boolean) => ({ id: `v${version}`, version, keyframe });
    expect(versionPrune([v(1, true), v(2, false), v(3, false), v(4, false)], 2)).toEqual([]);
    expect(versionPrune([v(1, true), v(2, true), v(3, false), v(4, false)], 2)).toEqual(["v1"]);
    expect(versionPrune([v(1, true), v(2, true), v(3, true)], 0)).toEqual([]);
  });

  it("refuses a NUL anywhere", async () => {
    const { lib, projectId } = await fresh();
    await expect(
      lib.createSavedQuery({ projectId, name: "a\u0000b", query: "SELECT 1" }),
    ).rejects.toMatchObject({ code: "INVALID_ARGUMENT" });
    await expect(
      lib.createConnection({
        projectId,
        name: "x",
        type: "postgres",
        host: "h\u0000",
        port: 5432,
        databaseName: "d",
        username: "u",
      }),
    ).rejects.toMatchObject({ code: "INVALID_ARGUMENT" });
  });

  it("refuses secrets: the demo has no keychain", async () => {
    const { db, lib, projectId } = await fresh();
    await expect(
      lib.createConnection(
        {
          projectId,
          name: "x",
          type: "postgres",
          host: "h",
          port: 5432,
          databaseName: "d",
          username: "u",
          savePassword: true,
        },
        { db: "pw" },
      ),
    ).rejects.toMatchObject({ code: "NOT_SUPPORTED" });
    expect(await db.query("SELECT id FROM connections")).toEqual([]);
  });

  it("numbers its change sequence per write and keeps one epoch", async () => {
    const { lib, projectId } = await fresh();
    const a = await lib.listConnections();
    const created = await lib.createSavedQuery({ projectId, name: "Q", query: "SELECT 1" });
    const b = await lib.listSavedQueries(projectId);
    expect(created.seq.epoch).toBe(a.seq.epoch);
    expect(created.seq.n).toBeGreaterThan(a.seq.n);
    expect(b.seq.n).toBe(created.seq.n);
  });

  it("appends a keyframe of the previous text, only when the text changes", async () => {
    const { lib, projectId } = await fresh();
    const q = (await lib.createSavedQuery({ projectId, name: "Q", query: "SELECT 1" })).value;
    const same = await lib.updateSavedQuery(q.id, { query: "SELECT 1" });
    expect(same.value.version).toBeNull();
    const changed = await lib.updateSavedQuery(q.id, { query: "SELECT 2" });
    expect(changed.value.version).toMatchObject({ version: 1, snapshot: "SELECT 1", diff: null });
    expect(changed.value.query.query).toBe("SELECT 2");
  });
});
