/**
 * Where the demo keeps its metadata file between visits (phase 8):
 * IndexedDB database `seaquel-demo`, object store `files`, key
 * `meta.db` (the snapshot) and `meta.db.unreadable` (a snapshot that didn't
 * open, kept aside rather than lost).
 *
 * Saves are applied in the order they were made: each is its own readwrite
 * transaction on the one store, and IndexedDB runs overlapping readwrite
 * transactions in the order they were created. So a save started on
 * `pagehide` while an earlier one is still in flight lands after it.
 */

export const DB_NAME = "seaquel-demo";
export const STORE = "files";
export const SNAPSHOT_KEY = "meta.db";
export const UNREADABLE_KEY = "meta.db.unreadable";

export interface SnapshotStore {
  /**
   * The stored snapshot, or `null` if there is none. Rejects when it can't
   * be read, or what's stored isn't bytes.
   */
  load(): Promise<Uint8Array | null>;
  /** Stores `bytes` as the snapshot. Applied in call order. */
  save(bytes: Uint8Array): Promise<void>;
  /** Moves the snapshot to `meta.db.unreadable`, replacing an older one. */
  moveAside(): Promise<void>;
}

function request<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("IndexedDB request failed"));
  });
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error ?? new Error("IndexedDB transaction failed"));
    tx.onabort = () => reject(tx.error ?? new Error("IndexedDB transaction aborted"));
  });
}

/**
 * Opens the store. Rejects when IndexedDB can't be used: missing, blocked
 * site data, or a private mode that refuses it. The caller then runs in
 * memory and says so once (`STORAGE_UNAVAILABLE`).
 */
export async function openSnapshotStore(
  factory: IDBFactory | null | undefined = globalThis.indexedDB,
): Promise<SnapshotStore> {
  if (!factory) throw new Error("IndexedDB isn't available");
  const open = factory.open(DB_NAME, 1);
  open.onupgradeneeded = () => {
    if (!open.result.objectStoreNames.contains(STORE)) open.result.createObjectStore(STORE);
  };
  const db = await request(open);

  return {
    async load() {
      const tx = db.transaction(STORE, "readonly");
      const value = await request(tx.objectStore(STORE).get(SNAPSHOT_KEY));
      if (value === undefined) return null;
      if (value instanceof Uint8Array) return value;
      if (value instanceof ArrayBuffer) return new Uint8Array(value);
      // Something else under the key: not ours to open or to overwrite.
      throw new TypeError("the stored snapshot isn't bytes");
    },
    async save(bytes) {
      const tx = db.transaction(STORE, "readwrite");
      tx.objectStore(STORE).put(bytes, SNAPSHOT_KEY);
      await done(tx);
    },
    async moveAside() {
      const tx = db.transaction(STORE, "readwrite");
      const files = tx.objectStore(STORE);
      // Inside the request's own callback, so the transaction is still
      // active whatever the browser does with promise jobs.
      const get = files.get(SNAPSHOT_KEY);
      get.onsuccess = () => {
        if (get.result !== undefined) files.put(get.result, UNREADABLE_KEY);
        files.delete(SNAPSHOT_KEY);
      };
      await done(tx);
    },
  };
}
