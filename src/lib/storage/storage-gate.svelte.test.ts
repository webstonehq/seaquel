import { describe, it, expect, vi, beforeEach } from "vitest";
import { CoreCallError } from "./rust-client";

let probe: () => Promise<string | null> = async () => null;
const appStateGet = vi.fn((_key: string) => probe());

vi.mock("$lib/storage", () => ({ getStorage: () => ({ appState: { get: appStateGet } }) }));
vi.mock("$lib/utils/logger", () => ({
  log: { info: vi.fn(), warn: vi.fn(), error: vi.fn(), debug: vi.fn(), trace: vi.fn() },
}));

const { classifyStorageError, StorageGate } = await import("./storage-gate.svelte");

const LEGACY =
  "/Users/me/Library/Application Support/app.seaquel.desktop holds data from a Seaquel release " +
  "older than 2026.4.5 (database_connections.json, projects.json), which this release can't " +
  "read. Install a release from 2026.4.5 through 2026.9.x, launch it once so it imports that " +
  "data, then upgrade to this release.";
const CORRUPT_PATH = "/Users/me/Library/Application Support/app.seaquel.desktop/seaquel.db";
const CORRUPT_UNTOUCHED = `${CORRUPT_PATH} isn't a readable Seaquel database (file is not a database). The file wasn't changed.`;
const CORRUPT_CHANGED = `${CORRUPT_PATH} isn't a readable Seaquel database (database disk image is malformed).`;

const fail = (code: string, message: string) => new CoreCallError({ code, message });

describe("classifyStorageError", () => {
  it("shows the legacy screen for LEGACY_STORAGE, with the server's text as the detail", () => {
    expect(classifyStorageError(fail("LEGACY_STORAGE", LEGACY))).toEqual({
      kind: "legacy",
      detail: LEGACY,
    });
  });

  it("shows the corrupt screen with the path and 'wasn't changed' only when the server says so", () => {
    expect(classifyStorageError(fail("STORAGE_CORRUPT", CORRUPT_UNTOUCHED))).toEqual({
      kind: "corrupt",
      detail: CORRUPT_UNTOUCHED,
      path: CORRUPT_PATH,
      untouched: true,
    });
    expect(classifyStorageError(fail("STORAGE_CORRUPT", CORRUPT_CHANGED))).toEqual({
      kind: "corrupt",
      detail: CORRUPT_CHANGED,
      path: CORRUPT_PATH,
      untouched: false,
    });
  });

  it("keeps the web's DATA_DIR path as the server sent it", () => {
    const message =
      "DATA_DIR/users/u1/meta.db isn't a readable Seaquel database (file is not a database). The file wasn't changed.";
    expect(classifyStorageError(fail("STORAGE_CORRUPT", message))).toMatchObject({
      kind: "corrupt",
      path: "DATA_DIR/users/u1/meta.db",
      untouched: true,
    });
  });

  it("gives no path when the message has an unexpected shape, and claims nothing", () => {
    expect(classifyStorageError(fail("STORAGE_CORRUPT", "something else went wrong"))).toEqual({
      kind: "corrupt",
      detail: "something else went wrong",
      path: null,
      untouched: false,
    });
  });

  it("shows the no-data-dir screen for NO_DATA_DIR", () => {
    const message = "no data directory: this platform reports none, and SEAQUEL_DATA_DIR isn't set";
    expect(classifyStorageError(fail("NO_DATA_DIR", message))).toEqual({
      kind: "no-data-dir",
      detail: message,
    });
  });

  it("accepts a bare RpcError object", () => {
    expect(classifyStorageError({ code: "LEGACY_STORAGE", message: LEGACY })).toEqual({
      kind: "legacy",
      detail: LEGACY,
    });
  });

  it("doesn't block on any other error", () => {
    expect(classifyStorageError(fail("STORAGE_ERROR", "disk I/O error"))).toBeNull();
    expect(classifyStorageError(fail("UPSTREAM_UNAVAILABLE", "502"))).toBeNull();
    expect(classifyStorageError(fail("INVALID_ARGUMENT", "bad"))).toBeNull();
    expect(classifyStorageError(new Error("network down"))).toBeNull();
    expect(classifyStorageError("LEGACY_STORAGE")).toBeNull();
    expect(classifyStorageError(null)).toBeNull();
  });
});

const slept: number[] = [];
const newGate = () =>
  new StorageGate({
    sleep: async (ms) => {
      slept.push(ms);
    },
  });

describe("StorageGate", () => {
  beforeEach(() => {
    slept.length = 0;
    appStateGet.mockClear();
    probe = async () => null;
  });

  it("lets init go ahead when storage answers", async () => {
    const gate = newGate();
    expect(await gate.check()).toBe(true);
    expect(gate.blocked).toBeNull();
  });

  it("blocks on LEGACY_STORAGE", async () => {
    probe = async () => {
      throw fail("LEGACY_STORAGE", LEGACY);
    };
    const gate = newGate();
    expect(await gate.check()).toBe(false);
    expect(gate.blocked).toEqual({ kind: "legacy", detail: LEGACY });
  });

  it("blocks on STORAGE_CORRUPT", async () => {
    probe = async () => {
      throw fail("STORAGE_CORRUPT", CORRUPT_UNTOUCHED);
    };
    const gate = newGate();
    expect(await gate.check()).toBe(false);
    expect(gate.blocked).toMatchObject({ kind: "corrupt", untouched: true });
  });

  it("retries STORAGE_ERROR, UPSTREAM_UNAVAILABLE and network errors twice, then starts", async () => {
    for (const error of [
      fail("STORAGE_ERROR", "disk I/O error"),
      fail("UPSTREAM_UNAVAILABLE", "bad gateway"),
      new TypeError("Failed to fetch"),
    ]) {
      appStateGet.mockClear();
      slept.length = 0;
      probe = async () => {
        throw error;
      };
      const gate = newGate();
      expect(await gate.check()).toBe(true);
      expect(gate.blocked).toBeNull();
      expect(appStateGet).toHaveBeenCalledTimes(3);
      expect(slept).toEqual([250, 1000]);
    }
  });

  it("starts as soon as a retry succeeds", async () => {
    let calls = 0;
    probe = async () => {
      calls += 1;
      if (calls === 1) throw fail("UPSTREAM_UNAVAILABLE", "bad gateway");
      return null;
    };
    const gate = newGate();
    expect(await gate.check()).toBe(true);
    expect(calls).toBe(2);
    expect(slept).toEqual([250]);
  });

  it("blocks when a retry finds legacy or corrupt storage", async () => {
    let calls = 0;
    probe = async () => {
      calls += 1;
      throw calls === 1 ? fail("STORAGE_ERROR", "busy") : fail("LEGACY_STORAGE", LEGACY);
    };
    const gate = newGate();
    expect(await gate.check()).toBe(false);
    expect(gate.blocked).toEqual({ kind: "legacy", detail: LEGACY });
  });

  it("blocks on NO_DATA_DIR without retrying", async () => {
    probe = async () => {
      throw fail("NO_DATA_DIR", "no data directory");
    };
    const gate = newGate();
    expect(await gate.check()).toBe(false);
    expect(gate.blocked).toEqual({ kind: "no-data-dir", detail: "no data directory" });
    expect(slept).toEqual([]);
  });

  it("makes the probe call once for every caller", async () => {
    const gate = newGate();
    const results = await Promise.all([gate.check(), gate.check(), gate.check()]);
    expect(results).toEqual([true, true, true]);
    expect(appStateGet).toHaveBeenCalledTimes(1);
  });
});
