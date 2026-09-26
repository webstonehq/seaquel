import { beforeEach, describe, expect, it, vi } from "vitest";

const prompt = vi.fn<(host: string, port: number, fingerprint: string) => Promise<boolean>>();
vi.mock("$lib/stores/ssh-host-key-prompt.svelte", () => ({
  sshHostKeyPromptStore: { prompt: (...args: [string, number, string]) => prompt(...args) },
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { CoreCallError, type CoreTransport } from "$lib/storage/rust-client";
import {
  closeSshTunnel,
  createSshTunnel,
  createSshTunnelWithHostKeyCheck,
  type TunnelConfig,
} from "./ssh-tunnel";

const config: TunnelConfig = {
  sshHost: "bastion.example",
  sshPort: 2222,
  sshUsername: "me",
  authMethod: "password",
  password: "pw-SECRET",
  remoteHost: "db",
  remotePort: 5432,
};

const FINGERPRINT = "SHA256:abcDEF123+/=";

/** A transport that records request bodies and answers from `reply`. */
function fakeTransport(reply: (request: any) => unknown) {
  const bodies: string[] = [];
  const transport: CoreTransport = async (body) => {
    const text = new TextDecoder().decode(body);
    bodies.push(text);
    return reply(JSON.parse(text));
  };
  return { transport, bodies };
}

const opened = (tunnelId = "tunnel-1", localPort = 54321) => ({
  method: "ssh",
  result: { method: "open", result: { tunnelId, localPort } },
});

const unknownHostKey = () =>
  new CoreCallError({
    code: "UNKNOWN_HOST_KEY",
    message: `The host key for bastion.example:2222 is not in known_hosts.\nFingerprint: ${FINGERPRINT}`,
  });

beforeEach(() => {
  prompt.mockReset();
});

describe("createSshTunnel", () => {
  it("sends Ssh::Open with method before params and no trusted key", async () => {
    const { transport, bodies } = fakeTransport(() => opened());
    const result = await createSshTunnel(config, transport);

    expect(result).toEqual({ tunnelId: "tunnel-1", localPort: 54321 });
    expect(bodies).toHaveLength(1);
    expect(bodies[0]).toMatch(/^\{"method":"ssh","params":\{"method":"open","params":/);
    expect(JSON.parse(bodies[0]).params.params.config).toEqual({
      sshHost: "bastion.example",
      sshPort: 2222,
      sshUsername: "me",
      authMethod: "password",
      password: "pw-SECRET",
      remoteHost: "db",
      remotePort: 5432,
    });
  });

  it("rejects a response for another method without echoing it", async () => {
    const { transport } = fakeTransport(() => ({
      method: "ssh",
      result: { method: "close", result: null },
    }));
    const error = await createSshTunnel(config, transport).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreCallError);
    expect((error as CoreCallError).code).toBe("PROTOCOL_ERROR");
  });
});

describe("closeSshTunnel", () => {
  it("sends Ssh::Close with the tunnel id", async () => {
    const { transport, bodies } = fakeTransport(() => ({
      method: "ssh",
      result: { method: "close", result: null },
    }));
    await closeSshTunnel("tunnel-7", transport);
    expect(bodies).toEqual([
      '{"method":"ssh","params":{"method":"close","params":{"tunnelId":"tunnel-7"}}}',
    ]);
  });

  it("keeps the Rust error code", async () => {
    const transport: CoreTransport = async () => {
      throw new CoreCallError({ code: "TUNNEL_NOT_FOUND", message: "Tunnel not found: tunnel-7" });
    };
    await expect(closeSshTunnel("tunnel-7", transport)).rejects.toMatchObject({
      code: "TUNNEL_NOT_FOUND",
    });
  });
});

describe("createSshTunnelWithHostKeyCheck", () => {
  it("prompts with the fingerprint on UNKNOWN_HOST_KEY and retries pinning it", async () => {
    let calls = 0;
    const { transport, bodies } = fakeTransport(() => {
      calls += 1;
      if (calls === 1) throw unknownHostKey();
      return opened("tunnel-2", 50000);
    });
    prompt.mockResolvedValue(true);

    const result = await createSshTunnelWithHostKeyCheck(config, transport);

    expect(prompt).toHaveBeenCalledWith("bastion.example", 2222, FINGERPRINT);
    expect(result).toEqual({ tunnelId: "tunnel-2", localPort: 50000 });
    expect(bodies.map((b) => JSON.parse(b).params.params.config.trustHostKey)).toEqual([
      undefined,
      FINGERPRINT,
    ]);
  });

  it("rethrows when the user doesn't trust the key", async () => {
    const { transport, bodies } = fakeTransport(() => {
      throw unknownHostKey();
    });
    prompt.mockResolvedValue(false);
    await expect(createSshTunnelWithHostKeyCheck(config, transport)).rejects.toMatchObject({
      code: "UNKNOWN_HOST_KEY",
    });
    expect(bodies).toHaveLength(1);
  });

  it("doesn't offer trust when the error has no fingerprint", async () => {
    const { transport, bodies } = fakeTransport(() => {
      throw new CoreCallError({ code: "UNKNOWN_HOST_KEY", message: "not in known_hosts" });
    });
    await expect(createSshTunnelWithHostKeyCheck(config, transport)).rejects.toMatchObject({
      code: "UNKNOWN_HOST_KEY",
    });
    expect(prompt).not.toHaveBeenCalled();
    expect(bodies).toHaveLength(1);
  });

  it("surfaces the retry's refusal when the server shows a different key", async () => {
    const other = "SHA256:otherKEY999";
    let calls = 0;
    const { transport } = fakeTransport(() => {
      calls += 1;
      if (calls === 1) throw unknownHostKey();
      throw new CoreCallError({
        code: "UNKNOWN_HOST_KEY",
        message: `not the approved key.\nFingerprint: ${other}`,
      });
    });
    prompt.mockResolvedValue(true);
    await expect(createSshTunnelWithHostKeyCheck(config, transport)).rejects.toMatchObject({
      code: "UNKNOWN_HOST_KEY",
    });
    expect(prompt).toHaveBeenCalledTimes(1);
    expect(calls).toBe(2);
  });

  it("never offers HOST_KEY_MISMATCH for trust", async () => {
    const { transport, bodies } = fakeTransport(() => {
      throw new CoreCallError({
        code: "HOST_KEY_MISMATCH",
        message: `does not match.\nFingerprint: ${FINGERPRINT}`,
      });
    });
    await expect(createSshTunnelWithHostKeyCheck(config, transport)).rejects.toMatchObject({
      code: "HOST_KEY_MISMATCH",
    });
    expect(prompt).not.toHaveBeenCalled();
    expect(bodies).toHaveLength(1);
  });

  it("passes other errors straight through", async () => {
    const { transport } = fakeTransport(() => {
      throw new CoreCallError({ code: "AUTH_FAILED", message: "Authentication failed" });
    });
    const error = await createSshTunnelWithHostKeyCheck(config, transport).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreCallError);
    expect((error as CoreCallError).message).toBe("AUTH_FAILED: Authentication failed");
    expect(prompt).not.toHaveBeenCalled();
  });
});
