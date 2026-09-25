//! DuckDB introspection: the catalog SQL and the pure functions that turn its
//! results into Seaquel's schema, statistics and EXPLAIN types.
//!
//! Ported from the demo's `duckdb.ts` (`src/lib/db/duckdb.ts`) and the
//! DuckDB branches of `TsEngineClient` (per-table row counts).
//! `tests/introspect_parity.rs` pins the SQL and the parsers against the
//! recorded TypeScript output and `tests/fixtures/bugfixes.json`.
//!
//! Changes from the TypeScript, each a numbered bug fix:
//! - 1: schema and table are bound (`$1`, `$2`) instead of checked with
//!   `validateIdentifier`, which rejected any name with a space or a quote;
//!   row counts quote catalog, schema and table with `"` doubled.
//! - 2: indexes come from `duckdb_indexes()`, one row per key.
//! - 3: the overview's size is `block_size * total_blocks` from
//!   `pragma_database_size()`, instead of "In-memory".
//! - 5: Estimated Cardinality is read from `~100`, `1,000`, `12 rows`, ….
//! - 6: catalogs are told apart: `system` and `temp` are left out, the
//!   default catalog's schemas are listed as they are, any other attached
//!   catalog's as `catalog.schema` (a part holding `.` or `"`
//!   double-quoted), and every query matches that same expression.
//! - 7: a failing foreign-key query leaves the columns without foreign keys
//!   (the driver catches it).
//! - 13: a FILTER node's `Expression` is its filter.
//! - 14: `relationName` is the table's own name, the last part of 1.5's
//!   `catalog.schema.table`.
//! - 15: the overview is named after `current_database()`.
//!
//! Every catalog column the queries return is VARCHAR, BOOLEAN or a BIGINT
//! count: LIST columns are unnested or tested in SQL, never read as a cell,
//! and the overview's sum is cast to BIGINT (HUGEINT otherwise).

use seaquel_engine::introspect::{format_bytes, js_trim, node, rows, Ids, Row};
use seaquel_engine::{QueryResult, Value};
use seaquel_types::{
    DatabaseOverview, ExplainPlanNode, ExplainResult, ForeignKeyRef, IndexUsageInfo, SchemaColumn,
    SchemaIndex, SchemaTable, TableKind, TableSizeInfo,
};
use serde_json::{Map, Value as Json};

use crate::dialect::{parse_dotted, qi};

// ── SQL ──────────────────────────────────────────────────────────────────────

/// A catalog or schema name as the tree shows it: double-quoted (`""` for
/// `"`) when it holds `.` or `"`.
fn part(x: &str) -> String {
    format!(
        "CASE WHEN {x} LIKE '%.%' OR {x} LIKE '%\"%' THEN '\"' || replace({x}, '\"', '\"\"') || '\"' ELSE {x} END"
    )
}

/// The schema as the tree lists it: the default catalog's bare, any other's
/// as `catalog.schema`.
fn shown(catalog: &str, schema: &str) -> String {
    format!(
        "CASE WHEN {catalog} = current_database() THEN {} ELSE {} || '.' || {} END",
        part(schema),
        part(catalog),
        part(schema)
    )
}

/// Tables and views of every catalog but `system` and `temp` (TS
/// `getSchemaQuery`; bug fix 6).
pub fn schema_sql() -> String {
    format!(
        "SELECT
  {} AS schema_name,
  table_name,
  table_type
FROM information_schema.tables
WHERE table_type IN ('BASE TABLE', 'VIEW')
  AND table_catalog NOT IN ('system', 'temp')
  AND table_schema NOT IN ('pg_catalog', 'information_schema')
ORDER BY table_catalog <> current_database(), table_catalog, table_schema, table_name",
        shown("table_catalog", "table_schema")
    )
}

/// Schema names, as listed (TS `getSchemasQuery`; bug fix 6).
pub fn schemas_sql() -> String {
    format!(
        "SELECT
  {} AS schema_name
FROM information_schema.schemata
WHERE catalog_name NOT IN ('system', 'temp')
ORDER BY catalog_name <> current_database(), catalog_name, schema_name",
        shown("catalog_name", "schema_name")
    )
}

