/**
 * The Node request behind a SvelteKit request, so a route can tell when the
 * client's connection closes (phase 5c probe, I1).
 *
 * SvelteKit's `request.signal` aborts only when the client leaves before its
 * body was read, so a route that waits on something slow after reading the
 * body (`/api/rpc` waiting on Rust) never hears that the browser gave up.
 * The client's socket closing says so:
 *
 * - under adapter-node (`server.js`), the route gets the request as
 *   `platform.req`;
 * - the Vite dev server passes no `platform`, so its `seaquel-client-request`
 *   plugin (`vite.config.js`) runs each request inside `withClientRequest`,
 *   and the route reads it back with `currentClientRequest`.
 *
 * The storage lives on `globalThis`: Vite loads this module once for its
 * config and again for the SSR bundle, and both must share it.
 *
 * Plain ESM JS (no TS) so `vite.config.js` can import it without a build step.
 */

import { AsyncLocalStorage } from "node:async_hooks";

const KEY = Symbol.for("seaquel.clientRequest");

/** @type {AsyncLocalStorage<import("node:http").IncomingMessage>} */
const storage =
  /** @type {any} */ (globalThis)[KEY] ??
  (/** @type {any} */ (globalThis)[KEY] = new AsyncLocalStorage());

/**
 * Run `fn` with `req` as the current client request.
 * @template T
 * @param {import("node:http").IncomingMessage} req
 * @param {() => T} fn
 * @returns {T}
 */
export function withClientRequest(req, fn) {
  return storage.run(req, fn);
}

/**
 * The request `withClientRequest` is running, if any.
 * @returns {import("node:http").IncomingMessage | undefined}
 */
export function currentClientRequest() {
  return storage.getStore();
}

/**
 * The callbacks waiting on each socket. One `close` listener per socket,
 * however many requests a keep-alive socket serves or how many of them
 * wait at once (HTTP/1.1 pipelining), so Node never warns about listeners.
 * @type {WeakMap<object, Set<() => void>>}
 */
const waiting = new WeakMap();

/**
 * Call `onClose` once when `req`'s socket closes (the client aborted or went
 * away), or at once when it already has. Returns a function that stops
 * listening: call it when the work is done, since a keep-alive socket serves
 * later requests too. A request without a socket is never watched.
 *
 * @param {{ socket?: import("node:events").EventEmitter & { destroyed?: boolean } | null } | undefined} req
 * @param {() => void} onClose
 * @returns {() => void}
 */
export function onClientClose(req, onClose) {
  const socket = req?.socket;
  if (!socket) return () => {};
  if (socket.destroyed) {
    onClose();
    return () => {};
  }
  let callbacks = waiting.get(socket);
  if (!callbacks) {
    const set = new Set();
    callbacks = set;
    waiting.set(socket, set);
    socket.once("close", () => {
      waiting.delete(socket);
      for (const callback of set) callback();
    });
  }
  const callback = () => onClose();
  callbacks.add(callback);
  return () => {
    callbacks.delete(callback);
  };
}
