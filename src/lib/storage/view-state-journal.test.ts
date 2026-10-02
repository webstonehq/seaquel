import { describe, expect, it } from "vitest";
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import {
  replayViewStateJournal,
  VIEW_STATE_JOURNAL_KEY,
  VIEW_STATE_JOURNAL_MAX_BYTES,
  writeViewStateJournal,
} from "./view-state-journal";

class MemoryStorage implements Storage {
  readonly map = new Map<string, string>();
  get length() {
    return this.map.size;
  }
  clear() {
    this.map.clear();
  }
  getItem(k: string) {
    return this.map.get(k) ?? null;
  }
  key(i: number) {
    return [...this.map.keys()][i] ?? null;
  }
  removeItem(k: string) {
    this.map.delete(k);
  }
  setItem(k: string, v: string) {
    this.map.set(k, String(v));
  }
}

const save = (rev: number, text = "SELECT 1"): CoreRequest =>
  ({
    method: "ui",
    params: {
      method: "windowStateSave",
      params: { windowId: "demo", projectId: "p1", rev, state: { tabs: [text] } },
    },
  }) as CoreRequest;

describe("the demo's view-state journal", () => {
  it("keeps the save as its request, synchronously, well-formed", () => {
    const storage = new MemoryStorage();
    expect(writeViewStateJournal(save(3, "a\ud800b"), storage)).toBe(true);
    const kept = JSON.parse(storage.getItem(VIEW_STATE_JOURNAL_KEY)!);
    expect(kept.params.params.rev).toBe(3);
    expect(kept.params.params.state.tabs).toEqual(["a�b"]);
  });

  it("keeps nothing past the size cap, and nothing that isn't a view-state save", () => {
    const storage = new MemoryStorage();
    expect(writeViewStateJournal(save(1, "x".repeat(VIEW_STATE_JOURNAL_MAX_BYTES)), storage)).toBe(
      false,
    );
    expect(
      writeViewStateJournal(
        { method: "storage", params: { method: "vaultStateLoad" } } as CoreRequest,
        storage,
      ),
    ).toBe(false);
    expect(storage.map.size).toBe(0);
  });

  it("says false when storage refuses", () => {
    const storage = new MemoryStorage();
    storage.setItem = () => {
      throw new DOMException("quota", "QuotaExceededError");
    };
    expect(writeViewStateJournal(save(1), storage)).toBe(false);
    expect(writeViewStateJournal(save(1), null)).toBe(false);
  });

  it("replays the kept save through Core once, then forgets it", async () => {
    const storage = new MemoryStorage();
    writeViewStateJournal(save(7), storage);
    const sent: unknown[] = [];
    expect(
      await replayViewStateJournal(
        async (body) => sent.push(JSON.parse(new TextDecoder().decode(body))),
        storage,
      ),
    ).toBe("sent");
    expect(sent).toEqual([JSON.parse(JSON.stringify(save(7)))]);
    expect(storage.getItem(VIEW_STATE_JOURNAL_KEY)).toBeNull();
    expect(await replayViewStateJournal(async () => sent.push("again"), storage)).toBe("none");
    expect(sent.length).toBe(1);
  });

  it("drops an entry that isn't a view-state save, and one Core refuses, without throwing", async () => {
    const storage = new MemoryStorage();
    storage.setItem(VIEW_STATE_JOURNAL_KEY, "{not json");
    expect(await replayViewStateJournal(async () => {}, storage)).toBe("dropped");
    storage.setItem(
      VIEW_STATE_JOURNAL_KEY,
      JSON.stringify({ method: "library", params: { method: "projectsList" } }),
    );
    expect(await replayViewStateJournal(async () => {}, storage)).toBe("dropped");
    writeViewStateJournal(save(2), storage);
    expect(
      await replayViewStateJournal(async () => {
        throw new Error("refused");
      }, storage),
    ).toBe("failed");
    expect(storage.map.size).toBe(0);
  });
});
