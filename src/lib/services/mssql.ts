import { invoke } from "@tauri-apps/api/core";

export interface MssqlConfig {
	host: string;
	port: number;
	database: string;
	username: string;
	password: string;
	encrypt?: boolean;
	trustCert?: boolean;
}

export interface MssqlConnection {
	connectionId: string;
}

export interface MssqlQueryResult {
	columns: string[];
	rows: Record<string, unknown>[];
	rowsAffected: number;
}

interface MssqlError {
	message: string;
	code: string;
}

function isMssqlError(error: unknown): error is MssqlError {
	return typeof error === "object" && error !== null && "message" in error && "code" in error;
}

function formatError(error: unknown): Error {
	if (isMssqlError(error)) {
		return new Error(`${error.code}: ${error.message}`);
	}
	if (error instanceof Error) {
		return error;
	}
	if (typeof error === "string") {
		return new Error(error);
	}
	console.error("Unknown MSSQL error:", error);
	return new Error("An unknown error occurred");
}

export async function mssqlConnect(config: MssqlConfig): Promise<MssqlConnection> {
	try {
		const result = await invoke<{ connection_id: string }>("mssql_connect", {
			config: {
				host: config.host,
				port: config.port,
				database: config.database,
				username: config.username,
				password: config.password,
				encrypt: config.encrypt,
				trust_cert: config.trustCert,
			},
		});

		return {
			connectionId: result.connection_id,
		};
	} catch (error) {
		throw formatError(error);
	}
}

export async function mssqlDisconnect(connectionId: string): Promise<void> {
	try {
		await invoke("mssql_disconnect", { connectionId });
	} catch (error) {
		throw formatError(error);
	}
}

export async function mssqlQuery(
	connectionId: string,
	sql: string
): Promise<MssqlQueryResult> {
	try {
		const result = await invoke<{
			columns: string[];
			rows: Record<string, unknown>[];
			rows_affected: number;
		}>("mssql_query", {
			connectionId,
			sql,
		});

		return {
			columns: result.columns,
			rows: result.rows,
			rowsAffected: result.rows_affected,
		};
	} catch (error) {
		throw formatError(error);
	}
}

export async function mssqlExecute(
	connectionId: string,
	sql: string
): Promise<MssqlQueryResult> {
	try {
		const result = await invoke<{
			columns: string[];
			rows: Record<string, unknown>[];
			rows_affected: number;
		}>("mssql_execute", {
			connectionId,
			sql,
		});

		return {
			columns: result.columns,
			rows: result.rows,
			rowsAffected: result.rows_affected,
		};
	} catch (error) {
		throw formatError(error);
	}
}
