//! Postgres introspection: the catalog SQL and the pure functions that turn
//! its results into Seaquel's schema, statistics and EXPLAIN types.
//!
//! Ported from `src/lib/db/postgres.ts` (deleted in phase 1). The SQL is the TypeScript text
//! except for bug fixes 1 (bound filters), 4 (index columns from the catalog),
//! 5 (table sizes by `relid`) and 7 (`cast_type` in the columns query); `tests/introspect_parity.rs` pins both the
//! SQL and the parsers against the recorded TypeScript output.
//!
//! Parsers read cells by column name. The driver decodes natively, so
//! `QUERY PLAN` arrives as `Value::Json`, `text[]` as an `Array` of `Text`,
//! and counts as `Int` or `Decimal`. The parsers also accept the other shapes
//! a cell can take (a JSON array as `Value::Array`, plan or numbers as
//! `Text`), which the parity tests feed them from the recorded rows.

use serde_json::Value as Json;

use seaquel_engine::{DbError, QueryResult, Value};
use seaquel_types::{
    DatabaseOverview, ExplainPlanNode, ExplainResult, ForeignKeyRef, IndexUsageInfo, SchemaColumn,
    SchemaIndex, SchemaTable, TableKind, TableSizeInfo,
};

// ── SQL ──────────────────────────────────────────────────────────────────────

/// Schema names (TS `getSchemasQuery`). Column: `schema_name`.
pub const SCHEMAS_SQL: &str = "SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast') ORDER BY schema_name;";

/// Tables, views and materialized views (TS `getSchemaQuery`).
pub const SCHEMA_SQL: &str = "SELECT table_schema AS schema_name, table_name, table_type\n\t\tFROM information_schema.tables\n\t\tWHERE table_type IN ('BASE TABLE', 'VIEW')\n\t\t\tAND table_schema NOT IN ('pg_catalog', 'information_schema')\n\t\tUNION ALL\n\t\tSELECT schemaname AS schema_name, matviewname AS table_name, 'MATERIALIZED VIEW' AS table_type\n\t\tFROM pg_matviews\n\t\tWHERE schemaname NOT IN ('pg_catalog', 'information_schema')\n\t\tORDER BY schema_name, table_name";

