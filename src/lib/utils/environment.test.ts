/**
 * Which build this is: the demo is the demo build only,
 * never a desktop or dev build that happens to run in a plain browser.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { isDemo, isSupportedBuild, isWeb } from "./environment";

afterEach(() => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
});

describe("the build", () => {
  it("is the demo only in the demo build", () => {
    vi.stubEnv("VITE_BUILD_TARGET", "demo");
    expect(isDemo()).toBe(true);
    expect(isSupportedBuild()).toBe(true);
  });

  it("is the demo when VITE_IS_DEMO says so", () => {
    vi.stubEnv("VITE_BUILD_TARGET", "desktop");
    vi.stubEnv("VITE_IS_DEMO", "true");
    expect(isDemo()).toBe(true);
  });

  it("a desktop build in a plain browser is not the demo, and not supported", () => {
    vi.stubEnv("VITE_BUILD_TARGET", "desktop");
    vi.stubEnv("VITE_IS_DEMO", "false");
    vi.stubGlobal("window", { location: { protocol: "http:" } });
    expect(isDemo()).toBe(false);
    expect(isWeb()).toBe(false);
    expect(isSupportedBuild()).toBe(false);
  });

  it("the web build and the desktop app are supported", () => {
    vi.stubEnv("VITE_BUILD_TARGET", "web");
    expect(isSupportedBuild()).toBe(true);
    vi.stubEnv("VITE_BUILD_TARGET", "desktop");
    vi.stubGlobal("window", { __TAURI_INTERNALS__: {}, location: { protocol: "tauri:" } });
    expect(isSupportedBuild()).toBe(true);
  });
});
