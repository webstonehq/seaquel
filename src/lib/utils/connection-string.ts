import type { ConnectionFormData } from "$lib/types";
import { databaseTypes } from "$lib/stores/connection-wizard.svelte.js";
import { getKeyringService } from "$lib/services/keyring";

/**
 * Build a connection string from form data.
 */
export function buildConnectionString(formData: ConnectionFormData): string {
  const data = formData;

  if (data.type === "sqlite") {
    return `sqlite://${data.databaseName}`;
  }

  if (data.type === "duckdb") {
    return `duckdb://${data.databaseName || ":memory:"}`;
  }

  const credentials = data.username
    ? `${encodeURIComponent(data.username)}${data.password ? `:${encodeURIComponent(data.password)}` : ""}@`
    : "";

  const selectedDbType = databaseTypes.find((t) => t.value === data.type);
  const protocol = selectedDbType?.protocol[0] || data.type;
  const port = data.port !== selectedDbType?.defaultPort ? `:${data.port}` : "";

  let connectionString = `${protocol}://${credentials}${data.host}${port}/${data.databaseName}`;

  // Add sslmode parameter for PostgreSQL and MySQL
  // Always include sslmode to be explicit (driver may default to TLS otherwise)
  if (
    (data.type === "postgres" || data.type === "mysql" || data.type === "mariadb") &&
    data.sslMode
  ) {
    const separator = connectionString.includes("?") ? "&" : "?";
    const isMysql = data.type === "mysql" || data.type === "mariadb";
    const sslParam = isMysql ? "ssl-mode" : "sslmode";
    // MySQL uses uppercase values: DISABLED, PREFERRED, REQUIRED, VERIFY_CA, VERIFY_IDENTITY
    const mysqlSslMap: Record<string, string> = {
      disable: "DISABLED",
      allow: "PREFERRED",
      prefer: "PREFERRED",
      require: "REQUIRED",
    };
    const sslValue = isMysql ? mysqlSslMap[data.sslMode] || data.sslMode : data.sslMode;
    connectionString += `${separator}${sslParam}=${sslValue}`;
  }

  return connectionString;
}

/** Query params TablePlus adds that database drivers don't understand. */
const TABLEPLUS_PARAMS = new Set([
  "statuscolor",
  "env",
  "name",
  "tlsmode",
  "useprivatekey",
  "safemodelevel",
  "advancedsafemodelevel",
  "driverversion",
  "lazyload",
]);

/**
 * Whether a connection string uses TablePlus-only syntax, in which case it
 * must be rebuilt from the parsed form fields before reaching the driver.
 */
function isTablePlusUrl(connStr: string): boolean {
  const scheme = connStr.slice(0, connStr.indexOf(":"));
  if (scheme.endsWith("+ssh")) return true;
  const queryIndex = connStr.indexOf("?");
  if (queryIndex === -1) return false;
  const params = new URLSearchParams(connStr.slice(queryIndex + 1));
  return [...params.keys()].some((key) => TABLEPLUS_PARAMS.has(key.toLowerCase()));
}

/**
 * Get connection data object from form data, suitable for passing to db.connections.add/reconnect/update.
 */
