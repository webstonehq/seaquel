/**
 * `getShared` and `getImports`: the page's `SharedService` and
 * `ImportsService`. Core's groups on desktop (`CoreShared`, `CoreImports`,
 * sharing the storage client's write queue); `NoShared`/`NoImports` on web
 * and in the demo (Decision 48).
 */
import { getStorage } from "$lib/storage/db";
import type { RustStorageClient } from "$lib/storage/rust-client";
import { isTauri } from "$lib/utils/environment";
import { CoreImports, CoreShared } from "./core-shared";
import { NoImports, NoShared } from "./no-shared";
import type { ImportsService, SharedService } from "./types";

export * from "./types";
export { CoreShared, CoreImports, type SharedCaller, type ImportsCaller } from "./core-shared";
export { NoShared, NoImports } from "./no-shared";

let sharedOverride: SharedService | null = null;
let importsOverride: ImportsService | null = null;
let shared: SharedService | null = null;
let imports: ImportsService | null = null;

/** Replace the page's `SharedService` (tests); `null` goes back to the default. */
export function setShared(next: SharedService | null): void {
  sharedOverride = next;
}

/** Replace the page's `ImportsService` (tests); `null` goes back to the default. */
export function setImports(next: ImportsService | null): void {
  importsOverride = next;
}

const client = () => getStorage() as unknown as RustStorageClient;

export function getShared(): SharedService {
  if (sharedOverride) return sharedOverride;
  return (shared ??= isTauri() ? new CoreShared(client) : new NoShared());
}

export function getImports(): ImportsService {
  if (importsOverride) return importsOverride;
  return (imports ??= isTauri() ? new CoreImports(client) : new NoImports());
}
