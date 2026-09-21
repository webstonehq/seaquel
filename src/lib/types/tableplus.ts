import type { DatabaseType, SSHTunnelConfig } from "$lib/types";

/**
 * TablePlus connection as stored in the Connections.plist file
 */
export interface TablePlusConnection {
  id: string;
  connectionName: string;
  driver: string;
  databaseHost: string;
  databasePort: string;
  databaseName: string;
  databaseUser: string;
  /** File path for SQLite connections (TablePlus keeps DatabaseName as a label) */
  databasePath: string;
  /** TablePlus SSL mode as an integer (0 = PREFERRED, 1 = DISABLED, 2 = REQUIRED) */
  tlsMode: number | null;
  overSSH: boolean;
  sshHost: string;
  sshPort: string;
  sshUser: string;
  sshUsePrivateKey: boolean;
}

/**
 * A TablePlus connection that has been processed and is ready for import
 */
export interface TablePlusImportableConnection {
  original: TablePlusConnection;
  name: string;
  type: DatabaseType;
  host: string;
  port: number;
  databaseName: string;
  username: string;
  sslMode?: string;
  sshTunnel?: SSHTunnelConfig;
  isDuplicate: boolean;
  selected: boolean;
}
