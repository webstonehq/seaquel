/**
 * `TsUi`: the demo's `UiService` (phase 5d-2, Decisions 22 and 26), Core's
 * `ui` rules in TypeScript over the demo's sql.js file (the `windows` and
 * `window_state` tables), until phase 8.
 *
 * It follows `crates/seaquel-core/src/state.rs`:
 * - the window id must be the caller's origin (the demo's is `demo`);
 * - a window's first load of a project copies the project's most recently
 *   used window's state, else today's `project_state` and `tabs` rows
 *   (without the saved workflows, the connection order and the starred
 *   lists), else nothing, and stores a copy as the window's own at once;
 * - a save is stored only when its `rev` is higher than the stored one;
 * - `windowActivate` sets the window's active project and
 *   `lastActiveProjectId`; `windowGet` answers the window's, else the most
 *   recent window's, else `lastActiveProjectId`.
 *
 * What it leaves out, since the demo has one window and no older release
 * reading its file: the legacy mirror, the prunes and the web limits.
 * Tests build it with other origins to stand in for several windows.
 *
 * Calls run one at a time.
 */
import type { SqliteDatabase } from "$lib/storage/sqlite-types";
import { projectStateRepo } from "$lib/storage/repos/project-state-repo";
import { DEMO_WINDOW_ID } from "$lib/core/window-id";
import {
  LibraryCallError,
  INVALID_ARGUMENT,
  type ChangeSeq,
  type Seqd,
  type UiService,
  type ViewState,
  type ViewStateLoaded,
  type WindowActive,
  type WindowStateSaved,
} from "./types";

const LAST_ACTIVE_PROJECT_KEY = "lastActiveProjectId";

/** Legacy fields a view state doesn't carry (`seaquel_workspace::state::NOT_VIEW_STATE`). */
const NOT_VIEW_STATE = [
  "savedWorkflows",
  "connectionOrder",
  "starredSharedQueryIds",
  "starredSharedDashboardIds",
] as const;

export interface TsUiOptions {
  /** The caller's origin; the demo's page is `demo`. */
  origin?: () => string | null;
  now?: () => Date;
  epoch?: string;
}

type Row = Record<string, unknown>;

function parseState(text: unknown): ViewState | null {
  if (typeof text !== "string") return null;
  try {
    const value: unknown = JSON.parse(text);
    return typeof value === "object" && value !== null && !Array.isArray(value)
      ? (value as ViewState)
      : null;
  } catch {
    return null;
  }
}

export class TsUi implements UiService {
  private readonly origin: () => string | null;
  private readonly now: () => Date;
  private readonly epoch: string;
  private n = 0;
  private lastStamp = 0;
  private queue: Promise<unknown> = Promise.resolve();

  constructor(
    private readonly db: SqliteDatabase,
    options: TsUiOptions = {},
  ) {
    this.origin = options.origin ?? (() => DEMO_WINDOW_ID);
    this.now = options.now ?? (() => new Date());
    this.epoch = options.epoch ?? crypto.randomUUID();
  }

  private run<T>(fn: () => Promise<T>): Promise<T> {
    const next = this.queue.then(fn);
    this.queue = next.catch(() => {});
    return next;
  }

  private seq(): ChangeSeq {
    return { epoch: this.epoch, n: this.n };
  }

  private bump(): ChangeSeq {
    this.n += 1;
    return this.seq();
  }

  /** A time for `updated_at`, strictly after the last one: "most recent" never ties. */
  private stamp(): string {
    const t = Math.max(this.now().getTime(), this.lastStamp + 1);
    this.lastStamp = t;
    return new Date(t).toISOString();
  }

  /** Core's `check_window`: a window may only name itself. */
  private checkWindow(windowId: string): void {
    if (windowId !== this.origin()) {
      throw new LibraryCallError(INVALID_ARGUMENT, "The window id isn't this window's.");
    }
  }

  private async projectOrFail(projectId: string): Promise<void> {
    const rows = await this.db.query<Row>("SELECT id FROM projects WHERE id = ?", [projectId]);
    if (rows.length === 0) throw new LibraryCallError("PROJECT_NOT_FOUND", "Project not found.");
  }

  private touchWindow(windowId: string, at: string) {
    return {
      sql: `INSERT INTO windows (window_id, updated_at) VALUES (?, ?)
            ON CONFLICT(window_id) DO UPDATE SET updated_at = excluded.updated_at`,
      params: [windowId, at],
    };
  }

  private putState(windowId: string, projectId: string, rev: number, state: string, at: string) {
    return {
      sql: `INSERT INTO window_state (window_id, project_id, state, rev, updated_at)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(window_id, project_id) DO UPDATE SET
              state = excluded.state, rev = excluded.rev, updated_at = excluded.updated_at`,
      params: [windowId, projectId, state, rev, at],
    };
  }

