/**
 * The environment `server.js` gives the Rust service (`seaquel-server`).
 *
 * Only what Rust needs, never the whole of Node's environment. sqlx takes
 * its defaults from libpq's variables (`PGPASSWORD`, `PGUSER`, `PGHOST`,
 * `PGSSLKEY`, `PGPASSFILE`, …) and reads `~/.pgpass` for a URL without a
 * password, so a self-hoster's database credentials in the container's
 * environment or home directory would otherwise reach a web user's
 * connection to a host of their choosing. The same goes for anything else
 * a driver might read. So this is an allow-list, and `HOME` points nowhere.
 *
 * Plain ESM JS (no TS) so `server.js` can import it directly at runtime
 * without a build step. The Dockerfile copies this directory into the image.
 */

/**
 * Variables passed through by exact name (compared in upper case, since
 * Windows names are case-insensitive).
 */
export const RUST_ENV_NAMES = new Set([
  // seaquel-server and seaquel-license (every `*_ENV` constant they read).
  "DATA_DIR",
  "BIND_ADDR",
  "SEAQUEL_INTERNAL_SECRET",
  "SEAQUEL_ALLOW_NON_LOOPBACK",
  "SEAQUEL_CONTROL_URL",
  "SEAQUEL_LICENSE_SOFT_TTL",
  "SEAQUEL_LICENSE_GRACE_TTL",
  "SEAQUEL_BUNDLE_TRUSTED_PUBKEY",
  // TLS roots: the control-plane client adds NODE_EXTRA_CA_CERTS; rustls'
  // native store honours SSL_CERT_FILE / SSL_CERT_DIR.
  "NODE_EXTRA_CA_CERTS",
  "SSL_CERT_FILE",
  "SSL_CERT_DIR",
  // Outbound proxies (reqwest), in both spellings.
  "HTTP_PROXY",
  "HTTPS_PROXY",
  "ALL_PROXY",
  "NO_PROXY",
  // The process basics.
  "PATH",
  "TZ",
  "LANG",
  "TMPDIR",
  "TEMP",
  "TMP",
  "SYSTEMROOT",
  "WINDIR",
]);

/**
 * Where `HOME` points for the Rust service: a path that doesn't exist and
 * that nobody but root can create, so sqlx finds no `~/.pgpass`.
 */
export const RUST_HOME = "/nonexistent";

/**
 * The environment for the Rust child, from Node's.
 *
 * @param {Record<string, string | undefined>} env Node's environment.
 * @param {Record<string, string>} overrides Set last (BIND_ADDR, the secret).
 * @returns {Record<string, string>}
 */
export function rustEnv(env, overrides = {}) {
  /** @type {Record<string, string>} */
  const out = {};
  for (const [name, value] of Object.entries(env)) {
    if (value === undefined) continue;
    if (RUST_ENV_NAMES.has(name.toUpperCase())) out[name] = value;
  }
  out.HOME = RUST_HOME;
  return { ...out, ...overrides };
}
