/**
 * This page's write origin (phase 5d, Decision 18): the id Core puts on the
 * `storageChanged` event of every write the page makes, so the page can
 * skip its own changes (its write's answer already updated it).
 *
 * Since phase 5d-2 it is the page's window id (`window-id.ts`), which the
 * `ui` group's calls must name (Core refuses any other):
 * - Desktop: the webview's label. `core_call` reads it from the webview
 *   itself; the page never sends it.
 * - Web: the tab's `win-<uuid>`, kept in `sessionStorage` across reloads,
 *   sent as `X-Seaquel-Origin` with every `/api/rpc` call and as
 *   `?origin=` on the stream socket. Node forwards it only when it matches
 *   `^[A-Za-z0-9_-]{1,64}$`, and Rust checks it again. It is settled before
 *   the page's first Core call.
 *
 * It isn't a security boundary: a wrong origin only hides a change from
 * the user's own page, or its own view state from itself.
 */

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isTauri } from "$lib/utils/environment";
import { ORIGIN_PATTERN, windowId } from "./window-id";

export { ORIGIN_PATTERN, newOrigin } from "./window-id";

/** The header the web client sends its origin in. */
export const ORIGIN_HEADER = "x-seaquel-origin";

/**
 * The web page's origin: its window id, or `null` before the id is
 * settled (`windowIdReady()`).
 */
export function webPageOrigin(): string | null {
  return windowId();
}

/**
 * The origin this page's writes carry: the webview label on desktop (or
 * `null` when the label isn't a valid origin, as Core then records none),
 * the tab's window id on web (`null` until it is settled).
 */
export function pageOrigin(): string | null {
  if (!isTauri()) return webPageOrigin();
  const label = getCurrentWebview().label;
  return ORIGIN_PATTERN.test(label) ? label : null;
}
