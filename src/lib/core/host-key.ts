/**
 * The SSH trust-on-first-use prompt around a Core `connect` or `test`.
 *
 * Core opens the tunnel. When the SSH server's key isn't in known_hosts
 * yet, the call fails with `UNKNOWN_HOST_KEY`, its message carrying the
 * key's `SHA256:…` fingerprint. The user is asked whether to trust it; if
 * they do, the call is made again with that fingerprint as `trustHostKey`,
 * and Core records the key only if the server presents that same one. A
 * changed key (`HOST_KEY_MISMATCH`) is never offered for trust.
 */

import { sshHostKeyPromptStore } from "$lib/stores/ssh-host-key-prompt.svelte";
import { errorCode, UNKNOWN_HOST_KEY } from "./client";

/** The SSH server the prompt names. */
export interface SshServer {
  host: string;
  port: number;
}

/** The `SHA256:…` fingerprint Core puts in an `UNKNOWN_HOST_KEY` message. */
export function fingerprintFromError(error: unknown): string {
  if (typeof error !== "object" || error === null || !("message" in error)) return "";
  const { message } = error as { message?: unknown };
  if (typeof message !== "string") return "";
  return message.match(/SHA256:[A-Za-z0-9+/=]+/)?.[0] ?? "";
}

/**
 * Run `attempt` with no trusted key; on `UNKNOWN_HOST_KEY`, prompt, and run
 * it once more with the fingerprint the user approved. Any other failure,
 * a missing fingerprint, or a "no" rethrows the original error.
 */
export async function withHostKeyPrompt<T>(
  attempt: (trustHostKey?: string) => Promise<T>,
  server: SshServer,
): Promise<T> {
  try {
    return await attempt();
  } catch (error) {
    if (errorCode(error) !== UNKNOWN_HOST_KEY) throw error;
    const fingerprint = fingerprintFromError(error);
    if (!fingerprint) throw error;
    const trusted = await sshHostKeyPromptStore.prompt(server.host, server.port, fingerprint);
    if (!trusted) throw error;
    return attempt(fingerprint);
  }
}
