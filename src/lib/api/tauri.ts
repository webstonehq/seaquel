/**
 * Typed Tauri API layer.
 * Centralizes all Tauri invoke() calls with compile-time type safety.
 * Eliminates magic command strings scattered across the codebase.
 */

import { Channel, invoke } from "@tauri-apps/api/core";
import { encodeCoreRequest } from "$lib/storage/rust-client";
import type { CoreResponse } from "$lib/types/generated/CoreResponse";
import type { DesktopLicenseRequest } from "$lib/types/generated/DesktopLicenseRequest";
import type { LicenseResponse } from "$lib/types/generated/LicenseResponse";
import type { RpcError } from "$lib/types/generated/RpcError";

export type { LicenseResponse };

// Re-export existing well-typed service modules
export * as git from "$lib/services/git";

// === App Commands ===

export async function copyImageToClipboard(path: string): Promise<void> {
  await invoke("copy_image_to_clipboard", { path });
}

export async function openPath(path: string): Promise<void> {
  await invoke("open_path", { path });
}

export async function getDataDir(): Promise<string> {
  return invoke<string>("get_data_dir");
}

export async function readLogFile(): Promise<string> {
  return invoke<string>("read_log_file");
}

export async function clearLogFile(): Promise<void> {
  return invoke<void>("clear_log_file");
}

export interface UpdateInfo {
  version: string;
  date: string | null;
  size: number | null;
}

export async function installUpdate(): Promise<void> {
  await invoke("install_update");
}

export async function checkForUpdate(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>("check_for_update_command");
}

export async function getUsername(): Promise<string> {
  return invoke<string>("get_username");
}

// === Command line tool (MCP settings) ===

/** The downloaded `seaquel-cli` (`src-tauri/src/cli_info.rs`). */
export interface CliInfo {
  /** The stable install path in the user's app data directory. */
  binaryPath: string;
  binaryExists: boolean;
  binaryCurrent: boolean;
  /** The path an MCP host should run. */
  commandPath: string;
  /** Whether `seaquel-cli` typed in a terminal runs this app's tool. */
  pathStatus: "installed" | "outdated" | "other" | "missing";
  /** What `seaquel-cli` resolves to, when found. */
  foundPath: string | null;
  /** Whether the install button is offered on this desktop platform. */
  canInstall: boolean;
  appImage: boolean;
  /** This version's DuckDB helper, which the app and the CLI both run. */
  duckdbHelper: "installed" | "missing" | "outdated" | "unsafe" | "unknown";
}

export async function getCliInfo(): Promise<CliInfo> {
  return invoke<CliInfo>("cli_info");
}

/** The menu's "Install Command Line Tool…": shows its own dialogs, resolves when done. */
export async function installCli(): Promise<void> {
  await invoke("install_cli");
}

// === DuckDB helper (desktop only; `src-tauri/src/duckdb_helper.rs`) ===

/** What the install dialog asks with (`duckdb_helper_offer`). */
export interface DuckdbHelperOffer {
  status: "installed" | "missing" | "outdated" | "unsafe";
  /** The app's version, which the helper must match. */
  version: string;
  /** The download's compressed size in bytes; `null` when installed or unknown. */
  size: number | null;
  /** Why the size couldn't be had (`NETWORK_ERROR`, `RELEASE_NOT_FOUND`, …). */
  sizeError: RpcError | null;
  /** The release asset's file name (`seaquel-duckdb-<triple>[.exe].gz`); `null` when installed or unknown. */
  assetName: string | null;
  /** Whether "Install from a file…" can check a file (the digest is built in). */
  fromFile: boolean;
}

/** Compressed bytes received of the download's total. */
export interface DuckdbHelperProgress {
  bytes: number;
  total: number;
}

export interface DuckdbHelperInstalled {
  /** `false` when this version was already there and intact. */
  downloaded: boolean;
  /** Old version folders removed. */
  pruned: number;
}

/**
 * A refused helper command. `code` is Core's (`NETWORK_ERROR`,
 * `DIGEST_MISMATCH`, `CANCELLED`, `WRONG_FILE`, …); `message` names no path
 * or URL.
 */
export class DuckdbHelperError extends Error {
  readonly code: string;
  constructor(error: RpcError) {
    super(error.message);
    this.name = "DuckdbHelperError";
    this.code = error.code;
  }
}

async function helperCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await (args === undefined ? invoke<T>(command) : invoke<T>(command, args));
  } catch (error) {
    if (isRpcError(error)) throw new DuckdbHelperError(error);
    throw error;
  }
}

/** This version's helper's status and download size. */
export async function duckdbHelperOffer(): Promise<DuckdbHelperOffer> {
  return helperCommand<DuckdbHelperOffer>("duckdb_helper_offer");
}

/**
 * Downloads and installs the helper; `onProgress` gets the download's
 * progress. An install already running (the prefetch) is joined, not
 * repeated. Rejects with `CANCELLED` after {@link cancelDuckdbHelperInstall}.
 */
export async function installDuckdbHelper(
  onProgress?: (progress: DuckdbHelperProgress) => void,
): Promise<DuckdbHelperInstalled> {
  const channel = new Channel<DuckdbHelperProgress>();
  if (onProgress) channel.onmessage = onProgress;
  return helperCommand<DuckdbHelperInstalled>("duckdb_helper_install", { channel });
}

/**
 * Stops the running install (download or file). The download is shared, so
 * this also cancels a prefetch's or any other install that joined it; each
 * of them rejects with `CANCELLED`. Resolves whether an install was
 * registered when it was called (one finishing at that moment may still
 * resolve with its result).
 */
export async function cancelDuckdbHelperInstall(): Promise<boolean> {
  return helperCommand<boolean>("duckdb_helper_cancel");
}

/** "Install from a file…": the release's `.gz`, checked against the built-in digest. */
export async function installDuckdbHelperFromFile(path: string): Promise<DuckdbHelperInstalled> {
  return helperCommand<DuckdbHelperInstalled>("duckdb_helper_install_file", { path });
}

// === License Commands ===

/**
 * A failed license call. `message` is the license server's wording, shown to
 * the user as is; `code` is `NETWORK_ERROR`, `ACTIVATION_ERROR`,
 * `VALIDATION_ERROR`, `DEACTIVATION_ERROR` or `PARSE_ERROR` (or an RPC code).
 */
export class LicenseError extends Error {
  readonly code: string;
  constructor(error: RpcError) {
    super(error.message);
    this.name = "LicenseError";
    this.code = error.code;
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

/** One `license` call through `core_call`. The key never reaches a log. */
async function callLicense(request: DesktopLicenseRequest): Promise<LicenseResponse> {
  let response: CoreResponse;
  try {
    response = await invoke<CoreResponse>(
      "core_call",
      encodeCoreRequest({ method: "license", params: request }),
    );
  } catch (error) {
    if (isRpcError(error)) throw new LicenseError(error);
    throw error;
  }
  if (response?.method !== "license" || response.result?.method !== request.method) {
    throw new LicenseError({
      code: "PROTOCOL_ERROR",
      message: `expected a license ${request.method} response`,
    });
  }
  return response.result.result;
}

export async function activateLicense(key: string, instanceName: string): Promise<LicenseResponse> {
  return callLicense({ method: "activate", params: { key, instanceName } });
}

export async function validateLicense(key: string, instanceId: string): Promise<LicenseResponse> {
  return callLicense({ method: "validate", params: { key, instanceId } });
}

export async function deactivateLicense(key: string, instanceId: string): Promise<LicenseResponse> {
  return callLicense({ method: "deactivate", params: { key, instanceId } });
}