/// Columns of one table (TS `getColumnsQuery`). Bug fix 1: binds `$1` =
/// table, `$2` = schema instead of splicing them. Bug fix 7: adds
/// `cast_type`, the column's type for `CAST($n AS …)`:
/// - Types outside `pg_catalog` (enums, domains, composites, extension types,
///   arrays of them) are schema-qualified from the catalog with `quote_ident`
///   (`app."Weird Mood"`, `app.mood[]`, `public.citext`), whatever the loading
///   connection's search_path. Their typmods are left to assignment.
/// - Built-in types are `format_type(atttypid, atttypmod)`: `integer[]`,
///   `numeric(10,2)`, `timestamp(0) without time zone`.
/// - When the column's type resolves, through any chain of domains and array
///   elements, to a character or bit type, it is that base type without a
///   length (`character varying`, `bpchar`, `"bit"`, `bit varying`, plus `[]`
///   for arrays): an explicit cast to `varchar(5)`, `bit(16)` or a domain over
///   them truncates or pads silently, while assigning the unbounded value
///   checks the length (and the domain's constraints). `"bit"` (quoted, as
///   `format_type(bit, -1)` prints it) has no length, unlike `bit`, which is
///   `bit(1)`; a `bit(n)` key cast to it still uses the index.
pub const COLUMNS_SQL: &str = "SELECT\n\t\t\tcolumn_name,\n\t\t\tdata_type,\n\t\t\tis_nullable,\n\t\t\tcolumn_default,\n\t\t\t(SELECT EXISTS (\n\t\t\t\tSELECT 1 FROM information_schema.key_column_usage kcu\n\t\t\t\tJOIN information_schema.table_constraints tc ON kcu.constraint_name = tc.constraint_name\n\t\t\t\t\tAND kcu.table_schema = tc.table_schema\n\t\t\t\t\tAND kcu.table_name = tc.table_name\n\t\t\t\tWHERE kcu.column_name = c.column_name\n\t\t\t\t\tAND kcu.table_schema = c.table_schema\n\t\t\t\t\tAND kcu.table_name = c.table_name\n\t\t\t\t\tAND tc.constraint_type = 'PRIMARY KEY'\n\t\t\t)) as is_primary_key,\n\t\t\t(SELECT EXISTS (\n\t\t\t\tSELECT 1 FROM information_schema.key_column_usage kcu\n\t\t\t\tJOIN information_schema.table_constraints tc ON kcu.constraint_name = tc.constraint_name\n\t\t\t\t\tAND kcu.table_schema = tc.table_schema\n\t\t\t\t\tAND kcu.table_name = tc.table_name\n\t\t\t\tWHERE kcu.column_name = c.column_name\n\t\t\t\t\tAND kcu.table_schema = c.table_schema\n\t\t\t\t\tAND kcu.table_name = c.table_name\n\t\t\t\t\tAND tc.constraint_type = 'FOREIGN KEY'\n\t\t\t)) as is_foreign_key,\n\t\t\t(SELECT ccu.table_schema || '.' || ccu.table_name || '.' || ccu.column_name\n\t\t\t\tFROM information_schema.key_column_usage kcu\n\t\t\t\tJOIN information_schema.table_constraints tc ON kcu.constraint_name = tc.constraint_name\n\t\t\t\t\tAND kcu.table_schema = tc.table_schema\n\t\t\t\t\tAND kcu.table_name = tc.table_name\n\t\t\t\tJOIN information_schema.referential_constraints rc ON tc.constraint_name = rc.constraint_name\n\t\t\t\t\tAND tc.table_schema = rc.constraint_schema\n\t\t\t\tJOIN information_schema.constraint_column_usage ccu ON rc.unique_constraint_name = ccu.constraint_name\n\t\t\t\t\tAND rc.unique_constraint_schema = ccu.constraint_schema\n\t\t\t\tWHERE kcu.column_name = c.column_name\n\t\t\t\t\tAND kcu.table_schema = c.table_schema\n\t\t\t\t\tAND kcu.table_name = c.table_name\n\t\t\t\t\tAND tc.constraint_type = 'FOREIGN KEY'\n\t\t\t\tLIMIT 1\n\t\t\t) as foreign_key_ref,\n\t\t\t(SELECT CASE\n\t\t\t\tWHEN f.oid = ANY ('{bpchar,varchar,bit,varbit}'::regtype[]::oid[])\n\t\t\t\t\tTHEN format_type(f.oid, -1) || CASE WHEN f.arr THEN '[]' ELSE '' END\n\t\t\t\tWHEN e.typnamespace <> 'pg_catalog'::regnamespace\n\t\t\t\t\tTHEN quote_ident(en.nspname) || '.' || quote_ident(e.typname)\n\t\t\t\t\t\t|| CASE WHEN e.oid <> t.oid THEN '[]' ELSE '' END\n\t\t\t\tELSE format_type(a.atttypid, a.atttypmod) END\n\t\t\t\tFROM pg_attribute a\n\t\t\t\tJOIN pg_class cl ON cl.oid = a.attrelid\n\t\t\t\tJOIN pg_namespace ns ON ns.oid = cl.relnamespace\n\t\t\t\tJOIN pg_type t ON t.oid = a.atttypid\n\t\t\t\tJOIN pg_type e ON e.oid = CASE WHEN t.typtype = 'b' AND t.typcategory = 'A' AND t.typelem <> 0\n\t\t\t\t\tTHEN t.typelem ELSE t.oid END\n\t\t\t\tJOIN pg_namespace en ON en.oid = e.typnamespace\n\t\t\t\tCROSS JOIN LATERAL (\n\t\t\t\t\tWITH RECURSIVE chain(oid, arr, depth) AS (\n\t\t\t\t\t\tSELECT a.atttypid, false, 0\n\t\t\t\t\t\tUNION ALL\n\t\t\t\t\t\tSELECT CASE WHEN ty.typtype = 'd' THEN ty.typbasetype ELSE ty.typelem END,\n\t\t\t\t\t\t\tchain.arr OR ty.typtype <> 'd', chain.depth + 1\n\t\t\t\t\t\tFROM chain JOIN pg_type ty ON ty.oid = chain.oid\n\t\t\t\t\t\tWHERE ty.typtype = 'd' OR (ty.typtype = 'b' AND ty.typcategory = 'A' AND ty.typelem <> 0)\n\t\t\t\t\t)\n\t\t\t\t\tSELECT oid, arr FROM chain ORDER BY depth DESC LIMIT 1\n\t\t\t\t) f\n\t\t\t\tWHERE ns.nspname = c.table_schema AND cl.relname = c.table_name\n\t\t\t\t\tAND a.attname = c.column_name AND a.attnum > 0 AND NOT a.attisdropped\n\t\t\t) as cast_type\n\t\tFROM information_schema.columns c\n\t\tWHERE table_name = $1 AND table_schema = $2\n\t\tORDER BY ordinal_position";

