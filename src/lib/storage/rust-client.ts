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
 *   they were issued (`PersistenceManager`'s debounced saves rely on it).
 *   Reads skip the queue.
 * - **Mapping.** The generated `Persisted*` wire types stay in this file:
 *   callers see the app's types. `lastConnected` crosses as text and becomes
 *   a `Date`; saved workflows cross in their `toStorable` form.
 */

import { invoke } from "@tauri-apps/api/core";
import { ORIGIN_HEADER, webPageOrigin } from "$lib/core/origin";
import type { SavedWorkflow } from "$lib/types/workflow";
import type { PersistedProjectState } from "$lib/types";
import type { PersistedConnection } from "$lib/hooks/database/types";
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import type { CoreResponse } from "$lib/types/generated/CoreResponse";
import type { LibraryRequest } from "$lib/types/generated/LibraryRequest";
import type { LibraryResponse } from "$lib/types/generated/LibraryResponse";
import type { PersistedConnection as WireConnection } from "$lib/types/generated/PersistedConnection";
import type { PersistedProjectState as WireProjectState } from "$lib/types/generated/PersistedProjectState";
import type { RpcError } from "$lib/types/generated/RpcError";
import type { SecretRequest } from "$lib/types/generated/SecretRequest";
import type { SecretResponse } from "$lib/types/generated/SecretResponse";
import type { StorageRequest } from "$lib/types/generated/StorageRequest";
import type { StorageResponse } from "$lib/types/generated/StorageResponse";
import { isTauri } from "$lib/utils/environment";
import { log } from "$lib/utils/logger";
import { planDashboardVersionsPrune } from "$lib/utils/version-prune";
import { fromStorable, toStorable } from "$lib/values";
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

/** Web: `POST /api/rpc`, same bytes, same JSON back. Errors are `RpcError` JSON. */
export const httpCoreTransport: CoreTransport = async (body) => {
  let response: Response;
  try {
    response = await fetch("/api/rpc", {
      method: "POST",
      // The page's origin, so its own writes' events can be told apart.
      headers: { "content-type": "application/json", [ORIGIN_HEADER]: webPageOrigin() },
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
  aiChatsLoadByConnection: "read",
  aiChatsSaveChat: "write",
  aiChatsRemoveChat: "write",
  aiChatsRemoveByConnection: "write",
  aiChatsLoadMessages: "read",
  aiChatsReplaceAllMessages: "write",
  appStateGet: "read",
  appStateSet: "write",
  connectionOverridesLoad: "read",
  connectionOverridesLoadAll: "read",
  connectionOverridesSave: "write",
  connectionOverridesRemove: "write",
  dashboardVersionsLoadByDashboard: "read",
  dashboardVersionsLoadByProject: "read",
  dashboardVersionsInsert: "write",
  dashboardVersionsPrune: "write",
  dashboardsLoadByProject: "read",
  dashboardsSave: "write",
  dashboardsRemove: "write",
  dashboardsRemoveByProject: "write",
  importStateLoad: "read",
  importStateSave: "write",
  licenseLoad: "read",
  licenseSave: "write",
  onboardingLoad: "read",
  onboardingSave: "write",
  projectStateLoad: "read",
  projectStateSave: "write",
  projectStateRemove: "write",
  queryHistoryLoadByConnection: "read",
  queryHistoryAppend: "write",
  queryHistorySetFavorite: "write",
  queryHistoryRemoveByConnection: "write",
  sharedReposLoadAll: "read",
  sharedReposSaveAll: "write",
  themesLoadPreferences: "read",
  themesSavePreferences: "write",
  themesLoadUserThemes: "read",
  themesSaveUserThemes: "write",
  tutorialLoadAll: "read",
  tutorialSave: "write",
  tutorialRemoveLesson: "write",
  tutorialRemoveAll: "write",
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

function projectStateFromWire(wire: WireProjectState): PersistedProjectState {
  const { savedWorkflows, ...rest } = wire;
  // Result and chart nodes keep their rows, which can hold bigint, bytes and
  // decimals, tagged by `toStorable`. A workflow that won't decode is dropped,
  // one at a time, as the TypeScript repository did.
  const workflows: SavedWorkflow[] = [];
  for (const stored of savedWorkflows ?? []) {
    try {
      const workflow = fromStorable(stored) as SavedWorkflow | null;
      if (workflow !== null) workflows.push(workflow);
    } catch (error) {
      void log.warn("Dropping a saved workflow that doesn't decode:", error);
    }
  }
  // `activeView` is `ActiveViewType` in Rust's hands too; a stored `canvas`
  // (from before the workflow rename) passes through, as it always did.
  return {
    ...rest,
    activeView: rest.activeView as PersistedProjectState["activeView"],
    savedWorkflows: workflows,
  };
}

function projectStateToWire(state: PersistedProjectState): WireProjectState {
  const { savedWorkflows, ...rest } = state;
  // A workflow that can't be encoded fails the whole save, as before, so the
  // stored copy survives instead of being deleted and not replaced.
  return savedWorkflows === undefined
    ? rest
    : { ...rest, savedWorkflows: savedWorkflows.map((w) => toStorable(w)) };
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

  private call<M extends StorageMethod>(
    method: M,
    params: StorageParams<M>,
  ): Promise<StorageResult<M>> {
    const run = () => callStorage(this.transport, method, params);
    return STORAGE_METHOD_KIND[method] === "write" ? this.enqueueWrite(run) : run();
  }

  appState: StorageClient["appState"] = {
    get: (key) => this.call("appStateGet", { key }),
    set: async (key, value) => {
      await this.call("appStateSet", { key, value });
    },
  };

  connectionOverrides: StorageClient["connectionOverrides"] = {
    load: (sharedConnectionId) => this.call("connectionOverridesLoad", { sharedConnectionId }),
    loadAll: () => this.call("connectionOverridesLoadAll", undefined),
    save: async (connectionOverride) => {
      await this.call("connectionOverridesSave", { connectionOverride });
    },
    remove: async (sharedConnectionId) => {
      await this.call("connectionOverridesRemove", { sharedConnectionId });
    },
  };

  projectState: StorageClient["projectState"] = {
    load: async (projectId) => {
      const state = await this.call("projectStateLoad", { projectId });
      return state === null ? null : projectStateFromWire(state);
    },
    save: async (state) => {
      const wire = projectStateToWire(state);
      await this.call("projectStateSave", { state: wire });
    },
    remove: async (projectId) => {
      await this.call("projectStateRemove", { projectId });
    },
  };

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

  sharedRepos: StorageClient["sharedRepos"] = {
    loadAll: () => this.call("sharedReposLoadAll", undefined),
    saveAll: async (repos, activeRepoId) => {
      await this.call("sharedReposSaveAll", { repos, activeRepoId });
    },
  };

  themes: StorageClient["themes"] = {
    loadPreferences: () => this.call("themesLoadPreferences", undefined),
    savePreferences: async (lightThemeId, darkThemeId) => {
      await this.call("themesSavePreferences", { lightThemeId, darkThemeId });
    },
    loadUserThemes: () => this.call("themesLoadUserThemes", undefined),
    saveUserThemes: async (themes) => {
      await this.call("themesSaveUserThemes", { themes });
    },
  };

  license: StorageClient["license"] = {
    load: () => this.call("licenseLoad", undefined),
    save: async (data) => {
      await this.call("licenseSave", { data });
    },
  };

  onboarding: StorageClient["onboarding"] = {
    load: () => this.call("onboardingLoad", undefined),
    save: async (data) => {
      await this.call("onboardingSave", { data });
    },
  };

  tutorial: StorageClient["tutorial"] = {
    loadAll: () => this.call("tutorialLoadAll", undefined),
    save: async (lessonId, challengeId, state) => {
      await this.call("tutorialSave", { lessonId, challengeId, state });
    },
    removeLesson: async (lessonId) => {
      await this.call("tutorialRemoveLesson", { lessonId });
    },
    removeAll: async () => {
      await this.call("tutorialRemoveAll", undefined);
    },
  };

  importState: StorageClient["importState"] = {
    load: (source) => this.call("importStateLoad", { source }),
    save: async (source, hasOfferedImport, lastCheckTimestamp) => {
      await this.call("importStateSave", { source, hasOfferedImport, lastCheckTimestamp });
    },
  };

  dashboards: StorageClient["dashboards"] = {
    loadByProject: (projectId) => this.call("dashboardsLoadByProject", { projectId }),
    save: async (dashboard) => {
      await this.call("dashboardsSave", { dashboard });
    },
    remove: async (id) => {
      await this.call("dashboardsRemove", { id });
    },
    removeByProject: async (projectId) => {
      await this.call("dashboardsRemoveByProject", { projectId });
    },
  };

  dashboardVersions: StorageClient["dashboardVersions"] = {
    loadByDashboard: (dashboardId) =>
      this.call("dashboardVersionsLoadByDashboard", { dashboardId }),
    loadByProject: (projectId) => this.call("dashboardVersionsLoadByProject", { projectId }),
    insert: async (version) => {
      await this.call("dashboardVersionsInsert", { version });
    },
    pruneOldVersions: (dashboardId, keepCount) =>
      this.enqueueWrite(async () => {
        const versions = await callStorage(this.transport, "dashboardVersionsLoadByDashboard", {
          dashboardId,
        });
        const plan = planDashboardVersionsPrune(dashboardId, versions, keepCount);
        if (plan) await callStorage(this.transport, "dashboardVersionsPrune", plan);
      }),
  };

  aiChats: StorageClient["aiChats"] = {
    loadByConnection: (connectionId) => this.call("aiChatsLoadByConnection", { connectionId }),
    saveChat: async (chat) => {
      await this.call("aiChatsSaveChat", { chat });
    },
    removeChat: async (chatId) => {
      await this.call("aiChatsRemoveChat", { chatId });
    },
    removeByConnection: async (connectionId) => {
      await this.call("aiChatsRemoveByConnection", { connectionId });
    },
    loadMessages: (chatId) => this.call("aiChatsLoadMessages", { chatId }),
    replaceAllMessages: async (chatId, messages) => {
      await this.call("aiChatsReplaceAllMessages", { chatId, messages });
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
