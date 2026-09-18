import type { SqliteDatabase } from "../sqlite-types";

/**
 * One encrypted credential. `scope` maps to the `KeyringService` category
 * (`db`, `ssh`, `ssh-key`, `license`, `ai-api-key`, `ai-api-key-provider`).
 * `key` is the connection id / provider id, or empty string for singletons.
 *
 * `nonce` and `ciphertext` are base64-encoded — the /api/storage wire layer
 * is JSON, so we don't go through BLOB.
 */
export interface PersistedCredential {
  scope: string;
  key: string;
  nonce: string;
  ciphertext: string;
  updatedAt: string;
}

interface Row {
  scope: string;
  key: string;
  nonce: string;
  ciphertext: string;
  updated_at: string;
}

export const userCredentialsRepo = {
  async load(db: SqliteDatabase, scope: string, key: string): Promise<PersistedCredential | null> {
    const rows = await db.query<Row>(
      "SELECT scope, key, nonce, ciphertext, updated_at FROM user_credentials WHERE scope = ? AND key = ?",
      [scope, key],
    );
    if (rows.length === 0) return null;
    const r = rows[0];
    return {
      scope: r.scope,
      key: r.key,
      nonce: r.nonce,
      ciphertext: r.ciphertext,
      updatedAt: r.updated_at,
    };
  },

  async save(db: SqliteDatabase, cred: PersistedCredential): Promise<void> {
    await db.execute(
      "INSERT OR REPLACE INTO user_credentials (scope, key, nonce, ciphertext, updated_at) VALUES (?, ?, ?, ?, ?)",
      [cred.scope, cred.key, cred.nonce, cred.ciphertext, cred.updatedAt],
    );
  },

  async remove(db: SqliteDatabase, scope: string, key: string): Promise<void> {
    await db.execute("DELETE FROM user_credentials WHERE scope = ? AND key = ?", [scope, key]);
  },

  /** Remove every credential tied to a given `key` (e.g. a connection id). */
  async removeAllForKey(db: SqliteDatabase, key: string): Promise<void> {
    await db.execute("DELETE FROM user_credentials WHERE key = ?", [key]);
  },
};
