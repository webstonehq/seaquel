/**
 * The page's `CoreClient`: `TauriCoreClient` on desktop, `HttpCoreClient` on
 * web. The demo has no Core; nothing there asks for one.
 */

import { isTauri } from "$lib/utils/environment";
import type { CoreClient } from "./client";
import { HttpCoreClient } from "./http";
import { TauriCoreClient } from "./tauri";

export * from "./client";
export { withHostKeyPrompt, type SshServer } from "./host-key";

let override: CoreClient | null = null;
let tauriClient: TauriCoreClient | null = null;
let httpClient: HttpCoreClient | null = null;

/** Picked per call, so tests and late environment detection see the current mode. */
export function getCoreClient(): CoreClient {
  if (override) return override;
  if (isTauri()) return (tauriClient ??= new TauriCoreClient());
  return (httpClient ??= new HttpCoreClient());
}

/** Replace the page's client (tests); `null` goes back to the default. */
export function setCoreClient(next: CoreClient | null): void {
  override = next;
}
