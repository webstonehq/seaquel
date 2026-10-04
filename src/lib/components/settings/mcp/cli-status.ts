import type { CliInfo } from "$lib/api/tauri";

/**
 * Whether the panel warns that the CLI's DuckDB helper needs installing:
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
 * Whether the panel offers its install button: for the helper, or for the
 * CLI itself (on Windows when it isn't this version's, elsewhere when the
 * `seaquel-cli` on PATH isn't this version's install).
 */
export function offersInstall(info: CliInfo, os: string): boolean {
  if (!info.canInstall) return false;
  if (helperNeeded(info)) return true;
  return os === "windows" ? !info.binaryCurrent : info.pathStatus !== "installed";
}
