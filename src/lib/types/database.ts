/**
 * Database connection types and configuration.
 * @module types/database
 */

import type Database from '@tauri-apps/plugin-sql';

/**
 * Supported database engine types.
 */
export type DatabaseType = 'postgres' | 'mysql' | 'sqlite' | 'mongodb' | 'mariadb' | 'mssql' | 'duckdb';

/**
 * SSH tunnel authentication methods.
 */
export type SSHAuthMethod = 'password' | 'key';

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
	/** @deprecated Use providerConnectionId. Active database connection handle (tauri-plugin-sql) */
	database?: Database;
	/** Connection ID for MSSQL (uses custom Rust backend) */
	mssqlConnectionId?: string;
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
}
