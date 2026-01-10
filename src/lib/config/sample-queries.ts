import type { DatabaseType } from "$lib/types";

export interface SampleQuery {
	id: string;
	name: string;
	description: string;
	query: string;
	requiresTable?: boolean;
}

export const sampleQueries: Record<DatabaseType, SampleQuery[]> = {
	postgres: [
		{
			id: "pg-list-tables",
			name: "List all tables",
			description: "View all tables in your database",
			query: `SELECT table_name, table_type
FROM information_schema.tables
WHERE table_schema = 'public'
ORDER BY table_name;`,
		},
		{
			id: "pg-sample-data",
			name: "Preview table data",
			description: "View sample rows from a table",
			query: `SELECT *
FROM your_table
LIMIT 10;`,
			requiresTable: true,
		},
		{
			id: "pg-table-size",
			name: "Check table sizes",
			description: "See how much space each table uses",
			query: `SELECT
    relname AS table_name,
    pg_size_pretty(pg_total_relation_size(relid)) AS total_size
FROM pg_catalog.pg_statio_user_tables
ORDER BY pg_total_relation_size(relid) DESC;`,
		},
	],
	mysql: [
		{
			id: "mysql-list-tables",
			name: "List all tables",
			description: "View all tables in your database",
			query: `SHOW TABLES;`,
		},
		{
			id: "mysql-sample-data",
			name: "Preview table data",
			description: "View sample rows from a table",
			query: `SELECT *
FROM your_table
LIMIT 10;`,
			requiresTable: true,
		},
		{
			id: "mysql-table-size",
			name: "Check table sizes",
			description: "See how much space each table uses",
			query: `SELECT
    table_name,
    ROUND((data_length + index_length) / 1024 / 1024, 2) AS size_mb
FROM information_schema.tables
WHERE table_schema = DATABASE()
ORDER BY (data_length + index_length) DESC;`,
		},
	],
	mariadb: [
		{
			id: "mariadb-list-tables",
			name: "List all tables",
			description: "View all tables in your database",
			query: `SHOW TABLES;`,
		},
		{
			id: "mariadb-sample-data",
			name: "Preview table data",
			description: "View sample rows from a table",
			query: `SELECT *
FROM your_table
LIMIT 10;`,
			requiresTable: true,
		},
	],
	sqlite: [
		{
			id: "sqlite-list-tables",
			name: "List all tables",
			description: "View all tables in your database",
			query: `SELECT name, type
FROM sqlite_master
WHERE type IN ('table', 'view')
ORDER BY name;`,
		},
		{
			id: "sqlite-sample-data",
			name: "Preview table data",
			description: "View sample rows from a table",
			query: `SELECT *
FROM your_table
LIMIT 10;`,
			requiresTable: true,
		},
		{
			id: "sqlite-table-info",
			name: "View table structure",
			description: "See columns and types for a table",
			query: `PRAGMA table_info(your_table);`,
			requiresTable: true,
		},
	],
	mongodb: [
		{
			id: "mongo-list-collections",
			name: "List all collections",
			description: "View all collections in your database",
			query: `db.getCollectionNames()`,
		},
		{
			id: "mongo-sample-data",
			name: "Preview collection data",
			description: "View sample documents from a collection",
			query: `db.your_collection.find().limit(10)`,
			requiresTable: true,
		},
	],
	mssql: [
		{
			id: "mssql-list-tables",
			name: "List all tables",
			description: "View all tables in your database",
			query: `SELECT TABLE_NAME, TABLE_TYPE
FROM INFORMATION_SCHEMA.TABLES
WHERE TABLE_TYPE = 'BASE TABLE'
ORDER BY TABLE_NAME;`,
		},
		{
			id: "mssql-sample-data",
			name: "Preview table data",
			description: "View sample rows from a table",
			query: `SELECT TOP 10 *
FROM your_table;`,
			requiresTable: true,
		},
	],
	duckdb: [
		{
			id: "duckdb-list-tables",
			name: "List all tables",
			description: "View all tables in your database",
			query: `SELECT table_schema, table_name
FROM information_schema.tables
WHERE table_type = 'BASE TABLE'
ORDER BY table_schema, table_name;`,
		},
		{
			id: "duckdb-sample-data",
			name: "Preview table data",
			description: "View sample rows from a table",
			query: `SELECT *
FROM your_table
LIMIT 10;`,
			requiresTable: true,
		},
	],
};

export const savedQueryExample: SampleQuery = {
	id: "saved-example",
	name: "Active Users Report",
	description: "Find users who logged in recently",
	query: `SELECT
    name,
    email,
    last_login
FROM users
WHERE last_login > NOW() - INTERVAL '30 days'
ORDER BY last_login DESC;`,
};
