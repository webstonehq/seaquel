import type { SqliteDatabase } from "../sqlite-types";
import type { PersistedCredential } from "../client";

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
