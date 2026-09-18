/**
 * CLI-only config for `@better-auth/cli generate`.
 *
 * The real auth instance in `./auth.ts` uses Vite's `import.meta.glob` to
 * inline migration SQL at build time — the Better Auth CLI runs outside
 * Vite and can't evaluate that. Mirror the plugin-and-feature choices here
 * so `generate` knows which tables to emit.
 *
 * ⚠️  KEEP IN SYNC WITH `auth.ts` ⚠️
 *
 * Any auth feature in `auth.ts`'s `build()` that affects the schema —
 * Better Auth plugins (`organization`, `twoFactor`, etc.),
 * `emailAndPassword` toggles, `user.additionalFields`, or any other
 * schema-impacting option — must be mirrored here. Drift between the
 * two files is silent: existing installs keep working off their
 * already-applied SQL migrations, but the next `npx @better-auth/cli
 * generate` will emit a schema that doesn't match production.
 *
 * Runtime options that DON'T affect tables (rateLimit, trustedOrigins,
 * secret, baseURL, session expiry, advanced.crossSubDomainCookies, …)
 * intentionally live only in `auth.ts`.
 */

import Database from "better-sqlite3";
import { betterAuth } from "better-auth";

export const auth = betterAuth({
  // In-memory SQLite — the CLI only uses the config to figure out what
  // tables to emit; the database is never actually queried.
  database: new Database(":memory:"),
  emailAndPassword: { enabled: true },
});
