/**
 * Reactive vault state for the web build.
 *
 * Holds the in-memory Vault Key (as a non-extractable CryptoKey) for the
 * tab session, plus a reactive `status` and `waitersPending` signal that a
 * global `<VaultGate>` component watches to show setup / unlock dialogs.
 *
 * The key is **never** stored in sessionStorage or localStorage — a new tab
 * or a page reload always starts with the vault locked.
 */
import { getDatabase } from "$lib/storage";
import type { SqliteDatabase } from "$lib/storage/sqlite-types";
import { vaultStateRepo } from "$lib/storage/repos/vault-state-repo";
import {
  DEFAULT_KDF_PARAMS,
  VERIFIER_PLAINTEXT,
  decryptToString,
  deriveVaultKey,
  encrypt,
  fromBase64,
  randomSalt,
  toBase64,
  type KdfParams,
} from "./crypto";

export type VaultStatus =
  | "unknown" // haven't checked the DB yet
  | "uninitialized" // DB has no vault_state row
  | "locked" // vault_state exists, but VK is not in memory
  | "unlocked"; // VK cached, encrypt/decrypt calls can proceed

/**
 * Thrown when `VaultKeyringService` is asked to encrypt/decrypt but the
 * user cancels the unlock / setup prompt. Surfaces as a toast upstream.
 */
export class VaultCancelledError extends Error {
  constructor(reason = "vault unlock cancelled") {
    super(reason);
    this.name = "VaultCancelledError";
  }
}

interface Waiter {
  resolve: (key: CryptoKey) => void;
  reject: (err: Error) => void;
}

export class Vault {
  /** Reactive status — the gate component watches this. */
  status: VaultStatus = $state("unknown");

  /**
   * Reactive signal: at least one caller is waiting for the vault to
   * unlock. The gate component reads this to decide whether to show the
   * setup/unlock dialog. We keep it separate from `status` so a "locked"
   * vault doesn't trigger dialogs just because it exists — only when
   * someone actually needs it.
   */
  waitersPending: boolean = $state(false);

  private key: CryptoKey | null = null;
  private waiters: Waiter[] = [];
  private dbPromise: Promise<SqliteDatabase> | null = null;
  private refreshPromise: Promise<void> | null = null;

  // Rate limiting on unlock attempts. Exponential backoff caps at 30s so the
  // dialog doesn't appear frozen, but ramps up fast enough to make casual
  // brute force (malicious extension, XSS) uninteresting at Argon2id cost.
  private failedUnlocks = 0;
  private nextUnlockAllowedAt = 0;

  // Idle auto-lock. Reset by `notifyUserActivity()` — NOT by `requireKey()`.
  // Background work (connection save, periodic sync) calls `requireKey()`
  // too, so bumping the timer there would make "idle" mean "no crypto
  // calls in the last 30 min", defeating the whole point. The gate
  // component in (app)/+layout wires this to real keydown/pointerdown
  // events so walking away from the keyboard actually counts as idle.
  // New tab / reload already starts locked; this covers the "left laptop
  // open" case.
  private idleTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly IDLE_TIMEOUT_MS = 30 * 60 * 1000;

  private armIdleTimer(): void {
    this.clearIdleTimer();
    if (typeof window === "undefined") return;
    this.idleTimer = setTimeout(() => this.lock(), this.IDLE_TIMEOUT_MS);
  }

  /**
   * Reset the auto-lock countdown. Call from user-interaction listeners
   * (keydown, pointerdown) — not from crypto paths.
   */
  notifyUserActivity(): void {
    if (this.status !== "unlocked" || !this.key) return;
    this.armIdleTimer();
  }

  private clearIdleTimer(): void {
    if (this.idleTimer !== null) {
      clearTimeout(this.idleTimer);
      this.idleTimer = null;
    }
  }

  private async db(): Promise<SqliteDatabase> {
    this.dbPromise ??= getDatabase();
    return this.dbPromise;
  }

  /**
   * Read `vault_state` to determine if the vault is `uninitialized` or
   * `locked`. Idempotent; safe to call from multiple places.
   */
  async refresh(): Promise<void> {
    if (this.status === "unlocked") return;
    this.refreshPromise ??= this.doRefresh();
    await this.refreshPromise;
  }

  private async doRefresh(): Promise<void> {
    try {
      const db = await this.db();
      const row = await vaultStateRepo.load(db);
      if (this.key) {
        // Already unlocked — don't clobber.
        this.status = "unlocked";
      } else if (!row) {
        this.status = "uninitialized";
      } else {
        this.status = "locked";
      }
    } finally {
      this.refreshPromise = null;
    }
  }

