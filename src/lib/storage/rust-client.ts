/**
 * `StorageClient` over the Rust core: every method is one typed
 * `StorageRequest`, sent as a workspace call (`CoreRequest`) through
 * `core_call` on desktop or `POST /api/rpc` on web.
 *
 * - **Bytes, not objects.** A request goes out as its JSON text encoded to
 *   bytes. Rust needs `method` before `params` at both levels, and stored
 *   JSON (workflows, themes, license state) must reach it byte for byte, so
 *   nothing between here and `serde_json::from_slice` may re-parse it.
 * - **No params, no key.** A method without params sends `{"method": …}`;
 *   Rust refuses `"params": {}`.
 * - **Write order.** Writes go through one queue, so they land in the order
 *   they were issued (the debounced saves rely on it).
 *   Reads skip the queue.
 * - **Mapping.** The generated `Persisted*` wire types stay in this file:
 *   callers see the app's types. `lastConnected` crosses as text and becomes
 *   a `Date`.
 * - **Phase 5d-2** moved app state, project state, dashboards, chats,
 *   themes, onboarding, tutorial and import state to the `library`,
 *   `settings` and `ui` groups (`library()`, `settings()`, `ui()` here,
 *   used by `CoreLibrary`, `CoreSettings` and `CoreUi`). The storage group
 *   keeps query history, the license and the web vault.
 * - **Phase 5e** moved the shared repos to the `shared` group and added the
 *   `imports` group (`shared()`, `imports()` here, used by `CoreShared` and
 *   `CoreImports`; desktop only).
 */

import { invoke } from "@tauri-apps/api/core";
import { ORIGIN_HEADER } from "$lib/core/origin";
import { windowId, windowIdReady } from "$lib/core/window-id";
import type { PersistedConnection } from "$lib/hooks/database/types";
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import type { CoreResponse } from "$lib/types/generated/CoreResponse";
import type { LibraryRequest } from "$lib/types/generated/LibraryRequest";
import type { LibraryResponse } from "$lib/types/generated/LibraryResponse";
import type { PersistedConnection as WireConnection } from "$lib/types/generated/PersistedConnection";
import type { RpcError } from "$lib/types/generated/RpcError";
import type { SecretRequest } from "$lib/types/generated/SecretRequest";
import type { ImportsRequest } from "$lib/types/generated/ImportsRequest";
import type { ImportsResponse } from "$lib/types/generated/ImportsResponse";
import type { SharedRequest } from "$lib/types/generated/SharedRequest";
import type { SharedResponse } from "$lib/types/generated/SharedResponse";
import type { SettingsRequest } from "$lib/types/generated/SettingsRequest";
import type { SettingsResponse } from "$lib/types/generated/SettingsResponse";
import type { UiRequest } from "$lib/types/generated/UiRequest";
import type { UiResponse } from "$lib/types/generated/UiResponse";
import type { SecretResponse } from "$lib/types/generated/SecretResponse";
import type { StorageRequest } from "$lib/types/generated/StorageRequest";
import type { StorageResponse } from "$lib/types/generated/StorageResponse";
import { isTauri } from "$lib/utils/environment";
import { log } from "$lib/utils/logger";
import type { StorageClient } from "./client";

// -------- Transport --------

/**
 * A failed workspace call. `code` is the `RpcError` code (`LEGACY_STORAGE`,
 * `STORAGE_ERROR`, `INVALID_ARGUMENT`, `NOT_SUPPORTED`, …); the message reads
 * `"CODE: message"`, like the other Rust-backed calls.
 */
export class CoreCallError extends Error {
  readonly code: string;
  /** For `NAME_TAKEN` (a library call): the id of the row that has the name. */
  readonly takenBy?: string;
  constructor(error: RpcError) {
    super(`${error.code}: ${error.message}`);
    this.name = "CoreCallError";
    this.code = error.code;
    if (typeof error.takenBy === "string") this.takenBy = error.takenBy;
  }
}

function isRpcError(value: unknown): value is RpcError {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as RpcError).code === "string" &&
    typeof (value as RpcError).message === "string"
  );
}

function toCoreCallError(error: unknown): Error {
  if (error instanceof CoreCallError) return error;
  if (isRpcError(error)) return new CoreCallError(error);
  if (error instanceof Error) return error;
  return new CoreCallError({ code: "UNKNOWN", message: String(error) });
}

