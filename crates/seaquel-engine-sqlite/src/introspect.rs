//! SQLite introspection: the catalog SQL and the pure functions that turn its
//! results into Seaquel's schema, statistics and EXPLAIN types.
//!
//! Ported from `src/lib/db/sqlite.ts` (deleted in phase 2) and the SQLite
//! branches of `TsEngineClient` (per-table row counts; EXPLAIN ANALYZE
//! timing, which the driver does). `tests/introspect_parity.rs` pins the SQL
//! and the parsers against the recorded TypeScript output.
//!
//! Changes from the TypeScript, each a numbered bug fix:
//! - 1: table names are bound (`pragma_table_info(?)` and friends) or quoted
//!   with `"` doubled (row counts), not checked with `validateIdentifier`,
//!   which rejected any name with a space, `-` or `'` and so hid its columns,
//!   indexes and row count.
//! - 5: index columns come from `pragma_index_info` (the TS listed none).
//! - 7: the autoindexes behind UNIQUE and PRIMARY KEY constraints
//!   (`sqlite_autoindex_*`) are listed like any index, and counted in the
//!   statistics.
//! - 8: EXPLAIN QUERY PLAN details with names that aren't one ASCII word
//!   (`order items`, `café`, `(subquery-2)`), numbered subqueries
//!   (`SCALAR SUBQUERY 1`, `LIST SUBQUERY 1`), `SCAN CONSTANT ROW`,
//!   `BLOOM FILTER`, `LEFT-JOIN`/`RIGHT-JOIN`, virtual tables and
//!   `LAST n TERMS OF ORDER BY`.
//! - 9 (found while recording): a foreign key declared without parent
//!   columns (`REFERENCES customers`) has a NULL `to`; it's the parent's
//!   primary key column, which the TS showed as `null`.
//! - 10 (found while recording): `NOT LIKE 'sqlite_%'` treats `_` as a
//!   wildcard, so the TS hid user tables such as `sqlitex` or `SQLiteData`;
//!   only the reserved `sqlite_` prefix is hidden now.
//!
//! Parsers read cells by column name, as the TS row objects did.

use seaquel_engine::introspect::{node, rows, Ids};
use seaquel_engine::QueryResult;
use seaquel_types::{
    DatabaseOverview, ExplainPlanNode, ExplainResult, ForeignKeyRef, IndexUsageInfo, SchemaColumn,
    SchemaIndex, SchemaTable, TableKind, TableSizeInfo,
};

use crate::dialect::qi;

// ── SQL ──────────────────────────────────────────────────────────────────────

/// Tables and views (TS `getSchemaQuery`). Bug fix 10: `_` escaped.
pub const SCHEMA_SQL: &str = "SELECT name, type FROM sqlite_master\n\t\t\tWHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'\n\t\t\tORDER BY name";

/// Columns of one table (TS `PRAGMA table_info('<table>')`). Bug fix 1:
/// the table-valued function takes the name as a bound parameter.
pub const COLUMNS_SQL: &str =
    "SELECT cid, name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?)";

