/**
 * Database connection types and configuration.
 * @module types/database
 */

/**
 * Supported database engine types.
 */
export type DatabaseType = "postgres" | "mysql" | "sqlite" | "mariadb" | "mssql" | "duckdb";

/**
 * SSH tunnel authentication methods.
 */
export type SSHAuthMethod = "password" | "key";

/**
 * Configuration for SSH tunnel connections.
 * Used to connect to databases through an SSH jump host.
 */
export interface SSHTunnelConfig {
  /** Whether SSH tunneling is enabled */
  enabled: boolean;
  /** SSH server hostname */
  host: string;
  /** SSH server port (typically 22) */
  port: number;
  /** SSH username for authentication */
  username: string;
  /** Authentication method: password or SSH key */
  authMethod: SSHAuthMethod;
  /** Path to SSH private key file (for key auth) */
  keyPath?: string;
}

/**
 * Represents a database connection configuration and runtime state.
 *
 * @example
 * const connection: DatabaseConnection = {
 *   id: 'conn-localhost-5432',
 *   name: 'Local Development',
 *   type: 'postgres',
 *   host: 'localhost',
 *   port: 5432,
 *   databaseName: 'myapp_dev',
 *   username: 'postgres',
 *   password: ''
 * };
 */
export interface DatabaseConnection {
  /** Unique identifier for the connection */
  id: string;
  /** User-friendly display name */
  name: string;
  /** Database engine type */
  type: DatabaseType;
  /** Database server hostname or IP address */
  host: string;
  /** Database server port */
  port: number;
  /** Name of the database to connect to */
  databaseName: string;
  /** Username for authentication */
  username: string;
  /** Password for authentication (not persisted) */
  password: string;
  /** SSL/TLS mode for the connection */
  sslMode?: string;
  /** Original connection string if parsed from one */
  connectionString?: string;
  /** Timestamp of last successful connection */
  lastConnected?: Date;
  /** Provider connection ID for database operations */
  providerConnectionId?: string;
  /** SSH tunnel configuration */
  sshTunnel?: SSHTunnelConfig;
  /** Local port for SSH tunnel forwarding */
  tunnelLocalPort?: number;
  /** Whether the database password is saved in keychain */
  savePassword?: boolean;
  /** Whether the SSH password is saved in keychain */
  saveSshPassword?: boolean;
  /** Whether the SSH key passphrase is saved in keychain */
  saveSshKeyPassphrase?: boolean;
  /** ID of the project this connection belongs to */
  projectId: string;
  /** Array of label IDs assigned to this connection */
  labelIds: string[];
  /** If created from a shared connection template, its ID */
  sharedConnectionId?: string;
  /**
   * Where a linked connection came from (Core's, read only): `exported`
   * (shared from this project) or `imported` (a template the repo brought).
   * Absent when not known; a linked connection without it came from the repo.
   */
  sharedOrigin?: "exported" | "imported";
  /** Whether this connection is excluded from Git sharing (local-only) */
  isLocalOnly?: boolean;
  /** Whether to share schema with AI for this connection (undefined = use global default) */
  aiShareSchema?: boolean;
  /** Whether to share data with AI for this connection (undefined = use global default) */
  aiShareData?: boolean;
  /** Active AI provider ID for this connection */
  activeAIProviderId?: string;
  /** Active AI model for this connection */
  activeAIModel?: string;
}