/**
 * Sends one request's JSON bytes and resolves to the parsed `CoreResponse`,
 * or rejects with a `CoreCallError`.
 */
export type CoreTransport = (body: Uint8Array<ArrayBuffer>) => Promise<unknown>;

/** Desktop: `core_call` takes the bytes as its raw IPC body. */
export const tauriCoreTransport: CoreTransport = async (body) => {
  try {
    return await invoke<unknown>("core_call", body);
  } catch (error) {
    throw toCoreCallError(error);
  }
};

/**
 * Web: `POST /api/rpc`, same bytes, same JSON back. Errors are `RpcError`
 * JSON. Every call waits for the page's window id (Decision 22), which is
 * its origin: the first call can't go out under another id.
 */
export const httpCoreTransport: CoreTransport = async (body) => {
  const origin = await windowIdReady();
  let response: Response;
  try {
    response = await fetch("/api/rpc", {
      method: "POST",
      // The page's origin, so its own writes' events can be told apart.
      headers: { "content-type": "application/json", [ORIGIN_HEADER]: origin },
      body,
      credentials: "same-origin",
    });
  } catch (error) {
    throw new CoreCallError({
      code: "NETWORK_ERROR",
      message: error instanceof Error ? error.message : String(error),
    });
  }
  const text = await response.text();
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    const snippet = text.length > 200 ? `${text.slice(0, 200)}…` : text;
    throw new CoreCallError({
      code: response.ok ? "PROTOCOL_ERROR" : `HTTP_${response.status}`,
      message: `/api/rpc returned a body that isn't JSON (${response.status}): ${JSON.stringify(snippet)}`,
    });
  }
  if (!response.ok) {
    throw isRpcError(parsed)
      ? new CoreCallError(parsed)
      : new CoreCallError({ code: `HTTP_${response.status}`, message: response.statusText });
  }
  return parsed;
};

/**
 * The largest `keepalive` body sent. Browsers carry at most 64 KiB of
 * keepalive bodies in flight together, so this leaves room for headers and
 * any other request the page sends while it goes.
 */
export const KEEPALIVE_MAX_BYTES = 60 * 1024;

/** Picked per call, so tests and late environment detection see the current mode. */
const defaultTransport: CoreTransport = (body) =>
  isTauri() ? tauriCoreTransport(body) : httpCoreTransport(body);

/** The request as bytes. `method` is built first, so it's serialized first. */
export function encodeCoreRequest(request: CoreRequest): Uint8Array<ArrayBuffer> {
  return new TextEncoder().encode(JSON.stringify(request)) as Uint8Array<ArrayBuffer>;
}

async function send(transport: CoreTransport, request: CoreRequest): Promise<CoreResponse> {
  try {
    return (await transport(encodeCoreRequest(request))) as CoreResponse;
  } catch (error) {
    throw toCoreCallError(error);
  }
}

// -------- Typed calls --------

export type StorageMethod = StorageRequest["method"];
type RequestOf<M extends StorageMethod> = Extract<StorageRequest, { method: M }>;
/** A method's params, or `undefined` for a method that takes none. */
export type StorageParams<M extends StorageMethod> =
  RequestOf<M> extends { params: infer P } ? P : undefined;
export type StorageResult<M extends StorageMethod> = Extract<
  StorageResponse,
  { method: M }
>["result"];

/**
 * Whether each call writes. Writes are queued; reads aren't. Typed as a
 * `Record` over every method, so a new Rust method doesn't compile until
 * it's classified here.
 */
export const STORAGE_METHOD_KIND: Record<StorageMethod, "read" | "write"> = {
  licenseLoad: "read",
  licenseSave: "write",
  queryHistoryLoadByConnection: "read",
  queryHistoryAppend: "write",
  queryHistorySetFavorite: "write",
  queryHistoryRemoveByConnection: "write",
  userCredentialsLoad: "read",
  userCredentialsSave: "write",
  userCredentialsRemove: "write",
  userCredentialsRemoveAllForKey: "write",
  vaultStateLoad: "read",
  vaultStateSave: "write",
  vaultStateReset: "write",
};

function protocolError(expected: string, got: unknown): CoreCallError {
  return new CoreCallError({
    code: "PROTOCOL_ERROR",
    message: `expected a ${expected} response, got ${JSON.stringify(got)?.slice(0, 200)}`,
  });
}

