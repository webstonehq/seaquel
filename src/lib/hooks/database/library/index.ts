/**
 * `getLibrary`: the page's `LibraryService`, picked as `getEditService`
 * picks: desktop and web store the library through Core (`CoreLibrary`,
 * the `library` group, sharing the storage client's write queue); the demo,
 * which has no Core, keeps it in TypeScript over its sql.js database
 * (`TsLibrary`) until phase 8.
 */
import { demoDatabase, getStorage } from "$lib/storage/db";
import type { RustStorageClient } from "$lib/storage/rust-client";
import { isTauri, isWeb } from "$lib/utils/environment";
import { CoreLibrary } from "./core-library";
import type { TsLibrary } from "./ts-library";
import type { ConnectionDraft, LibraryService, Seqd, WireConnection } from "./types";

export * from "./types";
export { CoreLibrary, type LibraryCaller } from "./core-library";
export { ChangeFeed, type StorageChange, type ReloadReason } from "./change-feed";
export { RowSeqs, NEW } from "./seqs";

/** The demo's library: `TsLibrary` plus its fixed demo connection. */
export interface DemoLibrary extends LibraryService {
  putDemoConnection(id: string, draft: ConnectionDraft): Promise<Seqd<WireConnection>>;
}

let override: LibraryService | null = null;
let coreLibrary: CoreLibrary | null = null;
let demoLibrary: DemoLibrary | null = null;

/** Replace the page's library (tests); `null` goes back to the default. */
export function setLibrary(next: LibraryService | null): void {
  override = next;
}

export function getLibrary(): LibraryService {
  if (override) return override;
  if (isTauri() || isWeb()) {
    return (coreLibrary ??= new CoreLibrary(() => getStorage() as unknown as RustStorageClient));
  }
  return (demoLibrary ??= lazyDemoLibrary());
}

/** Whether `library` stores the demo's fixed connection (`TsLibrary`). */
export function isDemoLibrary(library: LibraryService): library is DemoLibrary {
  return typeof (library as Partial<DemoLibrary>).putDemoConnection === "function";
}

/**
 * The demo's `TsLibrary`, made on first use over the database its storage
 * client uses, so the other builds never bundle it. A failed open is
 * retried on the next call.
 */
function lazyDemoLibrary(): DemoLibrary {
  let pending: Promise<TsLibrary> | null = null;
  const open = () => {
    pending ??= Promise.all([demoDatabase(), import("./ts-library")])
      .then(([db, { TsLibrary }]) => new TsLibrary(db))
      .catch((error: unknown) => {
        pending = null;
        throw error;
      });
    return pending;
  };
  return new Proxy({} as DemoLibrary, {
    get(_target, method: string) {
      return async (...args: unknown[]) => {
        const library = (await open()) as unknown as Record<string, (...a: unknown[]) => unknown>;
        return library[method](...args);
      };
    },
  });
}
