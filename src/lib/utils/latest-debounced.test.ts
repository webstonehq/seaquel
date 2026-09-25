import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { latestDebounced } from "./latest-debounced";

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("latestDebounced", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("runs once after the delay with the last scheduled job", async () => {
    const onResult = vi.fn();
    const d = latestDebounced<string>(150, onResult);
    const first = vi.fn(async () => "a");
    const second = vi.fn(async () => "b");

    d.schedule(first);
    await vi.advanceTimersByTimeAsync(100);
    d.schedule(second);
    await vi.advanceTimersByTimeAsync(149);
    expect(second).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);

    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
    expect(onResult).toHaveBeenCalledExactlyOnceWith("b");
  });

  it("ignores a slow reply once a newer request was scheduled", async () => {
    const onResult = vi.fn();
    const d = latestDebounced<string>(150, onResult);
    const slow = deferred<string>();

    d.schedule(() => slow.promise);
    await vi.advanceTimersByTimeAsync(150); // slow request is now in flight
    d.schedule(async () => "new");
    await vi.advanceTimersByTimeAsync(150);
    expect(onResult).toHaveBeenCalledExactlyOnceWith("new");

    slow.resolve("old");
    await vi.advanceTimersByTimeAsync(0);
    expect(onResult).toHaveBeenCalledTimes(1);
  });

  it("reports only the newest request's error", async () => {
    const onResult = vi.fn();
    const onError = vi.fn();
    const d = latestDebounced<string>(150, onResult, onError);
    const stale = deferred<string>();

    d.schedule(() => stale.promise);
    await vi.advanceTimersByTimeAsync(150);
    d.schedule(async () => {
      throw new Error("boom");
    });
    await vi.advanceTimersByTimeAsync(150);
    stale.reject(new Error("stale"));
    await vi.advanceTimersByTimeAsync(0);

    expect(onResult).not.toHaveBeenCalled();
    expect(onError).toHaveBeenCalledOnce();
    expect((onError.mock.calls[0][0] as Error).message).toBe("boom");
  });

  it("cancel drops both a pending timer and an in-flight reply", async () => {
    const onResult = vi.fn();
    const d = latestDebounced<string>(150, onResult);
    const inFlight = deferred<string>();

    d.schedule(() => inFlight.promise);
    await vi.advanceTimersByTimeAsync(150);
    d.cancel();
    inFlight.resolve("late");
    await vi.advanceTimersByTimeAsync(0);

    const pending = vi.fn(async () => "never");
    d.schedule(pending);
    d.cancel();
    await vi.advanceTimersByTimeAsync(500);

    expect(pending).not.toHaveBeenCalled();
    expect(onResult).not.toHaveBeenCalled();
  });

  it("a synchronous throw inside the job goes to onError", async () => {
    const onError = vi.fn();
    const d = latestDebounced<string>(10, vi.fn(), onError);
    d.schedule(() => {
      throw new Error("sync");
    });
    await vi.advanceTimersByTimeAsync(10);
    expect(onError).toHaveBeenCalledOnce();
  });
});
