/**
 * Keyring service for secure credential storage. Three implementations:
 * - Desktop (Tauri): OS-native keychain
 *   - macOS: Keychain
 *   - Windows: Credential Manager
 *   - Linux: Secret Service (GNOME Keyring, KWallet)
 * - Web (hosted / self-hosted): `VaultKeyringService` — browser-derived VK
 *   encrypts payloads, server stores ciphertext in `user_credentials`. See
 *   `src/lib/services/vault/`.
 * - Demo (in-browser only): no-op — credentials are not persisted.
 */

import { isTauri, isWeb } from "$lib/utils/environment";
import { VaultKeyringService } from "$lib/services/vault/vault-keyring";

const SERVICE = "app.seaquel.desktop";

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

  setAIApiKey(key: string): Promise<void>;
  getAIApiKey(): Promise<string | null>;
  deleteAIApiKey(): Promise<void>;

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
 * Tauri implementation using the native keyring plugin.
 */
class TauriKeyringService implements KeyringService {
  private keyringApi: typeof import("tauri-plugin-keyring-api") | null = null;
  private initPromise: Promise<void> | null = null;

  private async init(): Promise<void> {
    if (this.keyringApi) return;
    if (this.initPromise) return this.initPromise;

    this.initPromise = import("tauri-plugin-keyring-api").then((api) => {
      this.keyringApi = api;
    });

    return this.initPromise;
  }

  async setDbPassword(connectionId: string, password: string): Promise<void> {
    await this.init();
    await this.keyringApi!.setPassword(SERVICE, `db:${connectionId}`, password);
  }

  async getDbPassword(connectionId: string): Promise<string | null> {
    await this.init();
    try {
      return await this.keyringApi!.getPassword(SERVICE, `db:${connectionId}`);
    } catch {
      // Entry doesn't exist or keychain error
      return null;
    }
  }

  async deleteDbPassword(connectionId: string): Promise<void> {
    await this.init();
    try {
      await this.keyringApi!.deletePassword(SERVICE, `db:${connectionId}`);
    } catch {
      // Ignore - entry may not exist
    }
  }

  async setSshPassword(connectionId: string, password: string): Promise<void> {
    await this.init();
    await this.keyringApi!.setPassword(SERVICE, `ssh:${connectionId}`, password);
  }

  async getSshPassword(connectionId: string): Promise<string | null> {
    await this.init();
    try {
      return await this.keyringApi!.getPassword(SERVICE, `ssh:${connectionId}`);
    } catch {
      return null;
    }
  }

  async deleteSshPassword(connectionId: string): Promise<void> {
    await this.init();
    try {
      await this.keyringApi!.deletePassword(SERVICE, `ssh:${connectionId}`);
    } catch {
      // Ignore
    }
  }

  async setSshKeyPassphrase(connectionId: string, passphrase: string): Promise<void> {
    await this.init();
    await this.keyringApi!.setPassword(SERVICE, `ssh-key:${connectionId}`, passphrase);
  }

  async getSshKeyPassphrase(connectionId: string): Promise<string | null> {
    await this.init();
    try {
      return await this.keyringApi!.getPassword(SERVICE, `ssh-key:${connectionId}`);
    } catch {
      return null;
    }
  }

  async deleteSshKeyPassphrase(connectionId: string): Promise<void> {
    await this.init();
    try {
      await this.keyringApi!.deletePassword(SERVICE, `ssh-key:${connectionId}`);
    } catch {
      // Ignore
    }
  }

  async deleteAllForConnection(connectionId: string): Promise<void> {
    await Promise.all([
      this.deleteDbPassword(connectionId),
      this.deleteSshPassword(connectionId),
      this.deleteSshKeyPassphrase(connectionId),
    ]);
  }

  async setLicenseKey(key: string): Promise<void> {
    await this.init();
    await this.keyringApi!.setPassword(SERVICE, "license-key", key);
  }

  async getLicenseKey(): Promise<string | null> {
    await this.init();
    try {
      return await this.keyringApi!.getPassword(SERVICE, "license-key");
    } catch {
      return null;
    }
  }

  async deleteLicenseKey(): Promise<void> {
    await this.init();
    try {
      await this.keyringApi!.deletePassword(SERVICE, "license-key");
    } catch {
      // Ignore - entry may not exist
    }
  }

  async setAIApiKey(key: string): Promise<void> {
    await this.init();
    await this.keyringApi!.setPassword(SERVICE, "ai-api-key", key);
  }

  async getAIApiKey(): Promise<string | null> {
    await this.init();
    try {
      return await this.keyringApi!.getPassword(SERVICE, "ai-api-key");
    } catch {
      return null;
    }
  }

  async deleteAIApiKey(): Promise<void> {
    await this.init();
    try {
      await this.keyringApi!.deletePassword(SERVICE, "ai-api-key");
    } catch {
      // Ignore - entry may not exist
    }
  }

  async setAIApiKeyForProvider(id: string, key: string): Promise<void> {
    await this.init();
    await this.keyringApi!.setPassword(SERVICE, `ai-api-key:${id}`, key);
  }

  async getAIApiKeyForProvider(id: string): Promise<string | null> {
    await this.init();
    try {
      return await this.keyringApi!.getPassword(SERVICE, `ai-api-key:${id}`);
    } catch {
      return null;
    }
  }

  async deleteAIApiKeyForProvider(id: string): Promise<void> {
    await this.init();
    try {
      await this.keyringApi!.deletePassword(SERVICE, `ai-api-key:${id}`);
    } catch {
      // Ignore
    }
  }

  isAvailable(): boolean {
    return true;
  }

  isUnlocked(): boolean {
    // OS keychain is unlocked for the logged-in user — `get*` calls
    // complete synchronously without further interaction.
    return true;
  }
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
  async setAIApiKey(): Promise<void> {}
  async getAIApiKey(): Promise<string | null> {
    return null;
  }
  async deleteAIApiKey(): Promise<void> {}
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