/// Columns of `$1`.`$2` (TS `getColumnsQuery`; bug fixes 1 and 6).
pub fn columns_sql() -> String {
    format!(
        "SELECT
  c.column_name,
  c.data_type,
  c.is_nullable,
  c.column_default,
  EXISTS (
    SELECT 1
    FROM duckdb_constraints() dc
    WHERE dc.constraint_type = 'PRIMARY KEY'
      AND dc.database_name = c.table_catalog
      AND dc.schema_name = c.table_schema
      AND dc.table_name = c.table_name
      AND list_contains(dc.constraint_column_names, c.column_name)
  ) AS is_primary_key
FROM information_schema.columns c
WHERE c.table_catalog NOT IN ('system', 'temp')
  AND {} = $1
  AND c.table_name = $2
ORDER BY c.ordinal_position",
        shown("c.table_catalog", "c.table_schema")
    )
}

/// Foreign-key column pairs of `$1`.`$2`, in order (TS
/// `getForeignKeysQuery`; bug fixes 1 and 6). The referenced schema is the
/// schema as listed: DuckDB has no cross-schema foreign keys.
pub fn foreign_keys_sql() -> String {
    format!(
        "SELECT
  $1 AS schema_name,
  referenced_table,
  unnest(constraint_column_names) AS source_column,
  unnest(referenced_column_names) AS referenced_column,
  unnest(generate_series(1, len(constraint_column_names))) AS position
FROM duckdb_constraints()
WHERE constraint_type = 'FOREIGN KEY'
  AND database_name NOT IN ('system', 'temp')
  AND {} = $1
  AND table_name = $2
ORDER BY constraint_index, position",
        shown("database_name", "schema_name")
    )
}

/// Indexes of `$1`.`$2`, one row per key (bug fix 2; the TS listed none).
/// `expressions` is cast to `VARCHAR[]` and unnested in order; a key that is
/// one double-quoted identifier is un-quoted, any other expression kept as
/// DuckDB prints it.
pub fn indexes_sql() -> String {
    format!(
        "SELECT
  i.index_name,
  i.is_unique,
  k.position,
  CASE
    WHEN regexp_full_match(k.expression, '\"([^\"]|\"\")*\"') THEN replace(k.expression[2:-2], '\"\"', '\"')
    ELSE k.expression
  END AS column_name
FROM duckdb_indexes() i,
  unnest(CAST(i.expressions AS VARCHAR[])) WITH ORDINALITY AS k(expression, position)
WHERE i.database_name NOT IN ('system', 'temp')
  AND {} = $1
  AND i.table_name = $2
ORDER BY i.index_name, k.position",
        shown("i.database_name", "i.schema_name")
    )
}

/// Every column of `$1`.`$2` in a UNIQUE constraint, one row per column and
/// constraint, with whether the constraint is that column alone (`single`).
/// Not in the TypeScript. Sets `is_unique` (single) and
/// `in_unique_constraint` (any) for bug fix 10's ALTER TABLE rules.
pub fn unique_columns_sql() -> String {
    format!(
        "SELECT
  unnest(constraint_column_names) AS column_name,
  len(constraint_column_names) = 1 AS single
FROM duckdb_constraints()
WHERE constraint_type = 'UNIQUE'
  AND database_name NOT IN ('system', 'temp')
  AND {} = $1
  AND table_name = $2
ORDER BY constraint_index",
        shown("database_name", "schema_name")
    )
}

/// Base tables for the statistics, with the catalog and schema to count
/// their rows in (TS `getTableSizesQuery`; bug fix 6).
pub fn table_sizes_sql() -> String {
    format!(
        "SELECT
  {} AS schema_name,
  table_name,
  table_catalog,
  table_schema
FROM information_schema.tables
WHERE table_type = 'BASE TABLE'
  AND table_catalog NOT IN ('system', 'temp')
  AND table_schema NOT IN ('pg_catalog', 'information_schema')
ORDER BY table_catalog <> current_database(), table_catalog, table_schema, table_name",
        shown("table_catalog", "table_schema")
    )
}

