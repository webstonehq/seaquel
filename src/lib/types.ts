import type Database from "@tauri-apps/plugin-sql";
import type { QueryType } from './db/query-utils';

export type DatabaseType = "postgres" | "mysql" | "sqlite" | "mongodb" | "mariadb" | "mssql";

export type SSHAuthMethod = "password" | "key";

export interface SSHTunnelConfig {
	enabled: boolean;
	host: string;
	port: number;
	username: string;
	authMethod: SSHAuthMethod;
}

export interface DatabaseConnection {
	id: string;
	name: string;
	type: DatabaseType;
	host: string;
	port: number;
	databaseName: string;
	username: string;
	password: string;
	sslMode?: string;
	connectionString?: string;
	lastConnected?: Date;
	database?: Database;
	sshTunnel?: SSHTunnelConfig;
	tunnelLocalPort?: number;
}

export interface SchemaTable {
	name: string;
	schema: string;
	type: "table" | "view";
	rowCount?: number;
	columns: SchemaColumn[];
	indexes: SchemaIndex[];
}

export interface SchemaColumn {
	name: string;
	type: string;
	nullable: boolean;
	defaultValue?: string;
	isPrimaryKey: boolean;
	isForeignKey: boolean;
	foreignKeyRef?: string;
}

export interface SchemaIndex {
	name: string;
	columns: string[];
	unique: boolean;
	type: string;
}

export interface QueryTab {
	id: string;
	name: string;
	query: string;
	results?: QueryResult;
	isExecuting: boolean;
	savedQueryId?: string;
}

export interface SchemaTab {
	id: string;
	table: SchemaTable;
}

export interface QueryResult {
	columns: string[];
	rows: any[];
	rowCount: number;
	totalRows: number;
	executionTime: number;
	affectedRows?: number;
	lastInsertId?: number;
	queryType?: QueryType;
	sourceTable?: {
		schema: string;
		name: string;
		primaryKeys: string[];
	};
	page: number;
	pageSize: number;
	totalPages: number;
}

export interface QueryHistoryItem {
	id: string;
	query: string;
	timestamp: Date;
	executionTime: number;
	rowCount: number;
	connectionId: string;
	favorite: boolean;
}

export interface SavedQuery {
	id: string;
	name: string;
	query: string;
	connectionId: string;
	createdAt: Date;
	updatedAt: Date;
}

export interface AIMessage {
	id: string;
	role: "user" | "assistant";
	content: string;
	timestamp: Date;
	query?: string;
}