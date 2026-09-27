/**
 * Typed Tauri API layer.
 * Centralizes all Tauri invoke() calls with compile-time type safety.
 * Eliminates magic command strings scattered across the codebase.
 */

import { invoke } from "@tauri-apps/api/core";
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

export async function readDbeaverConfig(): Promise<string | null> {
  return invoke<string | null>("read_dbeaver_config");
}

export async function readTablePlusConfig(): Promise<string | null> {
  return invoke<string | null>("read_tableplus_config");
}

export async function getUsername(): Promise<string> {
  return invoke<string>("get_username");
}

// === Command line tool (MCP settings) ===

/** The bundled `seaquel-cli` (`src-tauri/src/cli_info.rs`). */
export interface CliInfo {
  /** The sidecar next to the app's executable. */
  sidecarPath: string;
  sidecarExists: boolean;
  /** The path an MCP host should run (an AppImage's stable copy, else the sidecar). */
  commandPath: string;
  /** Whether `seaquel-cli` typed in a terminal runs this app's tool. */
  pathStatus: "installed" | "outdated" | "other" | "missing";
  /** What `seaquel-cli` resolves to, when found. */
  foundPath: string | null;
  /** Whether the install button is offered (macOS, Linux AppImage). */
  canInstall: boolean;
  appImage: boolean;
}

export async function getCliInfo(): Promise<CliInfo> {
  return invoke<CliInfo>("cli_info");
}

/** The menu's "Install Command Line Tool…": shows its own dialogs, resolves when done. */
export async function installCli(): Promise<void> {
  await invoke("install_cli");
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
