//! MySQL/MariaDB introspection: the catalog SQL and the pure functions that
//! turn its results into Seaquel's schema, statistics and EXPLAIN types.
//!
//! Ported from `src/lib/db/mysql.ts` (deleted in phase 2). The SQL is the
//! TypeScript text except for bug fixes 1 (bound catalog filters), 3 (foreign keys from
//! `KEY_COLUMN_USAGE` alone, returned as three columns), 4 (`table_type`
//! aliased so it's read) and 9 (index usage reads `database_name`);
//! `tests/introspect_parity.rs` pins the SQL and the parsers against the
//! recorded TypeScript output of both servers.
//!
//! Parser changes from the TypeScript, each a numbered bug fix:
//! - 3: `foreign_key_schema`/`_table`/`_column`, when present, give the
//!   reference (the TS split a `CONCAT` on every `.`).
//! - 4: catalog text that arrives as bytes is UTF-8, not Latin-1.
//! - 5: `EXPLAIN ANALYZE` text with fractional or scientific numbers, cost
//!   ranges (`cost=1.84..3.06`), `(never executed)` and relation names that
//!   aren't one ASCII word (`<temporary>`, `fx order items`, `fx_café`).
//! - 8: defaults are SQL expressions (MySQL's literal values quoted, MariaDB's
//!   bare `NULL` dropped), so the table editor can copy them into DDL.
//! - 10: JSON plans the TS showed as empty: a schema v2 plan at the root
//!   (MySQL 8.3+ with `explain_json_format_version = 2`; the TS only looked
//!   under `query_plan`), and a v1 `windowing` block (window functions).
//!
//! MariaDB's EXPLAIN JSON has neither `cost_info` nor
//! `rows_examined_per_scan` and wraps tables in `filesort`,
//! `temporary_table`, …, so it has its own branch ([`Flavor::Mariadb`]),
//! which also reads `ANALYZE FORMAT=JSON` (MariaDB has no `EXPLAIN ANALYZE`).
//!
//! Parsers read cells by column name, as the TS row objects did.

use serde_json::{Map, Value as Json};

use seaquel_engine::introspect::{js_string_to_number, js_trim, node, rows, truthy, Ids, Row};
use seaquel_engine::{DbError, QueryResult, Value};
use seaquel_types::{
    DatabaseOverview, ExplainPlanNode, ExplainResult, ForeignKeyRef, IndexUsageInfo, SchemaColumn,
    SchemaIndex, SchemaTable, TableKind, TableSizeInfo,
};

/// Which server a connection talks to: `SELECT VERSION()` contains
/// `MariaDB` on MariaDB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Mysql,
    Mariadb,
}

impl Flavor {
    pub fn from_version(version: &str) -> Self {
        if version.to_ascii_lowercase().contains("mariadb") {
            Flavor::Mariadb
        } else {
            Flavor::Mysql
        }
    }
}

// ── SQL ──────────────────────────────────────────────────────────────────────

/// `SELECT VERSION()`, run at connect to tell MariaDB from MySQL.
pub const VERSION_SQL: &str = "SELECT VERSION() AS version";

/// Schema (database) names (TS `getSchemasQuery`). Column: `schema_name`.
pub const SCHEMAS_SQL: &str =
    "SELECT SCHEMA_NAME as schema_name FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY SCHEMA_NAME;";

/// Tables and views of the current database (TS `getSchemaQuery`). Bug fix
/// 4: `table_type AS table_type`. MySQL 8 names the bare column
/// `TABLE_TYPE`, so the TS never found it and listed views as tables.
pub const SCHEMA_SQL: &str = "SELECT\n\t\t\ttable_schema AS schema_name,\n\t\t\ttable_name AS table_name,\n\t\t\ttable_type AS table_type\n\t\tFROM\n\t\t\tinformation_schema.tables\n\t\tWHERE\n\t\t\ttable_type IN ('BASE TABLE', 'VIEW')\n\t\t\tAND table_schema = DATABASE()\n\t\tORDER BY\n\t\t\ttable_schema, table_name";

/// Columns of one table (TS `getColumnsQuery`). Bug fix 1: binds `?` =
/// table, `?` = schema instead of splicing them. Bug fix 3: a column is a
/// foreign key when `KEY_COLUMN_USAGE` has a reference for it, whatever its
/// `COLUMN_KEY` (the TS required `MUL`, so a PRI or UNI column that is also a
/// foreign key wasn't one), and the reference comes back as three columns
/// from the same foreign key (the first by constraint name) instead of a
/// `CONCAT` that a dotted name breaks. Bug fix 8: `EXTRA`, which tells a
/// MySQL expression default (`DEFAULT_GENERATED`) from a literal one.
pub const COLUMNS_SQL: &str = "SELECT
\t\t\tc.COLUMN_NAME AS column_name,
\t\t\tc.COLUMN_TYPE AS data_type,
\t\t\tc.IS_NULLABLE AS is_nullable,
\t\t\tc.COLUMN_DEFAULT AS column_default,
\t\t\tc.EXTRA AS extra,
\t\t\tIF(c.COLUMN_KEY = 'PRI', 1, 0) AS is_primary_key,
\t\t\tIF(EXISTS (
\t\t\t\tSELECT 1 FROM information_schema.KEY_COLUMN_USAGE kcu
\t\t\t\tWHERE kcu.TABLE_SCHEMA = c.TABLE_SCHEMA
\t\t\t\t\tAND kcu.TABLE_NAME = c.TABLE_NAME
\t\t\t\t\tAND kcu.COLUMN_NAME = c.COLUMN_NAME
\t\t\t\t\tAND kcu.REFERENCED_TABLE_NAME IS NOT NULL
\t\t\t), 1, 0) AS is_foreign_key,
\t\t\t(SELECT kcu.REFERENCED_TABLE_SCHEMA
\t\t\t\tFROM information_schema.KEY_COLUMN_USAGE kcu
\t\t\t\tWHERE kcu.TABLE_SCHEMA = c.TABLE_SCHEMA
\t\t\t\t\tAND kcu.TABLE_NAME = c.TABLE_NAME
\t\t\t\t\tAND kcu.COLUMN_NAME = c.COLUMN_NAME
\t\t\t\t\tAND kcu.REFERENCED_TABLE_NAME IS NOT NULL
\t\t\t\tORDER BY kcu.CONSTRAINT_NAME
\t\t\t\tLIMIT 1
\t\t\t) AS foreign_key_schema,
\t\t\t(SELECT kcu.REFERENCED_TABLE_NAME
\t\t\t\tFROM information_schema.KEY_COLUMN_USAGE kcu
\t\t\t\tWHERE kcu.TABLE_SCHEMA = c.TABLE_SCHEMA
\t\t\t\t\tAND kcu.TABLE_NAME = c.TABLE_NAME
\t\t\t\t\tAND kcu.COLUMN_NAME = c.COLUMN_NAME
\t\t\t\t\tAND kcu.REFERENCED_TABLE_NAME IS NOT NULL
\t\t\t\tORDER BY kcu.CONSTRAINT_NAME
\t\t\t\tLIMIT 1
\t\t\t) AS foreign_key_table,
\t\t\t(SELECT kcu.REFERENCED_COLUMN_NAME
\t\t\t\tFROM information_schema.KEY_COLUMN_USAGE kcu
\t\t\t\tWHERE kcu.TABLE_SCHEMA = c.TABLE_SCHEMA
\t\t\t\t\tAND kcu.TABLE_NAME = c.TABLE_NAME
\t\t\t\t\tAND kcu.COLUMN_NAME = c.COLUMN_NAME
\t\t\t\t\tAND kcu.REFERENCED_TABLE_NAME IS NOT NULL
\t\t\t\tORDER BY kcu.CONSTRAINT_NAME
\t\t\t\tLIMIT 1
\t\t\t) AS foreign_key_column
\t\tFROM information_schema.COLUMNS c
\t\tWHERE c.TABLE_NAME = ? AND c.TABLE_SCHEMA = ?
\t\tORDER BY c.ORDINAL_POSITION";

