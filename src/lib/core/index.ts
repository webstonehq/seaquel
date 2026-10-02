/**
 * The page's `CoreClient`: `TauriCoreClient` on desktop, `HttpCoreClient` on
 * web. In the demo, Core runs in the page (phase 8): once the demo's start
 * has opened it (`$lib/core/browser`, `useBrowserCore`), it's that page's
 * `BrowserCoreClient`. This file never imports the browser module, so
 * desktop and web bundles don't contain it.
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
let browserClient: CoreClient | null = null;

/** Picked per call, so tests and late environment detection see the current mode. */
export function getCoreClient(): CoreClient {
  if (override) return override;
  if (import.meta.env.VITE_BUILD_TARGET === "demo" && browserClient) return browserClient;
  if (isTauri()) return (tauriClient ??= new TauriCoreClient());
  return (httpClient ??= new HttpCoreClient());
}

/** Replace the page's client (tests); `null` goes back to the default. */
export function setCoreClient(next: CoreClient | null): void {
  override = next;
}

/** The demo's in-page client, set by `useBrowserCore` (`$lib/core/browser`). */
export function setBrowserCoreClient(next: CoreClient | null): void {
  browserClient = next;
}
