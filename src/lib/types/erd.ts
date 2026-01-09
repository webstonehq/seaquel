/**
 * Entity Relationship Diagram (ERD) types.
 * @module types/erd
 */

/**
 * Represents an open ERD viewer tab.
 */
export interface ErdTab {
	/** Unique tab identifier */
	id: string;
	/** Tab display name */
	name: string;
}