/// Index key columns of one table, one row per column (TS
/// `getIndexesQuery`). Bug fix 1: binds `?` = table, `?` = schema.
pub const INDEXES_SQL: &str = "SELECT\n\t\t\tINDEX_NAME AS index_name,\n\t\t\tCOLUMN_NAME AS column_name,\n\t\t\tNON_UNIQUE AS non_unique,\n\t\t\tINDEX_TYPE AS index_type\n\t\tFROM information_schema.STATISTICS\n\t\tWHERE TABLE_NAME = ? AND TABLE_SCHEMA = ?\n\t\tORDER BY INDEX_NAME, SEQ_IN_INDEX";

/// Table sizes (TS `getTableSizesQuery`).
pub const TABLE_SIZES_SQL: &str = "SELECT\n\t\t\tTABLE_SCHEMA AS schema_name,\n\t\t\tTABLE_NAME AS table_name,\n\t\t\tTABLE_ROWS AS row_count,\n\t\t\tCONCAT(ROUND((DATA_LENGTH + INDEX_LENGTH) / 1024, 2), ' KB') AS total_size,\n\t\t\t(DATA_LENGTH + INDEX_LENGTH) AS total_size_bytes,\n\t\t\tCONCAT(ROUND(DATA_LENGTH / 1024, 2), ' KB') AS data_size,\n\t\t\tCONCAT(ROUND(INDEX_LENGTH / 1024, 2), ' KB') AS index_size\n\t\tFROM information_schema.TABLES\n\t\tWHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE'\n\t\tORDER BY (DATA_LENGTH + INDEX_LENGTH) DESC";

/// Index sizes from InnoDB's persistent statistics (TS
/// `getIndexUsageQuery`). Bug fix 9: `mysql.innodb_index_stats` has
/// `database_name`, not `TABLE_SCHEMA`; the TS query failed on both servers
/// and took the whole Statistics view down. Reading the table needs a grant
/// on `mysql`; the driver treats a permission error as no index usage.
pub const INDEX_USAGE_SQL: &str = "SELECT\n\t\t\tdatabase_name AS schema_name,\n\t\t\tTABLE_NAME AS table_name,\n\t\t\tINDEX_NAME AS index_name,\n\t\t\tCONCAT(ROUND(STAT_VALUE * @@innodb_page_size / 1024, 2), ' KB') AS size,\n\t\t\t0 AS scans,\n\t\t\t0 AS rows_read,\n\t\t\t0 AS unused\n\t\tFROM mysql.innodb_index_stats\n\t\tWHERE stat_name = 'size' AND database_name = DATABASE()\n\t\tORDER BY STAT_VALUE DESC";

/// Database overview (TS `getDatabaseOverviewQuery`).
pub const OVERVIEW_SQL: &str = "SELECT\n\t\t\tDATABASE() AS database_name,\n\t\t\tCONCAT(ROUND(SUM(DATA_LENGTH + INDEX_LENGTH) / 1024, 2), ' KB') AS total_size,\n\t\t\tSUM(DATA_LENGTH + INDEX_LENGTH) AS total_size_bytes,\n\t\t\tCOUNT(*) AS table_count,\n\t\t\t(SELECT COUNT(*) FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = DATABASE()) AS index_count,\n\t\t\t(SELECT COUNT(*) FROM information_schema.PROCESSLIST) AS connection_count\n\t\tFROM information_schema.TABLES\n\t\tWHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE'";

/// The EXPLAIN statement a driver runs. MySQL uses the dialect's (TS)
/// `EXPLAIN FORMAT=JSON` / `EXPLAIN ANALYZE`; MariaDB has no
/// `EXPLAIN ANALYZE` (a syntax error), so it analyzes with
/// `ANALYZE FORMAT=JSON`.
pub fn explain_sql(sql: &str, analyze: bool, flavor: Flavor) -> String {
    let base = sql.strip_suffix(';').unwrap_or(sql);
    match (flavor, analyze) {
        (Flavor::Mariadb, true) => format!("ANALYZE FORMAT=JSON {base}"),
        (_, true) => format!("EXPLAIN ANALYZE {base}"),
        (_, false) => format!("EXPLAIN FORMAT=JSON {base}"),
    }
}

// ── Row access ───────────────────────────────────────────────────────────────

/// MySQL's readings of a catalog cell, on top of the shared [`Row`]
/// (`catalog_text`, not `text`: `Row::text` would shadow it).
trait MyRow {
    fn catalog_text(&self, column: &str) -> String;
    fn opt_catalog_text(&self, column: &str) -> Option<String>;
}

impl MyRow for Row<'_> {
    /// TS `decodeValue` (bug fix 4: bytes are UTF-8, not Latin-1).
    fn catalog_text(&self, column: &str) -> String {
        catalog_text(self.get(column))
    }

    /// A text cell, `None` when `Null`.
    fn opt_catalog_text(&self, column: &str) -> Option<String> {
        match self.get(column) {
            Value::Null => None,
            v => Some(catalog_text(v)),
        }
    }
}