/// Indexes of one table, from the catalog (bug fix 4). Key columns come back
/// as plain attribute names (matching `SchemaColumn::name`, unquoted), and
/// expression keys as `pg_get_indexdef(oid, k, true)` prints them, e.g.
/// `lower(name::text)`. `INCLUDE` columns are left out (`indnkeyatts`),
/// uniqueness comes from `indisunique`, the type from the access method.
/// Ordered by index name (C-collated `name`). Binds `$1` = table, `$2` =
/// schema.
pub const INDEXES_SQL: &str = "SELECT
\t\t\tic.relname AS index_name,
\t\t\tARRAY(
\t\t\t\tSELECT CASE WHEN i.indkey[k - 1] <> 0 THEN a.attname::text
\t\t\t\t\tELSE pg_get_indexdef(i.indexrelid, k, true) END
\t\t\t\tFROM generate_series(1, i.indnkeyatts) k
\t\t\t\tLEFT JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[k - 1]
\t\t\t\tORDER BY k
\t\t\t) AS columns,
\t\t\ti.indisunique AS is_unique,
\t\t\tam.amname AS index_type
\t\tFROM pg_index i
\t\tJOIN pg_class ic ON ic.oid = i.indexrelid
\t\tJOIN pg_class t ON t.oid = i.indrelid
\t\tJOIN pg_namespace n ON n.oid = t.relnamespace
\t\tJOIN pg_am am ON am.oid = ic.relam
\t\tWHERE t.relname = $1 AND n.nspname = $2
\t\tORDER BY ic.relname";

/// Table sizes (TS `getTableSizesQuery`). Bug fix 5: sizes by `relid`, not
/// by a spliced `schemaname || '.' || relname`, which fails for any name that
/// needs quoting.
pub const TABLE_SIZES_SQL: &str = "SELECT\n\t\t\tschemaname AS schema_name,\n\t\t\trelname AS table_name,\n\t\t\tCOALESCE(n_live_tup, 0) AS row_count,\n\t\t\tpg_size_pretty(pg_total_relation_size(relid)) AS total_size,\n\t\t\tpg_total_relation_size(relid) AS total_size_bytes,\n\t\t\tpg_size_pretty(pg_relation_size(relid)) AS data_size,\n\t\t\tpg_size_pretty(pg_indexes_size(relid)) AS index_size\n\t\tFROM pg_stat_user_tables\n\t\tORDER BY pg_total_relation_size(relid) DESC";

/// Index usage (TS `getIndexUsageQuery`). Already keyed by `indexrelid`.
pub const INDEX_USAGE_SQL: &str = "SELECT\n\t\t\tschemaname AS schema_name,\n\t\t\trelname AS table_name,\n\t\t\tindexrelname AS index_name,\n\t\t\tpg_size_pretty(pg_relation_size(indexrelid)) AS size,\n\t\t\tidx_scan AS scans,\n\t\t\tidx_tup_read AS rows_read,\n\t\t\tidx_scan = 0 AS unused\n\t\tFROM pg_stat_user_indexes\n\t\tORDER BY idx_scan ASC, pg_relation_size(indexrelid) DESC";