/// Indexes for the statistics (TS `getIndexUsageQuery`; bug fix 6).
pub fn index_usage_sql() -> String {
    format!(
        "SELECT
  {} AS schema_name,
  table_name,
  index_name,
  is_unique
FROM duckdb_indexes()
WHERE database_name NOT IN ('system', 'temp')
ORDER BY database_name <> current_database(), database_name, schema_name, table_name, index_name",
        shown("database_name", "schema_name")
    )
}

/// Database overview (TS `getDatabaseOverviewQuery`; bug fixes 3, 6 and 15).
pub const OVERVIEW_SQL: &str = "SELECT
  current_database() AS database_name,
  (
    SELECT count(*)
    FROM information_schema.tables
    WHERE table_type = 'BASE TABLE'
      AND table_catalog NOT IN ('system', 'temp')
      AND table_schema NOT IN ('pg_catalog', 'information_schema')
  ) AS table_count,
  (SELECT count(*) FROM duckdb_indexes() WHERE database_name NOT IN ('system', 'temp')) AS index_count,
  (
    SELECT CAST(COALESCE(sum(block_size * total_blocks), 0) AS BIGINT)
    FROM pragma_database_size()
    WHERE database_name NOT IN ('system', 'temp')
  ) AS total_size_bytes";

/// Rows of one table (TS `getTableRowCountQuery`). Bug fixes 1 and 6:
/// catalog, schema and table are quoted with `"` doubled. They are the
/// `table_catalog` and `table_schema` columns of [`table_sizes_sql`], not the
/// listed schema.
pub fn row_count_sql(catalog: &str, schema: &str, table: &str) -> String {
    format!(
        "SELECT COUNT(*) AS row_count FROM {}.{}.{}",
        qi(catalog),
        qi(schema),
        qi(table)
    )
}

/// `EXPLAIN (FORMAT JSON) <sql>`, or `EXPLAIN (ANALYZE, FORMAT JSON)`, with
/// one trailing `;` stripped (TS `query.replace(/;$/, "")`).
pub fn explain_sql(sql: &str, analyze: bool) -> String {
    let base = sql.strip_suffix(';').unwrap_or(sql);
    if analyze {
        format!("EXPLAIN (ANALYZE, FORMAT JSON) {base}")
    } else {
        format!("EXPLAIN (FORMAT JSON) {base}")
    }
}

// ── Row access ───────────────────────────────────────────────────────────────

/// `row[column] || fallback` for a text cell.
fn text_or(r: &Row, column: &str, fallback: &str) -> String {
    r.non_empty(column).unwrap_or(fallback).to_string()
}

// ── Schema ───────────────────────────────────────────────────────────────────

/// Rows of [`schemas_sql`].
pub fn parse_schemas(result: &QueryResult) -> Vec<String> {
    rows(result).map(|r| r.text("schema_name")).collect()
}

/// Rows of [`schema_sql`] (TS `parseSchemaResult`).
pub fn parse_schema(result: &QueryResult) -> Vec<SchemaTable> {
    rows(result)
        .map(|r| SchemaTable {
            name: r.text("table_name"),
            schema: r.text("schema_name"),
            kind: if r.str("table_type") == Some("VIEW") {
                TableKind::View
            } else {
                TableKind::Table
            },
            row_count: None,
            columns: vec![],
            indexes: vec![],
        })
        .collect()
}