/// TS `decodeValue`: a string as is, a byte array as text, anything else
/// `String(value ?? "")`. Bug fix 4: bytes (the driver's integer arrays for
/// binary-collated catalog columns such as `COLUMN_TYPE` and
/// `COLUMN_DEFAULT`, or `Bytes`) are decoded as UTF-8; the TS used
/// `String.fromCharCode`, i.e. Latin-1, so `é` showed as `Ã©`.
fn catalog_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Text(s) | Value::Decimal(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => js_number_string(*f),
        Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        Value::Array(items) => {
            let bytes: Option<Vec<u8>> = items
                .iter()
                .map(|i| match i {
                    Value::Int(n) => u8::try_from(*n).ok(),
                    _ => None,
                })
                .collect();
            match bytes {
                Some(b) => String::from_utf8_lossy(&b).into_owned(),
                None => items.iter().map(catalog_text).collect::<Vec<_>>().join(","),
            }
        }
        Value::Json(j) => j.to_string(),
    }
}

/// JS `String(n)` for the numbers catalog cells hold.
fn js_number_string(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else if f == f.trunc() && f.abs() < 1e21 {
        format!("{}", f as i64)
    } else {
        f.to_string()
    }
}

// ── Schema ───────────────────────────────────────────────────────────────────

/// Rows of [`SCHEMAS_SQL`].
pub fn parse_schemas(result: &QueryResult) -> Vec<String> {
    rows(result)
        .map(|r| r.catalog_text("schema_name"))
        .collect()
}

/// Rows of [`SCHEMA_SQL`] (TS `parseSchemaResult`). Rows whose name decodes
/// to `""` are dropped, as the TS did.
pub fn parse_schema(result: &QueryResult) -> Vec<SchemaTable> {
    rows(result)
        .map(|r| SchemaTable {
            name: r.catalog_text("table_name"),
            schema: r.catalog_text("schema_name"),
            // Bug fix 4: decoded too (the TS compared the raw cell).
            kind: if r.catalog_text("table_type") == "VIEW" {
                TableKind::View
            } else {
                TableKind::Table
            },
            row_count: None,
            columns: vec![],
            indexes: vec![],
        })
        .filter(|t| !t.name.is_empty())
        .collect()
}

/// Rows of [`COLUMNS_SQL`] (TS `parseColumnsResult`). Also reads the TS
/// query's rows (`foreign_key_ref` as `schema.table.column`), which the
/// parity fixtures hold.
pub fn parse_columns(result: &QueryResult, flavor: Flavor) -> Vec<SchemaColumn> {
    rows(result)
        .map(|r| {
            let foreign_key_ref = if r.has("foreign_key_table") {
                // Bug fix 3.
                r.opt_catalog_text("foreign_key_table")
                    .filter(|t| !t.is_empty())
                    .map(|table| ForeignKeyRef {
                        referenced_schema: r.catalog_text("foreign_key_schema"),
                        referenced_table: table,
                        referenced_column: r.catalog_text("foreign_key_column"),
                    })
            } else {
                // `col.foreign_key_ref ? decodeValue(…) : null`, then split on
                // every '.' and kept with exactly three parts.
                Some(r.get("foreign_key_ref"))
                    .filter(|v| truthy(v))
                    .map(catalog_text)
                    .filter(|s| !s.is_empty())
                    .and_then(|s| match s.split('.').collect::<Vec<_>>()[..] {
                        [schema, table, column] => Some(ForeignKeyRef {
                            referenced_schema: schema.to_string(),
                            referenced_table: table.to_string(),
                            referenced_column: column.to_string(),
                        }),
                        _ => None,
                    })
            };
            SchemaColumn {
                name: r.catalog_text("column_name"),
                ty: r.catalog_text("data_type"),
                cast_type: None,
                nullable: r.catalog_text("is_nullable") == "YES",
                default_value: column_default(&r, flavor),
                is_primary_key: r.truthy("is_primary_key"),
                is_foreign_key: r.truthy("is_foreign_key"),
                foreign_key_ref,
                collation: None,
                is_unique: false,
                in_unique_constraint: false,
            }
        })
        .collect()
}

/// Bug fix 8: `SchemaColumn::default_value` is the default as a SQL
/// expression, as Postgres reports it (`'active'::character varying`): the
/// table editor copies it into `DEFAULT …` verbatim.
/// - MariaDB already reports one (`'active'`, `'it''s'`, `current_timestamp()`,
///   `0.00`), except that a column without a default is the bare word `NULL`,
///   which is no default.
/// - MySQL reports a literal default as its value (`active`, `it's`, nothing
///   for an empty string), which is quoted here unless the column is numeric or
///   BIT (the value is a valid literal) or `EXTRA` says `DEFAULT_GENERATED`
///   (an expression: `CURRENT_TIMESTAMP` as is, others in parentheses, as
///   MySQL requires). It needs [`COLUMNS_SQL`]'s `extra` column; the TS
///   query's rows (the parity fixtures) have none and keep the TS value.
fn column_default(r: &Row, flavor: Flavor) -> Option<String> {
    let raw = r.get("column_default");
    match flavor {
        Flavor::Mariadb => Some(raw)
            .filter(|v| truthy(v))
            .map(catalog_text)
            .filter(|d| d != "NULL"),
        Flavor::Mysql if r.has("extra") => match raw {
            Value::Null => None,
            v => Some(mysql_default(
                &catalog_text(v),
                &r.catalog_text("extra"),
                &r.catalog_text("data_type"),
            )),
        },
        // `col.column_default ? decodeValue(col.column_default) : undefined`
        Flavor::Mysql => Some(raw).filter(|v| truthy(v)).map(catalog_text),
    }
}