/// Database overview (TS `getDatabaseOverviewQuery`).
pub const OVERVIEW_SQL: &str = "SELECT\n\t\t\tcurrent_database() AS database_name,\n\t\t\tpg_size_pretty(pg_database_size(current_database())) AS total_size,\n\t\t\tpg_database_size(current_database()) AS total_size_bytes,\n\t\t\t(SELECT count(*) FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog', 'information_schema')) AS table_count,\n\t\t\t(SELECT count(*) FROM pg_indexes WHERE schemaname NOT IN ('pg_catalog', 'information_schema')) AS index_count,\n\t\t\t(SELECT count(*) FROM pg_stat_activity WHERE datname = current_database()) AS connection_count";

// ── Row access ───────────────────────────────────────────────────────────────

static NULL: Value = Value::Null;

/// Rows of a result, read by column name like the TS row objects. A missing
/// column reads as `Null` (TS `undefined`).
struct Rows<'a> {
    result: &'a QueryResult,
}

struct Row<'a> {
    columns: &'a [String],
    cells: &'a [Value],
}

impl<'a> Rows<'a> {
    fn new(result: &'a QueryResult) -> Self {
        Self { result }
    }

    fn iter(&self) -> impl Iterator<Item = Row<'a>> + 'a {
        let columns = &self.result.columns;
        self.result
            .rows
            .iter()
            .map(move |cells| Row { columns, cells })
    }

    fn first(&self) -> Option<Row<'a>> {
        self.iter().next()
    }
}

impl<'a> Row<'a> {
    fn get(&self, column: &str) -> &'a Value {
        self.columns
            .iter()
            .position(|c| c == column)
            .and_then(|i| self.cells.get(i))
            .unwrap_or(&NULL)
    }

    /// A text cell. Other scalars are printed; `Null` is `None`.
    fn opt_text(&self, column: &str) -> Option<String> {
        match self.get(column) {
            Value::Null => None,
            Value::Text(s) | Value::Decimal(s) => Some(s.clone()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Int(i) => Some(i.to_string()),
            Value::Float(f) => Some(f.to_string()),
            other => Some(value_to_json(other).to_string()),
        }
    }

    /// A text cell, `""` when `Null`.
    fn text(&self, column: &str) -> String {
        self.opt_text(column).unwrap_or_default()
    }

    /// The TS reads these booleans as they arrive; only `true` is true.
    fn bool(&self, column: &str) -> bool {
        matches!(self.get(column), Value::Bool(true))
    }

    /// `Number(x) || 0`.
    fn number(&self, column: &str) -> i64 {
        number_or_zero(self.get(column))
    }
}

/// TS `Number(x) || 0`, truncated to an integer: `Int`, `Float`, `Decimal`
/// and numeric `Text` count; `Null`, NaN and anything unparsable are 0.
fn number_or_zero(v: &Value) -> i64 {
    let f = match v {
        Value::Int(i) => return *i,
        Value::Bool(b) => return i64::from(*b),
        Value::Float(f) => *f,
        Value::Decimal(s) | Value::Text(s) => {
            let s = s.trim();
            if s.is_empty() {
                return 0;
            }
            if let Ok(i) = s.parse::<i64>() {
                return i;
            }
            s.parse::<f64>().unwrap_or(f64::NAN)
        }
        _ => f64::NAN,
    };
    if f.is_finite() {
        // `as` saturates beyond the i64 range.
        f.trunc() as i64
    } else {
        0
    }
}

/// A cell as the JSON the TS adapter would have seen.
fn value_to_json(v: &Value) -> Json {
    match v {
        Value::Null | Value::Bytes(_) => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::from(*i),
        Value::Float(f) => serde_json::Number::from_f64(*f).map_or(Json::Null, Json::Number),
        Value::Decimal(s) => s
            .parse::<serde_json::Number>()
            .map_or_else(|_| Json::String(s.clone()), Json::Number),
        Value::Text(s) => Json::String(s.clone()),
        Value::Json(j) => j.clone(),
        Value::Array(items) => Json::Array(items.iter().map(value_to_json).collect()),
    }
}

// ── Schema ───────────────────────────────────────────────────────────────────

