import { describe, expect, it } from "vitest";
import type { CliInfo } from "$lib/api/tauri";
import { m } from "$lib/paraglide/messages.js";
import { helperNeeded, helperWarning, installTarget, offersInstall } from "./cli-status";

function info(overrides: Partial<CliInfo> = {}): CliInfo {
  return {
    binaryPath: "/data/bin/seaquel-cli",
    binaryExists: true,
    binaryCurrent: true,
    commandPath: "/data/bin/seaquel-cli",
    pathStatus: "installed",
    foundPath: "/usr/local/bin/seaquel-cli",
    canInstall: true,
    appImage: false,
    duckdbHelper: "installed",
    ...overrides,
  };
}

describe("helperNeeded", () => {
  it("is said for a missing, outdated or unsafe helper once the CLI is there", () => {
    for (const duckdbHelper of ["missing", "outdated", "unsafe"] as const) {
      expect(helperNeeded(info({ duckdbHelper }))).toBe(true);
    }
  });

  it("isn't said for an installed or unknown helper, or before the CLI exists", () => {
    expect(helperNeeded(info())).toBe(false);
    expect(helperNeeded(info({ duckdbHelper: "unknown" }))).toBe(false);
    expect(helperNeeded(info({ binaryExists: false, duckdbHelper: "missing" }))).toBe(false);
    expect(helperNeeded(null)).toBe(false);
  });
});

describe("offersInstall", () => {
  it("offers the button for the helper even when the CLI is current", () => {
    expect(offersInstall(info(), "macos")).toBe(false);
    expect(offersInstall(info({ duckdbHelper: "missing" }), "macos")).toBe(true);
    expect(offersInstall(info({ duckdbHelper: "unsafe" }), "windows")).toBe(true);
  });

  it("keeps the CLI's own rules per platform", () => {
    expect(offersInstall(info({ pathStatus: "missing" }), "linux")).toBe(true);
    expect(offersInstall(info({ pathStatus: "missing" }), "windows")).toBe(false);
    expect(offersInstall(info({ binaryCurrent: false }), "windows")).toBe(true);
    expect(offersInstall(info({ canInstall: false, duckdbHelper: "missing" }), "macos")).toBe(
      false,
    );
  });
});

describe("installTarget", () => {
  it("installs only the helper when the CLI itself is current", () => {
    expect(installTarget(info({ duckdbHelper: "missing" }), "macos")).toBe("helper");
    expect(installTarget(info({ duckdbHelper: "outdated" }), "linux")).toBe("helper");
    expect(installTarget(info({ duckdbHelper: "unsafe" }), "windows")).toBe("helper");
  });

  it("installs the CLI (and the helper after it) when the CLI needs it", () => {
    expect(installTarget(info({ pathStatus: "outdated", duckdbHelper: "missing" }), "macos")).toBe(
      "cli",
    );
    expect(installTarget(info({ binaryCurrent: false, duckdbHelper: "missing" }), "windows")).toBe(
      "cli",
    );
    expect(installTarget(info({ pathStatus: "missing" }), "linux")).toBe("cli");
  });

  it("offers nothing when both are fine or the platform can't install", () => {
    expect(installTarget(info(), "macos")).toBeNull();
    expect(installTarget(info({ pathStatus: "missing" }), "windows")).toBeNull();
    expect(installTarget(info({ canInstall: false, duckdbHelper: "missing" }), "macos")).toBeNull();
  });
});

describe("helperWarning", () => {
  it("names DuckDB support, not the command line tool's", () => {
    const missing = helperWarning(info({ duckdbHelper: "missing" }));
    const unsafe = helperWarning(info({ duckdbHelper: "unsafe" }));
    expect(missing).toBe(m.settings_mcp_duckdb_helper_missing());
    expect(unsafe).toBe(m.settings_mcp_duckdb_helper_unsafe());
    for (const text of [missing, unsafe]) {
      expect(text).toMatch(/DuckDB support/);
      expect(text).not.toMatch(/command line tool's DuckDB|for the command line tool/);
    }
    expect(helperWarning(info({ duckdbHelper: "outdated" }))).toBe(missing);
  });

  it("says nothing when no install is needed", () => {
    expect(helperWarning(info())).toBeNull();
    expect(helperWarning(info({ binaryExists: false, duckdbHelper: "missing" }))).toBeNull();
    expect(helperWarning(null)).toBeNull();
  });
});