/// MySQL's `COLUMN_DEFAULT` value as a SQL expression (see [`column_default`]).
fn mysql_default(value: &str, extra: &str, column_type: &str) -> String {
    if extra.to_ascii_uppercase().contains("DEFAULT_GENERATED") {
        let upper = value.to_ascii_uppercase();
        let bare = upper.split('(').next().is_some_and(|f| {
            matches!(
                f,
                "CURRENT_TIMESTAMP" | "NOW" | "LOCALTIME" | "LOCALTIMESTAMP"
            )
        });
        if bare {
            return value.to_string();
        }
        // MySQL escapes the quotes of an expression's string literals
        // (`concat(_utf8mb4\'a\')`), which isn't valid SQL.
        return format!("({})", value.replace("\\'", "'"));
    }
    let base = column_type
        .to_ascii_lowercase()
        .split(|c: char| c == '(' || c.is_whitespace())
        .next()
        .unwrap_or_default()
        .to_string();
    let literal = matches!(
        base.as_str(),
        "tinyint"
            | "smallint"
            | "mediumint"
            | "int"
            | "integer"
            | "bigint"
            | "decimal"
            | "numeric"
            | "float"
            | "double"
            | "real"
            | "bit"
            | "year"
            | "bool"
            | "boolean"
    );
    if literal {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
    }
}

/// Rows of [`INDEXES_SQL`] (TS `parseIndexesResult`): one row per key
/// column, grouped by index name in first-seen order. A functional key part
/// has no `COLUMN_NAME` and shows as `""`, as in the TS.
pub fn parse_indexes(result: &QueryResult) -> Vec<SchemaIndex> {
    let mut indexes: Vec<SchemaIndex> = Vec::new();
    for r in rows(result) {
        let name = r.catalog_text("index_name");
        let column = r.catalog_text("column_name");
        match indexes.iter_mut().find(|i| i.name == name) {
            Some(existing) => existing.columns.push(column),
            None => indexes.push(SchemaIndex {
                name,
                columns: vec![column],
                unique: !r.truthy("non_unique"),
                ty: r.catalog_text("index_type").to_lowercase(),
            }),
        }
    }
    indexes
}

// ── Statistics ───────────────────────────────────────────────────────────────

/// Rows of [`TABLE_SIZES_SQL`] (TS `parseTableSizesResult`).
pub fn parse_table_sizes(result: &QueryResult) -> Vec<TableSizeInfo> {
    rows(result)
        .map(|r| TableSizeInfo {
            schema: r.catalog_text("schema_name"),
            name: r.catalog_text("table_name"),
            row_count: r.number("row_count"),
            total_size: r.catalog_text("total_size"),
            total_size_bytes: r.number("total_size_bytes"),
            data_size: r.opt_catalog_text("data_size"),
            index_size: r.opt_catalog_text("index_size"),
        })
        .collect()
}

/// Rows of [`INDEX_USAGE_SQL`] (TS `parseIndexUsageResult`).
pub fn parse_index_usage(result: &QueryResult) -> Vec<IndexUsageInfo> {
    rows(result)
        .map(|r| IndexUsageInfo {
            schema: r.catalog_text("schema_name"),
            table: r.catalog_text("table_name"),
            index_name: r.catalog_text("index_name"),
            size: r.catalog_text("size"),
            scans: r.number("scans"),
            rows_read: Some(r.number("rows_read")),
            unused: r.truthy("unused"),
        })
        .collect()
}

/// The row of [`OVERVIEW_SQL`] (TS `parseDatabaseOverviewResult`), with the
/// TS fallbacks when there is no row or a cell is NULL.
pub fn parse_overview(result: &QueryResult) -> DatabaseOverview {
    let row = rows(result).next();
    let text = |c: &str, fallback: &str| {
        row.as_ref()
            .and_then(|r| r.opt_catalog_text(c))
            .unwrap_or_else(|| fallback.to_string())
    };
    let number = |c: &str| row.as_ref().map_or(0, |r| r.number(c));
    DatabaseOverview {
        database_name: text("database_name", "Unknown"),
        total_size: text("total_size", "0 bytes"),
        total_size_bytes: Some(number("total_size_bytes")),
        table_count: number("table_count"),
        index_count: number("index_count"),
        connection_count: Some(number("connection_count")),
    }
}

// ── EXPLAIN ──────────────────────────────────────────────────────────────────

static EMPTY: std::sync::OnceLock<Map<String, Json>> = std::sync::OnceLock::new();

/// A JSON value as an object; anything else reads as `{}` (a TS property
/// read on a non-object is `undefined`).
fn obj(v: Option<&Json>) -> &Map<String, Json> {
    v.and_then(Json::as_object)
        .unwrap_or_else(|| EMPTY.get_or_init(Map::new))
}

