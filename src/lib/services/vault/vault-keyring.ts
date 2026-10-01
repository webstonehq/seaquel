/**
 * Third implementation of `KeyringService`, used only in the web (hosted or
 * self-hosted) build. Desktop keeps using `TauriKeyringService`; demo keeps
 * using `NoopKeyringService`.
 *
 * Secrets flow through the vault: every `set*` derives/uses the cached VK
 * to encrypt the plaintext, every `get*` decrypts a previously stored
 * ciphertext. The `Vault` object owns the VK and the setup/unlock dance —
 * this service is just the KeyringService-shaped adapter around it.
 *
 * `set*` methods **block** on setup/unlock (the UI gate dialog will pop up
 * on web when `vault.waitersPending` turns true). `get*` methods behave
 * the same — returning null would be indistinguishable from "no credential
 * stored," which is a different state.
 *
 * If the user cancels a dialog, the pending call rejects with
 * `VaultCancelledError`. Callers already handle keyring errors (see
 * `connection-manager.svelte.ts`).
 */
import type { KeyringService } from "$lib/services/keyring";
import { getStorage } from "$lib/storage";
import { decryptToString, encrypt, fromBase64, toBase64 } from "./crypto";
import { getVault, type Vault } from "./vault-state.svelte";

type Scope = "db" | "ssh" | "ssh-key" | "license" | "ai-api-key-provider";

const LICENSE_KEY_ID = "";

export class VaultKeyringService implements KeyringService {
  constructor(private readonly vault: Vault = getVault()) {}

  private async encryptFor(plaintext: string): Promise<{ nonce: string; ciphertext: string }> {
    const key = await this.vault.ensureUnlocked();
    const blob = await encrypt(key, plaintext);
    return { nonce: toBase64(blob.nonce), ciphertext: toBase64(blob.ciphertext) };
  }

  private async setSecret(scope: Scope, id: string, plaintext: string): Promise<void> {
    const { nonce, ciphertext } = await this.encryptFor(plaintext);
    await getStorage().userCredentials.save({
      scope,
      key: id,
      nonce,
      ciphertext,
      updatedAt: new Date().toISOString(),
    });
  }

  private async getSecret(scope: Scope, id: string): Promise<string | null> {
    const row = await getStorage().userCredentials.load(scope, id);
    if (!row) return null;
    const key = await this.vault.ensureUnlocked();
    try {
      return await decryptToString(key, {
        nonce: fromBase64(row.nonce),
        ciphertext: fromBase64(row.ciphertext),
      });
    } catch {
      // Stored ciphertext can't be decrypted with the current VK — usually
      // means the user reset their vault or the row is corrupt. Best we
      // can do is surface "no credential" so the caller can re-prompt.
      return null;
    }
  }

  private async deleteSecret(scope: Scope, id: string): Promise<void> {
    // Deletion does not need the VK — just drop the row.
    await getStorage().userCredentials.remove(scope, id);
  }

  setDbPassword(connectionId: string, password: string): Promise<void> {
    return this.setSecret("db", connectionId, password);
  }
  getDbPassword(connectionId: string): Promise<string | null> {
    return this.getSecret("db", connectionId);
  }
  deleteDbPassword(connectionId: string): Promise<void> {
    return this.deleteSecret("db", connectionId);
  }

  setSshPassword(connectionId: string, password: string): Promise<void> {
    return this.setSecret("ssh", connectionId, password);
  }
  getSshPassword(connectionId: string): Promise<string | null> {
    return this.getSecret("ssh", connectionId);
  }
  deleteSshPassword(connectionId: string): Promise<void> {
    return this.deleteSecret("ssh", connectionId);
  }

  setSshKeyPassphrase(connectionId: string, passphrase: string): Promise<void> {
    return this.setSecret("ssh-key", connectionId, passphrase);
  }
  getSshKeyPassphrase(connectionId: string): Promise<string | null> {
    return this.getSecret("ssh-key", connectionId);
  }
  deleteSshKeyPassphrase(connectionId: string): Promise<void> {
    return this.deleteSecret("ssh-key", connectionId);
  }

  async deleteAllForConnection(connectionId: string): Promise<void> {
    await getStorage().userCredentials.removeAllForKey(connectionId);
  }

  setLicenseKey(key: string): Promise<void> {
    return this.setSecret("license", LICENSE_KEY_ID, key);
  }
  getLicenseKey(): Promise<string | null> {
    return this.getSecret("license", LICENSE_KEY_ID);
  }
  deleteLicenseKey(): Promise<void> {
    return this.deleteSecret("license", LICENSE_KEY_ID);
  }

  setAIApiKeyForProvider(id: string, key: string): Promise<void> {
    return this.setSecret("ai-api-key-provider", id, key);
  }
  getAIApiKeyForProvider(id: string): Promise<string | null> {
    return this.getSecret("ai-api-key-provider", id);
  }
  deleteAIApiKeyForProvider(id: string): Promise<void> {
    return this.deleteSecret("ai-api-key-provider", id);
  }

  /**
   * True iff we can store/retrieve credentials — the vault backing pipe is
   * available on web. Does **not** imply the vault is currently unlocked;
   * use `isUnlocked()` to branch on that.
   */
  isAvailable(): boolean {
    return true;
  }

  /**
   * True iff a `get*` call would return without prompting. Used by the
   * app-startup pre-fetch in `connection-manager.svelte.ts` to avoid
   * popping the unlock dialog on page load.
   */
  isUnlocked(): boolean {
    return this.vault.status === "unlocked";
  }
}
