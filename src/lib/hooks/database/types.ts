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
  sshTunnel?: SSHTunnelConfig | null;
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
  /** Whether this connection is excluded from Git sharing (local-only) */
  isLocalOnly?: boolean;
  /** If created from a shared connection template, its ID */
  sharedConnectionId?: string;
  /** Whether the AI is allowed to share the DB schema for this connection */
  aiShareSchema?: boolean;
  /** Whether the AI is allowed to share data (query results) for this connection */
  aiShareData?: boolean;
  /** Active AI provider ID for this connection */
  activeAIProviderId?: string;
  /** Active AI model for this connection */
  activeAIModel?: string;
}