/// JS truthiness of a JSON value (`undefined` is `None`).
fn json_truthy(v: Option<&Json>) -> bool {
    match v {
        None | Some(Json::Null) | Some(Json::Bool(false)) => false,
        Some(Json::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Json::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// JS `Number(v)` of a JSON value; `undefined` is NaN, `null` is 0.
fn json_number(v: Option<&Json>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Json::Null) => 0.0,
        Some(Json::Bool(b)) => f64::from(u8::from(*b)),
        Some(Json::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Json::String(s)) => js_string_to_number(s),
        Some(_) => f64::NAN,
    }
}

/// A TS number as it survives JSON: NaN and ±Infinity become `null`.
fn finite(f: f64) -> Option<f64> {
    f.is_finite().then_some(f)
}

fn json_str(map: &Map<String, Json>, key: &str) -> Option<String> {
    map.get(key).and_then(Json::as_str).map(str::to_string)
}

/// The first cell of the first row (TS `Object.values(rows[0])[0]`).
fn first_cell(result: &QueryResult) -> Option<&Value> {
    result.rows.first().and_then(|r| r.first())
}

/// The result of the driver's EXPLAIN: `EXPLAIN FORMAT=JSON` (v1 or v2),
/// `EXPLAIN ANALYZE` text on MySQL, `ANALYZE FORMAT=JSON` on MariaDB (TS
/// `parseExplainResult`, plus the MariaDB branch). Planning time is 0 and
/// MySQL reports no execution time, as in the TS.
pub fn parse_explain(
    result: &QueryResult,
    analyze: bool,
    flavor: Flavor,
) -> Result<ExplainResult, DbError> {
    if flavor == Flavor::Mysql && analyze {
        let text = result
            .rows
            .iter()
            .map(|r| match r.first() {
                Some(Value::Text(s)) => s.as_str(),
                _ => "",
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(ExplainResult {
            plan: parse_analyze_tree(&text),
            planning_time: 0.0,
            execution_time: None,
            is_analyze: true,
        });
    }

    let parsed = match first_cell(result) {
        None | Some(Value::Null) => Json::Null,
        Some(Value::Text(s)) => serde_json::from_str(s)
            .map_err(|e| DbError::query_error(format!("invalid EXPLAIN output: {e}")))?,
        Some(Value::Json(j)) => j.clone(),
        Some(other) => serde_json::to_value(other).unwrap_or(Json::Null),
    };

    if flavor == Flavor::Mariadb {
        return Ok(mariadb_explain(&parsed, analyze));
    }

    let mut ids = Ids::new();
    let root = parsed.as_object();
    let plan = if json_truthy(root.and_then(|r| r.get("query_plan"))) {
        convert_v2(obj(root.and_then(|r| r.get("query_plan"))), &mut ids)
    } else if json_truthy(root.and_then(|r| r.get("query_block"))) {
        convert_v1(obj(root.and_then(|r| r.get("query_block"))), &mut ids)
    } else if let Some(r) = root.filter(|r| r.get("operation").is_some_and(Json::is_string)) {
        // Bug fix 10: MySQL's schema v2 puts the plan at the root.
        convert_v2(r, &mut ids)
    } else {
        ExplainPlanNode {
            id: ids.next(),
            ..node("Query")
        }
    };
    Ok(ExplainResult {
        plan,
        planning_time: 0.0,
        execution_time: None,
        is_analyze: false,
    })
}

/// TS `ACCESS_TYPE_LABELS[accessType] ?? accessType`.
fn access_label(access_type: &str) -> String {
    match access_type {
        "ALL" => "Table Scan",
        "const" | "system" => "Const",
        "eq_ref" | "index" | "ref" | "ref_or_null" => "Index Scan",
        "fulltext" => "Fulltext Scan",
        "index_merge" => "Index Merge",
        "index_subquery" => "Index Subquery",
        "range" => "Range Scan",
        "unique_subquery" => "Unique Subquery",
        other => other,
    }
    .to_string()
}

/// JS `value || "ALL"` for `access_type`, as text.
fn access_type(table: &Map<String, Json>) -> String {
    let v = table.get("access_type");
    if !json_truthy(v) {
        return "ALL".into();
    }
    match v {
        Some(Json::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => "ALL".into(),
    }
}

/// TS `convertMysqlPlanNode`: schema v1 (`query_block`). Ids are pre-order,
/// except a plain query block, whose id follows its attached subqueries'.
fn convert_v1(n: &Map<String, Json>, ids: &mut Ids) -> ExplainPlanNode {
    let query_cost = if json_truthy(n.get("cost_info")) {
        finite(json_number(obj(n.get("cost_info")).get("query_cost")))
    } else {
        None
    };

    if json_truthy(n.get("table")) {
        let table = obj(n.get("table"));
        let costs = obj(table.get("cost_info"));
        let cost = |k: &str| json_number(Some(costs.get(k).unwrap_or(&Json::from(0))));
        let (read, eval) = (cost("read_cost"), cost("eval_cost"));
        let js_or = |a: f64| a != 0.0 && !a.is_nan();
        let total_cost = if js_or(read) || js_or(eval) {
            finite(read + eval)
        } else {
            None
        };
        let mut out = ExplainPlanNode {
            id: ids.next(),
            relation_name: json_str(table, "table_name"),
            index_name: json_str(table, "key"),
            filter: json_str(table, "attached_condition"),
            plan_rows: table.get("rows_examined_per_scan").and_then(Json::as_f64),
            total_cost,
            ..node(access_label(&access_type(table)))
        };
        let materialized = obj(table.get("materialized_from_subquery"));
        if json_truthy(materialized.get("query_block")) {
            out.children
                .push(convert_v1(obj(materialized.get("query_block")), ids));
        }
        return out;
    }

    let wrap = |node_type: &str, key: &str, ids: &mut Ids| {
        let id = ids.next();
        ExplainPlanNode {
            id,
            total_cost: query_cost,
            children: vec![convert_v1(obj(n.get(key)), ids)],
            ..node(node_type)
        }
    };

    if json_truthy(n.get("nested_loop")) {
        let id = ids.next();
        let items = n.get("nested_loop").and_then(Json::as_array);
        let children = items
            .map(|items| {
                items
                    .iter()
                    .map(|i| convert_v1(obj(Some(i)), ids))
                    .collect()
            })
            .unwrap_or_default();
        return ExplainPlanNode {
            id,
            total_cost: query_cost,
            children,
            ..node("Nested Loop")
        };
    }
    if json_truthy(n.get("grouping_operation")) {
        return wrap("Group", "grouping_operation", ids);
    }
    if json_truthy(n.get("ordering_operation")) {
        return wrap("Sort", "ordering_operation", ids);
    }
    if json_truthy(n.get("duplicates_removal")) {
        return wrap("Distinct", "duplicates_removal", ids);
    }
    if json_truthy(n.get("windowing")) {
        // Bug fix 10: window functions (the TS dropped the whole plan).
        return wrap("Window", "windowing", ids);
    }
    if json_truthy(n.get("union_result")) {
        let id = ids.next();
        let specs = obj(n.get("union_result"))
            .get("query_specifications")
            .and_then(Json::as_array);
        let children = specs
            .map(|specs| {
                specs
                    .iter()
                    .map(|spec| {
                        let qb = obj(Some(spec)).get("query_block");
                        if json_truthy(qb) {
                            convert_v1(obj(qb), ids)
                        } else {
                            ExplainPlanNode {
                                id: ids.next(),
                                ..node("Query Block")
                            }
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        return ExplainPlanNode {
            id,
            children,
            ..node("Union")
        };
    }

    // A plain query block: attached subqueries first, then its own id.
    let mut children = Vec::new();
    if json_truthy(n.get("attached_subqueries")) {
        for sub in n
            .get("attached_subqueries")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let qb = obj(Some(sub)).get("query_block");
            if json_truthy(qb) {
                children.push(convert_v1(obj(qb), ids));
            }
        }
    }
    ExplainPlanNode {
        id: ids.next(),
        total_cost: query_cost,
        children,
        ..node("Query Block")
    }
}

/// JS `Math.round`: halves round up (toward +∞).
fn js_round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// TS `convertMysqlV2PlanNode`: schema v2, one operation per node with its
/// `inputs` as children. Ids are pre-order.
fn convert_v2(n: &Map<String, Json>, ids: &mut Ids) -> ExplainPlanNode {
    let id = ids.next();
    let operation = match n.get("operation") {
        None | Some(Json::Null) => "Operator".to_string(),
        Some(Json::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    };
    let head = operator_name(&operation);
    let node_type = if head.is_empty() {
        operation.clone()
    } else {
        head
    };
    let children = n
        .get("inputs")
        .and_then(Json::as_array)
        .map(|inputs| {
            inputs
                .iter()
                .map(|i| convert_v2(obj(Some(i)), ids))
                .collect()
        })
        .unwrap_or_default();
    ExplainPlanNode {
        id,
        relation_name: json_str(n, "table_name"),
        alias: json_str(n, "alias"),
        index_name: json_str(n, "index_name"),
        join_type: json_str(n, "join_type"),
        index_cond: json_str(n, "lookup_condition"),
        filter: json_str(n, "condition"),
        plan_rows: n.get("estimated_rows").and_then(Json::as_f64).map(js_round),
        total_cost: n.get("estimated_total_cost").and_then(Json::as_f64),
        children,
        ..node(node_type)
    }
}

/// JS `\s` (ASCII whitespace plus Unicode spaces).
fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{FEFF}'
}

/// TS `text.split(/\s+on\s+|\s*\(/)[0].trim()`: the operator name, before
/// the first ` on ` or `(`.
fn operator_name(text: &str) -> String {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, &(pos, c)) in chars.iter().enumerate() {
        // Alternative 1: \s+on\s+
        if is_js_space(c) {
            let mut j = i;
            while j < chars.len() && is_js_space(chars[j].1) {
                j += 1;
            }
            if j + 2 < chars.len()
                && chars[j].1 == 'o'
                && chars[j + 1].1 == 'n'
                && is_js_space(chars[j + 2].1)
            {
                return js_trim(&text[..pos]).to_string();
            }
            // Alternative 2 from a space: \s*\(
            if j < chars.len() && chars[j].1 == '(' {
                return js_trim(&text[..pos]).to_string();
            }
        }
        if c == '(' {
            return js_trim(&text[..pos]).to_string();
        }
    }
    js_trim(text).to_string()
}

/// A number as MySQL prints it in `EXPLAIN ANALYZE`: digits, `.`, and
/// (bug fix 5) an exponent (`1e+6`, `170e-6`).
fn scan_number(s: &str) -> Option<(f64, &str)> {
    let end = s
        .char_indices()
        .find(|&(i, c)| {
            !(c.is_ascii_digit()
                || c == '.' && !s[i..].starts_with("..")
                || matches!(c, 'e' | 'E')
                || matches!(c, '+' | '-') && i > 0 && matches!(s.as_bytes()[i - 1], b'e' | b'E'))
        })
        .map_or(s.len(), |(i, _)| i);
    if end == 0 {
        return None;
    }
    let n = s[..end].parse::<f64>().ok()?;
    Some((n, &s[end..]))
}

/// `(cost=A rows=R)` or `(cost=A..B rows=R)`: (startup, total, rows).
fn parse_estimate(block: &str) -> Option<(Option<f64>, f64, f64)> {
    let rest = block.strip_prefix("(cost=")?;
    let (first, rest) = scan_number(rest)?;
    let (startup, total, rest) = match rest.strip_prefix("..") {
        Some(r) => {
            let (second, r) = scan_number(r)?;
            (Some(first), second, r)
        }
        None => (None, first, rest),
    };
    let rest = rest.trim_start_matches(is_js_space).strip_prefix("rows=")?;
    let (rows, rest) = scan_number(rest)?;
    (rest == ")").then_some((startup, total, rows))
}

/// `(actual time=A..B rows=R loops=L)`: (startup, total, rows, loops).
fn parse_actual(block: &str) -> Option<(f64, f64, f64, f64)> {
    let rest = block.strip_prefix("(actual time=")?;
    let (a, rest) = scan_number(rest)?;
    let (b, rest) = scan_number(rest.strip_prefix("..")?)?;
    let rest = rest.trim_start_matches(is_js_space).strip_prefix("rows=")?;
    let (rows, rest) = scan_number(rest)?;
    let rest = rest
        .trim_start_matches(is_js_space)
        .strip_prefix("loops=")?;
    let (loops, rest) = scan_number(rest)?;
    (rest == ")").then_some((a, b, rows, loops))
}

/// Every top-level `(…)` group of `s` with its byte range.
fn paren_groups(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            ')' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    out.push((start, i + 1));
                }
            }
            _ => {}
        }
    }
    out
}

/// A relation or index name as printed, without surrounding backticks.
fn unquote_name(s: &str) -> String {
    let s = js_trim(s);
    s.strip_prefix('`')
        .and_then(|s| s.strip_suffix('`'))
        .map_or_else(|| s.to_string(), |s| s.replace("``", "`"))
}

/// Bug fix 5: `on <relation> [using <index>]`, looked for only in the
/// operator's text before its first ` (`, so a condition's `' on '` isn't a
/// relation. The names run to ` using ` / the end (MySQL prints them
/// unquoted: `fx order items`, `<temporary>`, `fx_café`); a range scan's
/// trailing ` over` isn't part of the index.
fn relation_and_index(head: &str) -> (Option<String>, Option<String>) {
    let prefix = head.find(" (").map_or(head, |i| &head[..i]);
    // `on` between spaces (the TS matched `\bon\s+`).
    let at = prefix
        .match_indices("on")
        .map(|(i, _)| i)
        .find(|&i| prefix[..i].ends_with(is_js_space) && prefix[i + 2..].starts_with(is_js_space));
    let at = at.map(|i| i + 2);
    let Some(at) = at else {
        return (None, None);
    };
    let rest = js_trim(&prefix[at..]);
    let (relation, index) = match rest.find(" using ") {
        Some(u) => (&rest[..u], Some(&rest[u + " using ".len()..])),
        None => (rest, None),
    };
    let index = index.map(|i| i.strip_suffix(" over").unwrap_or(i));
    (
        Some(unquote_name(relation)).filter(|r| !r.is_empty()),
        index.map(unquote_name).filter(|i| !i.is_empty()),
    )
}

/// TS `parseMysqlAnalyzeTree` with bug fix 5: MySQL 8.0.18+ `EXPLAIN
/// ANALYZE` text, one `-> Operator  (cost=…) (actual …)` per line, children
/// indented deeper than their parent. A single top-level operator is the
/// root; otherwise a `Query` node holds them.
fn parse_analyze_tree(text: &str) -> ExplainPlanNode {
    let mut ids = Ids::new();
    let mut root = ExplainPlanNode {
        id: ids.next(),
        ..node("Query")
    };
    // (indent, path of child indexes from the root)
    let mut stack: Vec<(isize, Vec<usize>)> = vec![(-1, vec![])];

    for raw in text.split('\n').filter(|l| !js_trim(l).is_empty()) {
        let Some(indent) = raw.find("->") else {
            continue;
        };
        let body = js_trim(&raw[indent + 2..]);
        let indent = raw[..indent].encode_utf16().count() as isize;

        let mut n = ExplainPlanNode {
            id: ids.next(),
            ..node("Operator")
        };
        let mut head = body.to_string();
        let mut never_executed = false;
        for (start, end) in paren_groups(body).into_iter().rev() {
            let block = &body[start..end];
            let consumed = if let Some((startup, total, rows)) = parse_estimate(block) {
                n.startup_cost = startup;
                n.total_cost = Some(total);
                n.plan_rows = Some(rows);
                true
            } else if let Some((a, b, rows, loops)) = parse_actual(block) {
                n.actual_startup_time = Some(a);
                n.actual_total_time = Some(b);
                n.actual_rows = Some(rows);
                n.actual_loops = Some(loops as i64);
                true
            } else if block == "(never executed)" {
                never_executed = true;
                true
            } else {
                false
            };
            // Only the trailing metadata groups: stop at the operator's own.
            if !consumed {
                break;
            }
            head.replace_range(start..end, "");
        }
        if never_executed {
            n.actual_rows = Some(0.0);
            n.actual_loops = Some(0);
        }

        let head = js_trim(&head);
        let (relation, index) = relation_and_index(head);
        n.relation_name = relation;
        n.index_name = index;
        let name = operator_name(head);
        n.node_type = if name.is_empty() {
            "Operator".into()
        } else {
            name
        };

        while stack.len() > 1 && stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let parent_path = stack.last().map(|(_, p)| p.clone()).unwrap_or_default();
        let parent = node_at(&mut root, &parent_path);
        parent.children.push(n);
        let mut path = parent_path;
        path.push(parent.children.len() - 1);
        stack.push((indent, path));
    }

    if root.children.len() == 1 {
        return root.children.remove(0);
    }
    root
}

fn node_at<'a>(root: &'a mut ExplainPlanNode, path: &[usize]) -> &'a mut ExplainPlanNode {
    path.iter().fold(root, |n, &i| &mut n.children[i])
}

// ── MariaDB EXPLAIN ──────────────────────────────────────────────────────────

/// MariaDB's `EXPLAIN FORMAT=JSON` / `ANALYZE FORMAT=JSON`. Every query
/// block holds its tables in `nested_loop` (or `table`), possibly wrapped in
/// `filesort`, `temporary_table`, `read_sorted_file`, … Tables report
/// `rows` (estimated rows examined, like MySQL's `rows_examined_per_scan`)
/// and `cost`; `ANALYZE` adds `r_loops`, `r_rows` and times. A query block
/// with a single operation is that operation, carrying the block's cost,
/// loops and time when it has none of its own. Ids are pre-order.
fn mariadb_explain(parsed: &Json, analyze: bool) -> ExplainResult {
    let root = obj(Some(parsed));
    let qb = root.get("query_block").and_then(Json::as_object);
    let mut plan = match qb {
        Some(qb) => mariadb_block(qb),
        None => node("Query"),
    };
    number_nodes(&mut plan, &mut Ids::new());
    let time = |m: Option<&Map<String, Json>>| {
        m.and_then(|m| m.get("r_total_time_ms"))
            .and_then(Json::as_f64)
    };
    ExplainResult {
        plan,
        planning_time: if analyze {
            time(root.get("query_optimization").and_then(Json::as_object)).unwrap_or(0.0)
        } else {
            0.0
        },
        execution_time: if analyze { time(qb) } else { None },
        is_analyze: analyze,
    }
}

fn number_nodes(n: &mut ExplainPlanNode, ids: &mut Ids) {
    n.id = ids.next();
    for c in &mut n.children {
        number_nodes(c, ids);
    }
}

fn f64_of(m: &Map<String, Json>, key: &str) -> Option<f64> {
    m.get(key).and_then(Json::as_f64)
}

fn loops_of(m: &Map<String, Json>) -> Option<i64> {
    m.get("r_loops")
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
}

/// MariaDB's `r_*_time_ms` add up over all `r_loops`; the UI multiplies
/// `actualTotalTime` by `actualLoops` (as Postgres reports it), so it is
/// the time per loop.
fn per_loop(total: Option<f64>, m: &Map<String, Json>) -> Option<f64> {
    match loops_of(m) {
        Some(loops) if loops > 0 => total.map(|t| t / loops as f64),
        _ => total,
    }
}

fn mariadb_block(qb: &Map<String, Json>) -> ExplainPlanNode {
    let mut children = mariadb_contents(qb);
    // HAVING filters the block's grouped rows.
    if let Some(having) = json_str(qb, "having_condition") {
        children = vec![ExplainPlanNode {
            filter: Some(having),
            children,
            ..node("Having")
        }];
    }
    let mut n = if children.len() == 1 {
        children.remove(0)
    } else {
        ExplainPlanNode {
            children,
            ..node("Query Block")
        }
    };
    n.total_cost = n.total_cost.or_else(|| f64_of(qb, "cost"));
    n.actual_loops = n.actual_loops.or_else(|| loops_of(qb));
    n.actual_total_time = n
        .actual_total_time
        .or_else(|| per_loop(f64_of(qb, "r_total_time_ms"), qb));
    n
}

/// Keys whose values hold no operations (metadata, conditions, or handled
/// by the node that owns them), never walked by the fallback.
const MARIADB_METADATA: &[&str] = &[
    "r_engine_stats",
    "index_merge",
    "possible_keys",
    "used_key_parts",
    "ref",
    "key_parts",
    "sort_key",
    "query_specifications",
    "sorts",
];

/// The operations an object holds, in a fixed order of keys.
fn mariadb_contents(m: &Map<String, Json>) -> Vec<ExplainPlanNode> {
    let mut out = Vec::new();
    const KNOWN: &[&str] = &[
        "table",
        "nested_loop",
        "block-nl-join",
        "range-checked-for-each-record",
        "filesort",
        "temporary_table",
        "read_sorted_file",
        "duplicates_removal",
        "union_result",
        "query_block",
        "subquery_cache",
        "materialized",
        "subqueries",
        "window_functions_computation",
    ];
    let unknown = m.iter().filter(|(k, v)| {
        !KNOWN.contains(&k.as_str())
            && !MARIADB_METADATA.contains(&k.as_str())
            && (v.is_object() || v.is_array())
    });
    for (key, value) in KNOWN
        .iter()
        .filter_map(|k| m.get(*k).map(|v| (*k, v)))
        .chain(unknown.map(|(k, v)| (k.as_str(), v)))
    {
        match key {
            "table" => out.push(mariadb_table(obj(Some(value)))),
            "nested_loop" => {
                let items: Vec<ExplainPlanNode> = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|i| mariadb_contents(obj(Some(i))))
                    .collect();
                if items.len() > 1 {
                    out.push(ExplainPlanNode {
                        children: items,
                        ..node("Nested Loop")
                    });
                } else {
                    out.extend(items);
                }
            }
            "block-nl-join" | "range-checked-for-each-record" => {
                let join = obj(Some(value));
                for mut t in mariadb_contents(join) {
                    t.join_type = t.join_type.or_else(|| json_str(join, "join_type"));
                    t.filter = t.filter.or_else(|| json_str(join, "attached_condition"));
                    out.push(t);
                }
            }
            "filesort" => {
                let fs = obj(Some(value));
                out.push(ExplainPlanNode {
                    sort_key: json_str(fs, "sort_key").map(|k| vec![k]),
                    actual_loops: loops_of(fs),
                    actual_total_time: per_loop(f64_of(fs, "r_total_time_ms"), fs),
                    children: mariadb_contents(fs),
                    ..node("Sort")
                });
            }
            "temporary_table" => out.push(ExplainPlanNode {
                children: mariadb_contents(obj(Some(value))),
                ..node("Temporary Table")
            }),
            "duplicates_removal" => {
                let children = match value {
                    Json::Array(items) => items
                        .iter()
                        .flat_map(|i| mariadb_contents(obj(Some(i))))
                        .collect(),
                    other => mariadb_contents(obj(Some(other))),
                };
                out.push(ExplainPlanNode {
                    children,
                    ..node("Distinct")
                });
            }
            "union_result" => {
                let u = obj(Some(value));
                let children = u
                    .get("query_specifications")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .flat_map(|s| mariadb_contents(obj(Some(s))))
                    .collect();
                out.push(ExplainPlanNode {
                    relation_name: json_str(u, "table_name"),
                    actual_rows: f64_of(u, "r_rows"),
                    actual_loops: loops_of(u),
                    children,
                    ..node("Union")
                });
            }
            "query_block" => {
                if let Some(qb) = value.as_object() {
                    out.push(mariadb_block(qb));
                }
            }
            "window_functions_computation" => {
                let w = obj(Some(value));
                // Each window's sort, then the rows they read.
                let mut children: Vec<ExplainPlanNode> = w
                    .get("sorts")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .flat_map(|s| mariadb_contents(obj(Some(s))))
                    .collect();
                children.extend(mariadb_contents(w));
                out.push(ExplainPlanNode {
                    children,
                    ..node("Window")
                });
            }
            "subqueries" => out.extend(
                value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|s| mariadb_contents(obj(Some(s)))),
            ),
            // `read_sorted_file`, `subquery_cache`, `materialized`, and any
            // wrapper this parser doesn't know yet: walk into it, so newer
            // servers' plans still show their tables.
            _ => match value {
                Json::Array(items) => {
                    out.extend(items.iter().flat_map(|i| mariadb_contents(obj(Some(i)))))
                }
                other => out.extend(mariadb_contents(obj(Some(other)))),
            },
        }
    }
    out
}

