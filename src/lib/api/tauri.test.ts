import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
let rejectWith: unknown = null;
vi.mock("@tauri-apps/api/core", () => ({
  invoke: async (...args: unknown[]) => {
    if (rejectWith) throw rejectWith;
    return invoke(...args);
  },
}));

const { activateLicense, validateLicense, deactivateLicense, LicenseError } =
  await import("./tauri");

const LICENSE = {
  id: "lic_1",
  status: "active",
  key: "SQ-KEY",
  tier: "business",
  activation: 1,
  activation_limit: 3,
  expires_at: null,
  instance_id: "inst_1",
};

function sentBody(): unknown {
  const [command, body] = invoke.mock.calls[0] as [string, Uint8Array];
  expect(command).toBe("core_call");
  expect(body).toBeInstanceOf(Uint8Array);
  return JSON.parse(new TextDecoder().decode(body));
}

describe("desktop license calls", () => {
  beforeEach(() => {
    invoke.mockReset();
    rejectWith = null;
  });

  it("activate sends a license call as bytes, method first", async () => {
    invoke.mockResolvedValue({
      method: "license",
      result: { method: "activate", result: LICENSE },
    });
    await expect(activateLicense("SQ-KEY", "host__me")).resolves.toEqual(LICENSE);
    const [, body] = invoke.mock.calls[0] as [string, Uint8Array];
    expect(new TextDecoder().decode(body)).toBe(
      '{"method":"license","params":{"method":"activate","params":{"key":"SQ-KEY","instanceName":"host__me"}}}',
    );
  });

  it("validate and deactivate send the instance id", async () => {
    invoke.mockResolvedValueOnce({
      method: "license",
      result: { method: "validate", result: LICENSE },
    });
    await validateLicense("SQ-KEY", "inst_1");
    expect(sentBody()).toEqual({
      method: "license",
      params: { method: "validate", params: { key: "SQ-KEY", instanceId: "inst_1" } },
    });
    invoke.mockReset();
    invoke.mockResolvedValueOnce({
      method: "license",
      result: { method: "deactivate", result: LICENSE },
    });
    await deactivateLicense("SQ-KEY", "inst_1");
    expect(sentBody()).toEqual({
      method: "license",
      params: { method: "deactivate", params: { key: "SQ-KEY", instanceId: "inst_1" } },
    });
  });

  it("an error keeps the server's message as is, as the Tauri commands did", async () => {
    // A plain object, as Tauri rejects with the command's serialized error.
    // Thrown from a non-mock function: vitest reports a plain-object
    // rejection returned by a `vi.fn` as unhandled.
    rejectWith = {
      code: "ACTIVATION_ERROR",
      message: "License activation failed (400 Bad Request): nope",
    };
    const error = await activateLicense("SQ-KEY", "n").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(LicenseError);
    expect((error as InstanceType<typeof LicenseError>).message).toBe(
      "License activation failed (400 Bad Request): nope",
    );
    expect((error as InstanceType<typeof LicenseError>).code).toBe("ACTIVATION_ERROR");
  });

  it("a response for another call is a protocol error", async () => {
    invoke.mockResolvedValue({
      method: "license",
      result: { method: "activate", result: LICENSE },
    });
    const error = await validateLicense("SQ-KEY", "i").catch((e: unknown) => e);
    expect((error as InstanceType<typeof LicenseError>).code).toBe("PROTOCOL_ERROR");
  });
});