  /**
   * Wait for the vault to be unlocked. If the vault is `unknown`, refresh
   * first. Registers a waiter and returns a promise that resolves when
   * setup/unlock completes, or rejects when the user cancels.
   */
  async ensureUnlocked(): Promise<CryptoKey> {
    if (this.status === "unknown") await this.refresh();
    if (this.status === "unlocked" && this.key) return this.key;
    return new Promise<CryptoKey>((resolve, reject) => {
      this.waiters.push({ resolve, reject });
      this.waitersPending = true;
    });
  }

  /**
   * First-time setup: creates the `vault_state` row, derives VK, caches it.
   */
  async setup(passphrase: string, params: KdfParams = DEFAULT_KDF_PARAMS): Promise<void> {
    if (this.status === "unknown") await this.refresh();
    if (this.status === "locked" || this.status === "unlocked") {
      throw new Error("vault already initialized — use unlock() instead");
    }

    const db = await this.db();
    const salt = randomSalt();
    const key = await deriveVaultKey(passphrase, salt, params);
    const verifier = await encrypt(key, VERIFIER_PLAINTEXT);

    await vaultStateRepo.save(db, {
      salt: toBase64(salt),
      kdfParams: params,
      verifier: toBase64(verifier.ciphertext),
      verifierNonce: toBase64(verifier.nonce),
      createdAt: new Date().toISOString(),
    });

    this.key = key;
    this.status = "unlocked";
    this.armIdleTimer();
    this.resolveWaiters();
  }

  /**
   * Subsequent unlock with an existing passphrase. Verifies the passphrase
   * by decrypting the stored verifier blob before trusting any other
   * ciphertext.
   */
  async unlock(passphrase: string): Promise<void> {
    if (this.status === "unknown") await this.refresh();
    if (this.status === "uninitialized") {
      throw new Error("vault not initialized — use setup() instead");
    }
    if (this.status === "unlocked" && this.key) return;

    // Enforce the cooldown imposed by prior failed attempts before doing
    // any expensive KDF work. This stops a caller from pipelining a
    // thousand unlock attempts.
    const wait = this.nextUnlockAllowedAt - Date.now();
    if (wait > 0) {
      await new Promise<void>((r) => setTimeout(r, wait));
    }

    const db = await this.db();
    const row = await vaultStateRepo.load(db);
    if (!row) {
      this.status = "uninitialized";
      throw new Error("vault not initialized");
    }

    const salt = fromBase64(row.salt);
    const key = await deriveVaultKey(passphrase, salt, row.kdfParams);

    const registerFailure = () => {
      this.failedUnlocks++;
      const backoff = Math.min(30_000, 1000 * 2 ** (this.failedUnlocks - 1));
      this.nextUnlockAllowedAt = Date.now() + backoff;
    };

    let confirmed: string;
    try {
      confirmed = await decryptToString(key, {
        nonce: fromBase64(row.verifierNonce),
        ciphertext: fromBase64(row.verifier),
      });
    } catch {
      registerFailure();
      throw new Error("incorrect passphrase");
    }
    if (confirmed !== VERIFIER_PLAINTEXT) {
      registerFailure();
      throw new Error("incorrect passphrase");
    }

    this.failedUnlocks = 0;
    this.nextUnlockAllowedAt = 0;
    this.key = key;
    this.status = "unlocked";
    this.armIdleTimer();
    this.resolveWaiters();
  }

  /** Forget the cached VK. Status flips back to `locked`. */
  lock(): void {
    this.key = null;
    this.clearIdleTimer();
    if (this.status === "unlocked") this.status = "locked";
  }

  /** Reject every pending waiter. Called when the user cancels a dialog. */
  cancelPending(reason = "vault unlock cancelled"): void {
    const waiters = this.waiters;
    this.waiters = [];
    this.waitersPending = false;
    const err = new VaultCancelledError(reason);
    for (const w of waiters) w.reject(err);
  }

  /**
   * Reset everything — wipes `vault_state` and every row in
   * `user_credentials`. Used when the user forgets the passphrase.
   */
  async reset(): Promise<void> {
    const db = await this.db();
    await vaultStateRepo.reset(db);
    this.key = null;
    this.status = "uninitialized";
    this.clearIdleTimer();
    this.failedUnlocks = 0;
    this.nextUnlockAllowedAt = 0;
    this.cancelPending("vault reset");
  }

  /** For `VaultKeyringService` only — used after `ensureUnlocked()`. */
  requireKey(): CryptoKey {
    if (!this.key) throw new Error("vault is locked");
    return this.key;
  }

  private resolveWaiters(): void {
    if (!this.key) return;
    const key = this.key;
    const waiters = this.waiters;
    this.waiters = [];
    this.waitersPending = false;
    for (const w of waiters) w.resolve(key);
  }
}

let singleton: Vault | null = null;

export function getVault(): Vault {
  singleton ??= new Vault();
  return singleton;
}
