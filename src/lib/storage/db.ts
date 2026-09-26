import type { StorageClient } from "./client";
import { RustStorageClient } from "./rust-client";
import { isTauri, isWeb } from "$lib/utils/environment";

let instance: StorageClient | null = null;

/**
 * The app's storage. Desktop and web talk to `seaquel-storage` in Rust; the
 * demo keeps the TypeScript repositories over sql.js, loaded on first use so
 * the other builds never bundle them.
 */
export function getStorage(): StorageClient {
  instance ??= isTauri() || isWeb() ? new RustStorageClient() : lazyClient(openDemoStorage);
  return instance;
}

async function openDemoStorage(): Promise<StorageClient> {
  const { openSqljsStorageClient } = await import("./sqljs-client");
  return openSqljsStorageClient();
}

/**
 * A client whose methods wait for `open()` first. A failed open is retried
 * on the next call instead of returning the same rejection forever.
 */
function lazyClient(open: () => Promise<StorageClient>): StorageClient {
  let pending: Promise<StorageClient> | null = null;
  const client = () => {
    pending ??= open().catch((error: unknown) => {
      pending = null;
      throw error;
    });
    return pending;
  };
  return new Proxy({} as StorageClient, {
    get(_target, repo: string) {
      return new Proxy(
        {},
        {
          get(_t, method: string) {
            return async (...args: unknown[]) => {
              const c = (await client()) as unknown as Record<
                string,
                Record<string, (...a: unknown[]) => unknown>
              >;
              return c[repo][method](...args);
            };
          },
        },
      );
    },
  });
}