/// Rows of [`columns_sql`] and [`foreign_keys_sql`] (TS
/// `parseColumnsResult`). A column's foreign key is the last one listing it
/// (TS `Map.set`); rows missing a column, table or referenced column are
/// skipped. Without foreign-key rows (bug fix 7: the query failed) no column
/// is a foreign key.
pub fn parse_columns(
    columns: &QueryResult,
    foreign_keys: Option<&QueryResult>,
) -> Vec<SchemaColumn> {
    let mut fks: Vec<(String, ForeignKeyRef)> = Vec::new();
    if let Some(result) = foreign_keys {
        for fk in rows(result) {
            let (source, table, column) = (
                fk.text("source_column"),
                fk.text("referenced_table"),
                fk.text("referenced_column"),
            );
            if source.is_empty() || table.is_empty() || column.is_empty() {
                continue;
            }
            let reference = ForeignKeyRef {
                referenced_schema: fk.text("schema_name"),
                referenced_table: table,
                referenced_column: column,
            };
            match fks.iter_mut().find(|(f, _)| *f == source) {
                Some(entry) => entry.1 = reference,
                None => fks.push((source, reference)),
            }
        }
    }
    rows(columns)
        .map(|c| {
            let name = c.text("column_name");
            let foreign_key_ref = fks.iter().find(|(f, _)| *f == name).map(|(_, r)| r.clone());
            SchemaColumn {
                ty: c.text("data_type"),
                cast_type: None,
                nullable: c.str("is_nullable") == Some("YES"),
                default_value: c
                    .str("column_default")
                    .filter(|d| !d.is_empty())
                    .map(str::to_string),
                is_primary_key: c.truthy("is_primary_key"),
                is_foreign_key: foreign_key_ref.is_some(),
                foreign_key_ref,
                collation: None,
                name,
                is_unique: false,
                in_unique_constraint: false,
            }
        })
        .collect()
}

/// Rows of [`indexes_sql`], one per key, grouped by index in order (bug fix
/// 2). Every index is an ART index.
pub fn parse_indexes(result: &QueryResult) -> Vec<SchemaIndex> {
    let mut out: Vec<SchemaIndex> = Vec::new();
    for r in rows(result) {
        let name = r.text("index_name");
        let column = r.text("column_name");
        match out.iter_mut().find(|i| i.name == name) {
            Some(index) => index.columns.push(column),
            None => out.push(SchemaIndex {
                columns: vec![column],
                unique: r.get("is_unique") == &Value::Bool(true),
                ty: "art".to_string(),
                name,
            }),
        }
    }
    out
}

/// Sets `is_unique` and `in_unique_constraint` on `columns` from the rows
/// of [`unique_columns_sql`].
pub fn apply_unique_columns(columns: &mut [SchemaColumn], result: &QueryResult) {
    for r in rows(result) {
        let name = r.text("column_name");
        if let Some(c) = columns.iter_mut().find(|c| c.name == name) {
            c.in_unique_constraint = true;
            if r.get("single") == &Value::Bool(true) {
                c.is_unique = true;
            }
        }
    }
}

// ── Statistics ───────────────────────────────────────────────────────────────

/// Rows of [`table_sizes_sql`] (TS `parseTableSizesResult`). DuckDB keeps no
/// per-table sizes; the driver fills in `row_count` with [`row_count_sql`]
/// on [`row_count_targets`].
pub fn parse_table_sizes(result: &QueryResult) -> Vec<TableSizeInfo> {
    rows(result)
        .map(|r| TableSizeInfo {
            schema: text_or(&r, "schema_name", "main"),
            name: r.text("table_name"),
            row_count: 0,
            total_size: "N/A".to_string(),
            total_size_bytes: 0,
            data_size: None,
            index_size: None,
        })
        .collect()
}

/// For each row of [`table_sizes_sql`], the [`row_count_sql`] that counts
/// its table in its own catalog.
pub fn row_count_targets(result: &QueryResult) -> Vec<String> {
    rows(result)
        .map(|r| {
            row_count_sql(
                &r.text("table_catalog"),
                &r.text("table_schema"),
                &r.text("table_name"),
            )
        })
        .collect()
}

/// A result of [`row_count_sql`] (TsEngineClient: `Number(row_count) || 0`).
pub fn parse_row_count(result: &QueryResult) -> i64 {
    rows(result).next().map_or(0, |r| r.number("row_count"))
}

/// Rows of [`index_usage_sql`] (TS `parseIndexUsageResult`). DuckDB tracks
/// no index usage.
pub fn parse_index_usage(result: &QueryResult) -> Vec<IndexUsageInfo> {
    rows(result)
        .map(|r| IndexUsageInfo {
            schema: text_or(&r, "schema_name", "main"),
            table: r.text("table_name"),
            index_name: r.text("index_name"),
            size: "N/A".to_string(),
            scans: 0,
            rows_read: None,
            unused: false,
        })
        .collect()
}

