/**
 * Starter tab types for the no-connection state.
 * Starter tabs are shown when no database connection is active,
 * providing quick actions and migration guidance.
 * @module types/starter-tabs
 */

/**
 * Types of starter tabs available.
 */
export type StarterTabType = 'getting-started' | 'migration-tips';

/**
 * Represents a starter tab shown when no connection is active.
 */
export interface StarterTab {
	/** Unique tab identifier */
	id: string;
	/** Type of starter tab content */
	type: StarterTabType;
	/** Tab display name */
	name: string;
	/** Whether the tab can be closed by the user */
	closable: boolean;
}

/**
 * Default starter tabs to show for new projects.
 */
export const DEFAULT_STARTER_TABS: StarterTab[] = [
	{
		id: 'getting-started',
		type: 'getting-started',
		name: 'Getting Started',
		closable: true
	},
	{
		id: 'migration-tips',
		type: 'migration-tips',
		name: 'Migration Tips',
		closable: true
	}
];
