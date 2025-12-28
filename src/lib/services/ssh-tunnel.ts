import { invoke } from "@tauri-apps/api/core";

export interface TunnelConfig {
	sshHost: string;
	sshPort: number;
	sshUsername: string;
	authMethod: "password" | "key";
	password?: string;
	keyPath?: string;
	keyPassphrase?: string;
	remoteHost: string;
	remotePort: number;
}

export interface TunnelResult {
	tunnelId: string;
	localPort: number;
}

export async function createSshTunnel(config: TunnelConfig): Promise<TunnelResult> {
	const result = await invoke<{ tunnel_id: string; local_port: number }>("create_ssh_tunnel", {
		config: {
			ssh_host: config.sshHost,
			ssh_port: config.sshPort,
			ssh_username: config.sshUsername,
			auth_method: config.authMethod,
			password: config.password,
			key_path: config.keyPath,
			key_passphrase: config.keyPassphrase,
			remote_host: config.remoteHost,
			remote_port: config.remotePort,
		},
	});

	return {
		tunnelId: result.tunnel_id,
		localPort: result.local_port,
	};
}

export async function closeSshTunnel(tunnelId: string): Promise<void> {
	await invoke("close_ssh_tunnel", { tunnelId });
}
