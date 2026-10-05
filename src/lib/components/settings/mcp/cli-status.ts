import type { CliInfo } from "$lib/api/tauri";
import { m } from "$lib/paraglide/messages.js";

/**
 * Whether the panel warns that the DuckDB helper needs installing:
 * missing, outdated or unsafe (an install makes its folder private again),
 * and only once the CLI itself is there.
 */
export function helperNeeded(info: CliInfo | null): boolean {
  return (
    info !== null &&
    info.binaryExists &&
    (info.duckdbHelper === "missing" ||
      info.duckdbHelper === "outdated" ||
      info.duckdbHelper === "unsafe")
  );
}

/**
 * The helper warning: the helper is the app's own DuckDB
 * support as well as the CLI's, so it is named "DuckDB support", not the
 * command line tool's.
 */
export function helperWarning(info: CliInfo | null): string | null {
  if (!helperNeeded(info)) return null;
  return info?.duckdbHelper === "unsafe"
    ? m.settings_mcp_duckdb_helper_unsafe()
    : m.settings_mcp_duckdb_helper_missing();
}

/** Whether the CLI itself needs installing: on Windows when it isn't this version's, elsewhere when `seaquel-cli` on PATH isn't this version's install. */
function cliNeeded(info: CliInfo, os: string): boolean {
  return os === "windows" ? !info.binaryCurrent : info.pathStatus !== "installed";
}

/**
 * What the panel's install button installs: the CLI (which installs the
 * helper after it), only the helper when the CLI is current, or nothing.
 */
export function installTarget(info: CliInfo, os: string): "cli" | "helper" | null {
  if (!info.canInstall) return null;
  if (cliNeeded(info, os)) return "cli";
  return helperNeeded(info) ? "helper" : null;
}

/** Whether the panel offers its install button, for the CLI or the helper. */
export function offersInstall(info: CliInfo, os: string): boolean {
  return installTarget(info, os) !== null;
}
