/**
 * Keyring service for secure credential storage. Three implementations:
 * - Desktop (Tauri): OS-native keychain, through Core (`seaquel-secrets`)
 *   - macOS: Keychain
 *   - Windows: Credential Manager
 *   - Linux: Secret Service (GNOME Keyring, KWallet)
 * - Web (hosted / self-hosted): `VaultKeyringService` — browser-derived VK
 *   encrypts payloads, server stores ciphertext in `user_credentials`. See
 *   `src/lib/services/vault/`.
 * - Demo (in-browser only): no-op — credentials are not persisted.
 */

import { isTauri, isWeb } from "$lib/utils/environment";
import { log } from "$lib/utils/logger";
import { VaultKeyringService } from "$lib/services/vault/vault-keyring";
import { callSecret } from "$lib/storage/rust-client";

export interface KeyringService {
  setDbPassword(connectionId: string, password: string): Promise<void>;
  getDbPassword(connectionId: string): Promise<string | null>;
  deleteDbPassword(connectionId: string): Promise<void>;

  setSshPassword(connectionId: string, password: string): Promise<void>;
  getSshPassword(connectionId: string): Promise<string | null>;
  deleteSshPassword(connectionId: string): Promise<void>;

  setSshKeyPassphrase(connectionId: string, passphrase: string): Promise<void>;
  getSshKeyPassphrase(connectionId: string): Promise<string | null>;
  deleteSshKeyPassphrase(connectionId: string): Promise<void>;

  deleteAllForConnection(connectionId: string): Promise<void>;

  setLicenseKey(key: string): Promise<void>;
  getLicenseKey(): Promise<string | null>;
  deleteLicenseKey(): Promise<void>;

  setAIApiKeyForProvider(id: string, key: string): Promise<void>;
  getAIApiKeyForProvider(id: string): Promise<string | null>;
  deleteAIApiKeyForProvider(id: string): Promise<void>;

  /** Plumbing check — can this service store/retrieve credentials at all? */
  isAvailable(): boolean;

  /**
   * Does a `get*` call complete without user interaction? Desktop's OS
   * keychain is always "unlocked" for the logged-in user; the web vault
   * requires an explicit passphrase unlock, tab-scoped. Startup-time
   * pre-fetch paths gate on this to avoid popping the unlock dialog on
   * every page load.
   */
  isUnlocked(): boolean;
}

/**
 * Desktop: the OS keychain through Core (`core_call` `Secret::*`), under the
 * service `app.seaquel.desktop`, with keys `db:<id>`, `ssh:<id>`,
 * `ssh-key:<id>`, `license-key` and `ai-api-key:<id>`. That's the layout
 * `tauri-plugin-keyring` used, so existing entries read back unchanged.
 *
 * `get*` returns `null` on any failure, as it always has, but logs the error
 * (never the value) so a locked or broken keychain shows up in the log.
 * `delete*` doesn't fail either: a missing entry is already fine in Rust, and
 * other errors are logged. `set*` throws.
 */
class TauriKeyringService implements KeyringService {
  private async set(key: string, value: string): Promise<void> {
    await callSecret({ method: "set", params: { key, value } });
  }

  private async get(key: string): Promise<string | null> {
    try {
      return await callSecret({ method: "get", params: { key } });
    } catch (error) {
      void log.warn(`Keychain read failed for ${key}:`, errorMessage(error));
      return null;
    }
  }

  private async delete(key: string): Promise<void> {
    try {
      await callSecret({ method: "delete", params: { key } });
    } catch (error) {
      void log.warn(`Keychain delete failed for ${key}:`, errorMessage(error));
    }
  }

  setDbPassword(connectionId: string, password: string): Promise<void> {
    return this.set(`db:${connectionId}`, password);
  }
  getDbPassword(connectionId: string): Promise<string | null> {
    return this.get(`db:${connectionId}`);
  }
  deleteDbPassword(connectionId: string): Promise<void> {
    return this.delete(`db:${connectionId}`);
  }

