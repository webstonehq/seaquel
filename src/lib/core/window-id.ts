/**
 * This page's window id (phase 5d-2, Decision 22): the id its view state
 * (open tabs, layout, active ids) is stored under, and its write origin
 * (Decision 18), so Core can check that a `ui` call names the caller's own
 * window.
 *
 * - **Desktop:** the webview's label. The main window's is `main` after
 *   every restart, so it gets its tabs back.
 * - **Web:** `win-<uuid>`, kept in `sessionStorage`, so a reload keeps its
 *   tabs and a new browser tab starts as a new window. Browsers copy
 *   `sessionStorage` into a duplicated tab (and on "reopen closed tab"), so
 *   at load the page claims its id on a `BroadcastChannel`; if another live
 *   tab answers holding it within 100 ms, the page makes a new id. Two pages
 *   checking one id at once (a tab reloading while its duplicate opens)
 *   tell each other apart by a random nonce in each claim: the lower one
 *   makes a new id. A page that makes a fresh id has nothing to check.
 * - **Demo:** `demo`. The demo's Core runs in the page (phase 8) and stamps
 *   every write with `demo`; each tab has its own copy of the file.
 *
 * The web id must be settled before the page's first Core call, since it is
 * the origin every call carries: `windowIdReady()` resolves it once, the
 * storage gate awaits it, and the web transports wait for it too
 * (`httpCoreTransport`, the stream socket's `?origin=`).
 */

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isTauri, isWeb } from "$lib/utils/environment";
import { log } from "$lib/utils/logger";

/** What Node and Rust accept as an origin, and so as a window id. */
export const ORIGIN_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;

/** The `sessionStorage` key the web window id is kept under. */
export const WINDOW_ID_KEY = "seaquel.windowId";
/** The `BroadcastChannel` pages claim their window id on. */
export const WINDOW_ID_CHANNEL = "seaquel-window-ids";
/** How long a page waits for another tab to say it holds the same id. */
export const DUPLICATE_CHECK_MS = 100;
/** The demo's one window. */
export const DEMO_WINDOW_ID = "demo";

/**
 * A random id: `crypto.randomUUID()`, or 16 random bytes as hex where the
 * page has no `randomUUID` (it's only in secure contexts, and a self-hosted
 * install may be reached over plain http on a LAN address).
 */
export function newOrigin(
  source: Pick<Crypto, "getRandomValues"> & Partial<Crypto> = crypto,
): string {
  if (typeof source.randomUUID === "function") return source.randomUUID();
  const bytes = source.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * The messages pages exchange on the channel. A claim carries the checking
 * page's nonce; `taken` answers it (from a live page holding the id, or from
 * a page checking the same id with a higher nonce).
 */
type ClaimMessage = { type: "claim" | "taken"; id: string; nonce?: string };

/** What the check needs of a `BroadcastChannel`. */
export interface ChannelLike {
  postMessage(message: unknown): void;
  onmessage: ((event: { data: unknown }) => void) | null;
  close(): void;
}

export interface WebWindowIdDeps {
  /** `sessionStorage`, or `null` where the page can't use it. */
  storage: Pick<Storage, "getItem" | "setItem"> | null;
  /** A channel named `WINDOW_ID_CHANNEL`, or `null` where there is none. */
  openChannel: () => ChannelLike | null;
  newId?: () => string;
  /** The claim's nonce; random by default. */
  nonce?: () => string;
  waitMs?: number;
}

function isClaim(data: unknown, type: ClaimMessage["type"], id: string): boolean {
  return (
    typeof data === "object" &&
    data !== null &&
    (data as ClaimMessage).type === type &&
    (data as ClaimMessage).id === id
  );
}

/** A stored web id: `win-` and a well-formed origin. */
function isWebWindowId(value: string | null): value is string {
  return value !== null && value.startsWith("win-") && ORIGIN_PATTERN.test(value);
}

