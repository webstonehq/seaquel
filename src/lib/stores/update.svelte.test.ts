import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { UpdateInfo } from "$lib/api/tauri";

const { storage, seq, setSetting, checkForUpdate, installUpdate } = vi.hoisted(() => {
  let n = 0;
  const storage = new Map<string, string | null>();
  const seq = () => ({ epoch: "e", n: ++n });
  return {
    storage,
    seq,
    setSetting: vi.fn(async (key: string, value: string | null) => {
      storage.set(key, value);
      return { value, seq: seq() };
    }),
    checkForUpdate: vi.fn<() => Promise<UpdateInfo | null>>(),
    installUpdate: vi.fn<() => Promise<void>>(),
  };
});

// The `settings` group (5d-2) over an in-memory `app_state`.
vi.mock("$lib/hooks/database/library/index", () => {
  const settings = {
    getSetting: async (key: string) => ({ value: storage.get(key) ?? null, seq: seq() }),
    setSetting,
  };
  return { getSettings: () => settings };
});

vi.mock("$lib/api/tauri", () => ({ checkForUpdate, installUpdate }));

const info = (version: string): UpdateInfo => ({ version, date: null, size: null });

async function freshStore(stored?: string) {
  storage.clear();
  if (stored !== undefined) storage.set("updateChannel", stored);
  vi.resetModules();
  const { updateStore } = await import("./update.svelte.js");
  await updateStore.initialize();
  return updateStore;
}

/** Another window's change of `keys`, as `LibrarySync` hands it on (same module graph as the store). */
async function changedElsewhere(keys: string[]) {
  const { applyStoredChange } = await import("./settings-sync");
  await applyStoredChange("setting", keys);
}

describe("updateStore channel", () => {
  beforeEach(() => {
    setSetting.mockClear();
    checkForUpdate.mockReset().mockResolvedValue(null);
    installUpdate.mockReset().mockResolvedValue(undefined);
    vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("has no channel when nothing is stored, and saves the one set", async () => {
    const store = await freshStore();
    expect(store.channel).toBeNull();

    await store.setChannel("beta");
    expect(store.channel).toBe("beta");
    expect(setSetting).toHaveBeenCalledWith("updateChannel", "beta");
  });

  it("forgets an update found on the old channel and checks the new one", async () => {
    const store = await freshStore("stable");
    store.setUpdateDownloaded(info("2026.10.1"));

    await store.setChannel("beta");
    expect(store.updateInfo).toBeNull();
    expect(store.isDownloaded).toBe(false);
    expect(checkForUpdate).toHaveBeenCalledTimes(1);
  });

  it("shows an update the new channel's check finds", async () => {
    const store = await freshStore("stable");
    store.setUpdateDownloaded(info("2026.10.1"));
    checkForUpdate.mockResolvedValue(info("2026.10.2-beta.1"));

    await store.setChannel("beta");
    expect(store.updateInfo).toEqual(info("2026.10.2-beta.1"));
    expect(store.isDownloaded).toBe(false);
  });

  it("loads a stored channel", async () => {
    expect((await freshStore("beta")).channel).toBe("beta");
    expect((await freshStore("stable")).channel).toBe("stable");
  });

  it("reads an unknown stored value as no channel", async () => {
    expect((await freshStore("nightly")).channel).toBeNull();
  });

  it("drops a stale download and checks again when install says UPDATE_STALE", async () => {
    const store = await freshStore("beta");
    store.setUpdateDownloaded(info("2026.10.1"));
    installUpdate.mockRejectedValue({ message: "stale", code: "UPDATE_STALE" });

    await store.install();
    expect(store.updateInfo).toBeNull();
    expect(store.isDownloaded).toBe(false);
    expect(store.isInstalling).toBe(false);
    expect(checkForUpdate).toHaveBeenCalledTimes(1);
  });

  it("keeps the download when install fails for another reason", async () => {
    const store = await freshStore("beta");
    store.setUpdateDownloaded(info("2026.10.1"));
    installUpdate.mockRejectedValue({ message: "boom", code: "UPDATE_ERROR" });

    await store.install();
    expect(store.updateInfo).toEqual(info("2026.10.1"));
    expect(store.isDownloaded).toBe(true);
    expect(store.isInstalling).toBe(false);
    expect(checkForUpdate).not.toHaveBeenCalled();
  });

  it("does nothing when the channel is already the one set", async () => {
    const store = await freshStore("beta");
    store.setUpdateDownloaded(info("2026.10.1"));

    await store.setChannel("beta");
    expect(store.isDownloaded).toBe(true);
    expect(setSetting).not.toHaveBeenCalled();
    expect(checkForUpdate).not.toHaveBeenCalled();
  });

  it("shows the stored channel again and still checks when the save is refused", async () => {
    const store = await freshStore("stable");
    setSetting.mockRejectedValueOnce({ message: "nope", code: "STORAGE_ERROR" });

    await store.setChannel("beta");
    expect(store.channel).toBe("stable");
    expect(storage.get("updateChannel")).toBe("stable");
    expect(checkForUpdate).toHaveBeenCalledTimes(1);
  });

  it("follows another window's switch and forgets this window's download", async () => {
    const store = await freshStore("stable");
    store.setUpdateDownloaded(info("2026.10.1"));

    storage.set("updateChannel", "beta");
    await changedElsewhere(["updateChannel"]);
    expect(store.channel).toBe("beta");
    expect(store.updateInfo).toBeNull();
    expect(store.isDownloaded).toBe(false);
  });

  it("keeps the download when another window's change leaves the channel as it was", async () => {
    const store = await freshStore("stable");
    store.setUpdateDownloaded(info("2026.10.1"));

    await changedElsewhere(["updateChannel"]);
    expect(store.isDownloaded).toBe(true);
  });

  it("ignores a check from an earlier switch that answers late", async () => {
    const store = await freshStore("stable");
    let answerFirst!: (value: UpdateInfo | null) => void;
    checkForUpdate.mockReturnValueOnce(new Promise((resolve) => (answerFirst = resolve)));

    const first = store.setChannel("beta");
    await vi.waitFor(() => expect(checkForUpdate).toHaveBeenCalledTimes(1));
    await store.setChannel("stable");
    answerFirst(info("2026.10.2-beta.1"));
    await first;
    expect(store.channel).toBe("stable");
    expect(store.updateInfo).toBeNull();
  });

  it("stops installing when install answers without restarting", async () => {
    const store = await freshStore("beta");
    store.setUpdateDownloaded(info("2026.10.1"));

    await store.install();
    expect(store.isInstalling).toBe(false);
    expect(store.isDownloaded).toBe(true);
  });
});