export function getConnectionData(formData: ConnectionFormData) {
  let connString = formData.connectionString;
  if (connString) {
    connString = connString.replace("postgresql://", "postgres://");
  } else {
    connString = buildConnectionString(formData);
  }

  if (!connString || connString.split(":").length !== 3 || isTablePlusUrl(connString)) {
    connString = buildConnectionString(formData);
  }

  const keyring = getKeyringService();
  const keychainAvailable = keyring.isAvailable();

  return {
    name: formData.name,
    type: formData.type,
    host: formData.host,
    port: formData.port,
    databaseName: formData.databaseName,
    username: formData.username,
    password: formData.password,
    sslMode: formData.sslMode,
    connectionString: connString,
    sshTunnel: formData.sshEnabled
      ? {
          enabled: true,
          host: formData.sshHost,
          port: formData.sshPort,
          username: formData.sshUsername,
          authMethod: formData.sshAuthMethod,
          keyPath: formData.sshKeyPath || undefined,
        }
      : undefined,
    sshPassword: formData.sshPassword,
    sshKeyPath: formData.sshKeyPath,
    sshKeyPassphrase: formData.sshKeyPassphrase,
    // Password storage flags (only if keychain is available)
    savePassword: keychainAvailable ? formData.savePassword : false,
    saveSshPassword: keychainAvailable ? formData.saveSshPassword : false,
    saveSshKeyPassphrase: keychainAvailable ? formData.saveSshKeyPassphrase : false,
    // AI privacy overrides (undefined = use global default)
    aiShareSchema: formData.aiShareSchema,
    aiShareData: formData.aiShareData,
  };
}

/**
 * TablePlus encodes its SSL mode as an integer. Only the values confirmed
 * against the TablePlus UI are mapped; others are left unmapped rather than
 * guessed, so the connection falls back to its default SSL mode.
 */
const TABLEPLUS_TLS_MODES: Record<string, string> = {
  "0": "prefer",
  "1": "disable",
  "2": "require",
};

export function tablePlusTlsModeToSslMode(tlsMode: string | number): string | undefined {
  return TABLEPLUS_TLS_MODES[String(tlsMode)];
}

/**
 * Case-insensitive lookup, since TablePlus spells some params in camelCase
 * (`tLSMode`) while other tools lowercase them.
 */
function getParam(params: URLSearchParams, name: string): string | null {
  const lower = name.toLowerCase();
  for (const [key, value] of params) {
    if (key.toLowerCase() === lower) return value;
  }
  return null;
}

function decode(value: string): string {
  return value ? decodeURIComponent(value) : "";
}

/**
 * Parse a file-based (SQLite/DuckDB) connection string into its path and query params.
 */
function parseFileConnectionString(connStr: string, scheme: string) {
  const rest = connStr.replace(new RegExp(`^${scheme}:(//)?`), "");
  const queryIndex = rest.indexOf("?");
  return {
    path: queryIndex === -1 ? rest : rest.slice(0, queryIndex),
    params: new URLSearchParams(queryIndex === -1 ? "" : rest.slice(queryIndex + 1)),
  };
}

/**
 * Parse a connection string and populate form data fields.
 * Returns the updated form data fields on success, or an error string on failure.
 *
 * Besides standard URLs, this understands TablePlus-style URLs: the `name` and
 * `tLSMode` query params, and `<scheme>+ssh://ssh_user[:ssh_pass]@ssh_host[:ssh_port]/db_user[:db_pass]@db_host[:db_port]/database`
 * for connections over an SSH tunnel.
 */