fn mariadb_table(t: &Map<String, Json>) -> ExplainPlanNode {
    let node_type = match (t.get("access_type"), json_str(t, "message")) {
        (None, Some(message)) => message,
        _ => access_label(&access_type(t)),
    };
    let times = [f64_of(t, "r_table_time_ms"), f64_of(t, "r_other_time_ms")];
    let actual_total_time = per_loop(
        times
            .iter()
            .any(Option::is_some)
            .then(|| times.iter().flatten().sum()),
        t,
    );
    ExplainPlanNode {
        relation_name: json_str(t, "table_name"),
        index_name: json_str(t, "key"),
        filter: json_str(t, "attached_condition"),
        index_cond: json_str(t, "index_condition"),
        plan_rows: f64_of(t, "rows"),
        total_cost: f64_of(t, "cost"),
        actual_rows: f64_of(t, "r_rows"),
        actual_loops: loops_of(t),
        actual_total_time,
        children: mariadb_contents(obj(t.get("materialized"))),
        ..node(node_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flavor_from_version() {
        assert_eq!(
            Flavor::from_version("11.8.6-MariaDB-ubu2404"),
            Flavor::Mariadb
        );
        assert_eq!(Flavor::from_version("8.4.8"), Flavor::Mysql);
    }

    #[test]
    fn js_numbers() {
        assert_eq!(js_string_to_number(" 4 "), 4.0);
        assert_eq!(js_string_to_number(""), 0.0);
        assert_eq!(js_string_to_number("0x1A"), 26.0);
        assert_eq!(js_string_to_number("1e3"), 1000.0);
        assert_eq!(js_string_to_number("-Infinity"), f64::NEG_INFINITY);
        assert!(js_string_to_number("inf").is_nan());
        assert!(js_string_to_number("nan").is_nan());
        assert!(js_string_to_number("x").is_nan());
        assert!(js_string_to_number("1.2.3").is_nan());
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(1000.0), 1000.0);
    }

    #[test]
    fn operator_names() {
        assert_eq!(operator_name("Table scan on t"), "Table scan");
        assert_eq!(operator_name("Limit: 5 row(s)"), "Limit: 5 row");
        assert_eq!(operator_name("Filter: (t.n > 1)"), "Filter:");
        assert_eq!(
            operator_name("Nested loop inner join"),
            "Nested loop inner join"
        );
        assert_eq!(operator_name("(x)"), "");
    }

    #[test]
    fn explain_sql_by_flavor() {
        assert_eq!(
            explain_sql("SELECT 1;", true, Flavor::Mariadb),
            "ANALYZE FORMAT=JSON SELECT 1"
        );
        assert_eq!(
            explain_sql("SELECT 1", true, Flavor::Mysql),
            "EXPLAIN ANALYZE SELECT 1"
        );
        assert_eq!(
            explain_sql("SELECT 1", false, Flavor::Mariadb),
            "EXPLAIN FORMAT=JSON SELECT 1"
        );
    }
}
