/**
 * When a connection keeps a connection string, and what of it is stored.
 *
 * Core connects with the string when there is one and ignores the host,
 * port, database, user and SSL mode fields, so a string must never outlive
 * what the user sees:
 * - The details step shows a non-empty string, and editing a field it
 *   encodes clears it (`forgetConnectionString`).
 * - A pasted string is dropped once its fields are filled in, unless it holds
 *   something the fields can't (`connectionStringHasExtras`).
 * - Rows written before phase 5a hold the string the old
 *   `buildConnectionString` rebuilt from their fields on every save. Such a
 *   string carries nothing of its own, and goes stale as soon as a field is
 *   edited, so it's dropped when the row loads (`isLegacyBuiltString`).
 * - Stored strings never keep a database or SSH password
 *   (`stripConnectionStringSecrets`).
 */

import type { ConnectionFormData } from "$lib/types";

/** The fields a connection string can stand for. */
export interface StringFields {
  type: string;
  host?: string;
  port?: number;
  databaseName?: string;
  username?: string;
  sslMode?: string;
}

/** Scheme and default port the old builder used, per type. */
const LEGACY_TYPES: Record<string, { protocol: string; defaultPort: number }> = {
  postgres: { protocol: "postgres", defaultPort: 5432 },
  mysql: { protocol: "mysql", defaultPort: 3306 },
  mariadb: { protocol: "mariadb", defaultPort: 3306 },
  mssql: { protocol: "mssql", defaultPort: 1433 },
};

const LEGACY_MYSQL_SSL: Record<string, string> = {
  disable: "DISABLED",
  allow: "PREFERRED",
  prefer: "PREFERRED",
  require: "REQUIRED",
};

/**
 * What the old `buildConnectionString` (before phase 5a) made of these
 * fields, without the password: stored strings had theirs stripped.
 */
export function legacyBuiltString(f: StringFields): string {
  const database = f.databaseName ?? "";
  if (f.type === "sqlite") return `sqlite://${database}`;
  if (f.type === "duckdb") return `duckdb://${database || ":memory:"}`;
  const known = LEGACY_TYPES[f.type];
  const protocol = known?.protocol ?? f.type;
  const credentials = f.username ? `${encodeURIComponent(f.username)}@` : "";
  const port = f.port !== known?.defaultPort ? `:${f.port}` : "";
  let s = `${protocol}://${credentials}${f.host ?? ""}${port}/${database}`;
  if ((f.type === "postgres" || f.type === "mysql" || f.type === "mariadb") && f.sslMode) {
    const mysql = f.type !== "postgres";
    const value = mysql ? (LEGACY_MYSQL_SSL[f.sslMode] ?? f.sslMode) : f.sslMode;
    s += `?${mysql ? "ssl-mode" : "sslmode"}=${value}`;
  }
  return s;
}

