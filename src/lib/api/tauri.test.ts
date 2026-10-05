import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
let rejectWith: unknown = null;
/** Stands in for Tauri's `Channel`: the test calls `onmessage` itself. */
class FakeChannel<T> {
  onmessage: (message: T) => void = () => {};
}
vi.mock("@tauri-apps/api/core", () => ({
  invoke: async (...args: unknown[]) => {
    if (rejectWith) throw rejectWith;
    return invoke(...args);
  },
  Channel: FakeChannel,
}));

const {
  activateLicense,
  validateLicense,
  deactivateLicense,
  LicenseError,
  duckdbHelperOffer,
  installDuckdbHelper,
  cancelDuckdbHelperInstall,
  installDuckdbHelperFromFile,
  DuckdbHelperError,
} = await import("./tauri");

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

describe("DuckDB helper commands (desktop DuckDB helper plan, Task 4)", () => {
  beforeEach(() => {
    invoke.mockReset();
    rejectWith = null;
  });

  it("the offer takes no arguments and passes the answer through", async () => {
    const offer = {
      status: "missing",
      version: "2026.10.1",
      size: 11_503_115,
      sizeError: null,
      fromFile: true,
    };
    invoke.mockResolvedValue(offer);
    await expect(duckdbHelperOffer()).resolves.toEqual(offer);
    expect(invoke.mock.calls).toEqual([["duckdb_helper_offer"]]);
  });

  it("the install sends a channel, and its messages reach onProgress", async () => {
    const seen: unknown[] = [];
    invoke.mockImplementation(async (_command: string, args: { channel: FakeChannel<unknown> }) => {
      args.channel.onmessage({ bytes: 10, total: 100 });
      args.channel.onmessage({ bytes: 100, total: 100 });
      return { downloaded: true, pruned: 1 };
    });
    await expect(installDuckdbHelper((p) => seen.push(p))).resolves.toEqual({
      downloaded: true,
      pruned: 1,
    });
    const [command, args] = invoke.mock.calls[0] as [string, Record<string, unknown>];
    expect(command).toBe("duckdb_helper_install");
    expect(Object.keys(args)).toEqual(["channel"]);
    expect(args.channel).toBeInstanceOf(FakeChannel);
    expect(seen).toEqual([
      { bytes: 10, total: 100 },
      { bytes: 100, total: 100 },
    ]);
  });

  it("the install without a listener still sends a channel (the prefetch)", async () => {
    invoke.mockImplementation(async (_command: string, args: { channel: FakeChannel<unknown> }) => {
      args.channel.onmessage({ bytes: 1, total: 2 });
      return { downloaded: false, pruned: 0 };
    });
    await expect(installDuckdbHelper()).resolves.toEqual({ downloaded: false, pruned: 0 });
    const [, args] = invoke.mock.calls[0] as [string, Record<string, unknown>];
    expect(args.channel).toBeInstanceOf(FakeChannel);
  });

  it("cancel and the file install send what their commands take", async () => {
    invoke.mockResolvedValueOnce(true);
    await expect(cancelDuckdbHelperInstall()).resolves.toBe(true);
    invoke.mockResolvedValueOnce({ downloaded: true, pruned: 0 });
    await installDuckdbHelperFromFile("/tmp/seaquel-duckdb-x.gz");
    expect(invoke.mock.calls).toEqual([
      ["duckdb_helper_cancel"],
      ["duckdb_helper_install_file", { path: "/tmp/seaquel-duckdb-x.gz" }],
    ]);
  });

  it("a refusal keeps Core's code and message", async () => {
    rejectWith = { code: "CANCELLED", message: "The DuckDB helper's install was cancelled." };
    const error = await installDuckdbHelper().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(DuckdbHelperError);
    expect((error as InstanceType<typeof DuckdbHelperError>).code).toBe("CANCELLED");
    expect((error as Error).message).toBe("The DuckDB helper's install was cancelled.");
    rejectWith = { code: "WRONG_FILE", message: "This isn't it." };
    const wrong = await installDuckdbHelperFromFile("/x").catch((e: unknown) => e);
    expect((wrong as InstanceType<typeof DuckdbHelperError>).code).toBe("WRONG_FILE");
  });
});
