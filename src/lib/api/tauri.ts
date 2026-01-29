/**
 * Typed Tauri API layer.
 * Centralizes all Tauri invoke() calls with compile-time type safety.
 * Eliminates magic command strings scattered across the codebase.
 */

import { invoke } from '@tauri-apps/api/core';

// Re-export existing well-typed service modules
export * as git from '$lib/services/git';
export * as mssql from '$lib/services/mssql';
export * as sshTunnel from '$lib/services/ssh-tunnel';

// === DuckDB Commands ===

export interface DuckDBConnectResult {
	connection_id: string;
}

export interface DuckDBQueryResult {
	columns: string[];
	rows: unknown[][];
}

export interface DuckDBExecuteResult {
	rows_affected: number;
}

export async function duckdbConnect(path: string): Promise<DuckDBConnectResult> {
	return invoke<DuckDBConnectResult>('duckdb_connect', { path });
}

export async function duckdbDisconnect(connectionId: string): Promise<void> {
	await invoke('duckdb_disconnect', { connectionId });
}

export async function duckdbQuery(connectionId: string, sql: string): Promise<DuckDBQueryResult> {
	return invoke<DuckDBQueryResult>('duckdb_query', { connectionId, sql });
}

export async function duckdbExecute(connectionId: string, sql: string): Promise<DuckDBExecuteResult> {
	return invoke<DuckDBExecuteResult>('duckdb_execute', { connectionId, sql });
}

export async function duckdbTest(path: string): Promise<void> {
	await invoke('duckdb_test', { path });
}

// === App Commands ===

export async function copyImageToClipboard(path: string): Promise<void> {
	await invoke('copy_image_to_clipboard', { path });
}

export async function openPath(path: string): Promise<void> {
	await invoke('open_path', { path });
}

export async function getDataDir(): Promise<string> {
	return invoke<string>('get_data_dir');
}

export async function installUpdate(): Promise<void> {
	await invoke('install_update');
}

export async function readDbeaverConfig(): Promise<string | null> {
	return invoke<string | null>('read_dbeaver_config');
}
