import type { DatabaseType, SSHAuthMethod } from "$lib/types";

export interface ConnectionDialogPrefill {
	id?: string;
	name?: string;
	type?: DatabaseType;
	host?: string;
	port?: number;
	databaseName?: string;
	username?: string;
	sslMode?: string;
	connectionString?: string;
	sshTunnel?: {
		enabled: boolean;
		host: string;
		port: number;
		username: string;
		authMethod: SSHAuthMethod;
	};
}

class ConnectionDialogStore {
	isOpen = $state(false);
	prefill = $state<ConnectionDialogPrefill | undefined>(undefined);

	open(prefill?: ConnectionDialogPrefill) {
		this.prefill = prefill;
		this.isOpen = true;
	}

	close() {
		this.isOpen = false;
		this.prefill = undefined;
	}
}

export const connectionDialogStore = new ConnectionDialogStore();