/** One storage call, without the write queue. */
async function callStorage<M extends StorageMethod>(
  transport: CoreTransport,
  method: M,
  params: StorageParams<M>,
): Promise<StorageResult<M>> {
  // Leave `params` out entirely for methods that take none: `{}` is refused.
  const inner = (params === undefined ? { method } : { method, params }) as StorageRequest;
  const response = await send(transport, { method: "storage", params: inner });
  if (response?.method !== "storage" || response.result?.method !== method) {
    throw protocolError(`storage ${method}`, response);
  }
  return response.result.result as StorageResult<M>;
}

// -------- The library group (phase 5d-1) --------

export type LibraryMethod = LibraryRequest["method"];
type LibraryRequestOf<M extends LibraryMethod> = Extract<LibraryRequest, { method: M }>;
/** A library method's params, or `undefined` for one that takes none. */
export type LibraryParams<M extends LibraryMethod> =
  LibraryRequestOf<M> extends { params: infer P } ? P : undefined;
export type LibraryResult<M extends LibraryMethod> = Extract<
  LibraryResponse,
  { method: M }
>["result"];

/**
 * Whether each library call writes. Writes join the storage group's write
 * queue (Decision 3), so every write this page issues lands in order; the
 * lists don't wait. A `Record` over every method, so a new one doesn't
 * compile until it's classified.
 */
export const LIBRARY_METHOD_KIND: Record<LibraryMethod, "read" | "write"> = {
  // Phase 5d-2.
  dashboardsList: "read",
  dashboardVersionsList: "read",
  dashboardVersionGet: "read",
  dashboardCreate: "write",
  dashboardUpdate: "write",
  dashboardRemove: "write",
  workflowsList: "read",
  workflowGet: "read",
  workflowCreate: "write",
  workflowUpdate: "write",
  workflowRemove: "write",
  workflowRename: "write",
  chatsList: "read",
  chatMessagesList: "read",
  chatCreate: "write",
  chatUpdate: "write",
  chatRemove: "write",
  chatMessagesPut: "write",
  chatMessagesRemove: "write",
  projectSidebarGet: "read",
  projectSidebarSet: "write",
  // Phase 5d-1.
  connectionsList: "read",
  projectsList: "read",
  savedQueriesList: "read",
  queryVersionsList: "read",
  connectionCreate: "write",
  connectionUpdate: "write",
  connectionRemove: "write",
  projectCreate: "write",
  // Writes when the file has no project yet.
  projectEnsureDefault: "write",
  projectUpdate: "write",
  projectRemove: "write",
  labelCreate: "write",
  labelUpdate: "write",
  labelRemove: "write",
  savedQueryCreate: "write",
  savedQueryUpdate: "write",
  savedQueryRemove: "write",
};

/** One library call, without the write queue. Never echoes the request (secrets). */
async function callLibraryOnce<M extends LibraryMethod>(
  transport: CoreTransport,
  method: M,
  params: LibraryParams<M>,
): Promise<LibraryResult<M>> {
  const inner = (params === undefined ? { method } : { method, params }) as LibraryRequest;
  const response = await send(transport, { method: "library", params: inner });
  if (response?.method !== "library" || response.result?.method !== method) {
    throw new CoreCallError({
      code: "PROTOCOL_ERROR",
      message: `expected a library ${method} response`,
    });
  }
  return response.result.result as LibraryResult<M>;
}

// -------- The settings and ui groups (phase 5d-2) --------

export type SettingsMethod = SettingsRequest["method"];
type SettingsRequestOf<M extends SettingsMethod> = Extract<SettingsRequest, { method: M }>;
/** A settings method's params, or `undefined` for one that takes none. */
export type SettingsParams<M extends SettingsMethod> =
  SettingsRequestOf<M> extends { params: infer P } ? P : undefined;
export type SettingsResult<M extends SettingsMethod> = Extract<
  SettingsResponse,
  { method: M }
>["result"];

/** Whether each settings call writes (writes join the write queue). */
export const SETTINGS_METHOD_KIND: Record<SettingsMethod, "read" | "write"> = {
  settingGet: "read",
  settingSet: "write",
  aiSettingsGet: "read",
  aiSettingsPatch: "write",
  aiProviderCreate: "write",
  aiProviderUpdate: "write",
  aiProviderRemove: "write",
  themesGet: "read",
  themePreferencesSet: "write",
  userThemeCreate: "write",
  userThemeUpdate: "write",
  userThemeRemove: "write",
  onboardingGet: "read",
  onboardingPatch: "write",
  tutorialList: "read",
  tutorialSave: "write",
  tutorialRemoveLesson: "write",
  tutorialReset: "write",
  importStateGet: "read",
  importStateSave: "write",
};

