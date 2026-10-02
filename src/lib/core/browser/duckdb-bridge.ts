/**
 * The bridge the browser module drives DuckDB-WASM through (phase 8,
 * Decision 12, as Task 4 built it). It is the one place that knows
 * DuckDB-WASM's API; the module's browser driver
 * (`crates/seaquel-engine-duckdb/src/browser/`) calls these methods and
 * decodes the Arrow IPC bytes they return.
 *
 * The contract the driver relies on:
 * - every method returns a Promise and **posts its request to DuckDB's
 *   worker before it returns** (AsyncDuckDB's methods do, unless OPFS file
 *   handling is configured), so a cancel, ROLLBACK or close the driver sends
 *   from a `Drop` reaches the worker before the next call's statement;
 * - `startPending` resolves with the IPC stream's header (the schema
 *   message), or `null` while the query runs (`pollPending` then);
 *   `fetchChunk` with the next chunk, empty at the end, `null` for "not
 *   yet";
 * - `runQuery` resolves with an IPC file (the driver's ROLLBACK on drop, and
 *   the ENUM re-read);
 * - `cancel` never rejects: it resolves `false` when nothing ran;
 * - a DuckDB whose worker is gone (`terminate()`, a worker that died)
 *   answers every request with `undefined` at once instead of posting it.
 *   The bridge rejects those, since the driver reads `null` as "not yet"
 *   and asks again, which on `undefined` was a loop that never yielded.
 *
 * `closeAll` closes every connection this bridge opened and hasn't closed:
 * after a trap, the dead instance's connections (Decision 16).
 */
import type { AsyncDuckDB } from "@duckdb/duckdb-wasm";

export interface DuckDbBridge {
  connect(): Promise<number>;
  runQuery(connection: number, sql: string): Promise<Uint8Array>;
  startPending(connection: number, sql: string): Promise<Uint8Array | null>;
  pollPending(connection: number): Promise<Uint8Array | null>;
  fetchChunk(connection: number): Promise<Uint8Array | null>;
  cancel(connection: number): Promise<boolean>;
  close(connection: number): Promise<void>;
}

export interface PageDuckDbBridge extends DuckDbBridge {
  /** Closes every connection still open through this bridge. Never rejects. */
  closeAll(): Promise<void>;
}

/** The part of `AsyncDuckDB` the bridge uses (`getVersion` is its liveness ping). */
export type BridgeDuckDb = Pick<
  AsyncDuckDB,
  | "connectInternal"
  | "runQuery"
  | "startPendingQuery"
  | "pollPendingQuery"
  | "fetchQueryResults"
  | "cancelPendingQuery"
  | "disconnect"
> &
  Partial<Pick<AsyncDuckDB, "getVersion">>;

/** What a request fails with once DuckDB stopped answering (Task 7 probe, item 5). */
export const DUCKDB_STOPPED =
  "DuckDB stopped responding: its worker may have crashed. Reload the page to start it again.";

/**
 * The liveness check: once a request has waited `checkAfterMs`, DuckDB is
 * pinged (`getVersion`); a ping unanswered after `pingTimeoutMs` means its
 * worker is gone (killed from outside, or crashed: it then answers nothing,
 * not even `undefined`). A pending query keeps answering pings between its
 * polls, so a long query isn't mistaken for a dead worker. No verdict is
 * passed while a `runQuery` (the driver's ROLLBACK on drop, the ENUM
 * re-read) runs: it can't be interrupted, and the worker answers no ping
 * meanwhile. A verdict is undone when the ping answers after all: the
 * requests it failed stay failed, later ones run.
 */
export interface Liveness {
  checkAfterMs: number;
  pingTimeoutMs: number;
}

export const DEFAULT_LIVENESS: Liveness = { checkAfterMs: 5_000, pingTimeoutMs: 10_000 };

export interface BridgeOptions {
  /**
   * Called with each request as it goes out (`connect`, `connected 3`,
   * `start 3 SELECT …`, `runQuery 3 …`, `cancel 3`, `close 3`), for tests.
   * The SQL is cut at 40 characters. Never pass a logger: SQL isn't logged.
   */
  note?: (line: string) => void;
  /** The liveness check's timing; `false` turns it off. On when `getVersion` exists. */
  liveness?: Liveness | false;
}

