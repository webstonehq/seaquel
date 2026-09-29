/**
 * This page's write origin (phase 5d, Decision 18): the id Core puts on the
 * `storageChanged` event of every write the page makes, so the page can
 * skip its own changes (its write's answer already updated it).
 *
 * - Desktop: the webview's label. `core_call` reads it from the webview
 *   itself; the page never sends it.
 * - Web: a random id per page load, sent as `X-Seaquel-Origin` with every
 *   `/api/rpc` call. Node forwards it only when it matches
 *   `^[A-Za-z0-9_-]{1,64}$`, and Rust checks it again.
 *
 * It isn't a security boundary: a wrong origin only hides a change from
 * the user's own page.
 */

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isTauri } from "$lib/utils/environment";

/** What Node and Rust accept as an origin. */
export const ORIGIN_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;

/** The header the web client sends its origin in. */
export const ORIGIN_HEADER = "x-seaquel-origin";

let webOrigin: string | null = null;

/**
 * A random origin id: `crypto.randomUUID()`, or 16 random bytes as hex where
 * the page has no `randomUUID` (it's only in secure contexts, and a
 * self-hosted install may be reached over plain http on a LAN address).
 */
export function newOrigin(
  source: Pick<Crypto, "getRandomValues"> & Partial<Crypto> = crypto,
): string {
  if (typeof source.randomUUID === "function") return source.randomUUID();
  const bytes = source.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** The web page's origin: made once per page load. */
export function webPageOrigin(): string {
  webOrigin ??= newOrigin();
  return webOrigin;
}

/**
 * The origin this page's writes carry: the webview label on desktop (or
 * `null` when the label isn't a valid origin, as Core then records none),
 * the page's random id on web.
 */
export function pageOrigin(): string | null {
  if (!isTauri()) return webPageOrigin();
  const label = getCurrentWebview().label;
  return ORIGIN_PATTERN.test(label) ? label : null;
}
