/**
 * Tests for /api/airgap/bundle (POST, DELETE, GET): the Node side.
 *
 * Verification, the bundle row, revocations and the auth rule for uploads
 * run in Rust (`crates/seaquel-server/tests/internal_license.rs` covers
 * every verifier error, the replay, idempotency and subscription checks,
 * and the loose-vs-owner auth). Here `license-client` is mocked and the
 * handler's own work is checked: the CSRF guard, passing Rust's outcome
 * through, mapping its 401/403, and purging the revoked users' sessions in
 * a real temp auth.db.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";

// -- DB redirect must happen first ---------------------------------------
const tmp = mkdtempSync(join(tmpdir(), "seaquel-airgap-bundle-api-"));
process.env.DATA_DIR = tmp;
process.env.SEAQUEL_TRUSTED_ORIGINS = "http://localhost:5173";

vi.mock("$lib/server/license-client", async () => {
  const actual = await vi.importActual<typeof import("$lib/server/license-client")>(
    "$lib/server/license-client",
  );
  return {
    LicenseClientError: actual.LicenseClientError,
    airgapUpload: vi.fn(),
    airgapClear: vi.fn(),
    airgapStatus: vi.fn(),
  };
});

const { openAuthDb } = await import("$lib/server/auth");
const { _resetRateLimit } = await import("$lib/server/rate-limit");
const client = await import("$lib/server/license-client");
const { POST, DELETE, GET } = await import("./+server");

afterAll(() => {
  rmSync(tmp, { recursive: true, force: true });
});

// ---------------------------------------------------------------------------
// Helpers.

const ENVELOPE = '{"payload":"p","sig":"s","pubkey_fingerprint":"f"}';

function makeRequest(body: string | null, method: string, origin = "http://localhost:5173") {
  const init: RequestInit = { method, headers: { origin } };
  if (body !== null) {
    init.body = body;
    (init.headers as Record<string, string>)["Content-Type"] = "application/json";
  }
  return new Request("http://localhost:5173/api/airgap/bundle", init);
}

function makeEvent(request: Request, userId: string | null = null): Parameters<typeof POST>[0] {
  const user = userId ? { id: userId, email: `${userId}@example.test`, name: userId } : null;
  return {
    request,
    getClientAddress: () => "127.0.0.1",
    locals: { user, session: null, tenant: null, licenseState: "unregistered" },
  } as unknown as Parameters<typeof POST>[0];
}

function insertUser(userId: string): void {
  const now = new Date().toISOString();
  openAuthDb()
    .prepare(
      `INSERT INTO "user" (id, email, name, emailVerified, createdAt, updatedAt)
       VALUES (?, ?, ?, 0, ?, ?)`,
    )
    .run(userId, `${userId}@example.test`, userId, now, now);
}

function insertSession(sessionId: string, userId: string): void {
  const now = new Date().toISOString();
  const expires = new Date(Date.now() + 86_400_000).toISOString();
  openAuthDb()
    .prepare(
      `INSERT INTO "session" (id, expiresAt, token, createdAt, updatedAt, userId)
       VALUES (?, ?, ?, ?, ?, ?)`,
    )
    .run(sessionId, expires, `tok_${sessionId}`, now, now, userId);
}

function countSessionsFor(userId: string): number {
  const row = openAuthDb()
    .prepare(`SELECT COUNT(*) AS n FROM "session" WHERE userId = ?`)
    .get(userId) as { n: number };
  return row.n;
}

const ACCEPTED = {
  ok: true,
  unchanged: false,
  tier: "team",
  seats: 3,
  notAfter: 1_800_000_000,
  issuedAt: 1_700_000_000,
  pubkeyFingerprint: "f",
  revokedKeyCount: 0,
  rowsRevoked: 0,
};

beforeEach(() => {
  openAuthDb().prepare(`DELETE FROM "session"`).run();
  openAuthDb().prepare(`DELETE FROM "user"`).run();
  _resetRateLimit();
  vi.mocked(client.airgapUpload).mockReset();
  vi.mocked(client.airgapClear).mockReset();
  vi.mocked(client.airgapStatus).mockReset();
});

// ---------------------------------------------------------------------------
// POST

describe("POST /api/airgap/bundle", () => {
  it("refuses an untrusted origin before reaching Rust", async () => {
    await expect(
      POST(makeEvent(makeRequest(ENVELOPE, "POST", "https://evil.example"))),
    ).rejects.toMatchObject({ status: 403 });
    expect(client.airgapUpload).not.toHaveBeenCalled();
  });

  it("refuses a domain Host's own origin (DNS rebinding) with nothing configured", async () => {
    const request = new Request("http://evil.example:8787/api/airgap/bundle", {
      method: "POST",
      headers: {
        origin: "http://evil.example:8787",
        host: "evil.example:8787",
        "Content-Type": "application/json",
      },
      body: ENVELOPE,
    });
    await expect(POST(makeEvent(request, "u_owner"))).rejects.toMatchObject({ status: 403 });
    expect(client.airgapUpload).not.toHaveBeenCalled();
  });

  it("trusts the install's own origin (Origin naming the Host) at an IP address with nothing configured", async () => {
    vi.mocked(client.airgapUpload).mockResolvedValue({
      status: 200,
      body: ACCEPTED,
      revokedUserIds: [],
    });
    const request = new Request("http://192.168.1.20:8787/api/airgap/bundle", {
      method: "POST",
      headers: {
        origin: "http://192.168.1.20:8787",
        host: "192.168.1.20:8787",
        "Content-Type": "application/json",
      },
      body: ENVELOPE,
    });
    const res = await POST(makeEvent(request, "u_owner"));
    expect(res.status).toBe(200);
  });

  it("sends the body's bytes and the signed-in user, and answers Rust's outcome", async () => {
    vi.mocked(client.airgapUpload).mockResolvedValue({
      status: 200,
      body: ACCEPTED,
      revokedUserIds: [],
    });
    const res = await POST(makeEvent(makeRequest(ENVELOPE, "POST"), "u_owner"));
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual(ACCEPTED);
    const [bytes, user] = vi.mocked(client.airgapUpload).mock.calls[0];
    expect(new TextDecoder().decode(bytes)).toBe(ENVELOPE);
    expect(user).toBe("u_owner");
  });

  it("signed out, the user is null (Rust applies the fresh-install rule)", async () => {
    vi.mocked(client.airgapUpload).mockResolvedValue({
      status: 200,
      body: ACCEPTED,
      revokedUserIds: [],
    });
    await POST(makeEvent(makeRequest(ENVELOPE, "POST")));
    expect(vi.mocked(client.airgapUpload).mock.calls[0][1]).toBeNull();
  });

  it.each([
    [400, { ok: false, error: "malformed_envelope" }],
    [400, { ok: false, error: "untrusted_signer" }],
    [409, { ok: false, error: "bundle_older_than_current" }],
    [409, { ok: false, error: "subscription_mismatch" }],
  ])("passes a %i refusal through", async (status, body) => {
    vi.mocked(client.airgapUpload).mockResolvedValue({ status, body, revokedUserIds: [] });
    const res = await POST(makeEvent(makeRequest(ENVELOPE, "POST")));
    expect(res.status).toBe(status);
    expect(await res.json()).toEqual(body);
  });

  it.each([
    [401, "UNAUTHORIZED", "must be signed in"],
    [403, "FORBIDDEN", "owner only"],
  ])("maps Rust's %i to the same SvelteKit error", async (status, code, message) => {
    vi.mocked(client.airgapUpload).mockRejectedValue(
      new client.LicenseClientError(code, message, status),
    );
    await expect(POST(makeEvent(makeRequest(ENVELOPE, "POST")))).rejects.toMatchObject({
      status,
      body: { message },
    });
  });

  it("other failures propagate (a 500)", async () => {
    vi.mocked(client.airgapUpload).mockRejectedValue(
      new client.LicenseClientError("LICENSE_DB_ERROR", "disk full", 500),
    );
    await expect(POST(makeEvent(makeRequest(ENVELOPE, "POST")))).rejects.toBeInstanceOf(
      client.LicenseClientError,
    );
  });

  it("purges the sessions of the users Rust revoked, and only theirs", async () => {
    insertUser("u_alpha");
    insertUser("u_beta");
    insertSession("sess_alpha_1", "u_alpha");
    insertSession("sess_alpha_2", "u_alpha");
    insertSession("sess_beta_1", "u_beta");
    vi.mocked(client.airgapUpload).mockResolvedValue({
      status: 200,
      body: { ...ACCEPTED, revokedKeyCount: 1, rowsRevoked: 1 },
      revokedUserIds: ["u_beta"],
    });

    const res = await POST(makeEvent(makeRequest(ENVELOPE, "POST"), "u_alpha"));
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.rowsRevoked).toBe(1);
    expect(body).not.toHaveProperty("revokedUserIds");

    expect(countSessionsFor("u_beta")).toBe(0);
    expect(countSessionsFor("u_alpha")).toBe(2);
  });
});

describe("POST /api/airgap/bundle — a failed purge", () => {
  it("still answers Rust's body, and a retry purges", async () => {
    insertUser("u_beta");
    insertSession("sess_beta_1", "u_beta");
    // Make the purge fail the way a locked or broken table would.
    openAuthDb().exec(
      `CREATE TRIGGER no_purge BEFORE DELETE ON "session" BEGIN SELECT RAISE(ABORT, 'boom'); END`,
    );
    vi.mocked(client.airgapUpload).mockResolvedValue({
      status: 200,
      body: { ...ACCEPTED, revokedKeyCount: 1, rowsRevoked: 1 },
      revokedUserIds: ["u_beta"],
    });
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const res = await POST(makeEvent(makeRequest(ENVELOPE, "POST"), "u_alpha"));
    expect(res.status).toBe(200);
    expect((await res.json()).rowsRevoked).toBe(1);
    expect(errors).toHaveBeenCalled();
    errors.mockRestore();
    expect(countSessionsFor("u_beta")).toBe(1);

    // The retry: Rust answers `unchanged` and lists every revoked user.
    openAuthDb().exec(`DROP TRIGGER no_purge`);
    vi.mocked(client.airgapUpload).mockResolvedValue({
      status: 200,
      body: { ...ACCEPTED, unchanged: true, revokedKeyCount: 1, rowsRevoked: 0 },
      revokedUserIds: ["u_beta"],
    });
    const retry = await POST(makeEvent(makeRequest(ENVELOPE, "POST"), "u_alpha"));
    expect((await retry.json()).unchanged).toBe(true);
    expect(countSessionsFor("u_beta")).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// DELETE

describe("DELETE /api/airgap/bundle", () => {
  it("requires authentication before reaching Rust", async () => {
    await expect(DELETE(makeEvent(makeRequest(null, "DELETE")))).rejects.toMatchObject({
      status: 401,
    });
    expect(client.airgapClear).not.toHaveBeenCalled();
  });

  it("maps Rust's owner check to 403", async () => {
    vi.mocked(client.airgapClear).mockRejectedValue(
      new client.LicenseClientError("FORBIDDEN", "owner only", 403),
    );
    await expect(DELETE(makeEvent(makeRequest(null, "DELETE"), "u_member"))).rejects.toMatchObject({
      status: 403,
    });
  });

  it("clears as the owner", async () => {
    vi.mocked(client.airgapClear).mockResolvedValue(undefined);
    const res = await DELETE(makeEvent(makeRequest(null, "DELETE"), "u_owner"));
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
    expect(client.airgapClear).toHaveBeenCalledWith("u_owner");
  });
});

// ---------------------------------------------------------------------------
// GET

const STATUS = {
  mode: "airgap" as const,
  lastValidatedAt: 1,
  graceUntil: 2,
  member: { isOwner: true, revoked: false },
  bundle: null,
};

describe("GET /api/airgap/bundle", () => {
  it("requires authentication", async () => {
    await expect(GET(makeEvent(makeRequest(null, "GET")))).rejects.toMatchObject({ status: 401 });
  });

  it("requires bound membership", async () => {
    vi.mocked(client.airgapStatus).mockResolvedValue({ ...STATUS, member: null });
    await expect(GET(makeEvent(makeRequest(null, "GET"), "u_lonely"))).rejects.toMatchObject({
      status: 403,
    });
    vi.mocked(client.airgapStatus).mockResolvedValue({
      ...STATUS,
      member: { isOwner: false, revoked: true },
    });
    await expect(GET(makeEvent(makeRequest(null, "GET"), "u_revoked"))).rejects.toMatchObject({
      status: 403,
    });
  });

  it("returns { present: false } when no bundle is loaded", async () => {
    vi.mocked(client.airgapStatus).mockResolvedValue(STATUS);
    const res = await GET(makeEvent(makeRequest(null, "GET"), "u_o"));
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ present: false });
  });

  it("returns the full status block", async () => {
    const bundle = {
      tier: "team",
      seats: 3,
      notAfter: 1_800_000_000,
      issuedAt: 1_700_000_000,
      importedAt: 1_750_000_000,
      pubkeyFingerprint: "f",
      payloadSha256: "abc",
      revokedKeyCount: 0,
      expired: false,
    };
    vi.mocked(client.airgapStatus).mockResolvedValue({ ...STATUS, bundle });
    const res = await GET(makeEvent(makeRequest(null, "GET"), "u_o"));
    expect(await res.json()).toEqual({ present: true, ...bundle });
    expect(client.airgapStatus).toHaveBeenCalledWith("u_o");
  });
});

// Probe review: the body-limit hook fails a chunked body past the limit
// with a 413 HttpError while the route reads it; that must stay a 413.
describe("POST an oversized bundle", () => {
  it("rethrows the hook's 413 instead of a 400", async () => {
    const { error } = await import("@sveltejs/kit");
    let tooLarge: unknown;
    try {
      error(413, "Payload Too Large");
    } catch (e) {
      tooLarge = e;
    }
    const request = new Request("http://localhost:5173/api/airgap/bundle", {
      method: "POST",
      headers: { origin: "http://localhost:5173", "Content-Type": "application/json" },
      body: new ReadableStream({ start: (c) => c.error(tooLarge) }),
      duplex: "half",
    } as RequestInit);
    await expect(POST(makeEvent(request))).rejects.toMatchObject({ status: 413 });
  });
});