export type UiMethod = UiRequest["method"];
type UiRequestOf<M extends UiMethod> = Extract<UiRequest, { method: M }>;
/** A ui method's params (every ui method takes the window id). */
export type UiParams<M extends UiMethod> =
  UiRequestOf<M> extends { params: infer P } ? P : undefined;
export type UiResult<M extends UiMethod> = Extract<UiResponse, { method: M }>["result"];

/**
 * Whether each ui call writes. `windowStateLoad` counts as a write: a first
 * load copies another window's state into this window's row.
 */
export const UI_METHOD_KIND: Record<UiMethod, "read" | "write"> = {
  windowGet: "read",
  windowActivate: "write",
  windowStateLoad: "write",
  windowStateSave: "write",
};

/** The group's request with `params` left out when there are none. */
function inner<R>(method: string, params: unknown): R {
  return (params === undefined ? { method } : { method, params }) as R;
}

/** One settings call, without the write queue. Never echoes the request (API keys). */
async function callSettingsOnce<M extends SettingsMethod>(
  transport: CoreTransport,
  method: M,
  params: SettingsParams<M>,
): Promise<SettingsResult<M>> {
  const response = await send(transport, {
    method: "settings",
    params: inner<SettingsRequest>(method, params),
  });
  if (response?.method !== "settings" || response.result?.method !== method) {
    throw new CoreCallError({
      code: "PROTOCOL_ERROR",
      message: `expected a settings ${method} response`,
    });
  }
  return response.result.result as SettingsResult<M>;
}

/** One ui call, without the write queue. */
async function callUiOnce<M extends UiMethod>(
  transport: CoreTransport,
  method: M,
  params: UiParams<M>,
): Promise<UiResult<M>> {
  const response = await send(transport, {
    method: "ui",
    params: inner<UiRequest>(method, params),
  });
  if (response?.method !== "ui" || response.result?.method !== method) {
    throw new CoreCallError({
      code: "PROTOCOL_ERROR",
      message: `expected a ui ${method} response`,
    });
  }
  return response.result.result as UiResult<M>;
}

// -------- The shared and imports groups (phase 5e) --------

export type SharedMethod = SharedRequest["method"];
type SharedRequestOf<M extends SharedMethod> = Extract<SharedRequest, { method: M }>;
/** A shared method's params, or `undefined` for one that takes none. */
export type SharedParams<M extends SharedMethod> =
  SharedRequestOf<M> extends { params: infer P } ? P : undefined;
export type SharedResult<M extends SharedMethod> = Extract<SharedResponse, { method: M }>["result"];

/**
 * Whether each shared call writes (writes join the write queue, so a sync
 * lands after the library writes this page issued before it). `scan` and
 * the list read.
 */
export const SHARED_METHOD_KIND: Record<SharedMethod, "read" | "write"> = {
  reposList: "read",
  repoRegister: "write",
  repoUpdate: "write",
  repoRemove: "write",
  linkProject: "write",
  unlinkProject: "write",
  unlinkPreview: "read",
  scan: "read",
  importProjects: "write",
  sync: "write",
  syncRepo: "write",
};

export type ImportsMethod = ImportsRequest["method"];
type ImportsRequestOf<M extends ImportsMethod> = Extract<ImportsRequest, { method: M }>;
export type ImportsParams<M extends ImportsMethod> =
  ImportsRequestOf<M> extends { params: infer P } ? P : undefined;
export type ImportsResult<M extends ImportsMethod> = Extract<
  ImportsResponse,
  { method: M }
>["result"];

/** Whether each imports call writes. */
export const IMPORTS_METHOD_KIND: Record<ImportsMethod, "read" | "write"> = {
  candidates: "read",
  create: "write",
};

/** One shared call, without the write queue. Never echoes the request (paths). */
async function callSharedOnce<M extends SharedMethod>(
  transport: CoreTransport,
  method: M,
  params: SharedParams<M>,
): Promise<SharedResult<M>> {
  const response = await send(transport, {
    method: "shared",
    params: inner<SharedRequest>(method, params),
  });
  if (response?.method !== "shared" || response.result?.method !== method) {
    throw new CoreCallError({
      code: "PROTOCOL_ERROR",
      message: `expected a shared ${method} response`,
    });
  }
  return response.result.result as SharedResult<M>;
}

