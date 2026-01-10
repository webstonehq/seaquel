/**
 * Feature flags for demo mode vs desktop app.
 * Controls which features are available in each environment.
 */

import { isDemo as checkIsDemo } from '$lib/utils/environment';

/**
 * Feature flags interface.
 */
export interface FeatureFlags {
	/** Allow creating new database connections */
	newConnections: boolean;
	/** Show SSH tunnel configuration options */
	sshTunnels: boolean;
	/** Show MSSQL connection option */
	mssqlSupport: boolean;
	/** Allow file export (CSV, JSON, etc.) */
	fileExport: boolean;
	/** Show app updater UI */
	appUpdater: boolean;
	/** Allow editing connection settings */
	editConnections: boolean;
	/** Show the AI assistant */
	aiAssistant: boolean;
	/** Allow saving queries */
	savedQueries: boolean;
	/** Show connection type selector */
	connectionTypeSelector: boolean;
}

/**
 * Get feature flags for the current environment.
 */
export function getFeatures(): FeatureFlags {
	const demo = checkIsDemo();

	return {
		newConnections: !demo,
		sshTunnels: !demo,
		mssqlSupport: !demo,
		fileExport: !demo, // Could enable with browser download API
		appUpdater: !demo,
		editConnections: !demo,
		aiAssistant: true, // Works in demo
		savedQueries: true, // Uses localStorage in demo
		connectionTypeSelector: !demo
	};
}

/**
 * Check if a specific feature is enabled.
 */
export function isFeatureEnabled(feature: keyof FeatureFlags): boolean {
	return getFeatures()[feature];
}

/**
 * Check if running in demo mode.
 */
export function isDemo(): boolean {
	return checkIsDemo();
}
