import { describe, expect, test } from "vitest";
import { oncePerTask } from "./once-per-task";

describe("oncePerTask", () => {
  test("6,000 model changes in one input event run the work once", async () => {
    let runs = 0;
    const schedule = oncePerTask(() => runs++);
    // Firefox commits a typed text as one composition, and Monaco types it one
    // character at a time: one content change per character, all in one task.
    for (let i = 0; i < 6000; i++) schedule();
    expect(runs).toBe(0);
    await Promise.resolve();
    expect(runs).toBe(1);
  });

  test("a change after the work ran schedules it again", async () => {
    let runs = 0;
    const schedule = oncePerTask(() => runs++);
    schedule();
    await Promise.resolve();
    schedule();
    schedule();
    await Promise.resolve();
    expect(runs).toBe(2);
  });

  test("the work still runs before the next task, so nothing renders in between", async () => {
    const order: string[] = [];
    const schedule = oncePerTask(() => order.push("work"));
    schedule();
    await new Promise<void>((resolve) =>
      setTimeout(() => {
        order.push("next task");
        resolve();
      }, 0),
    );
    expect(order).toEqual(["work", "next task"]);
  });
});
