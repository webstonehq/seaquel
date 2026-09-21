import { readTablePlusConfig } from "$lib/api/tauri";
import type { DatabaseType } from "$lib/types";
import { tablePlusTlsModeToSslMode } from "$lib/utils/connection-string";
import type { TablePlusConnection, TablePlusImportableConnection } from "$lib/types/tableplus";

/**
 * Maps TablePlus driver names to Seaquel database types
 */
const DRIVER_MAP: Record<string, DatabaseType> = {
  PostgreSQL: "postgres",
  MySQL: "mysql",
  MariaDB: "mariadb",
  SQLite: "sqlite",
  "SQL Server": "mssql",
};

/**
 * Default ports for each database type
 */
const DEFAULT_PORTS: Record<DatabaseType, number> = {
  postgres: 5432,
  mysql: 3306,
  mariadb: 3306,
  sqlite: 0,
  mssql: 1433,
  duckdb: 0,
};

/**
 * Reads and parses TablePlus Connections.plist using a Rust command.
 * The Rust side converts the plist to JSON before returning.
 * Returns an array of connections or empty array if not found.
 */
export async function parseTablePlusConnections(): Promise<TablePlusConnection[]> {
  try {
    const content = await readTablePlusConfig();

    if (!content) {
      return [];
    }

    const data = JSON.parse(content) as Record<string, unknown>[];

    if (!Array.isArray(data)) {
      return [];
    }

    return data
      .filter((entry): entry is Record<string, unknown> => {
        return typeof entry === "object" && entry !== null && "ID" in entry;
      })
      .map(toTablePlusConnection);
  } catch (error) {
    console.error("Failed to read TablePlus config:", error);
    return [];
  }
}

/**
 * Converts a raw Connections.plist entry. Plist values are typed, so flags come
 * through as booleans and TLS mode as a number, while ports may be strings.
 */
export function toTablePlusConnection(entry: Record<string, unknown>): TablePlusConnection {
  const str = (key: string) => {
    const value = entry[key];
    return typeof value === "string" || typeof value === "number" || typeof value === "boolean"
      ? String(value)
      : "";
  };
  const tlsMode = Number(entry.tLSMode);
  return {
    id: str("ID"),
    connectionName: str("ConnectionName"),
    driver: str("Driver"),
    databaseHost: str("DatabaseHost"),
    databasePort: str("DatabasePort"),
    databaseName: str("DatabaseName"),
    databaseUser: str("DatabaseUser"),
    databasePath: str("DatabasePath"),
    tlsMode: entry.tLSMode == null || Number.isNaN(tlsMode) ? null : tlsMode,
    overSSH: entry.isOverSSH === true,
    sshHost: str("ServerAddress"),
    sshPort: str("ServerPort"),
    sshUser: str("ServerUser"),
    sshUsePrivateKey: entry.isUsePrivateKey === true,
  };
}

/**
 * Maps a TablePlus connection to an importable connection format.
 * Returns null if the database type is not supported.
 */
export function mapToImportable(
  conn: TablePlusConnection,
  existingConnectionIds: string[],
): TablePlusImportableConnection | null {
  const type = DRIVER_MAP[conn.driver];
  if (!type) {
    return null; // Unsupported database type
  }

  const isFileBased = type === "sqlite";
  const host = conn.databaseHost || "localhost";
  const port = parseInt(conn.databasePort || String(DEFAULT_PORTS[type]), 10);
  const databaseName = isFileBased ? conn.databasePath || conn.databaseName : conn.databaseName;
  const username = conn.databaseUser || "";

  // Generate the connection ID that Seaquel would use
  const expectedId = isFileBased ? `conn-sqlite-${databaseName}` : `conn-${host}-${port}`;

  const isDuplicate = existingConnectionIds.includes(expectedId);

  // SSL only applies to the network drivers that Seaquel exposes it for
  const supportsSsl = type === "postgres" || type === "mysql" || type === "mariadb";
  const sslMode =
    supportsSsl && conn.tlsMode !== null ? tablePlusTlsModeToSslMode(conn.tlsMode) : undefined;

  const sshTunnel =
    !isFileBased && conn.overSSH && conn.sshHost
      ? {
          enabled: true,
          host: conn.sshHost,
          port: parseInt(conn.sshPort, 10) || 22,
          username: conn.sshUser,
          // The plist has no usable key path, so the user picks the key file when connecting.
          authMethod: conn.sshUsePrivateKey ? ("key" as const) : ("password" as const),
        }
      : undefined;

  return {
    original: conn,
    name: conn.connectionName || `${host}:${port}`,
    type,
    host,
    port,
    databaseName,
    username,
    sslMode,
    sshTunnel,
    isDuplicate,
    selected: !isDuplicate, // Pre-select non-duplicates
  };
}

/**
 * Discovers and parses all TablePlus connections, filtering for supported types
 */
export async function discoverTablePlusConnections(
  existingConnectionIds: string[],
): Promise<TablePlusImportableConnection[]> {
  const tablePlusConnections = await parseTablePlusConnections();

  return tablePlusConnections
    .map((conn) => mapToImportable(conn, existingConnectionIds))
    .filter((conn): conn is TablePlusImportableConnection => conn !== null);
}
