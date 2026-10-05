/**
 * `withDuckdbHelper` (desktop DuckDB helper plan, Task 5, Decision 10):
 * `ENGINE_NOT_INSTALLED` from an interactive connect opens the install
 * dialog and, once it installed, makes the same attempt again; a
 * background one never asks; a decline rejects with
 * `DuckdbHelperDeclined`; a retry that is still refused says DuckDB
 * support can't be used and offers no second download; the other engine
 * codes are worded. The real store runs over a fake install service.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { FakeDuckdbInstall as Fake } from "$lib/stores/duckdb-install-testing";

const holder = vi.hoisted(() => ({ fake: null as unknown as Fake }));
const env = vi.hoisted(() => ({ tauri: true }));

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => env.tauri,
  isWeb: () => false,
  isDemo: () => false,
}));

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/stores/duckdb-install.svelte", async (importOriginal) => {
  const real = await importOriginal<typeof import("$lib/stores/duckdb-install.svelte")>();
  const { FakeDuckdbInstall } = await import("$lib/stores/duckdb-install-testing");
  holder.fake = new FakeDuckdbInstall();
  const store = new real.DuckdbInstallStore(holder.fake);
  return { ...real, duckdbInstallStore: store };
});

const {
  withDuckdbHelper,
  DuckdbHelperDeclined,
  DuckdbHelperUnusable,
  setInstallStoreLoader,
  prefetchDuckdbHelper,
  PREFETCH_DELAY_MS,
  installDuckdbHelperJoined,
} = await import("./duckdb-helper");
const { duckdbInstallStore } = await import("$lib/stores/duckdb-install.svelte");
const { CoreCallError } = await import("$lib/storage/rust-client");
const { ShownError, extractErrorMessage } = await import("$lib/errors");
const { errorCode } = await import("./client");
const { m } = await import("$lib/paraglide/messages.js");

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

const notInstalled = (message = "DuckDB support for Seaquel 2026.10.1 isn't installed: missing") =>
  new CoreCallError({ code: "ENGINE_NOT_INSTALLED", message });

/** Waits until the dialog asks (a closed dialog keeps its last step on screen). */
async function asked() {
  await vi.waitFor(() => {
    expect(duckdbInstallStore.open).toBe(true);
    expect(duckdbInstallStore.stage.step).toBe("ask");
  });
}

/** Accept the dialog's question and let the fake install finish. */
async function acceptAndInstall() {
  await asked();
  duckdbInstallStore.download();
  await holder.fake.started();
  holder.fake.finish();
}

beforeEach(() => {
  if (duckdbInstallStore.open) duckdbInstallStore.dismiss();
  holder.fake.calls.length = 0;
});

