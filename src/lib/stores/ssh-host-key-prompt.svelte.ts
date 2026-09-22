/**
 * Reactive store for the SSH trust-on-first-use host key prompt.
 *
 * The tunnel helper in `$lib/services/ssh-tunnel` calls `prompt()` when the
 * Rust side reports `UNKNOWN_HOST_KEY`; answering "Connect" retries the tunnel
 * with `trustNewHostKey`, which records the key in `~/.ssh/known_hosts`.
 * A changed key (`HOST_KEY_MISMATCH`) never reaches here — it always fails.
 */

class SshHostKeyPromptStore {
  open = $state(false);
  host = $state("");
  port = $state(22);
  fingerprint = $state("");

  private _resolve: ((trusted: boolean) => void) | null = null;

  /** Show the prompt and wait. Resolves true when the user trusts the key. */
  prompt(host: string, port: number, fingerprint: string): Promise<boolean> {
    this.host = host;
    this.port = port;
    this.fingerprint = fingerprint;
    this.open = true;

    return new Promise<boolean>((resolve) => {
      this._resolve = resolve;
    });
  }

  /** Resolve the prompt with the user's answer (called by the dialog component). */
  resolve(trusted: boolean): void {
    this.open = false;
    this._resolve?.(trusted);
    this._resolve = null;
  }
}

export const sshHostKeyPromptStore = new SshHostKeyPromptStore();