export function parseConnectionString(
  connStr: string,
): { success: true; formData: Partial<ConnectionFormData> } | { success: false; error: string } {
  try {
    // Handle SQLite
    if (connStr.startsWith("sqlite:")) {
      const { path: dbPath, params } = parseFileConnectionString(connStr, "sqlite");
      return {
        success: true,
        formData: {
          type: "sqlite",
          databaseName: dbPath,
          name: getParam(params, "name") || `SQLite - ${dbPath.split("/").pop() || "database"}`,
        },
      };
    }

    // Handle DuckDB
    if (connStr.startsWith("duckdb:")) {
      const { path: dbPath, params } = parseFileConnectionString(connStr, "duckdb");
      const isMemory = dbPath === ":memory:" || dbPath === "";
      return {
        success: true,
        formData: {
          type: "duckdb",
          databaseName: dbPath || ":memory:",
          name:
            getParam(params, "name") ||
            (isMemory ? "DuckDB - In-Memory" : `DuckDB - ${dbPath.split("/").pop() || "database"}`),
        },
      };
    }

    // Parse as URL
    const url = new URL(connStr);
    const scheme = url.protocol.replace(":", "");
    const isSsh = scheme.endsWith("+ssh");
    const protocol = isSsh ? scheme.slice(0, -"+ssh".length) : scheme;
    const dbType = databaseTypes.find((t) => t.protocol.includes(protocol));

    if (!dbType) {
      return { success: false, error: `Unsupported database type: ${protocol}` };
    }

    // For SSH URLs the outer URL is the SSH server and the path holds the database URL.
    let dbUrl = url;
    let sshFields: Partial<ConnectionFormData> = { sshEnabled: false };
    if (isSsh) {
      dbUrl = new URL(`${protocol}://${withDefaultHost(url.pathname.replace(/^\//, ""))}`);
      const usePrivateKey = getParam(url.searchParams, "usePrivateKey") === "true";
      sshFields = {
        sshEnabled: true,
        sshHost: url.hostname,
        sshPort: url.port ? parseInt(url.port) : 22,
        sshUsername: decode(url.username),
        sshAuthMethod: usePrivateKey ? "key" : "password",
        sshPassword: decode(url.password),
      };
    }

    const databaseName = decode(dbUrl.pathname.replace(/^\//, ""));
    const params = url.searchParams;
    const sslModeParam =
      getParam(params, "sslmode") ||
      getParam(params, "ssl-mode") ||
      tablePlusTlsModeToSslMode(getParam(params, "tLSMode") ?? "");

    return {
      success: true,
      formData: {
        type: dbType.value,
        host: dbUrl.hostname,
        port: dbUrl.port ? parseInt(dbUrl.port) : dbType.defaultPort,
        databaseName,
        username: decode(dbUrl.username),
        password: decode(dbUrl.password),
        ...(sslModeParam ? { sslMode: sslModeParam } : {}),
        ...sshFields,
        name: getParam(params, "name") || databaseName || `${dbType.label} Connection`,
      },
    };
  } catch {
    return { success: false, error: "Invalid connection string format" };
  }
}

/**
 * TablePlus allows omitting the database host in SSH URLs (`user@:5432/db`,
 * `user@/db`); it means the SSH server itself.
 */
function withDefaultHost(dbPart: string): string {
  const slashIndex = dbPart.indexOf("/", dbPart.lastIndexOf("@") + 1);
  const authority = slashIndex === -1 ? dbPart : dbPart.slice(0, slashIndex);
  const rest = slashIndex === -1 ? "" : dbPart.slice(slashIndex);
  const atIndex = authority.lastIndexOf("@");
  const userInfo = atIndex === -1 ? "" : authority.slice(0, atIndex + 1);
  const hostPort = authority.slice(atIndex + 1);
  const host = hostPort === "" || hostPort.startsWith(":") ? `127.0.0.1${hostPort}` : hostPort;
  return `${userInfo}${host}${rest}`;
}

/**
 * Check if all required credentials are present for auto-connect.
 */
export function hasAllCredentials(formData: ConnectionFormData): boolean {
  if (!formData.name.trim()) return false;
  if (!formData.databaseName.trim()) return false;
  const isFileBasedDb = formData.type === "sqlite" || formData.type === "duckdb";
  if (!isFileBasedDb && !formData.host.trim()) return false;

  // Password requirement (SQLite and DuckDB don't need password).
  // If the saved connection had `savePassword` on but nothing came back from the
  // keyring, it's an intentionally passwordless connection — allow auto-connect.
  // Otherwise (user opted out of saving) we still need them to re-enter it.
  if (!isFileBasedDb && !formData.password && !formData.savePassword) return false;

  // SSH requirements
  if (formData.sshEnabled) {
    if (!formData.sshHost.trim()) return false;
    if (!formData.sshUsername.trim()) return false;

    if (formData.sshAuthMethod === "password" && !formData.sshPassword) {
      return false;
    }
    if (formData.sshAuthMethod === "key" && !formData.sshKeyPath) {
      return false;
    }
  }

  return true;
}