/// Rows of [`OVERVIEW_SQL`] (TS `parseDatabaseOverviewResult`). Bug fix 3:
/// the size in bytes, formatted, instead of "In-memory" (an in-memory
/// database is `0 bytes`); bug fix 15: named after `current_database()`,
/// "DuckDB Database" only without a row.
pub fn parse_overview(result: &QueryResult) -> DatabaseOverview {
    let first = rows(result).next();
    let number = |c: &str| first.as_ref().map_or(0, |r| r.number(c));
    let bytes = number("total_size_bytes");
    DatabaseOverview {
        database_name: first
            .as_ref()
            .and_then(|r| r.str("database_name"))
            .filter(|n| !n.is_empty())
            .unwrap_or("DuckDB Database")
            .to_string(),
        total_size: format_bytes(bytes),
        total_size_bytes: Some(bytes),
        table_count: number("table_count"),
        index_count: number("index_count"),
        connection_count: None,
    }
}

// ── EXPLAIN ──────────────────────────────────────────────────────────────────

/// Bug fix 14: the last part of a dotted name (`seaquel_test.main.orders` is
/// `orders`), quote-aware; a name that doesn't parse stays as it is.
pub fn last_part(name: &str) -> String {
    parse_dotted(name)
        .and_then(|mut parts| parts.pop())
        .unwrap_or_else(|| name.to_string())
}

/// JS `/\s/`, near enough: Unicode whitespace and the BOM.
fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{FEFF}'
}

/// JS `Number(digits)` for a string of ASCII digits.
fn digits_to_number(s: &str) -> f64 {
    if s.is_empty() {
        0.0
    } else {
        s.parse::<f64>().unwrap_or(f64::NAN)
    }
}

/// The TypeScript reading of Estimated Cardinality: a number, or a string
/// of digits and `_`.
fn ts_cardinality(ec: &Json) -> Option<f64> {
    match ec {
        Json::Number(n) => n.as_f64(),
        Json::String(s) if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '_') => {
            Some(digits_to_number(&s.replace('_', "")))
        }
        _ => None,
    }
}

