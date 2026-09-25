//! Dialect DTOs: schema introspection, EXPLAIN plans, database statistics and
//! the Create Table form.
//!
//! These mirror the TypeScript definitions that used to live in
//! `src/lib/types/{schema,explain,statistics,create-table}.ts`; those files now
//! re-export the generated bindings. Fields are camelCase on the wire, and
//! `Option` fields are omitted when `None` (TS code checks them with
//! `=== undefined`).
//!
//! ts-rs maps `i64` to `bigint`, but serde_json sends plain JSON numbers, so
//! every integer field is overridden to `number`.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Schema (src/lib/types/schema.ts)
// ---------------------------------------------------------------------------

/// Reference to a foreign key target column.
/// Describes which column in another table this foreign key points to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct ForeignKeyRef {
    /// Schema name of the referenced table
    pub referenced_schema: String,
    /// Name of the referenced table
    pub referenced_table: String,
    /// Name of the referenced column
    pub referenced_column: String,
}

/// Represents a column in a database table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct SchemaColumn {
    /// Column name
    pub name: String,
    /// Data type (e.g., 'varchar(255)', 'integer', 'timestamp')
    #[serde(rename = "type")]
    pub ty: String,
    /// The column's SQL type for `CAST($n AS …)` in CRUD statements. Postgres
    /// builds it from the catalog: user types schema-qualified (`myschema.mood`,
    /// `myschema.mood[]`), built-ins as `format_type(atttypid, atttypmod)`
    /// (`integer[]`, `numeric(10,2)`), and character and bit types, found through
    /// any domains, without their length (`character varying`, `bpchar`, `"bit"`),
    /// because an explicit cast to a length truncates silently. Interpolated into
    /// SQL as is, so it must only come from the catalog. Absent for engines that
    /// don't report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cast_type: Option<String>,
    /// Whether the column allows NULL values
    pub nullable: bool,
    /// Default value expression, if any
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    /// Whether this column is part of the primary key
    pub is_primary_key: bool,
    /// Whether this column is a foreign key
    pub is_foreign_key: bool,
    /// Foreign key reference details, if this is a foreign key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_key_ref: Option<ForeignKeyRef>,
    /// The column's collation, only when it differs from the database
    /// default (MSSQL), so that an `ALTER COLUMN` built from it keeps the
    /// collation. Absent for engines that don't report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
    /// The column on its own is a UNIQUE constraint (the table editor's
    /// UNIQUE checkbox). Reported by DuckDB; other engines leave it false.
    #[serde(default, skip_serializing_if = "is_false")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub is_unique: bool,
    /// The column is in some UNIQUE constraint, single-column or composite
    /// (not the primary key, not a unique index). DuckDB can't drop or retype
    /// such a column, or drop one before it, so its ALTER TABLE rules read
    /// this. Not shown in the editor. Reported by DuckDB only.
    #[serde(default, skip_serializing_if = "is_false")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub in_unique_constraint: bool,
}

/// `skip_serializing_if` for flags that are absent when false.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Represents an index on a database table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct SchemaIndex {
    /// Index name
    pub name: String,
    /// Columns included in the index
    pub columns: Vec<String>,
    /// Whether this is a unique index
    pub unique: bool,
    /// Index type (e.g., 'btree', 'hash', 'gin')
    #[serde(rename = "type")]
    pub ty: String,
}

/// Whether a schema object is a table, a view or a materialized view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
}

/// Represents a table or view in the database schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct SchemaTable {
    /// Table or view name
    pub name: String,
    /// Schema name (e.g., 'public' in PostgreSQL)
    pub schema: String,
    /// Whether this is a table or view
    #[serde(rename = "type")]
    pub kind: TableKind,
    /// Approximate row count, if available
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>"))]
    pub row_count: Option<i64>,
    /// Column definitions
    pub columns: Vec<SchemaColumn>,
    /// Index definitions
    pub indexes: Vec<SchemaIndex>,
}

// ---------------------------------------------------------------------------
// EXPLAIN (src/lib/types/explain.ts)
// ---------------------------------------------------------------------------

/// A node in the query execution plan tree.
/// Represents a single operation in the database's query plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct ExplainPlanNode {
    /// Unique identifier for this node
    pub id: String,
    /// Type of operation (e.g., 'Seq Scan', 'Index Scan', 'Hash Join')
    pub node_type: String,
    /// Table or relation name being accessed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_name: Option<String>,
    /// Alias for the relation in the query
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Estimated cost to start returning rows (undefined for engines that don't expose cost)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_cost: Option<f64>,
    /// Estimated total cost to complete the operation (undefined for engines that don't expose cost)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost: Option<f64>,
    /// Estimated number of rows to be returned (undefined when the engine doesn't provide estimates)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_rows: Option<f64>,
    /// Estimated average width of rows in bytes (undefined when the engine doesn't provide it)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>"))]
    pub plan_width: Option<i64>,

    // ANALYZE fields (actual execution statistics)
    /// Actual time to start returning rows (ms)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_startup_time: Option<f64>,
    /// Actual total execution time (ms)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_total_time: Option<f64>,
    /// Actual number of rows returned
    // f64: PostgreSQL 18 reports fractional per-loop averages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_rows: Option<f64>,
    /// Number of times this node was executed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>"))]
    pub actual_loops: Option<i64>,

    // Conditions and additional info
    /// Filter condition applied to rows
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// Name of the index being used
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_name: Option<String>,
    /// Index condition for index scans
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_cond: Option<String>,
    /// Type of join (for join nodes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_type: Option<String>,
    /// Hash condition for hash joins
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_cond: Option<String>,
    /// Sort keys for sort operations
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_key: Option<Vec<String>>,

    /// Child nodes in the plan tree
    pub children: Vec<ExplainPlanNode>,
}

