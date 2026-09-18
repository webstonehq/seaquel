import type { SqliteDatabase } from "../sqlite-types";

/**
 * Singleton row that tells the browser everything it needs to re-derive the
 * Vault Key from a passphrase. `salt` and `verifier*` are base64-encoded so
 * they can travel unchanged through the JSON `/api/storage/*` pipe.
 */
export interface PersistedVaultState {
  salt: string; // base64
  kdfParams: { version: number; t: number; m: number; p: number };
  verifier: string; // base64 ciphertext (verifier plaintext under VK)
  verifierNonce: string; // base64
  createdAt: string;
}

interface Row {
  salt: string;
  kdf_params: string;
  verifier: string;
  verifier_nonce: string;
  created_at: string;
}

export const vaultStateRepo = {
  async load(db: SqliteDatabase): Promise<PersistedVaultState | null> {
    const rows = await db.query<Row>(
      "SELECT salt, kdf_params, verifier, verifier_nonce, created_at FROM vault_state WHERE id = 1",
    );
    if (rows.length === 0) return null;
    const r = rows[0];
    return {
      salt: r.salt,
      kdfParams: JSON.parse(r.kdf_params),
      verifier: r.verifier,
      verifierNonce: r.verifier_nonce,
      createdAt: r.created_at,
    };
  },

  async save(db: SqliteDatabase, state: PersistedVaultState): Promise<void> {
    await db.execute(
      "INSERT OR REPLACE INTO vault_state (id, salt, kdf_params, verifier, verifier_nonce, created_at) VALUES (1, ?, ?, ?, ?, ?)",
      [
        state.salt,
        JSON.stringify(state.kdfParams),
        state.verifier,
        state.verifierNonce,
        state.createdAt,
      ],
    );
  },

  /**
   * Nukes the vault. Callers must also clear `user_credentials` — the vault
   * config and the ciphertext that depends on it are only meaningful
   * together.
   */
  async reset(db: SqliteDatabase): Promise<void> {
    await db.transaction([
      { sql: "DELETE FROM vault_state", params: [] },
      { sql: "DELETE FROM user_credentials", params: [] },
    ]);
  },
};