/** One imports call, without the write queue. Never echoes the request (paths). */
async function callImportsOnce<M extends ImportsMethod>(
  transport: CoreTransport,
  method: M,
  params: ImportsParams<M>,
): Promise<ImportsResult<M>> {
  const response = await send(transport, {
    method: "imports",
    params: inner<ImportsRequest>(method, params),
  });
  if (response?.method !== "imports" || response.result?.method !== method) {
    throw new CoreCallError({
      code: "PROTOCOL_ERROR",
      message: `expected an imports ${method} response`,
    });
  }
  return response.result.result as ImportsResult<M>;
}

/**
 * Sends one request as a `keepalive` `POST /api/rpc` (web only), outside
 * any write queue and without waiting for its answer: for a page that is
 * going away. It carries the page's origin in `X-Seaquel-Origin` like
 * every call, which the `ui` group needs (the window id must equal it).
 * Returns false, sending nothing, on desktop, before the window id is
 * settled, or when the body is over `KEEPALIVE_MAX_BYTES`.
 */
export function sendKeepaliveRequest(request: CoreRequest, what: string): boolean {
  if (isTauri()) return false;
  const origin = windowId();
  if (origin === null) return false;
  const body = encodeCoreRequest(request);
  if (body.byteLength > KEEPALIVE_MAX_BYTES) {
    void log.warn(
      `keepalive ${what} not sent: ${body.byteLength} bytes is over the ${KEEPALIVE_MAX_BYTES}-byte cap`,
    );
    return false;
  }
  void fetch("/api/rpc", {
    method: "POST",
    headers: { "content-type": "application/json", [ORIGIN_HEADER]: origin },
    body,
    credentials: "same-origin",
    keepalive: true,
  }).catch((error: unknown) => {
    void log.error(`keepalive ${what} failed:`, error);
  });
  return true;
}

type SecretMethod = SecretRequest["method"];
export type SecretResult<M extends SecretMethod> = Extract<SecretResponse, { method: M }>["result"];

/** One secret call (desktop only; web and demo have no keychain). */
export async function callSecret<M extends SecretMethod>(
  request: Extract<SecretRequest, { method: M }>,
  transport: CoreTransport = defaultTransport,
): Promise<SecretResult<M>> {
  const response = await send(transport, { method: "secret", params: request });
  if (response?.method !== "secret" || response.result?.method !== request.method) {
    // Never echo the response: a `get` result is a secret.
    throw new CoreCallError({
      code: "PROTOCOL_ERROR",
      message: `expected a secret ${request.method} response`,
    });
  }
  const result: unknown = response.result.result;
  return result as SecretResult<M>;
}

// -------- Mapping between wire and app types --------

/** A stored connection as the app holds it (the library's lists and writes return these). */
export function connectionFromWire(wire: WireConnection): PersistedConnection {
  const { lastConnected, ...connection } = wire;
  // As the TypeScript repository did: `new Date(text)` for any non-empty text,
  // so text without a zone reads as local time and garbage is an Invalid Date.
  return lastConnected ? { ...connection, lastConnected: new Date(lastConnected) } : connection;
}

export function connectionToWire(connection: PersistedConnection): WireConnection {
  const { lastConnected, ...rest } = connection;
  // A `Date` crosses as its ISO text; anything else (a string that slipped
  // in, `null`) passes through as it did before, and `undefined` is dropped.
  const text: unknown = lastConnected instanceof Date ? lastConnected.toISOString() : lastConnected;
  return { ...rest, lastConnected: text } as WireConnection;
}

// -------- The client --------

export class RustStorageClient implements StorageClient {
  private writeQueue: Promise<void> = Promise.resolve();

  constructor(private readonly transport: CoreTransport = defaultTransport) {}

  /** Runs `fn` after every earlier write has settled; the next write waits for it. */
  private enqueueWrite<T>(fn: () => Promise<T>): Promise<T> {
    const prev = this.writeQueue;
    let release!: () => void;
    this.writeQueue = new Promise<void>((r) => {
      release = r;
    });
    const result = prev.then(fn);
    // Release the slot whether `fn` succeeds or fails.
    result.then(release, release);
    return result;
  }

