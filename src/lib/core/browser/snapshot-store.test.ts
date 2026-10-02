/**
 * The IndexedDB snapshot store against a small in-memory IndexedDB: just
 * the calls the store makes, with IndexedDB's ordering (requests answer
 * later, in order; a transaction completes after its requests).
 */
import { describe, expect, it } from "vitest";
import { DB_NAME, openSnapshotStore, SNAPSHOT_KEY, STORE, UNREADABLE_KEY } from "./snapshot-store";

type Handler = (() => void) | null;

class FakeRequest<T> {
  result!: T;
  error: Error | null = null;
  onsuccess: Handler = null;
  onerror: Handler = null;
  onupgradeneeded: Handler = null;
}

/** One object store's data per database, and a queue that runs every step in order. */
class FakeIdb {
  readonly databases = new Map<string, Map<string, Map<string, unknown>>>();
  private chain: Promise<void> = Promise.resolve();
  failOpen = false;

  /** Runs `step` after everything queued before it, like IndexedDB's task queue. */
  later(step: () => void) {
    this.chain = this.chain
      .then(() => new Promise<void>((resolve) => setTimeout(resolve, 0)))
      .then(step);
  }

  open(name: string) {
    const request = new FakeRequest<unknown>();
    this.later(() => {
      if (this.failOpen) {
        request.error = new Error("SecurityError");
        request.onerror?.();
        return;
      }
      let stores = this.databases.get(name);
      const fresh = !stores;
      if (!stores) this.databases.set(name, (stores = new Map()));
      const db = this.db(stores);
      request.result = db;
      if (fresh) request.onupgradeneeded?.();
      request.onsuccess?.();
    });
    return request;
  }

  private db(stores: Map<string, Map<string, unknown>>) {
    const later = (step: () => void) => this.later(step);
    return {
      objectStoreNames: { contains: (n: string) => stores.has(n) },
      createObjectStore: (n: string) => stores.set(n, new Map()),
      transaction: (storeName: string) => {
        const tx = {
          oncomplete: null as Handler,
          onerror: null as Handler,
          onabort: null as Handler,
          error: null,
          objectStore() {
            const data = stores.get(storeName)!;
            const ask = <T>(run: () => T) => {
              const request = new FakeRequest<T>();
              later(() => {
                request.result = run();
                request.onsuccess?.();
              });
              return request;
            };
            return {
              get: (key: string) => ask(() => data.get(key)),
              put: (value: unknown, key: string) => ask(() => void data.set(key, value)),
              delete: (key: string) => ask(() => void data.delete(key)),
            };
          },
        };
        // Completes after every request it made so far (and any made in
        // their callbacks, which queue behind this).
        later(() => later(() => later(() => tx.oncomplete?.())));
        return tx;
      },
    };
  }

  files() {
    return this.databases.get(DB_NAME)?.get(STORE);
  }
}

const bytes = (...b: number[]) => new Uint8Array(b);

describe("the IndexedDB snapshot store", () => {
  it("saves, loads and moves a snapshot aside", async () => {
    const idb = new FakeIdb();
    const store = await openSnapshotStore(idb as unknown as IDBFactory);
    expect(await store.load()).toBeNull();
    await store.save(bytes(1, 2, 3));
    expect(await store.load()).toEqual(bytes(1, 2, 3));
    await store.moveAside();
    expect(await store.load()).toBeNull();
    expect(idb.files()?.get(UNREADABLE_KEY)).toEqual(bytes(1, 2, 3));
    expect(idb.files()?.has(SNAPSHOT_KEY)).toBe(false);
  });

  it("applies saves in the order they were made", async () => {
    const idb = new FakeIdb();
    const store = await openSnapshotStore(idb as unknown as IDBFactory);
    await Promise.all([store.save(bytes(1)), store.save(bytes(2)), store.save(bytes(3))]);
    expect(await store.load()).toEqual(bytes(3));
  });

  it("refuses to open without IndexedDB, or when it won't open", async () => {
    await expect(openSnapshotStore(null)).rejects.toThrow("IndexedDB isn't available");
    const idb = new FakeIdb();
    idb.failOpen = true;
    await expect(openSnapshotStore(idb as unknown as IDBFactory)).rejects.toThrow("SecurityError");
  });
});