/** A string in a form two strings can be compared in: no password, one Postgres scheme. */
function comparable(s: string): string {
  const t = s.trim().replace(/^postgresql:\/\//i, "postgres://");
  try {
    const url = new URL(t);
    url.password = "";
    return url.toString();
  } catch {
    return t;
  }
}

/** Whether `s` is the string the old builder made from `fields` (so it says nothing more). */
export function isLegacyBuiltString(s: string | undefined, fields: StringFields): boolean {
  if (!s) return false;
  return comparable(s) === comparable(legacyBuiltString(fields));
}

/** A row's string, or `""` when it's one the old builder made from the row's own fields. */
export function storedConnectionString(row: StringFields & { connectionString?: string }): string {
  const s = row.connectionString ?? "";
  return isLegacyBuiltString(s, row) ? "" : s;
}

/** Parameters the form's fields stand for (the SSL mode, TablePlus's `tLSMode` and name). */
const FIELD_PARAMS = new Set(["sslmode", "ssl-mode", "tlsmode", "name"]);

/**
 * Whether `s` holds something the form's fields can't: query parameters other
 * than the SSL mode and name, a TablePlus `+ssh` URL, a fragment, or anything
 * that isn't a URL (a key=value string). Such a string is kept after a paste.
 */
export function connectionStringHasExtras(s: string): boolean {
  const t = s.trim();
  if (!t) return false;
  const file = /^(sqlite|duckdb):/i.exec(t);
  if (file) {
    const q = t.indexOf("?");
    if (q === -1) return false;
    const params = new URLSearchParams(t.slice(q + 1));
    return [...params.keys()].some((k) => k.toLowerCase() !== "name");
  }
  if (!t.includes("://")) return true;
  let url: URL;
  try {
    url = new URL(t);
  } catch {
    return true;
  }
  if (url.protocol.replace(/:$/, "").toLowerCase().endsWith("+ssh")) return true;
  if (url.hash) return true;
  return [...url.searchParams.keys()].some((k) => !FIELD_PARAMS.has(k.toLowerCase()));
}

/** Editing a field the string encodes: the fields are what connects from now on. */
export function forgetConnectionString(formData: Pick<ConnectionFormData, "connectionString">) {
  formData.connectionString = "";
}

// -------- Secrets --------

/** Keys whose value is a password, in key=value strings and URL queries. */
const SECRET_KEY = /^(password|pwd|sslpassword)$/i;
/** DuckDB options that can carry credentials (S3 and other secrets). */
const DUCKDB_SECRET_KEY = /secret|password|pwd|token|key_id|access_key|session/i;
/** What a stored string must never still contain. */
const LEFTOVER_SECRET = /(password|pwd)\s*=/i;

/** A file string (SQLite, DuckDB) without query options that can carry credentials. */
function stripFileString(t: string): string {
  const q = t.indexOf("?");
  if (q === -1) return t;
  const pairs = t.slice(q + 1).split("&");
  const kept = pairs.filter((pair) => {
    const key = pair.split("=")[0];
    let decoded = key;
    try {
      decoded = decodeURIComponent(key);
    } catch {
      // A malformed escape: judge it as written.
    }
    return !DUCKDB_SECRET_KEY.test(decoded);
  });
  // Left as typed, except for the dropped pairs.
  return kept.length > 0 ? `${t.slice(0, q)}?${kept.join("&")}` : t.slice(0, q);
}

/**
 * A key=value connection string without its password pairs: the ADO style
 * (`Server=h;Password=pw;`) and libpq's (`host=h password=pw`), with quoted
 * values (`'…'`, `"…"`, `{…}`). `null` when it can't be read safely.
 */
function stripKeyValue(s: string): string | null {
  const semicolons = s.includes(";");
  const parts: string[] = [];
  let i = 0;
  while (i < s.length) {
    // Separators before the next pair.
    const sep = semicolons ? /[;\s]/ : /\s/;
    while (i < s.length && sep.test(s[i])) i += 1;
    if (i >= s.length) break;
    const eq = s.indexOf("=", i);
    if (eq === -1) return null;
    const key = s.slice(i, eq).trim();
    if (!key || (semicolons ? key.includes(";") : /\s/.test(key))) return null;
    i = eq + 1;
    while (i < s.length && s[i] === " ") i += 1;
    const open = s[i];
    let value: string;
    if (open === "'" || open === '"' || open === "{") {
      const close = open === "{" ? "}" : open;
      let j = i + 1;
      let done = false;
      while (j < s.length) {
        if (open === "'" && !semicolons && s[j] === "\\") {
          j += 2;
          continue;
        }
        if (s[j] === close) {
          if (s[j + 1] === close) {
            j += 2;
            continue;
          }
          done = true;
          break;
        }
        j += 1;
      }
      if (!done) return null;
      value = s.slice(i, j + 1);
      i = j + 1;
    } else {
      const end = semicolons ? s.indexOf(";", i) : s.slice(i).search(/\s/) + i;
      const stop = end === -1 || end < i ? s.length : end;
      value = s.slice(i, stop).trim();
      i = stop;
    }
    if (!SECRET_KEY.test(key)) parts.push(`${key}=${value}`);
  }
  return semicolons ? parts.map((p) => `${p};`).join("") : parts.join(" ");
}

/**
 * The string as it may be stored: no password anywhere in it. A URL loses
 * its user-info password (and, for a TablePlus `+ssh` URL, the database
 * password in its path) and any password query parameter; a key=value
 * string loses its `Password=`/`Pwd=` pairs. A string that can't be read
 * safely (a URL `new URL` rejects, one with a fragment, which is where an
 * unescaped `#` in a password lands) is stored as `""`, and so is any result
 * that still has a `password=`/`pwd=` in it. File strings (SQLite, DuckDB)
 * lose options that can carry credentials (`s3_secret_access_key`, …) and
 * keep the rest as typed. The scheme is kept.
 */
export function stripConnectionStringSecrets(s: string | undefined): string | undefined {
  if (!s) return undefined;
  const stripped = stripSecrets(s.trim());
  // A last guard: whatever the rules above missed isn't stored.
  return LEFTOVER_SECRET.test(stripped) ? "" : stripped;
}

function stripSecrets(t: string): string {
  if (/^(sqlite|duckdb):/i.test(t)) return stripFileString(t);
  if (!t.includes("://")) {
    return stripKeyValue(t) ?? "";
  }
  let url: URL;
  try {
    url = new URL(t);
  } catch {
    return "";
  }
  if (url.hash) return "";
  url.password = "";
  if (url.protocol.toLowerCase().endsWith("+ssh:")) {
    // `/dbuser:dbpass@dbhost/db`: keep the user, drop the password. The
    // host is what follows the last `@` of the first path segment's
    // user info; a password with `/` or `@` in it spans more than one
    // segment, so anything but one clean `user:pass@` is refused.
    const match = /^\/([^@/:]*):[^/]*@([^@/]*)(\/.*)?$/.exec(url.pathname);
    if (match) {
      url.pathname = `/${match[1]}@${match[2]}${match[3] ?? ""}`;
    }
    if (/^\/[^@]*:[^@]*@/.test(url.pathname) || url.pathname.split("@").length > 2) return "";
  }
  // Only rewrite the query when it holds a password: rewriting re-encodes it.
  const secretKeys = [...url.searchParams.keys()].filter((key) => SECRET_KEY.test(key));
  for (const key of secretKeys) url.searchParams.delete(key);
  return url.toString();
}