/// Foreign keys of one table (TS `PRAGMA foreign_key_list('<table>')`). Bug
/// fix 1: bound. Bug fix 9: a NULL `to` (no parent columns declared) is the
/// parent's primary key column at the same position.
pub const FOREIGN_KEYS_SQL: &str = "SELECT fk.id, fk.seq, fk.\"table\", fk.\"from\",
\t\t\tCOALESCE(fk.\"to\", (SELECT p.name FROM pragma_table_info(fk.\"table\") p WHERE p.pk = fk.seq + 1)) AS \"to\",
\t\t\tfk.on_update, fk.on_delete, fk.\"match\"
\t\tFROM pragma_foreign_key_list(?) fk";

/// Indexes of one table with their key columns, one row per column (TS
/// `PRAGMA index_list('<table>')`, without columns). Bug fixes 1 (bound) and
/// 5 (`pragma_index_info`). An expression key is `<expression>`, the rowid
/// `rowid`.
pub const INDEXES_SQL: &str = "SELECT il.seq, il.name, il.\"unique\", il.origin, il.partial,
\t\t\tCASE ii.cid WHEN -2 THEN '<expression>' WHEN -1 THEN 'rowid' ELSE ii.name END AS column_name
\t\tFROM pragma_index_list(?) il
\t\tLEFT JOIN pragma_index_info(il.name) ii
\t\tORDER BY il.seq, ii.seqno";

/// Tables for the statistics (TS `getTableSizesQuery`). Bug fix 10.
pub const TABLE_SIZES_SQL: &str = "SELECT\n\t\t\tname AS table_name,\n\t\t\t'main' AS schema_name\n\t\tFROM sqlite_master\n\t\tWHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'\n\t\tORDER BY name";

/// Indexes for the statistics (TS `getIndexUsageQuery`). Bug fix 7: the
/// autoindexes are listed too.
pub const INDEX_USAGE_SQL: &str = "SELECT\n\t\t\tm.name AS index_name,\n\t\t\tm.tbl_name AS table_name,\n\t\t\t'main' AS schema_name\n\t\tFROM sqlite_master m\n\t\tWHERE m.type = 'index'\n\t\tORDER BY m.tbl_name, m.name";

/// Database overview (TS `getDatabaseOverviewQuery`). Bug fixes 7 (every
/// index is counted) and 10.
pub const OVERVIEW_SQL: &str = "SELECT\n\t\t\t(SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\') AS table_count,\n\t\t\t(SELECT COUNT(*) FROM sqlite_master WHERE type = 'index') AS index_count,\n\t\t\t(SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()) AS total_size_bytes";

/// Rows of one table (TS `getTableRowCountQuery`). Bug fix 1: the name is
/// quoted with `"` doubled instead of rejected.
pub fn row_count_sql(table: &str) -> String {
    format!("SELECT COUNT(*) AS row_count FROM {}", qi(table))
}

/// `EXPLAIN QUERY PLAN <sql>`, one trailing `;` stripped (TS
/// `query.replace(/;$/, "")`).
pub fn explain_sql(sql: &str) -> String {
    let base = sql.strip_suffix(';').unwrap_or(sql);
    format!("EXPLAIN QUERY PLAN {base}")
}

// ── Schema ───────────────────────────────────────────────────────────────────

/// SQLite has one schema per database file as far as Seaquel is concerned
/// (TS `getSchemasQuery`: `SELECT 'main'`).
pub fn schemas() -> Vec<String> {
    vec!["main".to_string()]
}

/// Rows of [`SCHEMA_SQL`] (TS `parseSchemaResult`).
pub fn parse_schema(result: &QueryResult) -> Vec<SchemaTable> {
    rows(result)
        .map(|r| SchemaTable {
            name: r.text("name"),
            schema: "main".to_string(),
            kind: if r.str("type") == Some("view") {
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

/// Rows of [`COLUMNS_SQL`] and [`FOREIGN_KEYS_SQL`] (TS
/// `parseColumnsResult`). A column's foreign key is the last one listing it
/// (TS `Map.set`). A NULL `to` (the TS query's rows, see fix 9) reads as `""`:
/// the reference type has no null.
pub fn parse_columns(
    columns: &QueryResult,
    foreign_keys: Option<&QueryResult>,
) -> Vec<SchemaColumn> {
    let mut fks: Vec<(String, ForeignKeyRef)> = Vec::new();
    if let Some(result) = foreign_keys {
        for fk in rows(result) {
            let from = fk.text("from");
            let reference = ForeignKeyRef {
                referenced_schema: "main".to_string(),
                referenced_table: fk.text("table"),
                referenced_column: fk.text("to"),
            };
            match fks.iter_mut().find(|(f, _)| *f == from) {
                Some(entry) => entry.1 = reference,
                None => fks.push((from, reference)),
            }
        }
    }
    rows(columns)
        .map(|c| {
            let name = c.text("name");
            let foreign_key_ref = fks.iter().find(|(f, _)| *f == name).map(|(_, r)| r.clone());
            SchemaColumn {
                // `col.type || "BLOB"`: SQLite allows no type at all.
                ty: c
                    .str("type")
                    .filter(|t| !t.is_empty())
                    .unwrap_or("BLOB")
                    .to_string(),
                cast_type: None,
                nullable: c.int("notnull") == Some(0),
                default_value: c
                    .str("dflt_value")
                    .filter(|d| !d.is_empty())
                    .map(str::to_string),
                is_primary_key: c.int("pk").is_some_and(|pk| pk > 0),
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

/// Rows of [`INDEXES_SQL`] (one per key column) or of the TS
/// `PRAGMA index_list` (one per index, no columns), grouped by index name in
/// order. Bug fix 7: `sqlite_autoindex_*` indexes are kept (the TS hid every
/// `sqlite_` name). Bug fix 5: `column_name`, where present, lists the columns.
pub fn parse_indexes(result: &QueryResult) -> Vec<SchemaIndex> {
    let mut out: Vec<SchemaIndex> = Vec::new();
    for r in rows(result) {
        let name = r.text("name");
        if name.is_empty() {
            continue;
        }
        let column = r.str("column_name").map(str::to_string);
        match out.iter_mut().find(|i| i.name == name) {
            Some(index) => index.columns.extend(column),
            None => out.push(SchemaIndex {
                columns: column.into_iter().collect(),
                unique: r.int("unique") == Some(1),
                ty: "btree".to_string(),
                name,
            }),
        }
    }
    out
}

/// Names of the partial indexes (`CREATE … INDEX … WHERE …`) in rows of
/// [`INDEXES_SQL`], which selects `partial` but the TS's parse drops it. Not
/// a column's UNIQUE (Task 18).
pub fn partial_indexes(result: &QueryResult) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in rows(result) {
        let name = r.text("name");
        if r.int("partial") == Some(1) && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

// ── Statistics ───────────────────────────────────────────────────────────────

/// Rows of [`TABLE_SIZES_SQL`] (TS `parseTableSizesResult`). SQLite keeps no
/// per-table sizes; the driver fills in `row_count` with [`row_count_sql`].
pub fn parse_table_sizes(result: &QueryResult) -> Vec<TableSizeInfo> {
    rows(result)
        .map(|r| TableSizeInfo {
            schema: r
                .str("schema_name")
                .filter(|s| !s.is_empty())
                .unwrap_or("main")
                .to_string(),
            name: r.text("table_name"),
            row_count: 0,
            total_size: "N/A".to_string(),
            total_size_bytes: 0,
            data_size: None,
            index_size: None,
        })
        .collect()
}

/// A result of [`row_count_sql`] (TsEngineClient: `Number(row_count) || 0`).
pub fn parse_row_count(result: &QueryResult) -> i64 {
    rows(result).next().map_or(0, |r| r.number("row_count"))
}

/// Rows of [`INDEX_USAGE_SQL`] (TS `parseIndexUsageResult`). SQLite tracks
/// no index usage.
pub fn parse_index_usage(result: &QueryResult) -> Vec<IndexUsageInfo> {
    rows(result)
        .map(|r| IndexUsageInfo {
            schema: r
                .str("schema_name")
                .filter(|s| !s.is_empty())
                .unwrap_or("main")
                .to_string(),
            table: r.text("table_name"),
            index_name: r.text("index_name"),
            size: "N/A".to_string(),
            scans: 0,
            rows_read: None,
            unused: false,
        })
        .collect()
}

/// Rows of [`OVERVIEW_SQL`] (TS `parseDatabaseOverviewResult`).
pub fn parse_overview(result: &QueryResult) -> DatabaseOverview {
    let first = rows(result).next();
    let number = |c: &str| first.as_ref().map_or(0, |r| r.number(c));
    let size = number("total_size_bytes");
    DatabaseOverview {
        database_name: "SQLite Database".to_string(),
        total_size: format_bytes(size),
        total_size_bytes: Some(size),
        table_count: number("table_count"),
        index_count: number("index_count"),
        connection_count: None,
    }
}

/// TS `formatBytes`: `parseFloat((bytes / 1024 ** i).toFixed(2)) + " " + unit`.
/// Beyond TB the TS printed `undefined` as the unit; this stays in TB.
fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 bytes".to_string();
    }
    let b = bytes as f64;
    let i = (b.ln() / 1024f64.ln()).floor();
    // A negative size has no logarithm (NaN): JS indexes `sizes[NaN]`.
    let i = if i.is_nan() {
        0
    } else {
        (i.max(0.0) as usize).min(UNITS.len() - 1)
    };
    let scaled = b / 1024f64.powi(i as i32);
    format!("{} {}", js_to_fixed_2_trimmed(scaled), UNITS[i])
}

/// JS `parseFloat(x.toFixed(2))` as a string, for finite `x` below 1e21:
/// `toFixed` rounds the exact binary value half up (Rust's `{:.2}` rounds
/// ties to even: 1.125 → "1.12", JS "1.13"), then `parseFloat` and `String`
/// drop trailing zeros.
fn js_to_fixed_2_trimmed(x: f64) -> String {
    // Exact decimal expansion: every f64 here has at most 52 fractional bits.
    let exact = format!("{:.60}", x.abs());
    let (int, frac) = exact.split_once('.').expect("fixed notation");
    let mut digits: Vec<u8> = format!("{int}{}", &frac[..2]).into_bytes();
    let rest = &frac[2..];
    if rest.as_bytes()[0] >= b'5' {
        // Round half up on the digit string.
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let s = String::from_utf8(digits).expect("ascii digits");
    let (int, frac) = s.split_at(s.len() - 2);
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let frac = frac.trim_end_matches('0');
    let sign = if x < 0.0 && (int != "0" || !frac.is_empty()) {
        "-"
    } else {
        ""
    };
    if frac.is_empty() {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac}")
    }
}

// ── EXPLAIN ──────────────────────────────────────────────────────────────────

/// Rows of `EXPLAIN QUERY PLAN` (`id`, `parent`, `notused`, `detail`) as a
/// tree (TS `parseExplainResult`). The TS leaves `execution_time` unset;
/// for ANALYZE the driver runs the statement and fills in the root's actual
/// rows and time and the execution time (TsEngineClient did that before).
pub fn parse_explain(result: &QueryResult, analyze: bool) -> ExplainResult {
    let mut ids = Ids::new();
    let plan = if result.rows.is_empty() {
        ExplainPlanNode {
            id: ids.next(),
            ..node("Query Plan")
        }
    } else {
        plan_tree(result, &mut ids)
    };
    ExplainResult {
        plan,
        planning_time: 0.0,
        execution_time: None,
        is_analyze: analyze,
    }
}

fn plan_tree(result: &QueryResult, ids: &mut Ids) -> ExplainPlanNode {
    struct Entry {
        node: Option<ExplainPlanNode>,
        id: Option<i64>,
        parent: Option<i64>,
        children: Vec<usize>,
    }
    let mut entries: Vec<Entry> = rows(result)
        .map(|r| Entry {
            node: Some(parse_detail(r.str("detail").unwrap_or_default(), ids)),
            id: r.int("id"),
            parent: r.int("parent"),
            children: vec![],
        })
        .collect();

    // `byId`: a repeated id resolves to its last row.
    let by_id = |id: i64, entries: &[Entry]| entries.iter().rposition(|e| e.id == Some(id));
    let mut roots = Vec::new();
    for i in 0..entries.len() {
        match entries[i].parent {
            Some(p) if p != 0 => match by_id(p, &entries) {
                Some(parent) => entries[parent].children.push(i),
                None => roots.push(i),
            },
            // `parent === 0`, or not a number (never in the map).
            Some(_) => roots.push(i),
            None => roots.push(i),
        }
    }

    // Build bottom-up from the roots; a cycle (never in SQLite's output)
    // drops the nodes on it instead of looping.
    fn build(i: usize, entries: &mut Vec<Entry>) -> Option<ExplainPlanNode> {
        let mut n = entries[i].node.take()?;
        let children = std::mem::take(&mut entries[i].children);
        n.children = children
            .into_iter()
            .filter_map(|c| build(c, entries))
            .collect();
        Some(n)
    }
    let mut roots: Vec<ExplainPlanNode> = roots
        .into_iter()
        .filter_map(|i| build(i, &mut entries))
        .collect();

    // TS `wrapSqliteRoots`: one root stands alone; two or more scans at the
    // top are a nested-loop join; anything else gets a neutral wrapper.
    if roots.len() == 1 {
        return roots.remove(0);
    }
    let scan_like = |n: &ExplainPlanNode| {
        matches!(
            n.node_type.as_str(),
            "Seq Scan" | "Index Scan" | "Index Only Scan"
        )
    };
    let all_scans = roots.len() >= 2 && roots.iter().all(scan_like);
    let mut wrapper = ExplainPlanNode {
        id: ids.next(),
        ..node(if all_scans {
            "Nested Loop"
        } else {
            "Query Plan"
        })
    };
    wrapper.children = roots;
    wrapper
}

/// `(table, alias)` from `t` or `t AS a` (pre-3.36 SQLite printed the alias
/// that way; the TS kept it only when it differs from the table).
fn table_and_alias(target: &str) -> (String, Option<String>) {
    match target.split_once(" AS ") {
        Some((table, alias)) if !table.is_empty() && !alias.is_empty() => (
            table.to_string(),
            (alias != table).then(|| alias.to_string()),
        ),
        _ => (target.to_string(), None),
    }
}

/// A trailing ` (<cond>)`, balanced: `("x (y)=?")` → (`x`, `y=?`).
fn split_condition(s: &str) -> (&str, Option<&str>) {
    if !s.ends_with(')') {
        return (s, None);
    }
    let mut depth = 0usize;
    for (i, c) in s.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    let inner = &s[i + 1..s.len() - 1];
                    return match s[..i].strip_suffix(' ') {
                        Some(head) if !inner.is_empty() => (head, Some(inner)),
                        _ => (s, None),
                    };
                }
            }
            _ => {}
        }
    }
    (s, None)
}

/// The access paths after ` USING ` that SQLite prints.
const USING_CLAUSES: [&str; 5] = [
    "COVERING INDEX ",
    "INDEX ",
    "AUTOMATIC ",
    "INTEGER PRIMARY KEY",
    "PRIMARY KEY",
];

/// `<target>[ USING <clause>]`, at the first ` USING ` that starts a known
/// clause (names may contain spaces, e.g. `order items`).
fn split_using(rest: &str) -> (&str, Option<&str>) {
    let mut from = 0;
    while let Some(pos) = rest[from..].find(" USING ") {
        let at = from + pos;
        let clause = &rest[at + " USING ".len()..];
        if USING_CLAUSES.iter().any(|c| clause.starts_with(c)) {
            return (&rest[..at], Some(clause));
        }
        from = at + 1;
    }
    (rest, None)
}

/// `SCAN …` / `SEARCH …` after the keyword. `None` for a shape it doesn't
/// know (the caller falls back to the first word, as the TS did).
fn access(rest: &str, search: bool, n: &mut ExplainPlanNode) -> Option<()> {
    if let Some((target, _)) = rest.split_once(" VIRTUAL TABLE INDEX ") {
        n.node_type = "Virtual Table Scan".into();
        n.relation_name = Some(target.to_string());
        return Some(());
    }
    let (target, clause) = split_using(rest);
    if target.is_empty() {
        return None;
    }
    let (table, alias) = table_and_alias(target);
    match clause {
        // An access path this parser doesn't know.
        None if target.contains(" USING ") => return None,
        None if search => {
            // `SEARCH t`: a seek on the rowid b-tree (e.g. `max(rowid)`).
            n.node_type = "Index Scan".into();
            n.index_name = Some("PRIMARY KEY".into());
        }
        None => n.node_type = "Seq Scan".into(),
        Some(clause) => {
            let (head, cond) = split_condition(clause);
            if let Some(index) = head.strip_prefix("COVERING INDEX ") {
                n.node_type = "Index Only Scan".into();
                n.index_name = Some(index.to_string());
            } else if let Some(index) = head.strip_prefix("INDEX ") {
                n.node_type = "Index Scan".into();
                n.index_name = Some(index.to_string());
            } else if let Some(auto) = head.strip_prefix("AUTOMATIC ") {
                let auto = auto.strip_prefix("PARTIAL ").unwrap_or(auto);
                n.node_type = match auto {
                    "COVERING INDEX" => "Index Only Scan",
                    "INDEX" => "Index Scan",
                    _ => return None,
                }
                .into();
                // Built at query time: no name.
                n.index_name = Some("<automatic>".into());
            } else if matches!(head, "INTEGER PRIMARY KEY" | "PRIMARY KEY") {
                n.node_type = "Index Scan".into();
                n.index_name = Some("PRIMARY KEY".into());
            } else {
                return None;
            }
            n.index_cond = cond.map(str::to_string);
        }
    }
    n.relation_name = Some(table);
    n.alias = alias;
    Some(())
}

/// One `detail` string (TS `parseSqliteDetail`; grammar in
/// https://www.sqlite.org/eqp.html). Same node types as the TS wherever its
/// patterns matched; bug fix 8 reads the shapes it fell back on.
fn parse_detail(detail: &str, ids: &mut Ids) -> ExplainPlanNode {
    let mut n = ExplainPlanNode {
        id: ids.next(),
        ..node("Step")
    };
    if read_detail(detail, &mut n).is_none() {
        // Unknown shape: the first word, not the raw detail in a field that
        // would imply structure the engine didn't confirm.
        n = ExplainPlanNode {
            id: n.id,
            ..node(detail.split(' ').next().unwrap_or_default())
        };
    }
    n
}

fn read_detail(detail: &str, n: &mut ExplainPlanNode) -> Option<()> {
    // `… LEFT-JOIN`: the right side of an outer join (SQLite 3.39+).
    let (d, left_join) = match detail.strip_suffix(" LEFT-JOIN") {
        Some(d) => (d, true),
        None => (detail, false),
    };
    let set = |n: &mut ExplainPlanNode, t: &str| n.node_type = t.to_string();

    if d == "SCAN CONSTANT ROW" {
        // A SELECT without FROM.
        set(n, "Result");
    } else if let Some(rest) = d.strip_prefix("SCAN ") {
        access(rest, false, n)?;
    } else if let Some(rest) = d.strip_prefix("SEARCH ") {
        access(rest, true, n)?;
    } else if let Some(kind) = d.strip_prefix("USE TEMP B-TREE FOR ") {
        set(
            n,
            if kind == "GROUP BY" {
                "Group"
            } else if kind.contains("DISTINCT") {
                "Distinct"
            } else if kind.ends_with("ORDER BY") {
                // ORDER BY, RIGHT/LEFT PART OF ORDER BY, LAST [n] TERM[S] OF ORDER BY
                "Sort"
            } else {
                return None;
            },
        );
    } else if let Some(name) = keyword_and_name(d, "MATERIALIZE") {
        // The alias matters: the outer query reads it with `SCAN <alias>`.
        set(n, "Materialize");
        n.relation_name = name;
    } else if let Some(name) = keyword_and_name(d, "CO-ROUTINE") {
        set(n, "Subquery");
        n.relation_name = name;
    } else if let Some(correlated) = subquery(d) {
        set(
            n,
            if correlated {
                "Correlated Subquery"
            } else {
                "Subquery"
            },
        );
    } else if d == "MULTI-INDEX OR" {
        set(n, "BitmapOr");
    } else if let Some(op) = d
        .strip_prefix("MERGE (")
        .and_then(|r| r.strip_suffix(')'))
        .filter(|op| matches!(*op, "UNION" | "UNION ALL" | "INTERSECT" | "EXCEPT"))
    {
        n.node_type = format!("Merge ({op})");
    } else if d == "LEFT" || d == "RIGHT" {
        set(
            n,
            if d == "LEFT" {
                "Left Input"
            } else {
                "Right Input"
            },
        );
    } else if d.starts_with("COMPOUND QUERY") {
        set(n, "Compound");
    } else if d == "LEFT-MOST SUBQUERY" {
        set(n, "Subquery");
    } else if let Some(kind) = compound_operator(d) {
        set(n, kind);
    } else if d == "CREATE BLOOM FILTER" {
        set(n, "Bloom Filter");
    } else if let Some(rest) = d.strip_prefix("BLOOM FILTER ON ") {
        let (table, cond) = split_condition(rest);
        set(n, "Bloom Filter");
        n.relation_name = Some(table.to_string());
        n.filter = cond.map(str::to_string);
    } else if let Some(table) = d.strip_prefix("RIGHT-JOIN ") {
        // The pass over the right table's unmatched rows.
        set(n, "Right Join");
        n.relation_name = Some(table.to_string());
    } else {
        return None;
    }
    if left_join {
        n.join_type = Some("Left".into());
    }
    Some(())
}

/// `KEYWORD` → `Some(None)`, `KEYWORD <name>` → `Some(Some(name))`.
fn keyword_and_name(d: &str, keyword: &str) -> Option<Option<String>> {
    let rest = d.strip_prefix(keyword)?;
    if rest.is_empty() {
        return Some(None);
    }
    let name = rest.strip_prefix(' ')?;
    (!name.is_empty()).then(|| Some(name.to_string()))
}

/// `[CORRELATED ][SCALAR |LIST ]SUBQUERY[ n]` → whether it's correlated.
fn subquery(d: &str) -> Option<bool> {
    let (correlated, rest) = match d.strip_prefix("CORRELATED ") {
        Some(rest) => (true, rest),
        None => (false, d),
    };
    let rest = rest
        .strip_prefix("SCALAR ")
        .or_else(|| rest.strip_prefix("LIST "))
        .unwrap_or(rest);
    let number = rest.strip_prefix("SUBQUERY")?;
    let ok = number.is_empty()
        || number
            .strip_prefix(' ')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    ok.then_some(correlated)
}

/// `UNION ALL|UNION|EXCEPT|INTERSECT[ USING TEMP B-TREE]`.
fn compound_operator(d: &str) -> Option<&'static str> {
    let op = d.strip_suffix(" USING TEMP B-TREE").unwrap_or(d);
    Some(match op {
        "UNION ALL" => "Union All",
        "UNION" => "Union",
        "EXCEPT" => "Except",
        "INTERSECT" => "Intersect",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use seaquel_engine::Value;

    fn detail(d: &str) -> ExplainPlanNode {
        parse_detail(d, &mut Ids::new())
    }

    #[test]
    fn to_fixed_rounds_ties_up_like_js() {
        assert_eq!(js_to_fixed_2_trimmed(1.125), "1.13");
        assert_eq!(js_to_fixed_2_trimmed(8.125), "8.13");
        assert_eq!(js_to_fixed_2_trimmed(212.0), "212");
        assert_eq!(js_to_fixed_2_trimmed(0.5), "0.5");
        assert_eq!(js_to_fixed_2_trimmed(1023.999), "1024");
        // Exact binary values: 99.995 is 99.99500000000000454…, 2.675 is 2.67499999999999982…
        assert_eq!(js_to_fixed_2_trimmed(99.995), "100");
        assert_eq!(js_to_fixed_2_trimmed(2.675), "2.67");
        assert_eq!(js_to_fixed_2_trimmed(1.005), "1");
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(1023), "1023 bytes");
        assert_eq!(format_bytes(1152), "1.13 KB");
        assert_eq!(format_bytes(1_048_575), "1024 KB");
        assert_eq!(format_bytes(1_048_576), "1 MB");
        assert_eq!(format_bytes(8_519_680), "8.13 MB");
        assert_eq!(format_bytes(1 << 40), "1 TB");
        assert_eq!(format_bytes(1 << 50), "1024 TB");
    }

    #[test]
    fn conditions_are_balanced() {
        assert_eq!(split_condition("INDEX i (a=?)"), ("INDEX i", Some("a=?")));
        assert_eq!(
            split_condition("INDEX i (f(a)=?)"),
            ("INDEX i", Some("f(a)=?"))
        );
        assert_eq!(split_condition("INDEX i"), ("INDEX i", None));
        assert_eq!(split_condition("INDEX (i)x)"), ("INDEX (i)x)", None));
    }

    /// Fix 8 on single details the TS fell back on.
    #[test]
    fn details_the_typescript_missed() {
        let n = detail("SEARCH order items USING COVERING INDEX order items sku idx (sku=?)");
        assert_eq!(n.node_type, "Index Only Scan");
        assert_eq!(n.relation_name.as_deref(), Some("order items"));
        assert_eq!(n.index_name.as_deref(), Some("order items sku idx"));
        assert_eq!(n.index_cond.as_deref(), Some("sku=?"));

        let n = detail("SEARCH o USING AUTOMATIC COVERING INDEX (total=?) LEFT-JOIN");
        assert_eq!(
            (
                n.node_type.as_str(),
                n.index_name.as_deref(),
                n.join_type.as_deref()
            ),
            ("Index Only Scan", Some("<automatic>"), Some("Left"))
        );
        assert_eq!(detail("SCAN café").relation_name.as_deref(), Some("café"));
        assert_eq!(detail("SCAN CONSTANT ROW").node_type, "Result");
        assert_eq!(
            detail("CORRELATED LIST SUBQUERY 3").node_type,
            "Correlated Subquery"
        );
        assert_eq!(detail("SCALAR SUBQUERY 12").node_type, "Subquery");
        assert_eq!(detail("SUBQUERY x").node_type, "SUBQUERY");
        let n = detail("CO-ROUTINE (subquery-2)");
        assert_eq!(n.relation_name.as_deref(), Some("(subquery-2)"));
        let n = detail("BLOOM FILTER ON c (score=?)");
        assert_eq!(
            (
                n.node_type.as_str(),
                n.relation_name.as_deref(),
                n.filter.as_deref()
            ),
            ("Bloom Filter", Some("c"), Some("score=?"))
        );
        assert_eq!(
            detail("USE TEMP B-TREE FOR LAST 2 TERMS OF ORDER BY").node_type,
            "Sort"
        );
        assert_eq!(
            detail("USE TEMP B-TREE FOR count(DISTINCT)").node_type,
            "Distinct"
        );
        assert_eq!(detail("USE TEMP B-TREE FOR SOMETHING").node_type, "USE");
        let n = detail("SCAN pragma_table_info VIRTUAL TABLE INDEX 0:");
        assert_eq!(n.node_type, "Virtual Table Scan");
        assert_eq!(n.relation_name.as_deref(), Some("pragma_table_info"));
        let n = detail("RIGHT-JOIN b");
        assert_eq!(
            (n.node_type.as_str(), n.relation_name.as_deref()),
            ("Right Join", Some("b"))
        );
        let n = detail("SEARCH customers");
        assert_eq!(n.index_name.as_deref(), Some("PRIMARY KEY"));
        // A name with " USING " in it that isn't an access path.
        let n = detail("SCAN a USING b USING INDEX i");
        assert_eq!(n.relation_name.as_deref(), Some("a USING b"));
        assert_eq!(n.index_name.as_deref(), Some("i"));
        // Unknown shapes keep the first word.
        assert_eq!(detail("SEARCH t USING ROWID SEARCH").node_type, "SEARCH");
        assert_eq!(detail("SEARCH t USING AUTOMATIC THING").node_type, "SEARCH");
        assert_eq!(detail("SCAN ").node_type, "SCAN");
        assert_eq!(detail("MATERIALIZE ").node_type, "MATERIALIZE");
    }

    #[test]
    fn cyclic_parents_do_not_loop() {
        let r = QueryResult {
            columns: vec!["id".into(), "parent".into(), "detail".into()],
            rows: vec![
                vec![Value::Int(1), Value::Int(2), Value::from("SCAN a")],
                vec![Value::Int(2), Value::Int(1), Value::from("SCAN b")],
                vec![Value::Int(3), Value::Int(0), Value::from("SCAN c")],
            ],
        };
        let plan = parse_explain(&r, false).plan;
        assert_eq!(plan.relation_name.as_deref(), Some("c"));
        assert!(plan.children.is_empty());
    }
}