  windowGet(windowId: string): Promise<Seqd<WindowActive>> {
    return this.run(async () => {
      this.checkWindow(windowId);
      const seq = this.seq();
      const [own] = await this.db.query<Row>(
        "SELECT active_project_id FROM windows WHERE window_id = ?",
        [windowId],
      );
      if (typeof own?.active_project_id === "string") {
        return { value: { activeProjectId: own.active_project_id, from: "window" }, seq };
      }
      const [recent] = await this.db.query<Row>(
        `SELECT active_project_id FROM windows WHERE active_project_id IS NOT NULL
         ORDER BY updated_at DESC, rowid DESC LIMIT 1`,
      );
      if (typeof recent?.active_project_id === "string") {
        return { value: { activeProjectId: recent.active_project_id, from: "recent" }, seq };
      }
      const [last] = await this.db.query<Row>("SELECT value FROM app_state WHERE key = ?", [
        LAST_ACTIVE_PROJECT_KEY,
      ]);
      return typeof last?.value === "string"
        ? { value: { activeProjectId: last.value, from: "lastActive" }, seq }
        : { value: { activeProjectId: null, from: null }, seq };
    });
  }

  windowActivate(windowId: string, projectId: string): Promise<Seqd<null>> {
    return this.run(async () => {
      this.checkWindow(windowId);
      await this.projectOrFail(projectId);
      const at = this.stamp();
      await this.db.transaction([
        {
          sql: `INSERT INTO windows (window_id, active_project_id, updated_at) VALUES (?, ?, ?)
                ON CONFLICT(window_id) DO UPDATE SET
                  active_project_id = excluded.active_project_id, updated_at = excluded.updated_at`,
          params: [windowId, projectId, at],
        },
        {
          sql: `INSERT INTO app_state (key, value) VALUES (?, ?)
                ON CONFLICT(key) DO UPDATE SET value = excluded.value`,
          params: [LAST_ACTIVE_PROJECT_KEY, projectId],
        },
      ]);
      return { value: null, seq: this.bump() };
    });
  }

  windowStateLoad(windowId: string, projectId: string): Promise<Seqd<ViewStateLoaded>> {
    return this.run(async () => {
      this.checkWindow(windowId);
      const [own] = await this.db.query<Row>(
        "SELECT state, rev FROM window_state WHERE window_id = ? AND project_id = ?",
        [windowId, projectId],
      );
      const ownState = parseState(own?.state);
      if (own && ownState) {
        return {
          value: { state: ownState, rev: Number(own.rev), copiedFrom: null },
          seq: this.seq(),
        };
      }
      await this.projectOrFail(projectId);
      const recent = await this.db.query<Row>(
        `SELECT state FROM window_state WHERE project_id = ? AND window_id <> ?
         ORDER BY updated_at DESC, rowid DESC`,
        [projectId, windowId],
      );
      let state = recent.map((r) => parseState(r.state)).find((s) => s !== null) ?? null;
      let copiedFrom: ViewStateLoaded["copiedFrom"] = "window";
      if (!state) {
        const legacy = await projectStateRepo.load(this.db, projectId);
        if (legacy) {
          const view: Record<string, unknown> = { ...legacy };
          for (const key of NOT_VIEW_STATE) delete view[key];
          state = view as ViewState;
          copiedFrom = "legacy";
        }
      }
      if (!state) {
        // A stored row that doesn't read keeps its rev: the page counts up from it.
        return {
          value: { state: null, rev: own ? Number(own.rev) : 0, copiedFrom: "empty" },
          seq: this.seq(),
        };
      }
      const rev = own ? Number(own.rev) + 1 : 0;
      const at = this.stamp();
      await this.db.transaction([
        this.touchWindow(windowId, at),
        this.putState(windowId, projectId, rev, JSON.stringify(state), at),
      ]);
      return { value: { state, rev, copiedFrom }, seq: this.bump() };
    });
  }

  windowStateSave(
    windowId: string,
    projectId: string,
    rev: number,
    state: ViewState,
  ): Promise<Seqd<WindowStateSaved>> {
    return this.run(async () => {
      this.checkWindow(windowId);
      if (!Number.isSafeInteger(rev) || rev < 0) {
        throw new LibraryCallError(INVALID_ARGUMENT, "The view state's rev isn't a count.");
      }
      if (state.projectId !== undefined && state.projectId !== projectId) {
        throw new LibraryCallError(INVALID_ARGUMENT, "The view state names another project.");
      }
      await this.projectOrFail(projectId);
      const [stored] = await this.db.query<Row>(
        "SELECT rev FROM window_state WHERE window_id = ? AND project_id = ?",
        [windowId, projectId],
      );
      if (stored && rev <= Number(stored.rev)) {
        return { value: { stale: true, rev: Number(stored.rev) }, seq: this.seq() };
      }
      const at = this.stamp();
      await this.db.transaction([
        this.touchWindow(windowId, at),
        this.putState(windowId, projectId, rev, JSON.stringify(state), at),
      ]);
      return { value: { stale: false, rev }, seq: this.bump() };
    });
  }

  /** The demo has no `pagehide` path: nothing is sent. */
  windowStateSaveKeepalive(): boolean {
    return false;
  }
}