/** What DuckDB-WASM answered, or a rejection when it answered nothing (its worker is gone). */
function answered<T>(request: Promise<T | undefined>): Promise<T> {
  return request.then((value) => {
    if (value === undefined) throw new Error("DuckDB isn't running");
    return value;
  });
}

export function makeDuckDbBridge(db: BridgeDuckDb, options: BridgeOptions = {}): PageDuckDbBridge {
  const note = options.note ?? (() => {});
  const open = new Set<number>();
  const liveness = options.liveness === false ? null : (options.liveness ?? DEFAULT_LIVENESS);
  const ping = db.getVersion?.bind(db);

  // Requests posted and not yet answered, failed together if DuckDB died.
  const waiting = new Set<(error: Error) => void>();
  let dead: Error | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  // `runQuery`s in flight: one can't be interrupted (the ENUM re-read, the
  // ROLLBACK on drop), and a worker busy in it answers no ping, so no
  // verdict is passed while one runs.
  let unjudged = 0;

  const die = () => {
    dead = new Error(DUCKDB_STOPPED);
    for (const fail of waiting) fail(dead);
    waiting.clear();
  };
  const check = () => {
    timer = null;
    if (waiting.size === 0 || dead || !ping || !liveness) return;
    if (unjudged > 0) {
      arm();
      return;
    }
    let answered = false;
    const timeout = setTimeout(() => {
      if (!answered) die();
    }, liveness.pingTimeoutMs);
    // Any answer, a late one included, means the worker is there: a verdict
    // passed before it is undone. What already failed stays failed (its
    // caller has moved on); later requests run.
    const alive = () => {
      answered = true;
      clearTimeout(timeout);
      dead = null;
      arm();
    };
    ping().then(alive, alive);
  };
  const arm = () => {
    if (timer === null && waiting.size > 0 && !dead && ping && liveness) {
      timer = setTimeout(check, liveness.checkAfterMs);
    }
  };

  /** `request`, failed with `DUCKDB_STOPPED` if DuckDB stops answering. Posted already. */
  function watched<T>(request: Promise<T>): Promise<T> {
    if (dead) {
      request.catch(() => {});
      return Promise.reject(dead);
    }
    return new Promise<T>((resolve, reject) => {
      let settled = false;
      const fail = (error: Error) => {
        if (settled) return;
        settled = true;
        reject(error);
      };
      waiting.add(fail);
      arm();
      request.then(
        (value) => {
          waiting.delete(fail);
          if (!settled) {
            settled = true;
            resolve(value);
          }
        },
        (error: unknown) => {
          waiting.delete(fail);
          fail(error instanceof Error ? error : new Error(String(error)));
        },
      );
    });
  }

  return {
    connect: () => {
      note("connect");
      return watched(answered(db.connectInternal())).then((connection) => {
        open.add(connection);
        note(`connected ${connection}`);
        return connection;
      });
    },
    runQuery: (connection, sql) => {
      note(`runQuery ${connection} ${sql.slice(0, 40)}`);
      const request = watched(answered(db.runQuery(connection, sql)));
      unjudged += 1;
      const done = () => {
        unjudged -= 1;
      };
      request.then(done, done);
      return request;
    },
    startPending: (connection, sql) => {
      note(`start ${connection} ${sql.slice(0, 40)}`);
      return watched(answered(db.startPendingQuery(connection, sql, true)));
    },
    pollPending: (connection) => watched(answered(db.pollPendingQuery(connection))),
    fetchChunk: (connection) => watched(answered(db.fetchQueryResults(connection))),
    cancel: (connection) => {
      note(`cancel ${connection}`);
      if (dead) return Promise.resolve(false);
      return watched(db.cancelPendingQuery(connection))
        .then((cancelled) => cancelled === true)
        .catch(() => false);
    },
    close: (connection) => {
      note(`close ${connection}`);
      open.delete(connection);
      if (dead) return Promise.resolve();
      return watched(db.disconnect(connection));
    },
    closeAll: async () => {
      const all = [...open];
      open.clear();
      if (dead) return;
      await Promise.all(
        all.map((connection) => {
          note(`close ${connection}`);
          return watched(db.disconnect(connection)).catch(() => {});
        }),
      );
    },
  };
}
