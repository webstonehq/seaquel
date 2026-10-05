/**
 * The demo's view-state journal (phase 8).
 *
 * In the demo the page's Core keeps its file in IndexedDB, saved on
 * `pagehide` (`$lib/core/browser`). A view-state save made while the page is
 * going away can't commit in time in Chromium, which runs no task between
 * `beforeunload` and `pagehide`, and WebKit may drop an IndexedDB write still
 * in flight at close. So in the demo the window state's `pagehide` save (the
 * one web sends as a `keepalive` request) goes, synchronously, into one small
 * `localStorage` key instead: the `ui.windowStateSave` request itself, capped
 * like a keepalive body. The next start replays it through Core right after
 * Core opens, and Core keeps it only if its `rev` is newer than what's
 * stored, as for any save.
 *
 * Demo only, and only this key: the old demo file's `seaquel_db*` keys stay
 * deleted unread.
 */
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import { wellFormedJson } from "$lib/core/well-formed";

export const VIEW_STATE_JOURNAL_KEY = "seaquel.demo.pendingViewState";

/** As `KEEPALIVE_MAX_BYTES`: a view state past it is flushed the usual way, as far as it gets. */
export const VIEW_STATE_JOURNAL_MAX_BYTES = 60 * 1024;

function isViewStateSave(request: unknown): request is CoreRequest {
  const r = request as { method?: unknown; params?: { method?: unknown; params?: unknown } };
  return (
    r?.method === "ui" &&
    r.params?.method === "windowStateSave" &&
    typeof r.params.params === "object" &&
    r.params.params !== null
  );
}

function pageStorage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/**
 * Keeps `request` (a `ui.windowStateSave`) as the journal, replacing any
 * earlier one. False when it wasn't kept: another request, past the cap, or
 * storage that refuses.
 */
export function writeViewStateJournal(
  request: CoreRequest,
  storage: Storage | null | undefined = pageStorage(),
): boolean {
  if (!storage || !isViewStateSave(request)) return false;
  const text = JSON.stringify(wellFormedJson(request));
  if (new TextEncoder().encode(text).byteLength > VIEW_STATE_JOURNAL_MAX_BYTES) return false;
  try {
    storage.setItem(VIEW_STATE_JOURNAL_KEY, text);
    return true;
  } catch {
    return false;
  }
}

/**
 * Sends the kept save through `send` (Core's `call`) and forgets it, sent or
 * not, so a bad entry never comes back: `none` (nothing kept), `sent`,
 * `dropped` (not a view-state save) or `failed` (Core refused it). Never
 * throws.
 */
export async function replayViewStateJournal(
  send: (body: Uint8Array) => Promise<unknown>,
  storage: Storage | null | undefined = pageStorage(),
): Promise<"none" | "sent" | "dropped" | "failed"> {
  if (!storage) return "none";
  let text: string | null;
  try {
    text = storage.getItem(VIEW_STATE_JOURNAL_KEY);
    if (text === null) return "none";
    storage.removeItem(VIEW_STATE_JOURNAL_KEY);
  } catch {
    return "none";
  }
  let request: unknown;
  try {
    request = JSON.parse(text);
  } catch {
    return "dropped";
  }
  if (!isViewStateSave(request)) return "dropped";
  try {
    await send(new TextEncoder().encode(JSON.stringify(request)));
    return "sent";
  } catch {
    return "failed";
  }
}
