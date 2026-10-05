/**
 * The fake install service joins a second `install()` as the real command
 * does (Task 4's `HelperInstalls`): the joiner gets the current progress
 * at once, then each update, and the one answer; cancel rejects every
 * caller with `CANCELLED`. Task 6's prefetch tests rely on it.
 */
import { describe, expect, it } from "vitest";
import { FakeDuckdbInstall, HELPER_SIZE } from "./duckdb-install-testing";
import type { DuckdbHelperProgress } from "$lib/api/tauri";

describe("FakeDuckdbInstall", () => {
  it("joins a second install: current progress at once, then one answer to both", async () => {
    const fake = new FakeDuckdbInstall();
    const first: DuckdbHelperProgress[] = [];
    const second: DuckdbHelperProgress[] = [];
    const a = fake.install((p) => first.push(p));
    fake.progress(1_000_000);
    const b = fake.install((p) => second.push(p));
    expect(second).toEqual([{ bytes: 1_000_000, total: HELPER_SIZE }]);
    fake.progress(2_000_000);
    expect(first.map((p) => p.bytes)).toEqual([1_000_000, 2_000_000]);
    expect(second.map((p) => p.bytes)).toEqual([1_000_000, 2_000_000]);
    fake.finish({ downloaded: true, pruned: 1 });
    await expect(a).resolves.toEqual({ downloaded: true, pruned: 1 });
    await expect(b).resolves.toEqual({ downloaded: true, pruned: 1 });
    expect(fake.calls).toEqual(["install", "install"]);
    expect(fake.downloads).toBe(1);
  });

  it("a joiner with nothing received yet gets no progress until the first", () => {
    const fake = new FakeDuckdbInstall();
    void fake.install();
    const seen: DuckdbHelperProgress[] = [];
    void fake.install((p) => seen.push(p));
    expect(seen).toEqual([]);
  });

  it("a failure reaches both", async () => {
    const fake = new FakeDuckdbInstall();
    const a = fake.install();
    const b = fake.install();
    fake.fail("NETWORK_ERROR");
    await expect(a).rejects.toMatchObject({ code: "NETWORK_ERROR" });
    await expect(b).rejects.toMatchObject({ code: "NETWORK_ERROR" });
  });

  it("cancel rejects every caller with CANCELLED", async () => {
    const fake = new FakeDuckdbInstall();
    const a = fake.install();
    const b = fake.install();
    await expect(fake.cancel()).resolves.toBe(true);
    await expect(a).rejects.toMatchObject({ code: "CANCELLED" });
    await expect(b).rejects.toMatchObject({ code: "CANCELLED" });
    expect(fake.busy).toBe(false);
    await expect(fake.cancel()).resolves.toBe(false);
  });

  it("an install after one ended is a new download", async () => {
    const fake = new FakeDuckdbInstall();
    const a = fake.install();
    fake.finish();
    await a;
    void fake.install();
    expect(fake.downloads).toBe(2);
  });
});
