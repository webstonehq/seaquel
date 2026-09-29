import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CoreClient,
  EventsUnavailableReason,
  ResubscribedInfo,
  WorkspaceEvent,
} from "$lib/core/client";
import { ChangeFeed, type StorageChange } from "./change-feed";
import { RowSeqs } from "./seqs";

/** A `CoreClient` whose event channel the test drives. */
function fakeClient() {
  const order: string[] = [];
  const handlers = {
    events: new Set<(e: WorkspaceEvent) => void>(),
    resubscribed: new Set<(i: ResubscribedInfo) => void>(),
    unavailable: new Set<(r: EventsUnavailableReason) => void>(),
  };
  const client = {
    call: vi.fn(),
    stream: vi.fn(),
    events(h: (e: WorkspaceEvent) => void) {
      order.push("events");
      handlers.events.add(h);
      return () => handlers.events.delete(h);
    },
    onResubscribed(h: (i: ResubscribedInfo) => void) {
      order.push("onResubscribed");
      handlers.resubscribed.add(h);
      return () => handlers.resubscribed.delete(h);
    },
    onEventsUnavailable(h: (r: EventsUnavailableReason) => void) {
      order.push("onEventsUnavailable");
      handlers.unavailable.add(h);
      return () => handlers.unavailable.delete(h);
    },
  } as unknown as CoreClient;
  return {
    client,
    order,
    emit: (e: WorkspaceEvent) => handlers.events.forEach((h) => h(e)),
    resubscribe: (initial: boolean) => handlers.resubscribed.forEach((h) => h({ initial })),
    unavailable: (r: EventsUnavailableReason) => handlers.unavailable.forEach((h) => h(r)),
    handlers,
  };
}

function changed(
  kind: StorageChange["kind"],
  over: Partial<Extract<WorkspaceEvent, { type: "storageChanged" }>> = {},
): WorkspaceEvent {
  return {
    type: "storageChanged",
    kind,
    scope: null,
    ids: null,
    origin: "other-tab",
    seq: { epoch: "e1", n: 1 },
    ...over,
  };
}

let fake: ReturnType<typeof fakeClient>;
let seqs: RowSeqs;
let feed: ChangeFeed;

beforeEach(() => {
  vi.useFakeTimers();
  fake = fakeClient();
  seqs = new RowSeqs();
  seqs.epoch = "e1";
  feed = new ChangeFeed({ client: () => fake.client, origin: () => "this-tab", seqs });
  feed.start();
});

afterEach(() => {
  feed.stop();
  vi.useRealTimers();
});

describe("ChangeFeed", () => {
  it("listens for resubscriptions before it listens for events", () => {
    expect(fake.order.indexOf("onResubscribed")).toBeLessThan(fake.order.indexOf("events"));
    expect(fake.order).toContain("onEventsUnavailable");
  });

  it("this tab's own write doesn't trigger a refetch", async () => {
    const handler = vi.fn();
    feed.subscribe("savedQuery", handler);
    fake.emit(changed("savedQuery", { origin: "this-tab", ids: ["q1"], scope: "p1" }));
    await vi.advanceTimersByTimeAsync(200);
    expect(handler).not.toHaveBeenCalled();
  });

  it("groups one kind and scope's events for 100 ms", async () => {
    const handler = vi.fn();
    feed.subscribe("savedQuery", handler);
    fake.emit(changed("savedQuery", { scope: "p1", ids: ["q1"], seq: { epoch: "e1", n: 3 } }));
    fake.emit(changed("savedQuery", { scope: "p1", ids: ["q2"], seq: { epoch: "e1", n: 5 } }));
    fake.emit(changed("savedQuery", { scope: "p2", ids: ["q9"], seq: { epoch: "e1", n: 4 } }));
    await vi.advanceTimersByTimeAsync(99);
    expect(handler).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(handler).toHaveBeenCalledTimes(2);
    expect(handler).toHaveBeenCalledWith({
      kind: "savedQuery",
      scope: "p1",
      ids: ["q1", "q2"],
      seq: { epoch: "e1", n: 5 },
    });
    expect(handler).toHaveBeenCalledWith({
      kind: "savedQuery",
      scope: "p2",
      ids: ["q9"],
      seq: { epoch: "e1", n: 4 },
    });
  });

  it("an event without ids makes the group a reload of the kind", async () => {
    const handler = vi.fn();
    feed.subscribe("connection", handler);
    fake.emit(changed("connection", { scope: "p1", ids: ["c1"] }));
    fake.emit(changed("connection", { scope: "p1", ids: null }));
    fake.emit(changed("connection", { scope: "p1", ids: ["c2"] }));
    await vi.advanceTimersByTimeAsync(100);
    expect(handler).toHaveBeenCalledWith(expect.objectContaining({ ids: null }));
  });

  it("only the kind's subscribers hear it, and connectionClosed isn't a change", async () => {
    const projects = vi.fn();
    feed.subscribe("project", projects);
    fake.emit(changed("connection", { ids: ["c1"] }));
    fake.emit({ type: "connectionClosed", connectionId: "x", code: "C", message: "m" });
    await vi.advanceTimersByTimeAsync(100);
    expect(projects).not.toHaveBeenCalled();
  });

  it("a new epoch reloads every list and drops the grouped events", async () => {
    const handler = vi.fn();
    const reload = vi.fn();
    feed.subscribe("savedQuery", handler);
    feed.onReload(reload);
    fake.emit(changed("savedQuery", { scope: "p1", ids: ["q1"] }));
    fake.emit(changed("savedQuery", { scope: "p1", ids: ["q2"], seq: { epoch: "e2", n: 1 } }));
    await vi.advanceTimersByTimeAsync(100);
    expect(reload).toHaveBeenCalledWith({ reason: "epoch" });
    expect(handler).not.toHaveBeenCalled();
  });

  it("a new epoch seen in a write's answer reloads too", () => {
    const reload = vi.fn();
    feed.onReload(reload);
    seqs.note("connection:c1", { epoch: "e9", n: 1 });
    expect(reload).toHaveBeenCalledWith({ reason: "epoch" });
  });

  it("a socket reconnect reloads every list", () => {
    const reload = vi.fn();
    feed.onReload(reload);
    fake.resubscribe(true);
    fake.resubscribe(false);
    expect(reload).toHaveBeenNthCalledWith(1, { reason: "resubscribed", initial: true });
    expect(reload).toHaveBeenNthCalledWith(2, { reason: "resubscribed", initial: false });
  });

  it("says when updates stop, and when they're back", () => {
    const status = vi.fn();
    feed.onStatus(status);
    expect(feed.unavailable).toBeNull();
    fake.unavailable("TOO_MANY_TABS");
    expect(feed.unavailable).toBe("TOO_MANY_TABS");
    expect(status).toHaveBeenLastCalledWith("TOO_MANY_TABS");
    fake.resubscribe(false);
    expect(feed.unavailable).toBeNull();
    expect(status).toHaveBeenLastCalledWith(null);
  });

  it("stop unsubscribes from the client and drops pending groups", async () => {
    const handler = vi.fn();
    feed.subscribe("project", handler);
    fake.emit(changed("project", { ids: ["p1"] }));
    feed.stop();
    await vi.advanceTimersByTimeAsync(100);
    expect(handler).not.toHaveBeenCalled();
    expect(fake.handlers.events.size).toBe(0);
    expect(fake.handlers.resubscribed.size).toBe(0);
  });
});
