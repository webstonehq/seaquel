import { describe, expect, it, vi } from "vitest";
import { RowSeqs } from "./seqs";

const at = (n: number, epoch = "e1") => ({ epoch, n });

describe("RowSeqs", () => {
  it("takes only a number higher than the one applied", () => {
    const s = new RowSeqs();
    expect(s.take("connection:c1", at(5))).toBe(true);
    expect(s.take("connection:c1", at(5))).toBe(false);
    expect(s.take("connection:c1", at(4))).toBe(false);
    expect(s.take("connection:c1", at(6))).toBe(true);
    expect(s.last("connection:c1")).toBe(6);
  });

  it("a refetch older than this page's own write answer is dropped", () => {
    const s = new RowSeqs();
    s.note("connection:c1", at(9)); // the write's answer
    expect(s.isNewer("connection:c1", at(8))).toBe(false);
    expect(s.take("connection:c1", at(8))).toBe(false);
    expect(s.take("connection:c1", at(10))).toBe(true);
  });

  it("note never lowers the recorded number", () => {
    const s = new RowSeqs();
    s.note("k", at(7));
    s.note("k", at(3));
    expect(s.last("k")).toBe(7);
  });

  it("a new epoch drops every number and tells the listeners once", () => {
    const s = new RowSeqs();
    const reload = vi.fn();
    s.onNewEpoch(reload);
    s.take("k", at(40));
    expect(s.observe(at(1, "e2"))).toBe("switched");
    expect(reload).toHaveBeenCalledTimes(1);
    expect(s.epoch).toBe("e2");
    // Numbers restart in the new workspace.
    expect(s.take("k", at(1, "e2"))).toBe(true);
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("a late result from a replaced epoch is ignored", () => {
    const s = new RowSeqs();
    const reload = vi.fn();
    s.onNewEpoch(reload);
    s.take("k", at(5));
    s.take("k", at(1, "e2")); // the workspace was reopened
    expect(s.observe(at(9))).toBe("stale");
    expect(s.take("k", at(9))).toBe(false);
    s.note("j", at(9));
    expect(s.last("j")).toBeUndefined();
    expect(s.isNewer("k", at(9))).toBe(false);
    expect(s.epoch).toBe("e2");
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("a value from another epoch counts as newer", () => {
    const s = new RowSeqs();
    s.take("k", at(40));
    expect(s.isNewer("k", at(1, "e2"))).toBe(true);
  });

  it("settled waits for the writes in flight for a prefix, failed ones too", async () => {
    const s = new RowSeqs();
    let finish!: () => void;
    let fail!: (e: Error) => void;
    const a = s.write(["savedQuery:q1"], () => new Promise<void>((r) => (finish = r)));
    const b = s
      .write(["savedQuery:q2"], () => new Promise<void>((_, j) => (fail = j)))
      .catch(() => {});
    expect(s.busy("savedQuery:")).toBe(true);
    expect(s.busy("connection:")).toBe(false);
    let done = false;
    const wait = s.settled("savedQuery:").then(() => (done = true));
    await Promise.resolve();
    expect(done).toBe(false);
    finish();
    fail(new Error("refused"));
    await Promise.all([a, b, wait]);
    expect(done).toBe(true);
    expect(s.busy("savedQuery:")).toBe(false);
  });
});
