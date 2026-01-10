import type { DatabaseConnection, SSHTunnelConfig } from "$lib/types";

// Type for persisted connection data (without password and database instance)
export interface PersistedConnection {
  id: string;
  name: string;
  type: DatabaseConnection["type"];
  host: string;
  port: number;
  databaseName: string;
  username: string;
  sslMode?: string;
  connectionString?: string;
  lastConnected?: Date;
  sshTunnel?: SSHTunnelConfig;
  /** Whether the database password is saved in keychain */
  savePassword?: boolean;
  /** Whether the SSH password is saved in keychain */
  saveSshPassword?: boolean;
  /** Whether the SSH key passphrase is saved in keychain */
  saveSshKeyPassphrase?: boolean;
}
