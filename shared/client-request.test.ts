/**
 * Watching the client's socket: the route aborts its call to Rust when the
 * browser's connection closes, and stops watching once Rust has answered.
 */

import { EventEmitter } from "node:events";
import { describe, expect, it, vi } from "vitest";
import { currentClientRequest, onClientClose, withClientRequest } from "./client-request.js";

function socket(destroyed = false) {
  return Object.assign(new EventEmitter(), { destroyed });
}

describe("client request", () => {
  it("calls back once when the socket closes, and stops when told", () => {
    const s = socket();
    const onClose = vi.fn();
    const stop = onClientClose({ socket: s }, onClose);
    s.emit("close");
    s.emit("close");
    expect(onClose).toHaveBeenCalledOnce();

    const s2 = socket();
    const onClose2 = vi.fn();
    onClientClose({ socket: s2 }, onClose2)();
    s2.emit("close");
    expect(onClose2).not.toHaveBeenCalled();
    stop();
  });

  it("holds one listener per socket, however many requests it serves", () => {
    const s = socket();
    const calls: number[] = [];
    const stops = Array.from({ length: 100 }, (_, i) =>
      onClientClose({ socket: s }, () => calls.push(i)),
    );
    expect(s.listenerCount("close")).toBe(1);
    stops.slice(0, 98).forEach((stop) => stop());
    expect(s.listenerCount("close")).toBe(1);
    s.emit("close");
    expect(calls).toEqual([98, 99]);
  });

  it("calls back at once for a socket already closed, and never without one", () => {
    const onClose = vi.fn();
    onClientClose({ socket: socket(true) }, onClose);
    expect(onClose).toHaveBeenCalledOnce();
    const never = vi.fn();
    onClientClose(undefined, never)();
    onClientClose({ socket: null }, never)();
    expect(never).not.toHaveBeenCalled();
  });

  it("carries the request through async work started inside it", async () => {
    const req = { socket: socket() } as never;
    expect(currentClientRequest()).toBeUndefined();
    const seen = await withClientRequest(req, async () => {
      await new Promise((r) => setTimeout(r, 1));
      return currentClientRequest();
    });
    expect(seen).toBe(req);
    expect(currentClientRequest()).toBeUndefined();
  });
});