/// Bug fix 5: a number, or a string that is digits after trimming, dropping
/// a leading `~`, a trailing `row`/`rows` and `_` or `,` separators
/// (`~100`, `1,000`, `12 rows`). Anything else is `None`.
fn cardinality(ec: &Json) -> Option<f64> {
    match ec {
        Json::Number(n) => n.as_f64(),
        Json::String(s) => {
            let mut t = js_trim(s);
            if let Some(rest) = t.strip_prefix('~') {
                t = rest.trim_start_matches(is_js_space);
            }
            let lower = t.to_ascii_lowercase();
            for unit in ["rows", "row"] {
                if lower.ends_with(unit) {
                    t = t[..t.len() - unit.len()].trim_end_matches(is_js_space);
                    break;
                }
            }
            let digits: String = t.chars().filter(|c| *c != '_' && *c != ',').collect();
            (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
                .then(|| digits_to_number(&digits))
        }
        _ => None,
    }
}

static EMPTY: std::sync::OnceLock<Map<String, Json>> = std::sync::OnceLock::new();

/// A JSON object, or an empty one for anything else (TS reads missing keys
/// of a non-object as `undefined`).
fn object(v: &Json) -> &Map<String, Json> {
    v.as_object().unwrap_or_else(|| EMPTY.get_or_init(Map::new))
}

/// A node's children, or none when `children` isn't an array.
fn children(node: &Map<String, Json>) -> &[Json] {
    node.get("children")
        .and_then(Json::as_array)
        .map_or(&[], Vec::as_slice)
}

/// A node's type from its `name` (plain) or `operator_name` (analyzed)
/// field, trimmed; `Operator` without one.
fn node_type(node: &Map<String, Json>, key: &str) -> String {
    node.get(key)
        .and_then(Json::as_str)
        .map_or("Operator", |s| js_trim(s))
        .to_string()
}

/// TS `applyDuckDbExtraInfo`, then bug fixes 5, 13 and 14.
fn apply_extra_info(out: &mut ExplainPlanNode, extra: &Map<String, Json>) {
    if let Some(Json::String(table)) = extra.get("Table") {
        out.relation_name = Some(table.clone());
    }
    let ec = extra.get("Estimated Cardinality").unwrap_or(&Json::Null);
    if let Some(rows) = ts_cardinality(ec) {
        out.plan_rows = Some(rows);
    }
    if let Some(Json::String(join)) = extra.get("Join Type") {
        out.join_type = Some(join.clone());
    }
    if let Some(Json::String(conditions)) = extra.get("Conditions") {
        let t = &out.node_type;
        if t.contains("HASH_JOIN") || t.contains("HASH JOIN") {
            out.hash_cond = Some(conditions.clone());
        } else if t.contains("INDEX") {
            out.index_cond = Some(conditions.clone());
        } else {
            out.filter = Some(conditions.clone());
        }
    }
    let no_filter = out.filter.as_deref().is_none_or(str::is_empty);
    match extra.get("Filters") {
        Some(Json::String(f)) if no_filter => out.filter = Some(f.clone()),
        Some(Json::Array(list)) if no_filter => {
            let parts: Vec<&str> = list.iter().filter_map(Json::as_str).collect();
            out.filter = Some(parts.join(", "));
        }
        _ => {}
    }
    // Fix 5.
    if let Some(rows) = cardinality(ec) {
        out.plan_rows = Some(rows);
    }
    // Fix 13.
    if out.node_type == "FILTER" && out.filter.is_none() {
        if let Some(Json::String(e)) = extra.get("Expression") {
            out.filter = Some(e.clone());
        }
    }
    // Fix 14.
    if let Some(name) = out.relation_name.take() {
        out.relation_name = Some(last_part(&name));
    }
}

/// TS `convertDuckDbPlanNode`: a plain plan node. Ids in pre-order.
fn plain_node(raw: &Json, ids: &mut Ids) -> ExplainPlanNode {
    let raw = object(raw);
    let id = ids.next();
    let node_type = node_type(raw, "name");
    let children = children(raw).iter().map(|c| plain_node(c, ids)).collect();
    let mut out = ExplainPlanNode {
        id,
        children,
        ..node(node_type)
    };
    apply_extra_info(
        &mut out,
        object(raw.get("extra_info").unwrap_or(&Json::Null)),
    );
    out
}

/// TS `convertDuckDbAnalyzeNode`: an analyzed node. The children are
/// converted first, so ids are in post-order. DuckDB's `operator_timing` is
/// the operator's own time; `actual_total_time` adds the children's, as
/// Postgres's cumulative times do.
fn analyzed_node(raw: &Json, ids: &mut Ids) -> ExplainPlanNode {
    let raw = object(raw);
    let exclusive_ms = raw
        .get("operator_timing")
        .and_then(Json::as_f64)
        .map(|s| s * 1000.0);
    let cardinality = raw.get("operator_cardinality").and_then(Json::as_f64);
    let children: Vec<ExplainPlanNode> = children(raw)
        .iter()
        .map(|c| analyzed_node(c, ids))
        .collect();
    let children_total = children
        .iter()
        .fold(0.0, |acc, c| c.actual_total_time.map_or(acc, |t| acc + t));
    let any_child_timing = children.iter().any(|c| c.actual_total_time.is_some());
    let cumulative = (exclusive_ms.is_some() || any_child_timing)
        .then(|| exclusive_ms.unwrap_or(0.0) + children_total);
    let mut out = ExplainPlanNode {
        id: ids.next(),
        actual_total_time: cumulative,
        actual_rows: cardinality,
        children,
        ..node(node_type(raw, "operator_name"))
    };
    apply_extra_info(
        &mut out,
        object(raw.get("extra_info").unwrap_or(&Json::Null)),
    );
    out
}

/// The plan text of an EXPLAIN result: `explain_value` when it is non-empty
/// text, else the first text cell that starts (after trimming) with `{`
/// (analyzed) or `[` (plain), else `""`.
fn plan_text(result: &QueryResult, analyze: bool) -> String {
    let Some(r) = rows(result).next() else {
        return String::new();
    };
    if let Some(v) = r.str("explain_value").filter(|v| !v.is_empty()) {
        return v.to_string();
    }
    let start = if analyze { '{' } else { '[' };
    r.cells
        .iter()
        .find_map(|c| match c {
            Value::Text(s) if js_trim(s).starts_with(start) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Rows of [`explain_sql`] (TS `parseExplainResult`): one row whose
/// `explain_value` is the plan as JSON. A plan that can't be read is one
/// "Query Plan" node carrying the text.
pub fn parse_explain(result: &QueryResult, analyze: bool) -> ExplainResult {
    let raw = plan_text(result, analyze);
    let mut ids = Ids::new();
    let unknown = |ids: &mut Ids| ExplainPlanNode {
        id: ids.next(),
        filter: Some(if raw.is_empty() {
            "No plan available".to_string()
        } else {
            raw.clone()
        }),
        ..node("Query Plan")
    };
    let result = |plan, execution_time| ExplainResult {
        plan,
        planning_time: 0.0,
        execution_time,
        is_analyze: analyze,
    };
    let parsed: Json = if raw.is_empty() {
        Json::Null
    } else {
        match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => return result(unknown(&mut ids), None),
        }
    };
    if analyze {
        // { latency, children: [EXPLAIN_ANALYZE wrapper whose child is the root] }
        let first = parsed
            .get("children")
            .and_then(Json::as_array)
            .and_then(|c| c.first());
        let wrapper = first
            .and_then(|f| f.get("operator_name"))
            .and_then(Json::as_str)
            .is_some_and(|n| js_trim(n) == "EXPLAIN_ANALYZE");
        let root = if wrapper {
            first
                .and_then(|f| f.get("children"))
                .and_then(Json::as_array)
                .and_then(|c| c.first())
        } else {
            first
        };
        let Some(root) = root.filter(|r| is_truthy(r)) else {
            return result(unknown(&mut ids), None);
        };
        let plan = analyzed_node(root, &mut ids);
        let latency = parsed.get("latency").and_then(Json::as_f64);
        return result(plan, latency.map(|s| s * 1000.0));
    }
    let root = parsed
        .as_array()
        .and_then(|a| a.first())
        .filter(|r| is_truthy(r));
    match root {
        Some(root) => result(plain_node(root, &mut ids), None),
        None => result(unknown(&mut ids), None),
    }
}

/// JS truthiness of a JSON value.
fn is_truthy(v: &Json) -> bool {
    match v {
        Json::Null => false,
        Json::Bool(b) => *b,
        Json::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Json::String(s) => !s.is_empty(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimated_cardinality() {
        let c = |s: &str| cardinality(&Json::String(s.into()));
        assert_eq!(c("100"), Some(100.0));
        assert_eq!(c("~100"), Some(100.0));
        assert_eq!(c("~ 7"), Some(7.0));
        assert_eq!(c("~1_000 rows"), Some(1000.0));
        assert_eq!(c("1,000"), Some(1000.0));
        assert_eq!(c("1 row"), Some(1.0));
        assert_eq!(c(" 12 "), Some(12.0));
        assert_eq!(c("12 ROWS"), Some(12.0));
        assert_eq!(c("n/a"), None);
        assert_eq!(c("rows"), None);
        assert_eq!(c(""), None);
        assert_eq!(cardinality(&serde_json::json!(5)), Some(5.0));
        assert_eq!(ts_cardinality(&Json::String("___".into())), Some(0.0));
    }

    #[test]
    fn last_parts() {
        assert_eq!(last_part("seaquel_test.main.orders"), "orders");
        assert_eq!(last_part("orders"), "orders");
        assert_eq!(
            last_part("seaquel_test.fx_sales.\"fx.dotted \"\"t\"\"\""),
            "fx.dotted \"t\""
        );
        assert_eq!(last_part("\"unclosed"), "\"unclosed");
    }

    #[test]
    fn row_counts_name_the_catalog() {
        assert_eq!(
            row_count_sql("fx_aux", "main", "it's \"x\""),
            "SELECT COUNT(*) AS row_count FROM \"fx_aux\".\"main\".\"it's \"\"x\"\"\""
        );
    }
}