/// Rows of [`SCHEMAS_SQL`].
pub fn parse_schemas(result: &QueryResult) -> Vec<String> {
    Rows::new(result)
        .iter()
        .map(|r| r.text("schema_name"))
        .collect()
}

/// Rows of [`SCHEMA_SQL`] (TS `parseSchemaResult`). Columns and indexes are
/// loaded per table, so they're empty here.
pub fn parse_schema(result: &QueryResult) -> Vec<SchemaTable> {
    Rows::new(result)
        .iter()
        .map(|r| SchemaTable {
            name: r.text("table_name"),
            schema: r.text("schema_name"),
            kind: match r.get("table_type").as_str() {
                Some("VIEW") => TableKind::View,
                Some("MATERIALIZED VIEW") => TableKind::MaterializedView,
                _ => TableKind::Table,
            },
            row_count: None,
            columns: vec![],
            indexes: vec![],
        })
        .collect()
}

/// Rows of [`COLUMNS_SQL`] (TS `parseColumnsResult`).
pub fn parse_columns(result: &QueryResult) -> Vec<SchemaColumn> {
    Rows::new(result)
        .iter()
        .map(|r| {
            // `if (col.foreign_key_ref)`, then split on every '.'.
            let foreign_key_ref = r
                .opt_text("foreign_key_ref")
                .filter(|s| !s.is_empty())
                .and_then(|s| match s.split('.').collect::<Vec<_>>()[..] {
                    [schema, table, column] => Some(ForeignKeyRef {
                        referenced_schema: schema.to_string(),
                        referenced_table: table.to_string(),
                        referenced_column: column.to_string(),
                    }),
                    _ => None,
                });
            SchemaColumn {
                name: r.text("column_name"),
                ty: r.text("data_type"),
                cast_type: r.opt_text("cast_type").filter(|s| !s.is_empty()),
                nullable: r.get("is_nullable").as_str() == Some("YES"),
                // `column_default || undefined`
                default_value: r.opt_text("column_default").filter(|s| !s.is_empty()),
                is_primary_key: r.bool("is_primary_key"),
                is_foreign_key: r.bool("is_foreign_key"),
                foreign_key_ref,
            }
        })
        .collect()
}

/// Rows of [`INDEXES_SQL`]. New with bug fix 4, so it has no TS original.
/// `columns` is a `text[]` (an `Array` of `Text`, or a JSON array once
/// decoded natively).
pub fn parse_indexes(result: &QueryResult) -> Vec<SchemaIndex> {
    Rows::new(result)
        .iter()
        .map(|r| {
            let columns = match value_to_json(r.get("columns")) {
                Json::Array(items) => items
                    .into_iter()
                    .map(|c| match c {
                        Json::String(s) => s,
                        other => other.to_string(),
                    })
                    .collect(),
                _ => vec![],
            };
            SchemaIndex {
                name: r.text("index_name"),
                columns,
                unique: r.bool("is_unique"),
                ty: r.text("index_type"),
            }
        })
        .collect()
}

// ── Statistics ───────────────────────────────────────────────────────────────

/// Rows of [`TABLE_SIZES_SQL`] (TS `parseTableSizesResult`).
pub fn parse_table_sizes(result: &QueryResult) -> Vec<TableSizeInfo> {
    Rows::new(result)
        .iter()
        .map(|r| TableSizeInfo {
            schema: r.text("schema_name"),
            name: r.text("table_name"),
            row_count: r.number("row_count"),
            total_size: r.text("total_size"),
            total_size_bytes: r.number("total_size_bytes"),
            data_size: r.opt_text("data_size"),
            index_size: r.opt_text("index_size"),
        })
        .collect()
}

/// Rows of [`INDEX_USAGE_SQL`] (TS `parseIndexUsageResult`).
pub fn parse_index_usage(result: &QueryResult) -> Vec<IndexUsageInfo> {
    Rows::new(result)
        .iter()
        .map(|r| IndexUsageInfo {
            schema: r.text("schema_name"),
            table: r.text("table_name"),
            index_name: r.text("index_name"),
            size: r.text("size"),
            scans: r.number("scans"),
            rows_read: Some(r.number("rows_read")),
            unused: r.bool("unused"),
        })
        .collect()
}

