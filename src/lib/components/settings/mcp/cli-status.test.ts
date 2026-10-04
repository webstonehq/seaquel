import { describe, expect, it } from "vitest";
import type { CliInfo } from "$lib/api/tauri";
import { helperNeeded, offersInstall } from "./cli-status";

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
