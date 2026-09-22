import { invoke } from "@tauri-apps/api/core";
import { sshHostKeyPromptStore } from "$lib/stores/ssh-host-key-prompt.svelte";

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
  /** Set only by the retry after the user accepted the host key prompt. */
  trustNewHostKey?: boolean;
}

export interface TunnelResult {
  tunnelId: string;
  localPort: number;
}

export async function createSshTunnel(config: TunnelConfig): Promise<TunnelResult> {
  const result = await invoke<{ tunnel_id: string; local_port: number }>("create_ssh_tunnel", {
    config: {
      ssh_host: config.sshHost,
      ssh_port: config.sshPort,
      ssh_username: config.sshUsername,
      auth_method: config.authMethod,
      password: config.password,
      key_path: config.keyPath,
      key_passphrase: config.keyPassphrase,
      remote_host: config.remoteHost,
      remote_port: config.remotePort,
      trust_new_host_key: config.trustNewHostKey ?? false,
    },
  });

  return {
    tunnelId: result.tunnel_id,
    localPort: result.local_port,
  };
}

/** Shape of the error `create_ssh_tunnel` rejects with (Rust's `TunnelError`). */
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
export async function createSshTunnelWithHostKeyCheck(config: TunnelConfig): Promise<TunnelResult> {
  try {
    return await createSshTunnel(config);
  } catch (error) {
    if (tunnelErrorCode(error) !== "UNKNOWN_HOST_KEY") throw error;

    const trusted = await sshHostKeyPromptStore.prompt(
      config.sshHost,
      config.sshPort,
      fingerprintFromError(error),
    );
    if (!trusted) throw error;

    return createSshTunnel({ ...config, trustNewHostKey: true });
  }
}

export async function closeSshTunnel(tunnelId: string): Promise<void> {
  await invoke("close_ssh_tunnel", { tunnelId });
}
