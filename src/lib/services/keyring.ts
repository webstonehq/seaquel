/**
 * Keyring service for secure password storage using OS-native keychains.
 * - macOS: Keychain
 * - Windows: Credential Manager
 * - Linux: Secret Service (GNOME Keyring, KWallet)
 *
 * Falls back to a no-op implementation in browser demo mode.
 */

import { isTauri } from '$lib/utils/environment';

const SERVICE = 'app.seaquel.desktop';

export interface KeyringService {
	setDbPassword(connectionId: string, password: string): Promise<void>;
	getDbPassword(connectionId: string): Promise<string | null>;
	deleteDbPassword(connectionId: string): Promise<void>;

	setSshPassword(connectionId: string, password: string): Promise<void>;
	getSshPassword(connectionId: string): Promise<string | null>;
	deleteSshPassword(connectionId: string): Promise<void>;

	setSshKeyPassphrase(connectionId: string, passphrase: string): Promise<void>;
	getSshKeyPassphrase(connectionId: string): Promise<string | null>;
	deleteSshKeyPassphrase(connectionId: string): Promise<void>;

	deleteAllForConnection(connectionId: string): Promise<void>;
	isAvailable(): boolean;
}

/**
 * Tauri implementation using the native keyring plugin.
 */
class TauriKeyringService implements KeyringService {
	private keyringApi: typeof import('tauri-plugin-keyring-api') | null = null;
	private initPromise: Promise<void> | null = null;

	private async init(): Promise<void> {
		if (this.keyringApi) return;
		if (this.initPromise) return this.initPromise;

		this.initPromise = import('tauri-plugin-keyring-api').then((api) => {
			this.keyringApi = api;
		});

		return this.initPromise;
	}

	async setDbPassword(connectionId: string, password: string): Promise<void> {
		await this.init();
		await this.keyringApi!.setPassword(SERVICE, `db:${connectionId}`, password);
	}

	async getDbPassword(connectionId: string): Promise<string | null> {
		await this.init();
		try {
			return await this.keyringApi!.getPassword(SERVICE, `db:${connectionId}`);
		} catch {
			// Entry doesn't exist or keychain error
			return null;
		}
	}

	async deleteDbPassword(connectionId: string): Promise<void> {
		await this.init();
		try {
			await this.keyringApi!.deletePassword(SERVICE, `db:${connectionId}`);
		} catch {
			// Ignore - entry may not exist
		}
	}

	async setSshPassword(connectionId: string, password: string): Promise<void> {
		await this.init();
		await this.keyringApi!.setPassword(SERVICE, `ssh:${connectionId}`, password);
	}

	async getSshPassword(connectionId: string): Promise<string | null> {
		await this.init();
		try {
			return await this.keyringApi!.getPassword(SERVICE, `ssh:${connectionId}`);
		} catch {
			return null;
		}
	}

	async deleteSshPassword(connectionId: string): Promise<void> {
		await this.init();
		try {
			await this.keyringApi!.deletePassword(SERVICE, `ssh:${connectionId}`);
		} catch {
			// Ignore
		}
	}

	async setSshKeyPassphrase(connectionId: string, passphrase: string): Promise<void> {
		await this.init();
		await this.keyringApi!.setPassword(SERVICE, `ssh-key:${connectionId}`, passphrase);
	}

	async getSshKeyPassphrase(connectionId: string): Promise<string | null> {
		await this.init();
		try {
			return await this.keyringApi!.getPassword(SERVICE, `ssh-key:${connectionId}`);
		} catch {
			return null;
		}
	}

	async deleteSshKeyPassphrase(connectionId: string): Promise<void> {
		await this.init();
		try {
			await this.keyringApi!.deletePassword(SERVICE, `ssh-key:${connectionId}`);
		} catch {
			// Ignore
		}
	}

	async deleteAllForConnection(connectionId: string): Promise<void> {
		await Promise.all([
			this.deleteDbPassword(connectionId),
			this.deleteSshPassword(connectionId),
			this.deleteSshKeyPassphrase(connectionId)
		]);
	}

	isAvailable(): boolean {
		return true;
	}
}

/**
 * No-op implementation for browser demo mode.
 */
class NoopKeyringService implements KeyringService {
	async setDbPassword(): Promise<void> {}
	async getDbPassword(): Promise<string | null> {
		return null;
	}
	async deleteDbPassword(): Promise<void> {}
	async setSshPassword(): Promise<void> {}
	async getSshPassword(): Promise<string | null> {
		return null;
	}
	async deleteSshPassword(): Promise<void> {}
	async setSshKeyPassphrase(): Promise<void> {}
	async getSshKeyPassphrase(): Promise<string | null> {
		return null;
	}
	async deleteSshKeyPassphrase(): Promise<void> {}
	async deleteAllForConnection(): Promise<void> {}
	isAvailable(): boolean {
		return false;
	}
}

let keyringService: KeyringService | null = null;

/**
 * Get the keyring service instance.
 * Returns a Tauri implementation in desktop app, or a no-op in browser demo.
 */
export function getKeyringService(): KeyringService {
	if (keyringService) return keyringService;

	if (isTauri()) {
		keyringService = new TauriKeyringService();
	} else {
		keyringService = new NoopKeyringService();
	}

	return keyringService;
}