describe("withDuckdbHelper", () => {
  it("passes a connect that works straight through", async () => {
    const attempt = vi.fn(async () => "pc-1");
    await expect(withDuckdbHelper(attempt, { interactive: true })).resolves.toBe("pc-1");
    expect(attempt).toHaveBeenCalledOnce();
    expect(holder.fake.calls).toEqual([]);
  });

  it("asks, installs, then makes the same attempt again", async () => {
    const attempt = vi
      .fn<() => Promise<string>>()
      .mockRejectedValueOnce(notInstalled())
      .mockResolvedValueOnce("pc-1");
    const connected = withDuckdbHelper(attempt, { interactive: true });
    await acceptAndInstall();
    await expect(connected).resolves.toBe("pc-1");
    expect(attempt).toHaveBeenCalledTimes(2);
    expect(holder.fake.calls).toEqual(["offer", "install"]);
  });

  it("a background connect never asks, and says what Core said without its code", async () => {
    const attempt = vi.fn().mockRejectedValue(notInstalled());
    const failed = withDuckdbHelper(attempt, { interactive: false });
    const error = await failed.catch((e: unknown) => e);
    expect(extractErrorMessage(error)).toBe(
      "DuckDB support for Seaquel 2026.10.1 isn't installed: missing",
    );
    expect(errorCode(error)).toBe("ENGINE_NOT_INSTALLED");
    expect(duckdbInstallStore.open).toBe(false);
    expect(holder.fake.calls).toEqual([]);
    expect(attempt).toHaveBeenCalledOnce();
  });

  it("a decline rejects with DuckdbHelperDeclined, which no one toasts again", async () => {
    const attempt = vi.fn().mockRejectedValue(notInstalled());
    const connected = withDuckdbHelper(attempt, { interactive: true });
    await asked();
    duckdbInstallStore.dismiss();
    const error = await connected.catch((e: unknown) => e);
    expect(error).toBeInstanceOf(DuckdbHelperDeclined);
    expect(error).toBeInstanceOf(ShownError);
    expect(extractErrorMessage(error)).toBe(m.duckdb_helper_declined());
    expect(extractErrorMessage(error)).toBe("DuckDB support isn't installed.");
    expect(attempt).toHaveBeenCalledOnce();
  });

  it("a cancelled download rejects the same way", async () => {
    const attempt = vi.fn().mockRejectedValue(notInstalled());
    const connected = withDuckdbHelper(attempt, { interactive: true });
    await asked();
    duckdbInstallStore.download();
    await holder.fake.started();
    duckdbInstallStore.dismiss();
    await expect(connected).rejects.toBeInstanceOf(DuckdbHelperDeclined);
    expect(holder.fake.calls).toEqual(["offer", "install", "cancel"]);
  });

  it("two connects at once: one dialog, one install, two retries", async () => {
    const first = vi
      .fn<() => Promise<string>>()
      .mockRejectedValueOnce(notInstalled())
      .mockResolvedValueOnce("pc-a");
    const second = vi
      .fn<() => Promise<string>>()
      .mockRejectedValueOnce(notInstalled())
      .mockResolvedValueOnce("pc-b");
    const a = withDuckdbHelper(first, { interactive: true });
    const b = withDuckdbHelper(second, { interactive: true });
    await acceptAndInstall();
    await expect(a).resolves.toBe("pc-a");
    await expect(b).resolves.toBe("pc-b");
    expect(holder.fake.calls).toEqual(["offer", "install"]);
    expect(first).toHaveBeenCalledTimes(2);
    expect(second).toHaveBeenCalledTimes(2);
  });

  it("a retry still refused says it can't be used, with Core's reason, and offers no second download", async () => {
    const attempt = vi
      .fn<() => Promise<string>>()
      .mockRejectedValueOnce(notInstalled())
      .mockRejectedValueOnce(notInstalled("the helper answered another version"));
    const connected = withDuckdbHelper(attempt, { interactive: true });
    await acceptAndInstall();
    const error = await connected.catch((e: unknown) => e);
    expect(error).toBeInstanceOf(DuckdbHelperUnusable);
    expect(extractErrorMessage(error)).toBe(
      m.duckdb_helper_unusable({ reason: "the helper answered another version" }),
    );
    expect(duckdbInstallStore.stage).toEqual({
      step: "unusable",
      reason: "the helper answered another version",
    });
    expect(holder.fake.calls).toEqual(["offer", "install"]);
    expect(attempt).toHaveBeenCalledTimes(2);
  });

  it("ENGINE_UNAVAILABLE is worded, with no download", async () => {
    const attempt = vi
      .fn()
      .mockRejectedValue(new CoreCallError({ code: "ENGINE_UNAVAILABLE", message: "silent" }));
    const failed = withDuckdbHelper(attempt, { interactive: true });
    await expect(failed).rejects.toThrow(m.duckdb_helper_unavailable());
    await failed.catch((error) => expect(errorCode(error)).toBe("ENGINE_UNAVAILABLE"));
    await settle();
    expect(duckdbInstallStore.open).toBe(false);
    expect(holder.fake.calls).toEqual([]);
  });

  it("ENGINE_UNAVAILABLE after an install is worded too", async () => {
    const attempt = vi
      .fn<() => Promise<string>>()
      .mockRejectedValueOnce(notInstalled())
      .mockRejectedValueOnce(new CoreCallError({ code: "ENGINE_UNAVAILABLE", message: "silent" }));
    const connected = withDuckdbHelper(attempt, { interactive: true });
    await acceptAndInstall();
    await expect(connected).rejects.toThrow(m.duckdb_helper_unavailable());
  });

  it("ENGINE_NOT_AVAILABLE says DuckDB can't be used here", async () => {
    const attempt = vi
      .fn()
      .mockRejectedValue(new CoreCallError({ code: "ENGINE_NOT_AVAILABLE", message: "no duckdb" }));
    await expect(withDuckdbHelper(attempt, { interactive: true })).rejects.toThrow(
      m.duckdb_helper_not_available(),
    );
    expect(holder.fake.calls).toEqual([]);
  });

  it("leaves every other error as it is", async () => {
    const original = new CoreCallError({ code: "FILE_NOT_FOUND", message: "no such file" });
    const attempt = vi.fn().mockRejectedValue(original);
    await expect(withDuckdbHelper(attempt, { interactive: true })).rejects.toBe(original);
  });

  it("a store that failed to load is loaded again next time (review M4)", async () => {
    const loader = vi
      .fn<() => Promise<typeof duckdbInstallStore>>()
      .mockRejectedValueOnce(new Error("chunk failed to load"))
      .mockResolvedValue(duckdbInstallStore);
    setInstallStoreLoader(loader);
    try {
      const refused = vi.fn().mockRejectedValue(notInstalled());
      const failed = await withDuckdbHelper(refused, { interactive: true }).catch(
        (e: unknown) => e,
      );
      // The connect's own error, worded: the dialog couldn't be shown.
      expect(errorCode(failed)).toBe("ENGINE_NOT_INSTALLED");
      const attempt = vi
        .fn<() => Promise<string>>()
        .mockRejectedValueOnce(notInstalled())
        .mockResolvedValueOnce("pc-1");
      const connected = withDuckdbHelper(attempt, { interactive: true });
      await acceptAndInstall();
      await expect(connected).resolves.toBe("pc-1");
      expect(loader).toHaveBeenCalledTimes(2);
    } finally {
      setInstallStoreLoader(null);
    }
  });
});