/// The row of [`OVERVIEW_SQL`] (TS `parseDatabaseOverviewResult`), with the
/// TS fallbacks when there is no row.
pub fn parse_overview(result: &QueryResult) -> DatabaseOverview {
    let rows = Rows::new(result);
    let row = rows.first();
    let text = |c: &str, fallback: &str| {
        row.as_ref()
            .and_then(|r| r.opt_text(c))
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

/// JS truthiness of a cell (`a || b` picks `b` when `a` is falsy).
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::Float(f) => *f != 0.0 && !f.is_nan(),
        Value::Text(s) => !s.is_empty(),
        _ => true,
    }
}

/// The result of `EXPLAIN (FORMAT JSON)` (TS `parseExplainResult`). The plan
/// is read from the `QUERY PLAN` (or `query plan`) column, as JSON or as JSON
/// text. Node ids are assigned post-order, children first.
///
/// With no rows the TS adapter throws; this returns an `Unknown` root with
/// planning time 0, as it does for an empty plan array.
pub fn parse_explain(result: &QueryResult, analyze: bool) -> Result<ExplainResult, DbError> {
    let rows = Rows::new(result);
    let plan_cell = rows.first().map(|r| {
        let upper = r.get("QUERY PLAN");
        if truthy(upper) {
            upper
        } else {
            r.get("query plan")
        }
    });
    let parsed = match plan_cell {
        Some(Value::Text(s)) => serde_json::from_str(s)
            .map_err(|e| DbError::query_error(format!("invalid EXPLAIN output: {e}")))?,
        Some(v) => value_to_json(v),
        None => Json::Null,
    };
    let empty = serde_json::Map::new();
    let top = parsed.get(0).and_then(Json::as_object).unwrap_or(&empty);
    let root = top.get("Plan").and_then(Json::as_object).unwrap_or(&empty);

    let mut next_id = 0usize;
    let plan = convert_node(root, analyze, &mut next_id);
    Ok(ExplainResult {
        plan,
        planning_time: top
            .get("Planning Time")
            .and_then(Json::as_f64)
            .unwrap_or(0.0),
        execution_time: top.get("Execution Time").and_then(Json::as_f64),
        is_analyze: analyze,
    })
}

fn convert_node(
    node: &serde_json::Map<String, Json>,
    analyze: bool,
    next_id: &mut usize,
) -> ExplainPlanNode {
    let children = node
        .get("Plans")
        .and_then(Json::as_array)
        .map(|plans| {
            plans
                .iter()
                .filter_map(Json::as_object)
                .map(|child| convert_node(child, analyze, next_id))
                .collect()
        })
        .unwrap_or_default();

    let id = format!("node-{next_id}");
    *next_id += 1;

    let text = |k: &str| node.get(k).and_then(Json::as_str).map(str::to_string);
    let float = |k: &str| node.get(k).and_then(Json::as_f64);
    let int = |k: &str| {
        node.get(k)
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
    };
    let actual = |v: Option<f64>| v.filter(|_| analyze);

    ExplainPlanNode {
        id,
        // String(node["Node Type"] ?? "Unknown")
        node_type: match node.get("Node Type") {
            None | Some(Json::Null) => "Unknown".to_string(),
            Some(Json::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        },
        relation_name: text("Relation Name"),
        alias: text("Alias"),
        startup_cost: float("Startup Cost"),
        total_cost: float("Total Cost"),
        plan_rows: float("Plan Rows"),
        plan_width: int("Plan Width"),
        actual_startup_time: actual(float("Actual Startup Time")),
        actual_total_time: actual(float("Actual Total Time")),
        actual_rows: actual(float("Actual Rows")),
        actual_loops: int("Actual Loops").filter(|_| analyze),
        filter: text("Filter"),
        index_name: text("Index Name"),
        index_cond: text("Index Cond"),
        join_type: text("Join Type"),
        hash_cond: text("Hash Cond"),
        sort_key: node.get("Sort Key").and_then(Json::as_array).map(|keys| {
            keys.iter()
                .map(|k| k.as_str().map_or_else(|| k.to_string(), str::to_string))
                .collect()
        }),
        children,
    }
}