  setSshPassword(connectionId: string, password: string): Promise<void> {
    return this.set(`ssh:${connectionId}`, password);
  }
  getSshPassword(connectionId: string): Promise<string | null> {
    return this.get(`ssh:${connectionId}`);
  }
  deleteSshPassword(connectionId: string): Promise<void> {
    return this.delete(`ssh:${connectionId}`);
  }

  setSshKeyPassphrase(connectionId: string, passphrase: string): Promise<void> {
    return this.set(`ssh-key:${connectionId}`, passphrase);
  }
  getSshKeyPassphrase(connectionId: string): Promise<string | null> {
    return this.get(`ssh-key:${connectionId}`);
  }
  deleteSshKeyPassphrase(connectionId: string): Promise<void> {
    return this.delete(`ssh-key:${connectionId}`);
  }

  async deleteAllForConnection(connectionId: string): Promise<void> {
    await Promise.all([
      this.deleteDbPassword(connectionId),
      this.deleteSshPassword(connectionId),
      this.deleteSshKeyPassphrase(connectionId),
    ]);
  }

  setLicenseKey(key: string): Promise<void> {
    return this.set("license-key", key);
  }
  getLicenseKey(): Promise<string | null> {
    return this.get("license-key");
  }
  deleteLicenseKey(): Promise<void> {
    return this.delete("license-key");
  }

  setAIApiKeyForProvider(id: string, key: string): Promise<void> {
    return this.set(`ai-api-key:${id}`, key);
  }
  getAIApiKeyForProvider(id: string): Promise<string | null> {
    return this.get(`ai-api-key:${id}`);
  }
  deleteAIApiKeyForProvider(id: string): Promise<void> {
    return this.delete(`ai-api-key:${id}`);
  }

  isAvailable(): boolean {
    return true;
  }

  isUnlocked(): boolean {
    // The OS keychain is unlocked for the logged-in user, so `get*` calls
    // complete without further interaction.
    return true;
  }
}

/** The error's message only. A keychain error never carries the value. */
function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * No-op implementation for browser demo mode.
 */
class NoopKeyringService implements KeyringService {
  async setDbPassword(): Promise<void> {}
  async getDbPassword(): Promise<string | null> {
    return null;
  }
  async deleteDbPassword(): Promise<void> {}
  async setSshPassword(): Promise<void> {}
  async getSshPassword(): Promise<string | null> {
    return null;
  }
  async deleteSshPassword(): Promise<void> {}
  async setSshKeyPassphrase(): Promise<void> {}
  async getSshKeyPassphrase(): Promise<string | null> {
    return null;
  }
  async deleteSshKeyPassphrase(): Promise<void> {}
  async deleteAllForConnection(): Promise<void> {}
  async setLicenseKey(): Promise<void> {}
  async getLicenseKey(): Promise<string | null> {
    return null;
  }
  async deleteLicenseKey(): Promise<void> {}
  async setAIApiKeyForProvider(): Promise<void> {}
  async getAIApiKeyForProvider(): Promise<string | null> {
    return null;
  }
  async deleteAIApiKeyForProvider(): Promise<void> {}
  isAvailable(): boolean {
    return false;
  }
  isUnlocked(): boolean {
    return false;
  }
}

let keyringService: KeyringService | null = null;

/**
 * Get the keyring service instance.
 * - Tauri desktop → OS keychain
 * - Web build (hosted / self-hosted) → `VaultKeyringService` (browser-
 *   derived key encrypts, server stores ciphertext)
 * - Anything else (demo) → no-op
 *
 * `isWeb()` resolves at build time from `VITE_BUILD_TARGET` so Vite's
 * tree-shaker drops the unused branches from desktop / demo bundles.
 */
export function getKeyringService(): KeyringService {
  if (keyringService) return keyringService;

  if (isTauri()) {
    keyringService = new TauriKeyringService();
  } else if (isWeb()) {
    keyringService = new VaultKeyringService();
  } else {
    keyringService = new NoopKeyringService();
  }

  return keyringService;
}
