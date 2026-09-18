/**
 * Better Auth configuration.
 *
 * Better Auth owns its own tables (`user`, `session`, `account`,
 * `verification`) plus our companion `member_license` table in a
 * dedicated **auth.db** file — separate from the per-user application
 * data under `users/<userId>/meta.db`. This split keeps tenant-level
 * tables (shared across all users in the tenant) out of every user's
 * individual data file.
 *
 * Multi-user / "organization" semantics live on the control plane
 * (seaquel-app) — each container instance IS a single tenant, so the
 * Better Auth `organization` plugin would just be a redundant
 * abstraction layer here. Membership is tracked in `member_license`
 * locally and `tenant_members` upstream.
 *
 * Migrations under `./migrations/*.sql` are applied lexicographically
 * on first DB open (idempotent via the `_seaquel_auth_migrations`
 * bookkeeping table).
 *
 * Cookie domain is configurable via env var so self-hosted deployments
 * can use their own domain without code changes.
 */

import { randomBytes } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { betterAuth } from "better-auth";
import type Database from "better-sqlite3";
import { backfillExistingMembers } from "./license-cache";

const require = createRequire(import.meta.url);

// -- Lazy, module-local better-sqlite3 instance for auth.db ----------------
//
// Why a module-local instance instead of sharing one with `db.ts`: Better
// Auth's tables belong to the tenant (shared across users); user data
// belongs to each user individually. Different files, different lifetimes.

let sqliteInstance: Database.Database | null = null;

/**
 * Open (or return the cached) auth.db handle. Exposed so other server
 * modules (e.g. `member-license.ts`) can read/write companion tables
 * without each one re-opening the file and re-running migrations.
 */
export function openAuthDb(): Database.Database {
  if (sqliteInstance) return sqliteInstance;

  const DatabaseCtor = require("better-sqlite3") as typeof Database;
  const dataDir = process.env.DATA_DIR ?? process.cwd();
  sqliteInstance = new DatabaseCtor(`${dataDir}/auth.db`);

  sqliteInstance.pragma("journal_mode = WAL");
  sqliteInstance.pragma("busy_timeout = 5000");
  sqliteInstance.pragma("foreign_keys = ON");

  // Apply Better Auth's schema on first open. Tracked via a bookkeeping
  // table so the idempotent INSERT OR IGNORE keeps startups fast once applied.
  applyAuthSchemaIfNeeded(sqliteInstance);

  // Phase-9 backfill: seed validation-cache columns on pre-existing
  // member_license rows so the rollout doesn't lock anyone out before
  // the first scheduled re-check. Idempotent — only touches NULL rows.
  backfillExistingMembers();

  return sqliteInstance;
}

// Pulled in at build time via Vite's glob — same mechanism the old
// Kysely-based migrator used. `query: "?raw"` keeps the SQL as a plain string.
// Glob pattern picks up every `.sql` file in `migrations/`, applied in
// filename order — keep numeric prefixes (`007_…`, `008_…`) so order
// stays deterministic across platforms.
const migrationModules = import.meta.glob("./migrations/*.sql", {
  eager: true,
  query: "?raw",
  import: "default",
}) as Record<string, string>;

function applyAuthSchemaIfNeeded(sqlite: Database.Database): void {
  sqlite.exec(`
    CREATE TABLE IF NOT EXISTS _seaquel_auth_migrations (
      name TEXT PRIMARY KEY,
      applied_at INTEGER NOT NULL DEFAULT (unixepoch())
    )
  `);

  const sortedPaths = Object.keys(migrationModules).sort();
  for (const path of sortedPaths) {
    const name = path.split("/").pop()!;
    const already = sqlite
      .prepare("SELECT 1 FROM _seaquel_auth_migrations WHERE name = ?")
      .get(name);
    if (already) continue;

    const sql = migrationModules[path];
    if (!sql) throw new Error(`auth migration not bundled: ${name}`);

    sqlite.transaction(() => {
      sqlite.exec(sql);
      sqlite.prepare("INSERT INTO _seaquel_auth_migrations (name) VALUES (?)").run(name);
    })();
    console.log(`[seaquel] applied auth migration: ${name}`);
  }
}

// -- Trusted origin allowlist ---------------------------------------------
//
// Single source of truth for the origins this install will accept browser
// requests from. Better Auth consumes the list for its own endpoints via
// `trustedOrigins`; custom handlers like `/api/signup` consume it through
// `isOriginTrusted()` to apply matching CSRF protection.
//
// Hosted: `.seaquel.app` so the apex dashboard and tenant subdomains
// share session cookies. Self-hosted: set via env var to whatever domain
// the operator points at this container.
//
// Localhost variants are always trusted so `npm run dev:web:full`,
// `npm run start:web`, and ssh-into-container debugging all work without
// a manual env var. This is safe — the browser won't let a non-localhost
// site claim `Origin: http://localhost:*`, so an entry in this list
// can't be exploited from a remote attacker's site.
export function getTrustedOrigins(): string[] {
  return [
    ...(process.env.SEAQUEL_TRUSTED_ORIGINS?.split(",")
      .map((s) => s.trim())
      .filter(Boolean) ?? []),
    "http://localhost:5173",
    "http://localhost:8787",
    "http://127.0.0.1:5173",
    "http://127.0.0.1:8787",
  ];
}

