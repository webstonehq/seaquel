import type { StorageClient } from "./client";
import { RustStorageClient } from "./rust-client";

let instance: StorageClient | null = null;

/**
 * The app's storage: `seaquel-storage` in Rust, through Core. Desktop calls
 * it over `core_call`, web over `/api/rpc`, and the demo runs Core in the
 * page (phase 8; the storage client's transport is picked per call).
 */
export function getStorage(): StorageClient {
  instance ??= new RustStorageClient();
  return instance;
}