  /** One `library` call (phase 5d-1); writes share the storage writes' queue. */
  library<M extends LibraryMethod>(method: M, params: LibraryParams<M>): Promise<LibraryResult<M>> {
    const run = () => callLibraryOnce(this.transport, method, params);
    return LIBRARY_METHOD_KIND[method] === "write" ? this.enqueueWrite(run) : run();
  }

  /** One `settings` call (phase 5d-2); writes share the write queue. */
  settings<M extends SettingsMethod>(
    method: M,
    params: SettingsParams<M>,
  ): Promise<SettingsResult<M>> {
    const run = () => callSettingsOnce(this.transport, method, params);
    return SETTINGS_METHOD_KIND[method] === "write" ? this.enqueueWrite(run) : run();
  }

  /**
   * One `ui` call (phase 5d-2) for this page's window. The window id must
   * equal the page's origin (`windowId()`; on web `X-Seaquel-Origin`).
   */
  ui<M extends UiMethod>(method: M, params: UiParams<M>): Promise<UiResult<M>> {
    const run = () => callUiOnce(this.transport, method, params);
    return UI_METHOD_KIND[method] === "write" ? this.enqueueWrite(run) : run();
  }

  /**
   * Sends a `windowStateSave` as a `keepalive` request (web only), for
   * `pagehide`: outside the write queue, `rev` orders it against queued
   * saves. False when nothing was sent (desktop, or over the size cap).
   */
  saveWindowStateKeepalive(params: UiParams<"windowStateSave">): boolean {
    return sendKeepaliveRequest(
      { method: "ui", params: { method: "windowStateSave", params } },
      "ui windowStateSave",
    );
  }

  /** One `shared` call (phase 5e, desktop only); writes share the write queue. */
  shared<M extends SharedMethod>(method: M, params: SharedParams<M>): Promise<SharedResult<M>> {
    const run = () => callSharedOnce(this.transport, method, params);
    return SHARED_METHOD_KIND[method] === "write" ? this.enqueueWrite(run) : run();
  }

  /** One `imports` call (phase 5e, desktop only); writes share the write queue. */
  imports<M extends ImportsMethod>(method: M, params: ImportsParams<M>): Promise<ImportsResult<M>> {
    const run = () => callImportsOnce(this.transport, method, params);
    return IMPORTS_METHOD_KIND[method] === "write" ? this.enqueueWrite(run) : run();
  }

  private call<M extends StorageMethod>(
    method: M,
    params: StorageParams<M>,
  ): Promise<StorageResult<M>> {
    const run = () => callStorage(this.transport, method, params);
    return STORAGE_METHOD_KIND[method] === "write" ? this.enqueueWrite(run) : run();
  }

  /**
   * Sends one storage call as a `keepalive` request (web only); see
   * `sendKeepaliveRequest`.
   */
  sendKeepalive<M extends StorageMethod>(method: M, params: StorageParams<M>): boolean {
    const inner = (params === undefined ? { method } : { method, params }) as StorageRequest;
    return sendKeepaliveRequest({ method: "storage", params: inner }, `storage ${method}`);
  }

  queryHistory: StorageClient["queryHistory"] = {
    loadByConnection: (connectionId) => this.call("queryHistoryLoadByConnection", { connectionId }),
    append: async (item) => {
      await this.call("queryHistoryAppend", { item });
    },
    setFavorite: async (id, favorite) => {
      await this.call("queryHistorySetFavorite", { id, favorite });
    },
    removeByConnection: async (connectionId) => {
      await this.call("queryHistoryRemoveByConnection", { connectionId });
    },
  };

  license: StorageClient["license"] = {
    load: () => this.call("licenseLoad", undefined),
    save: async (data) => {
      await this.call("licenseSave", { data });
    },
  };

  vaultState: StorageClient["vaultState"] = {
    load: () => this.call("vaultStateLoad", undefined),
    save: async (state) => {
      await this.call("vaultStateSave", { state });
    },
    reset: async () => {
      await this.call("vaultStateReset", undefined);
    },
  };

  userCredentials: StorageClient["userCredentials"] = {
    load: (scope, key) => this.call("userCredentialsLoad", { scope, key }),
    save: async (credential) => {
      await this.call("userCredentialsSave", { credential });
    },
    remove: async (scope, key) => {
      await this.call("userCredentialsRemove", { scope, key });
    },
    removeAllForKey: async (key) => {
      await this.call("userCredentialsRemoveAllForKey", { key });
    },
  };
}
