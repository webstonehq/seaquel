/**
 * SSH tunnels, owned by the Rust core: `Ssh::Open` and `Ssh::Close` sent as
 * workspace calls through `core_call`. Desktop only; web and the demo have
 * no SSH (`isFeatureEnabled("sshTunnels")`).
 *
 * Errors are `CoreCallError`s carrying the Rust codes (`UNKNOWN_HOST_KEY`,
 * `HOST_KEY_MISMATCH`, `AUTH_FAILED`, `TUNNEL_NOT_FOUND`, …); the message
 * reads `"CODE: message"`.
 */

import { sshHostKeyPromptStore } from "$lib/stores/ssh-host-key-prompt.svelte";
import { CoreCallError, encodeCoreRequest, tauriCoreTransport } from "$lib/storage/rust-client";
import type { CoreTransport } from "$lib/storage/rust-client";
import type { CoreResponse } from "$lib/types/generated/CoreResponse";
import type { SshRequest } from "$lib/types/generated/SshRequest";
import type { SshResponse } from "$lib/types/generated/SshResponse";

export interface TunnelConfig {
  sshHost: string;
  sshPort: number;
  sshUsername: string;
  authMethod: "password" | "key";
  password?: string;
  keyPath?: string;
  keyPassphrase?: string;
  remoteHost: string;
  remotePort: number;
  /**
   * The `SHA256:…` fingerprint the user approved in the host key prompt, set
   * only by the retry after it. Rust records the key only if the server
   * presents exactly this one.
   */
  trustHostKey?: string;
}

export interface TunnelResult {
  tunnelId: string;
  localPort: number;
}

type SshMethod = SshRequest["method"];
type SshResult<M extends SshMethod> = Extract<SshResponse, { method: M }>["result"];

/** One SSH call. Never echoes the request: it can hold a password. */
async function callSsh<M extends SshMethod>(
  request: Extract<SshRequest, { method: M }>,
  transport: CoreTransport,
): Promise<SshResult<M>> {
  let response: CoreResponse;
  try {
    response = (await transport(
      encodeCoreRequest({ method: "ssh", params: request }),
    )) as CoreResponse;
  } catch (error) {
    if (error instanceof Error) throw error;
    throw new CoreCallError({ code: "UNKNOWN", message: String(error) });
  }
  if (response?.method !== "ssh" || response.result?.method !== request.method) {
    throw new CoreCallError({
      code: "PROTOCOL_ERROR",
      message: `expected an ssh ${request.method} response`,
    });
  }
  return response.result.result as SshResult<M>;
}

export async function createSshTunnel(
  config: TunnelConfig,
  transport: CoreTransport = tauriCoreTransport,
): Promise<TunnelResult> {
  const { tunnelId, localPort } = await callSsh(
    {
      method: "open",
      params: {
        config: {
          sshHost: config.sshHost,
          sshPort: config.sshPort,
          sshUsername: config.sshUsername,
          authMethod: config.authMethod,
          password: config.password,
          keyPath: config.keyPath,
          keyPassphrase: config.keyPassphrase,
          remoteHost: config.remoteHost,
          remotePort: config.remotePort,
          trustHostKey: config.trustHostKey,
        },
      },
    },
    transport,
  );
  return { tunnelId, localPort };
}

/** The Rust error code, if `error` carries one (`CoreCallError`). */
function tunnelErrorCode(error: unknown): string | null {
  if (typeof error === "object" && error !== null && "code" in error) {
    const { code } = error as { code?: unknown };
    if (typeof code === "string") return code;
  }
  return null;
}

/** Extracts the `SHA256:...` fingerprint the Rust error embeds in its message. */
function fingerprintFromError(error: unknown): string {
  if (typeof error !== "object" || error === null || !("message" in error)) return "";
  const { message } = error as { message?: unknown };
  if (typeof message !== "string") return "";
  return message.match(/SHA256:[A-Za-z0-9+/=]+/)?.[0] ?? "";
}

/**
 * Creates a tunnel, showing the trust-on-first-use prompt when the server's
 * host key isn't in `~/.ssh/known_hosts` yet.
 *
 * A key that no longer matches the recorded one (`HOST_KEY_MISMATCH`) is never
 * offered for trust — it propagates so the caller surfaces the error.
 */
export async function createSshTunnelWithHostKeyCheck(
  config: TunnelConfig,
  transport: CoreTransport = tauriCoreTransport,
): Promise<TunnelResult> {
  try {
    return await createSshTunnel(config, transport);
  } catch (error) {
    if (tunnelErrorCode(error) !== "UNKNOWN_HOST_KEY") throw error;

    // Without a fingerprint there is nothing the user could check or Rust
    // could pin, so don't offer trust.
    const fingerprint = fingerprintFromError(error);
    if (!fingerprint) throw error;

    const trusted = await sshHostKeyPromptStore.prompt(config.sshHost, config.sshPort, fingerprint);
    if (!trusted) throw error;

    // Pin the key the user saw: if the retry meets a different one, Rust
    // refuses it (UNKNOWN_HOST_KEY with its fingerprint) and records nothing.
    return createSshTunnel({ ...config, trustHostKey: fingerprint }, transport);
  }
}

/**
 * Closes a tunnel. Connections still running through it are cut, so close
 * the database connection first.
 */
export async function closeSshTunnel(
  tunnelId: string,
  transport: CoreTransport = tauriCoreTransport,
): Promise<void> {
  await callSsh({ method: "close", params: { tunnelId } }, transport);
}
