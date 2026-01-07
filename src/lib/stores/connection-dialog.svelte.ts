import type { DatabaseType, SSHAuthMethod } from "$lib/types";
import { connectionWizardStore } from "./connection-wizard.svelte.js";

export type ConnectionDialogMode = "wizard" | "quick" | "reconnect";

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
	mode = $state<ConnectionDialogMode>("wizard");

	/**
	 * Opens the connection dialog.
	 * - New connections default to wizard mode
	 * - Reconnections use reconnect mode (password-only prompt)
	 * - Use mode: "quick" for power users who want the classic form
	 */
	open(prefill?: ConnectionDialogPrefill, mode?: ConnectionDialogMode) {
		// Determine mode based on context
		const resolvedMode = mode ?? (prefill?.id ? "reconnect" : "wizard");

		// Route to wizard for new connections and reconnections
		if (resolvedMode === "wizard" || resolvedMode === "reconnect") {
			connectionWizardStore.open(resolvedMode, prefill);
			return;
		}

		// Quick mode uses classic dialog
		this.prefill = prefill;
		this.mode = resolvedMode;
		this.isOpen = true;
	}

	close() {
		this.isOpen = false;
		this.prefill = undefined;
	}
}

export const connectionDialogStore = new ConnectionDialogStore();