/**
 * Check a request's `Origin` header against {@link getTrustedOrigins}.
 * Returns `false` for missing/empty Origin — every browser populates
 * `Origin` on cross-site and non-GET requests, so a missing header on a
 * mutation route means it's a non-browser caller (curl, server-to-server)
 * which should not be permitted to hit user-facing auth endpoints.
 */
export function isOriginTrusted(origin: string | null | undefined): boolean {
  if (!origin) return false;
  return getTrustedOrigins().includes(origin);
}

// -- Better Auth instance (lazy Proxy) ------------------------------------

type AuthInstance = ReturnType<typeof build>;
let authInstance: AuthInstance | null = null;

/**
 * Resolve the Better Auth session-signing secret.
 *
 * Priority:
 *   1. `SEAQUEL_AUTH_SECRET` env var. Set this in clustered deployments so
 *      all replicas share a key (a Kubernetes Secret, Doppler binding, etc.).
 *   2. `${DATA_DIR}/auth-secret` on disk. Auto-generated on first boot for
 *      single-container self-hosters — no operator action required and the
 *      secret survives container restarts because `/data` is a persistent
 *      volume. File mode is `0600`.
 *   3. `undefined` (in development with `DATA_DIR` unset) — Better Auth
 *      generates a random per-boot value. Sessions don't survive restarts,
 *      which is fine for `npm run dev:web` and avoids polluting the repo
 *      root with a stray secrets file.
 *
 * Multi-replica deployments MUST set the env var explicitly. The on-disk
 * fallback assumes a single writer.
 */
function getAuthSecret(): string | undefined {
  const fromEnv = process.env.SEAQUEL_AUTH_SECRET;
  if (fromEnv && fromEnv.length > 0) return fromEnv;

  const dataDir = process.env.DATA_DIR;
  if (!dataDir) return undefined;

  const secretPath = join(dataDir, "auth-secret");
  if (existsSync(secretPath)) {
    return readFileSync(secretPath, "utf8").trim();
  }
  const secret = randomBytes(32).toString("hex");
  writeFileSync(secretPath, secret + "\n", { mode: 0o600 });
  return secret;
}

function build() {
  return betterAuth({
    database: openAuthDb(),

    // Canonical URL Better Auth uses for absolute links (email verification,
    // OAuth callbacks when those are added, etc.). Optional in development —
    // Better Auth will derive it from the request — but worth setting in
    // production so links work behind reverse proxies that rewrite Host.
    // Operators set this via the `BETTER_AUTH_URL` env var (documented in
    // README.md's "Test Docker image locally" section).
    baseURL: process.env.BETTER_AUTH_URL,

    trustedOrigins: getTrustedOrigins(),

    // Cap brute-force attempts on the auth surface. Better Auth ships with
    // rate limiting enabled by default in production but the defaults are
    // lenient. Tighten sign-in / sign-up / forgot-password specifically.
    // The `customRules` map keys are paths relative to Better Auth's mount
    // point (`/api/auth`), so `/sign-in/email` → `/api/auth/sign-in/email`.
    rateLimit: {
      enabled: true,
      window: 60, // seconds
      max: 100,
      customRules: {
        "/sign-in/email": { window: 60, max: 5 },
        "/sign-up/email": { window: 60, max: 3 },
        "/forget-password": { window: 60, max: 3 },
      },
    },
    advanced: {
      crossSubDomainCookies: {
        enabled: Boolean(process.env.SEAQUEL_COOKIE_DOMAIN),
        domain: process.env.SEAQUEL_COOKIE_DOMAIN, // e.g. ".seaquel.app"
      },
    },

    // v1 auth surface: email + password. Magic links, social OAuth, SSO,
    // passkeys, MFA are all additive via Better Auth plugins — not wired in
    // this phase.
    emailAndPassword: {
      enabled: true,
      // No email verification required in v1; flip to true once we have
      // real email delivery (Phase 3d or later).
      requireEmailVerification: false,
    },

    session: {
      expiresIn: 60 * 60 * 24 * 30, // 30 days
      updateAge: 60 * 60 * 24, // refresh if older than 1 day
    },

    // Session-signing secret. See `getAuthSecret()` for the resolution
    // order: env var → on-disk `${DATA_DIR}/auth-secret` (auto-generated) →
    // Better Auth's per-boot random fallback in dev.
    secret: getAuthSecret(),
  });
}

export const auth = new Proxy({} as AuthInstance, {
  get(_target, prop, receiver) {
    if (!authInstance) authInstance = build();
    const value = Reflect.get(authInstance, prop, receiver);
    return typeof value === "function" ? value.bind(authInstance) : value;
  },
});