/**
 * The web tab's window id: the one in `sessionStorage` unless another live
 * tab answers the claim holding it, else a new `win-<id>` (stored). From
 * then on the page answers other tabs' claims of its id, on the returned
 * channel, for as long as the page lives.
 */
export async function resolveWebWindowId(
  deps: WebWindowIdDeps,
): Promise<{ id: string; channel: ChannelLike | null }> {
  const waitMs = deps.waitMs ?? DUPLICATE_CHECK_MS;
  let id: string | null = null;
  try {
    const stored = deps.storage?.getItem(WINDOW_ID_KEY) ?? null;
    if (isWebWindowId(stored)) id = stored;
  } catch {
    // Storage blocked: a new id, which then lives only as long as the page.
  }
  let channel: ChannelLike | null = null;
  try {
    channel = deps.openChannel();
  } catch {
    channel = null;
  }
  if (id !== null && channel !== null) {
    const claimed = id;
    const ch = channel;
    const mine = (deps.nonce ?? newOrigin)();
    const taken = await new Promise<boolean>((resolve) => {
      const timer = setTimeout(() => resolve(false), waitMs);
      const yieldId = () => {
        clearTimeout(timer);
        resolve(true);
      };
      ch.onmessage = (event) => {
        if (isClaim(event.data, "taken", claimed)) {
          yieldId();
        } else if (isClaim(event.data, "claim", claimed)) {
          // Another page checks the same id now: the lower nonce yields.
          const theirs = String((event.data as ClaimMessage).nonce ?? "");
          if (theirs < mine) {
            ch.postMessage({ type: "taken", id: claimed } satisfies ClaimMessage);
          } else {
            yieldId();
          }
        }
      };
      ch.postMessage({ type: "claim", id: claimed, nonce: mine } satisfies ClaimMessage);
    });
    if (taken) {
      void log.info("Another tab holds this tab's window id (a duplicated tab): making a new one");
      id = null;
    }
  }
  if (id === null) {
    id = `win-${(deps.newId ?? newOrigin)()}`;
    try {
      deps.storage?.setItem(WINDOW_ID_KEY, id);
    } catch {
      // As above: the id lasts as long as the page.
    }
  }
  if (channel !== null) {
    const mine = id;
    const ch = channel;
    ch.onmessage = (event) => {
      if (isClaim(event.data, "claim", mine)) {
        ch.postMessage({ type: "taken", id: mine } satisfies ClaimMessage);
      }
    };
  }
  return { id, channel };
}

function browserDeps(): WebWindowIdDeps {
  return {
    storage: (() => {
      try {
        return typeof sessionStorage === "undefined" ? null : sessionStorage;
      } catch {
        return null;
      }
    })(),
    openChannel: () =>
      typeof BroadcastChannel === "undefined"
        ? null
        : (new BroadcastChannel(WINDOW_ID_CHANNEL) as unknown as ChannelLike),
  };
}

let settled: string | null = null;
let pending: Promise<string> | null = null;
/** Kept open so this page answers later tabs' claims. */
let answering: ChannelLike | null = null;

async function resolveWindowId(): Promise<string> {
  if (isTauri()) return getCurrentWebview().label;
  if (isWeb()) {
    const { id, channel } = await resolveWebWindowId(browserDeps());
    answering = channel;
    return id;
  }
  return DEMO_WINDOW_ID;
}

/**
 * Resolves this page's window id, once for every caller. Every Core call
 * the page makes waits for it on web (it is the origin), and the storage
 * gate awaits it before the page's first call.
 */
export function windowIdReady(): Promise<string> {
  pending ??= resolveWindowId().then((id) => {
    settled = id;
    return id;
  });
  return pending;
}

/**
 * The page's window id once settled, else `null` (on web, while the
 * duplicate check runs). The desktop's is its webview label, known at once.
 */
export function windowId(): string | null {
  if (settled === null && isTauri()) settled = getCurrentWebview().label;
  return settled;
}

/** For tests: forget the resolved id, so the next page load resolves again. */
export function resetWindowIdForTests(): void {
  answering?.close();
  answering = null;
  settled = null;
  pending = null;
}