/// Complete result of an EXPLAIN or EXPLAIN ANALYZE query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct ExplainResult {
    /// Root node of the execution plan tree
    pub plan: ExplainPlanNode,
    /// Time spent planning the query (ms)
    pub planning_time: f64,
    /// Time spent executing the query (ms) - only for ANALYZE
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_time: Option<f64>,
    /// Whether this was an EXPLAIN ANALYZE (vs plain EXPLAIN)
    pub is_analyze: bool,
}

// ---------------------------------------------------------------------------
// Statistics (src/lib/types/statistics.ts)
// ---------------------------------------------------------------------------

/// Information about a table's size and storage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct TableSizeInfo {
    /// Schema name
    pub schema: String,
    /// Table name
    pub name: String,
    /// Number of rows in the table
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub row_count: i64,
    /// Human-readable total size (e.g., "1.2 GB")
    pub total_size: String,
    /// Total size in bytes for sorting
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total_size_bytes: i64,
    /// Human-readable data size
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_size: Option<String>,
    /// Human-readable index size
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_size: Option<String>,
}

/// Information about index usage and performance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct IndexUsageInfo {
    /// Schema name
    pub schema: String,
    /// Table the index belongs to
    pub table: String,
    /// Index name
    pub index_name: String,
    /// Human-readable index size
    pub size: String,
    /// Number of index scans performed
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub scans: i64,
    /// Number of rows read via this index
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>"))]
    pub rows_read: Option<i64>,
    /// Whether the index has never been used
    pub unused: bool,
}

/// Overview statistics for the entire database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct DatabaseOverview {
    /// Database name
    pub database_name: String,
    /// Human-readable total database size
    pub total_size: String,
    /// Total size in bytes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>"))]
    pub total_size_bytes: Option<i64>,
    /// Number of tables
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub table_count: i64,
    /// Number of indexes
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub index_count: i64,
    /// Number of active connections (if available)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>"))]
    pub connection_count: Option<i64>,
}

/// Complete statistics data for a database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct DatabaseStatistics {
    /// Overview metrics
    pub overview: DatabaseOverview,
    /// Size information for each table
    pub table_sizes: Vec<TableSizeInfo>,
    /// Usage information for each index
    pub index_usage: Vec<IndexUsageInfo>,
}

// ---------------------------------------------------------------------------
// Create Table (src/lib/types/create-table.ts)
// ---------------------------------------------------------------------------

/// Grouping category for the column type picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ColumnCategory {
    String,
    Numeric,
    #[serde(rename = "Date/Time")]
    DateTime,
    Boolean,
    #[serde(rename = "JSON")]
    Json,
    Binary,
    #[serde(rename = "UUID")]
    Uuid,
    Network,
    Other,
}

/// Describes a column type available for a specific database engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct ColumnTypeInfo {
    /// Type name as used in DDL, e.g. "VARCHAR", "INTEGER"
    pub name: String,
    /// Grouping category for the UI picker
    pub category: ColumnCategory,
    /// Whether the type accepts a length parameter, e.g. VARCHAR(255)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_length: Option<bool>,
    /// Whether the type accepts precision/scale, e.g. DECIMAL(10,2)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_precision: Option<bool>,
}

/// A single column definition in the Create Table form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct CreateTableColumn {
    /// Stable ID for keying in lists
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    /// Length parameter, e.g. 255 for VARCHAR(255)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<String>,
    /// Precision parameter, e.g. "10,2" for DECIMAL(10,2)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precision: Option<String>,
    pub nullable: bool,
    pub default_value: String,
    pub is_primary_key: bool,
    pub is_unique: bool,
    /// The collation `ALTER COLUMN` restates (MSSQL `COLLATE`), copied from
    /// [`SchemaColumn::collation`] by the table editor. Unset means the
    /// database default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
    /// Copied from [`SchemaColumn::in_unique_constraint`] by the table
    /// editor: the column is in a (possibly composite) UNIQUE constraint.
    /// DDL generation reads it, never writes a constraint from it.
    #[serde(default, skip_serializing_if = "is_false")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub in_unique_constraint: bool,
}

/// An index definition in the Create Table form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct CreateTableIndex {
    pub id: String,
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    #[serde(rename = "type")]
    pub ty: String,
}

/// A foreign key constraint in the Create Table form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct CreateTableForeignKey {
    pub id: String,
    pub column: String,
    pub referenced_schema: String,
    pub referenced_table: String,
    pub referenced_column: String,
}

/// Complete table definition being built in the Create Table form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct CreateTableDefinition {
    pub table_name: String,
    pub schema_name: String,
    pub columns: Vec<CreateTableColumn>,
    pub indexes: Vec<CreateTableIndex>,
    pub foreign_keys: Vec<CreateTableForeignKey>,
}
