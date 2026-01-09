/**
 * Database schema types for tables, columns, and indexes.
 * @module types/schema
 */

/**
 * Reference to a foreign key target column.
 * Describes which column in another table this foreign key points to.
 */
export interface ForeignKeyRef {
	/** Schema name of the referenced table */
	referencedSchema: string;
	/** Name of the referenced table */
	referencedTable: string;
	/** Name of the referenced column */
	referencedColumn: string;
}

/**
 * Represents a column in a database table.
 */
export interface SchemaColumn {
	/** Column name */
	name: string;
	/** Data type (e.g., 'varchar(255)', 'integer', 'timestamp') */
	type: string;
	/** Whether the column allows NULL values */
	nullable: boolean;
	/** Default value expression, if any */
	defaultValue?: string;
	/** Whether this column is part of the primary key */
	isPrimaryKey: boolean;
	/** Whether this column is a foreign key */
	isForeignKey: boolean;
	/** Foreign key reference details, if this is a foreign key */
	foreignKeyRef?: ForeignKeyRef;
}

/**
 * Represents an index on a database table.
 */
export interface SchemaIndex {
	/** Index name */
	name: string;
	/** Columns included in the index */
	columns: string[];
	/** Whether this is a unique index */
	unique: boolean;
	/** Index type (e.g., 'btree', 'hash', 'gin') */
	type: string;
}

/**
 * Represents a table or view in the database schema.
 */
export interface SchemaTable {
	/** Table or view name */
	name: string;
	/** Schema name (e.g., 'public' in PostgreSQL) */
	schema: string;
	/** Whether this is a table or view */
	type: 'table' | 'view';
	/** Approximate row count, if available */
	rowCount?: number;
	/** Column definitions */
	columns: SchemaColumn[];
	/** Index definitions */
	indexes: SchemaIndex[];
}

/**
 * Represents an open schema browser tab.
 */
export interface SchemaTab {
	/** Unique tab identifier */
	id: string;
	/** The table being viewed */
	table: SchemaTable;
}
