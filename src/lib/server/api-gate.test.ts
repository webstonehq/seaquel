/**
 * `handleApiGate`'s rules (replacing `hooks.server.test.ts`, which tested
 * the membership predicate against auth.db; that row now comes from the
 * Rust gate, tested in `crates/seaquel-license/tests/ladder.rs`).
 */

import { describe, expect, it } from "vitest";
import {
  apiGateResponse,
  isApiGateExempt,
  licenseServiceUnavailable,
  needsLicenseGate,
  originGateResponse,
} from "./api-gate";
import type { GateAnswer } from "./license-client";

function answer(member: GateAnswer["member"]): GateAnswer {
  return { state: "ok", tenant: null, member, hasTenant: true, bundlePresent: false };
}

const BOUND = answer({ isOwner: false, revoked: false });

describe("exemptions (unchanged)", () => {
  it.each([
    "/",
    "/settings",
    "/api/auth/sign-in/email",
    "/api/auth/get-session",
    "/api/signup",
    "/api/airgap/bundle",
    "/api/account/tenant",
  ])("%s passes without a session", (path) => {
    expect(isApiGateExempt(path)).toBe(true);
    expect(apiGateResponse(path, null, "unregistered", null)).toBeNull();
    expect(needsLicenseGate(path, "u")).toBe(false);
  });

  it.each(["/api/rpc", "/api/db/query", "/api/team", "/api/account/stream-access", "/api/authx"])(
    "%s is gated",
    (path) => {
      expect(isApiGateExempt(path)).toBe(false);
      expect(needsLicenseGate(path, "u")).toBe(true);
      expect(needsLicenseGate(path, null)).toBe(false);
    },
  );
});

describe("gated paths", () => {
  it("no session is 401", async () => {
    const res = apiGateResponse("/api/rpc", null, "ok", null)!;
    expect(res.status).toBe(401);
    expect(await res.text()).toBe("unauthorized");
  });

  it.each(["unregistered", "suspended", "revalidate"] as const)(
    "license %s is 403",
    async (state) => {
      const res = apiGateResponse("/api/rpc", "u", state, BOUND)!;
      expect(res.status).toBe(403);
      expect(await res.text()).toBe(`license ${state}`);
    },
  );

  it("no membership row is 403", async () => {
    const res = apiGateResponse("/api/rpc", "u", "ok", answer(null))!;
    expect(res.status).toBe(403);
    expect(await res.text()).toBe("not a bound member of this install");
  });

  it("a revoked row is 403", () => {
    const res = apiGateResponse("/api/rpc", "u", "ok", answer({ isOwner: true, revoked: true }));
    expect(res?.status).toBe(403);
  });

  it("a bound member of a licensed install passes", () => {
    expect(apiGateResponse("/api/rpc", "u", "ok", BOUND)).toBeNull();
  });
});

describe("license service unavailable", () => {
  it("API calls get a 503 JSON code", async () => {
    const res = licenseServiceUnavailable("/api/rpc");
    expect(res.status).toBe(503);
    expect(res.headers.get("content-type")).toBe("application/json");
    expect(await res.json()).toEqual({ code: "license_service_unavailable" });
  });

  it("pages get a plain 503 page for the operator", async () => {
    const res = licenseServiceUnavailable("/settings");
    expect(res.status).toBe(503);
    expect(res.headers.get("content-type")).toContain("text/html");
    const text = await res.text();
    expect(text).toContain("license service isn't responding");
    expect(text).toContain("container logs");
  });
});

describe("Origin gate on state-changing /api calls", () => {
  const HOST = "localhost:8787";
  const OWN = "http://localhost:8787";

  it.each(["POST", "PUT", "PATCH", "DELETE"])(
    "%s from the install's own origin passes",
    (method) => {
      expect(originGateResponse(method, "/api/rpc", OWN, HOST)).toBeNull();
    },
  );

  it.each([
    ["POST", "/api/rpc", null],
    ["POST", "/api/rpc", ""],
    ["POST", "/api/rpc", "null"],
    ["POST", "/api/rpc", "https://evil.example"],
    ["POST", "/api/rpc", "http://localhost:8787.evil.example"],
    ["DELETE", "/api/team/u_2", "https://evil.example"],
    ["post", "/api/rpc", "https://evil.example"],
  ])("%s %s with Origin %s is 403 ORIGIN_NOT_ALLOWED", async (method, path, origin) => {
    const res = originGateResponse(method, path, origin, HOST);
    expect(res?.status).toBe(403);
    expect(res?.headers.get("content-type")).toBe("application/json");
    expect(await res?.json()).toEqual({
      code: "ORIGIN_NOT_ALLOWED",
      message: "request origin not allowed",
    });
  });

  it("refuses a domain Host's own origin (DNS rebinding) with nothing configured", async () => {
    const res = originGateResponse(
      "POST",
      "/api/rpc",
      "http://evil.example:8787",
      "evil.example:8787",
    );
    expect(res?.status).toBe(403);
  });

  it.each(["GET", "HEAD", "OPTIONS"])("%s isn't checked", (method) => {
    expect(originGateResponse(method, "/api/rpc", null, HOST)).toBeNull();
    expect(originGateResponse(method, "/api/rpc", "https://evil.example", HOST)).toBeNull();
  });

  it("leaves Better Auth's routes and pages to their own checks", () => {
    expect(originGateResponse("POST", "/api/auth/sign-in/email", null, HOST)).toBeNull();
    expect(originGateResponse("POST", "/settings", null, HOST)).toBeNull();
  });
});
