/**
 * Core in the page for tests that need a real metadata database (vitest
 * only; nothing in the app imports it). Phase 8 moved the demo's twins'
 * tests here: they run the GUI's `Core*` services over the browser module
 * (the test build, `loadTestModule`), as the demo does.
 *
 * - `transport(origin)` sends each call to the module under a window's
 *   origin (`__test_call_as`); the page itself always sends `demo`. Several
 *   windows on one file each use their own.
 * - `query`/`execute` read and write the metadata file directly, for seeds
 *   and dumps: a read takes Core's snapshot into `node:sqlite`; writes are
 *   applied to a copy and Core reopens on it before the next call or read
 *   (Core's open runs as for a stored snapshot).
 *
 * DuckDB isn't started: the bridge refuses every call. A test that needs
 * DuckDB uses `bootDuckDb` and `makeDuckDbBridge` instead.
 */
import { DatabaseSync, type SQLInputValue } from "node:sqlite";
import { RustStorageClient, type CoreTransport } from "$lib/storage/rust-client";
import type { DuckDbBridge } from "../duckdb-bridge";
import type { TestModule } from "./node";

/** A bridge for tests that never reach DuckDB: every call rejects. */
export function noDuckDb(): DuckDbBridge {
  const refuse = () => Promise.reject(new Error("no DuckDB in this test"));
  return {
    connect: refuse,
    runQuery: refuse,
    startPending: refuse,
    pollPending: refuse,
    fetchChunk: refuse,
    cancel: async () => false,
    close: async () => {},
  };
}

type Row = Record<string, unknown>;

/** `node:sqlite`'s `serialize`/`deserialize` (Node 24), which the installed `@types/node` lacks. */
type SerializableDb = DatabaseSync & {
  serialize(): Uint8Array;
  deserialize(data: Uint8Array): void;
};

const memoryDb = () => new DatabaseSync(":memory:") as SerializableDb;

export interface ModuleCore {
  readonly module: TestModule;
  /** Calls the module under `origin` (the window id; `demo` by default). */
  transport(origin?: string): CoreTransport;
  /** A `RustStorageClient` over `transport(origin)`. */
  storage(origin?: string): RustStorageClient;
  /** Reads the metadata file as it stands (pending writes applied first). */
  query<T = Row>(sql: string, params?: unknown[]): Promise<T[]>;
  /** Writes to the metadata file; Core reopens on it before the next call or read. */
  execute(sql: string, params?: unknown[]): Promise<void>;
  /** Core's metadata file as bytes. */
  snapshot(): Promise<Uint8Array>;
  /** Opens a new Core on `image` (none: an empty file). */
  reopen(image?: Uint8Array | null): Promise<void>;
}

/** A value `node:sqlite` binds (booleans as 0/1, objects as JSON). */
function bindable(value: unknown): SQLInputValue {
  if (value === undefined || value === null) return null;
  if (typeof value === "boolean") return value ? 1 : 0;
  if (
    typeof value === "number" ||
    typeof value === "bigint" ||
    typeof value === "string" ||
    value instanceof Uint8Array
  ) {
    return value;
  }
  return JSON.stringify(value);
}

/** Opens Core on the module over an empty file (or `image`). */
export async function openModuleCore(
  module: TestModule,
  options: { bridge?: DuckDbBridge; image?: Uint8Array | null } = {},
): Promise<ModuleCore> {
  const bridge = options.bridge ?? noDuckDb();
  let pending: SerializableDb | null = null;

  const open = async (image: Uint8Array | null | undefined) => {
    await module.open(bridge, image ?? undefined, () => {});
  };

  const snapshot = async (): Promise<Uint8Array> => {
    // Refused while a call holds the file; it settles within a few turns.
    for (let attempt = 0; ; attempt++) {
      try {
        return module.snapshot();
      } catch (error) {
        if (attempt > 50) throw error;
        await new Promise((resolve) => setImmediate(resolve));
      }
    }
  };

  // One reopen at a time: a call that arrives while Core reopens on the
  // written file waits for it instead of reaching the Core being replaced.
  let reopening: Promise<void> = Promise.resolve();
  const flush = async () => {
    if (pending) {
      const image = pending.serialize();
      pending.close();
      pending = null;
      reopening = reopening.then(() => open(image));
    }
    await reopening;
  };

  const load = async (): Promise<SerializableDb> => {
    const db = memoryDb();
    db.deserialize(await snapshot());
    return db;
  };

  await open(options.image);

  const transport =
    (origin = "demo"): CoreTransport =>
    async (body) => {
      await flush();
      try {
        return JSON.parse(await module.__test_call_as(body, origin)) as unknown;
      } catch (error) {
        // A refusal is the `RpcError`'s JSON text, as the page's transport reads it.
        if (typeof error === "string") throw JSON.parse(error) as unknown;
        throw error;
      }
    };

  return {
    module,
    transport,
    storage: (origin) => new RustStorageClient(transport(origin)),
    async query<T>(sql: string, params: unknown[] = []): Promise<T[]> {
      await flush();
      const db = await load();
      try {
        return db
          .prepare(sql)
          .all(...params.map(bindable))
          .map((row) => ({ ...row }) as T);
      } finally {
        db.close();
      }
    },
    async execute(sql, params = []) {
      pending ??= await load();
      if (params.length === 0) pending.exec(sql);
      else pending.prepare(sql).run(...params.map(bindable));
    },
    async snapshot() {
      await flush();
      return snapshot();
    },
    async reopen(image) {
      if (pending) {
        pending.close();
        pending = null;
      }
      await open(image);
    },
  };
}