describe("prefetchDuckdbHelper (Task 6, Decision 11)", () => {
  const duckdb = { type: "duckdb" };
  const postgres = { type: "postgres" };
  /** The page once its connections loaded, in the main window. */
  const page = (connections: { type: string }[] = [postgres, duckdb]) => ({
    loaded: true,
    standalone: false,
    connections: () => connections,
  });

  beforeEach(() => {
    env.tauri = true;
    // A fresh store each time: the prefetch runs once per store (page).
    setInstallStoreLoader(async () => {
      const { DuckdbInstallStore } = await import("$lib/stores/duckdb-install.svelte");
      return new DuckdbInstallStore(holder.fake);
    });
  });

  afterEach(() => {
    setInstallStoreLoader(null);
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
  });

  it("installs in the background when a saved connection is DuckDB", async () => {
    const prefetched = prefetchDuckdbHelper(page(), { delayMs: 0 });
    await holder.fake.started();
    expect(holder.fake.calls).toEqual(["offer", "install"]);
    expect(duckdbInstallStore.open).toBe(false);
    holder.fake.finish();
    await expect(prefetched).resolves.toBe("installed");
  });

  it("does nothing without a saved DuckDB connection", async () => {
    await expect(prefetchDuckdbHelper(page([postgres]), { delayMs: 0 })).resolves.toBe("no-duckdb");
    expect(holder.fake.calls).toEqual([]);
  });

  it("reads the connections after the wait: one removed meanwhile counts", async () => {
    const connections = [duckdb];
    const prefetched = prefetchDuckdbHelper(
      { loaded: true, standalone: false, connections: () => connections },
      { delayMs: 20 },
    );
    connections.pop();
    await expect(prefetched).resolves.toBe("no-duckdb");
    expect(holder.fake.calls).toEqual([]);
  });

  it("waits before it starts, so the app's start goes first", async () => {
    vi.useFakeTimers();
    try {
      const prefetched = prefetchDuckdbHelper(page());
      await vi.advanceTimersByTimeAsync(PREFETCH_DELAY_MS - 1);
      expect(holder.fake.calls).toEqual([]);
      await vi.advanceTimersByTimeAsync(1);
      vi.useRealTimers();
      await holder.fake.started();
      expect(holder.fake.calls).toEqual(["offer", "install"]);
      holder.fake.finish();
      await prefetched;
    } finally {
      vi.useRealTimers();
    }
  });

  it("does nothing when the connections didn't load", async () => {
    await expect(prefetchDuckdbHelper({ ...page(), loaded: false }, { delayMs: 0 })).resolves.toBe(
      "not-loaded",
    );
    expect(holder.fake.calls).toEqual([]);
  });

  it("runs only in the main window, not in a standalone one", async () => {
    await expect(
      prefetchDuckdbHelper({ ...page(), standalone: true }, { delayMs: 0 }),
    ).resolves.toBe("standalone");
    expect(holder.fake.calls).toEqual([]);
  });

  it("does nothing outside the desktop app", async () => {
    env.tauri = false;
    await expect(prefetchDuckdbHelper(page(), { delayMs: 0 })).resolves.toBe("not-desktop");
    env.tauri = true;
    for (const target of ["web", "demo"]) {
      vi.stubEnv("VITE_BUILD_TARGET", target);
      await expect(prefetchDuckdbHelper(page(), { delayMs: 0 })).resolves.toBe("not-desktop");
    }
    expect(holder.fake.calls).toEqual([]);
  });

  it("doesn't download on a connection that asks to save data", async () => {
    vi.stubGlobal("navigator", { connection: { saveData: true } });
    await expect(prefetchDuckdbHelper(page(), { delayMs: 0 })).resolves.toBe("metered");
    expect(holder.fake.calls).toEqual([]);
  });

  it("a failure shows nothing", async () => {
    const prefetched = prefetchDuckdbHelper(page(), { delayMs: 0 });
    await holder.fake.started();
    holder.fake.fail("NETWORK_ERROR");
    await expect(prefetched).resolves.toBe("failed");
    expect(duckdbInstallStore.open).toBe(false);
  });

  it("a store that doesn't load is logged and shows nothing", async () => {
    setInstallStoreLoader(() => Promise.reject(new Error("chunk failed")));
    await expect(prefetchDuckdbHelper(page(), { delayMs: 0 })).resolves.toBe("failed");
    expect(holder.fake.calls).toEqual([]);
  });
});

describe("installDuckdbHelperJoined (Task 6 review)", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("installs through the dialog's store, so a dialog joins it", async () => {
    const seen: number[] = [];
    const before = holder.fake.downloads;
    const installed = installDuckdbHelperJoined((p) => seen.push(p.bytes));
    await holder.fake.started();
    holder.fake.progress(1_000_000);
    const requested = duckdbInstallStore.request();
    await vi.waitFor(() => expect(duckdbInstallStore.stage.step).toBe("downloading"));
    holder.fake.finish();
    await expect(installed).resolves.toMatchObject({ downloaded: true });
    await expect(requested).resolves.toBe(true);
    expect(seen).toEqual([1_000_000]);
    // The dialog's install joined the running one: one download.
    expect(holder.fake.calls).toEqual(["install", "offer", "install"]);
    expect(holder.fake.downloads).toBe(before + 1);
  });

  it("refuses outside the desktop build", async () => {
    vi.stubEnv("VITE_BUILD_TARGET", "web");
    await expect(installDuckdbHelperJoined()).rejects.toMatchObject({ code: "NOT_SUPPORTED" });
    expect(holder.fake.calls).toEqual([]);
  });
});
