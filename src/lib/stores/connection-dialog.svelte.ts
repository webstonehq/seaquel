import type { DatabaseType, SSHAuthMethod } from "$lib/types";
import { connectionWizardStore, type WizardMode } from "./connection-wizard.svelte.js";

export interface ConnectionDialogPrefill {
	id?: string;
	name?: string;
	type?: DatabaseType;
	host?: string;
	port?: number;
	databaseName?: string;
	username?: string;
	password?: string;
	sslMode?: string;
	connectionString?: string;
	sshTunnel?: {
		enabled: boolean;
		host: string;
		port: number;
		username: string;
		authMethod: SSHAuthMethod;
	};
	savePassword?: boolean;
	saveSshPassword?: boolean;
	saveSshKeyPassphrase?: boolean;
}

class ConnectionDialogStore {
	/**
	 * Opens the connection wizard.
	 * - New connections default to wizard mode
	 * - Reconnections use reconnect mode (password-only prompt)
	 */
	open(prefill?: ConnectionDialogPrefill, mode?: WizardMode) {
		const resolvedMode = mode ?? (prefill?.id ? "reconnect" : "wizard");
		connectionWizardStore.open(resolvedMode, prefill);
	}

	close() {
		connectionWizardStore.close();
	}
}

export const connectionDialogStore = new ConnectionDialogStore();
