/**
 * The per-route body limits: `server.js` moves the operator's
 * `BODY_SIZE_LIMIT` aside and sets adapter-node's to `Infinity`; the hook
 * gives `/api/rpc` 20 MiB and every other route the operator's limit.
 */

import { describe, expect, it } from "vitest";
import {
  BODY_LIMIT_ENV,
  bodyLimitFor,
  moveBodyLimit,
  parseBodyLimit,
  RPC_BODY_LIMIT,
  rpcBodyLimit,
} from "./body-limit.js";

describe("body limits", () => {
  it("parses limits exactly as adapter-node does", () => {
    expect(parseBodyLimit("512K")).toBe(512 * 1024);
    expect(parseBodyLimit("20m")).toBe(20 * 1024 * 1024);
    expect(parseBodyLimit("1G")).toBe(1024 ** 3);
    expect(parseBodyLimit("1000")).toBe(1000);
    expect(parseBodyLimit("1e6")).toBe(1e6);
    expect(parseBodyLimit("Infinity")).toBe(Infinity);
    for (const bad of ["abc", "12Q", "1.2.3K"]) {
      expect(parseBodyLimit(bad), bad).toBeNaN();
    }
  });

  it("moves the operator's limit aside and turns adapter-node's off", () => {
    const env: Record<string, string | undefined> = { BODY_SIZE_LIMIT: "2M" };
    expect(moveBodyLimit(env)).toBe(2 * 1024 * 1024);
    expect(env).toEqual({ BODY_SIZE_LIMIT: "Infinity", [BODY_LIMIT_ENV]: "2M" });

    const unset: Record<string, string | undefined> = {};
    expect(moveBodyLimit(unset)).toBe(512 * 1024);
    expect(unset[BODY_LIMIT_ENV]).toBe("512K");

    expect(() => moveBodyLimit({ BODY_SIZE_LIMIT: "lots" })).toThrow(/Invalid BODY_SIZE_LIMIT/);
  });

  it("gives /api/rpc 20 MiB, or the operator's higher limit up to 64 MiB", () => {
    expect(RPC_BODY_LIMIT).toBe(20 * 1024 * 1024);
    // Whatever the operator's lower limit: /api/rpc needs WEB_EDIT_LIMITS' 18 MiB.
    expect(bodyLimitFor("/api/rpc", { [BODY_LIMIT_ENV]: "1K" })).toBe(RPC_BODY_LIMIT);
    expect(bodyLimitFor("/api/rpc", { [BODY_LIMIT_ENV]: "30M" })).toBe(30 * 1024 * 1024);
    expect(rpcBodyLimit({ [BODY_LIMIT_ENV]: "Infinity" })).toBe(64 * 1024 * 1024);
    expect(rpcBodyLimit({})).toBe(RPC_BODY_LIMIT);
    expect(bodyLimitFor("/api/signup", { [BODY_LIMIT_ENV]: "1K" })).toBe(1024);
    expect(bodyLimitFor("/api/rpc/", {})).toBe(512 * 1024);
    expect(bodyLimitFor("/api/signup", {})).toBe(512 * 1024);
    expect(bodyLimitFor("/x", { [BODY_LIMIT_ENV]: "Infinity" })).toBe(Infinity);
  });
});
