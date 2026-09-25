//! SQL Server introspection: the catalog SQL and the pure functions that turn
//! its results into Seaquel's schema and EXPLAIN types.
//!
//! Ported from `src/lib/db/mssql.ts` (deleted in phase 2). The schema and
//! schema-list SQL are the TypeScript text; the columns and indexes queries carry bug fixes 1 (bound
//! filters, `@P1` table and `@P2` schema), 3 (views have columns), 4 (one row
//! per column), 6 (no INCLUDE columns), 10 (the full type) and 15 (the
//! collation when it isn't the database default). `tests/introspect_parity.rs`
//! pins the SQL and the parsers against the recorded TypeScript output and
//! `bugfixes.json`.
//!
//! EXPLAIN (bug fix 9) runs as three batches on the held connection (see
//! [`explain_batches`]); [`parse_explain`] reads the plan XML with roxmltree
//! the way the TypeScript `convertMssqlRelOp` read it with `DOMParser`, and
//! un-escapes `]]` in names (fix 11).

use seaquel_engine::introspect::{js_string_to_number, node, rows, Ids};
use seaquel_engine::{DbError, QueryResult, Value};
use seaquel_types::{
    ExplainPlanNode, ExplainResult, ForeignKeyRef, SchemaColumn, SchemaIndex, SchemaTable,
    TableKind,
};

use crate::session::ResultSet;

// ── SQL ──────────────────────────────────────────────────────────────────────

/// Schema names (TS `getSchemasQuery`). Column: `schema_name`.
pub const SCHEMAS_SQL: &str = "SELECT name as schema_name FROM sys.schemas WHERE name NOT IN ('sys', 'guest', 'INFORMATION_SCHEMA') ORDER BY name;";

/// Tables and views (TS `getSchemaQuery`).
pub const SCHEMA_SQL: &str = "SELECT s.name AS schema_name, t.name AS table_name, 'TABLE' AS object_type\n\t\tFROM sys.tables t\n\t\tINNER JOIN sys.schemas s ON t.schema_id = s.schema_id\n\t\tWHERE t.is_ms_shipped = 0\n\t\tUNION ALL\n\t\tSELECT s.name AS schema_name, v.name AS table_name, 'VIEW' AS object_type\n\t\tFROM sys.views v\n\t\tINNER JOIN sys.schemas s ON v.schema_id = s.schema_id\n\t\tWHERE v.is_ms_shipped = 0\n\t\tORDER BY schema_name, table_name";

/// Columns of one table or view, `@P1` the table and `@P2` the schema.
/// Fixes 1, 3, 4, 10 and 15 (see the module docs): `sys.objects` of type U
/// or V, the full type, the collation only when it differs from the
/// database's, and for a column in several foreign keys the reference of
/// the one whose name sorts first.
pub const COLUMNS_SQL: &str = "SELECT\n  c.name AS column_name,\n  CASE\n    WHEN TYPE_NAME(c.user_type_id) IN ('nvarchar', 'nchar')\n      THEN TYPE_NAME(c.user_type_id) + '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length / 2 AS varchar(10)) END + ')'\n    WHEN TYPE_NAME(c.user_type_id) IN ('varchar', 'char', 'varbinary', 'binary')\n      THEN TYPE_NAME(c.user_type_id) + '(' + CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length AS varchar(10)) END + ')'\n    WHEN TYPE_NAME(c.user_type_id) IN ('decimal', 'numeric')\n      THEN TYPE_NAME(c.user_type_id) + '(' + CAST(c.precision AS varchar(10)) + ',' + CAST(c.scale AS varchar(10)) + ')'\n    WHEN TYPE_NAME(c.user_type_id) IN ('datetime2', 'time', 'datetimeoffset')\n      THEN TYPE_NAME(c.user_type_id) + '(' + CAST(c.scale AS varchar(10)) + ')'\n    ELSE TYPE_NAME(c.user_type_id)\n  END AS data_type,\n  CASE WHEN c.is_nullable = 1 THEN 'YES' ELSE 'NO' END AS is_nullable,\n  dc.definition AS column_default,\n  CASE WHEN c.collation_name COLLATE DATABASE_DEFAULT <> CAST(DATABASEPROPERTYEX(DB_NAME(), 'Collation') AS sysname) COLLATE DATABASE_DEFAULT THEN c.collation_name END AS collation_name,\n  CASE WHEN pk.column_id IS NOT NULL THEN 1 ELSE 0 END AS is_primary_key,\n  CASE WHEN fk.referenced_table IS NOT NULL THEN 1 ELSE 0 END AS is_foreign_key,\n  fk.referenced_schema, fk.referenced_table, fk.referenced_column\nFROM sys.columns c\nINNER JOIN sys.objects o ON c.object_id = o.object_id AND o.type IN ('U', 'V')\nINNER JOIN sys.schemas s ON o.schema_id = s.schema_id\nLEFT JOIN sys.default_constraints dc ON c.default_object_id = dc.object_id\nLEFT JOIN (\n  SELECT ic.object_id, ic.column_id FROM sys.index_columns ic\n  INNER JOIN sys.indexes i ON ic.object_id = i.object_id AND ic.index_id = i.index_id\n  WHERE i.is_primary_key = 1\n) pk ON c.object_id = pk.object_id AND c.column_id = pk.column_id\nOUTER APPLY (\n  SELECT TOP (1) rs.name AS referenced_schema, rt.name AS referenced_table, rc.name AS referenced_column\n  FROM sys.foreign_key_columns fkc\n  INNER JOIN sys.foreign_keys f ON fkc.constraint_object_id = f.object_id\n  INNER JOIN sys.tables rt ON fkc.referenced_object_id = rt.object_id\n  INNER JOIN sys.schemas rs ON rt.schema_id = rs.schema_id\n  INNER JOIN sys.columns rc ON fkc.referenced_object_id = rc.object_id AND fkc.referenced_column_id = rc.column_id\n  WHERE fkc.parent_object_id = c.object_id AND fkc.parent_column_id = c.column_id\n  ORDER BY f.name\n) fk\nWHERE o.name = @P1 AND s.name = @P2\nORDER BY c.column_id";

/// Index key columns of one table or view, `@P1` the table and `@P2` the
/// schema (fixes 1 and 6).
pub const INDEXES_SQL: &str = "SELECT i.name AS index_name, c.name AS column_name, i.is_unique, i.type_desc AS index_type\nFROM sys.indexes i\nINNER JOIN sys.index_columns ic ON i.object_id = ic.object_id AND i.index_id = ic.index_id\nINNER JOIN sys.columns c ON ic.object_id = c.object_id AND ic.column_id = c.column_id\nINNER JOIN sys.objects o ON i.object_id = o.object_id\nINNER JOIN sys.schemas s ON o.schema_id = s.schema_id\nWHERE o.name = @P1 AND s.name = @P2 AND i.name IS NOT NULL AND ic.is_included_column = 0\nORDER BY i.name, ic.key_ordinal";

/// Unique indexes of a table that are filtered or have INCLUDE columns: not a
/// column's UNIQUE (Task 18), so `apply_unique_indexes` leaves them out. Not
/// in the TypeScript. Binds `@P1` = table, `@P2` = schema.
pub const FILTERED_UNIQUE_SQL: &str = "SELECT i.name AS index_name FROM sys.indexes i \
     INNER JOIN sys.objects o ON i.object_id = o.object_id INNER JOIN sys.schemas s ON o.schema_id = s.schema_id \
     WHERE o.name = @P1 AND s.name = @P2 AND i.is_unique = 1 AND (i.has_filter = 1 \
     OR EXISTS (SELECT 1 FROM sys.index_columns ic WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.is_included_column = 1))";

/// The column SQL Server names a plan result set's only column.
pub const SHOWPLAN_COLUMN: &str = "Microsoft SQL Server 2005 XML Showplan";

/// The batches EXPLAIN runs, in order, on one held connection (bug fix 9):
/// `SET SHOWPLAN_XML ON` (estimated plan) or `SET STATISTICS XML ON` (the
/// query runs, and its plan carries actual counts), the query as it is, and
/// the matching `OFF`. The SET statements must be alone in their batch.
pub fn explain_batches(sql: &str, analyze: bool) -> [String; 3] {
    let option = if analyze {
        "STATISTICS XML"
    } else {
        "SHOWPLAN_XML"
    };
    [
        format!("SET {option} ON"),
        sql.to_string(),
        format!("SET {option} OFF"),
    ]
}

/// `DECLARE @P1 <type>, …;` for the parameters of a plain EXPLAIN. Under
/// `SHOWPLAN_XML` a `sp_executesql` call returns no plan, so the query runs
/// as a batch, where the parameters are declared (with the types tiberius
/// binds them as) but never given a value: nothing runs, only the plan is
/// compiled. With no values to sniff, the plan's estimates are the
/// generic ones for unknown values, not those for the given parameters.
/// Arrays don't bind (`QUERY_ERROR`, as in `query`).
pub fn declare_params(params: &[Value]) -> Result<String, DbError> {
    let decls = params
        .iter()
        .enumerate()
        .map(|(i, v)| {
            // A decimal as the RPC binds it: numeric(p,s) when it fits.
            if let Some(n) = match v {
                Value::Decimal(s) => crate::bind::numeric(s),
                _ => None,
            } {
                return Ok(format!(
                    "@P{} numeric({},{})",
                    i + 1,
                    n.precision(),
                    n.scale()
                ));
            }
            let ty = match v {
                Value::Null => "nvarchar(4000)",
                Value::Bool(_) => "bit",
                Value::Int(_) => "bigint",
                Value::Float(_) => "float(53)",
                Value::Decimal(s) | Value::Text(s) if s.encode_utf16().count() <= 4000 => {
                    "nvarchar(4000)"
                }
                Value::Decimal(_) | Value::Text(_) | Value::Json(_) => "nvarchar(max)",
                Value::Bytes(b) if b.len() <= 8000 => "varbinary(8000)",
                Value::Bytes(_) => "varbinary(max)",
                Value::Array(_) => {
                    return Err(DbError::query_error("array parameters are not supported"))
                }
            };
            Ok(format!("@P{} {ty}", i + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(if decls.is_empty() {
        String::new()
    } else {
        format!("DECLARE {};\n", decls.join(", "))
    })
}

// ── Schema ───────────────────────────────────────────────────────────────────

/// Rows of [`SCHEMAS_SQL`].
pub fn parse_schemas(result: &QueryResult) -> Vec<String> {
    rows(result).map(|r| r.text("schema_name")).collect()
}

/// Rows of [`SCHEMA_SQL`] (TS `parseSchemaResult`). Columns and indexes are
/// loaded per table, so they're empty here.
pub fn parse_schema(result: &QueryResult) -> Vec<SchemaTable> {
    rows(result)
        .map(|r| SchemaTable {
            name: r.text("table_name"),
            schema: r.text("schema_name"),
            kind: if r.str("object_type") == Some("VIEW") {
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

/// Rows of [`COLUMNS_SQL`] (TS `parseColumnsResult`, plus the collation of
/// fix 15).
pub fn parse_columns(result: &QueryResult) -> Vec<SchemaColumn> {
    rows(result)
        .map(|r| {
            let is_foreign_key = r.int("is_foreign_key") == Some(1);
            let foreign_key_ref = (r.truthy("is_foreign_key") && r.truthy("referenced_table"))
                .then(|| ForeignKeyRef {
                    referenced_schema: r
                        .non_empty("referenced_schema")
                        .unwrap_or("dbo")
                        .to_string(),
                    referenced_table: r.text("referenced_table"),
                    referenced_column: r.text("referenced_column"),
                });
            SchemaColumn {
                name: r.text("column_name"),
                ty: r.text("data_type"),
                cast_type: None,
                nullable: r.str("is_nullable") == Some("YES"),
                default_value: r.non_empty("column_default").map(str::to_string),
                is_primary_key: r.int("is_primary_key") == Some(1),
                is_foreign_key,
                foreign_key_ref,
                collation: r.non_empty("collation_name").map(str::to_string),
                is_unique: false,
                in_unique_constraint: false,
            }
        })
        .collect()
}

/// Rows of [`INDEXES_SQL`] (TS `parseIndexesResult`): one row per index
/// column, grouped by index name in first-seen order. Fix 5:
/// `sys.indexes.is_unique` is a BIT, which arrives as a bool; the TS
/// compared it with `=== 1`, so no index was unique.
pub fn parse_indexes(result: &QueryResult) -> Vec<SchemaIndex> {
    let mut out: Vec<SchemaIndex> = Vec::new();
    for r in rows(result) {
        let name = r.text("index_name");
        let column = r.text("column_name");
        match out.iter_mut().find(|i| i.name == name) {
            Some(index) => index.columns.push(column),
            None => out.push(SchemaIndex {
                name,
                columns: vec![column],
                unique: r.truthy("is_unique"),
                ty: r.text("index_type").to_lowercase(),
            }),
        }
    }
    out
}

// ── EXPLAIN ──────────────────────────────────────────────────────────────────

const NO_PLAN: &str = "No XML plan returned. Check permissions for SHOWPLAN.";

/// The plan among a response's result sets: the first set whose columns
/// include [`SHOWPLAN_COLUMN`] (the second for a SELECT under STATISTICS
/// XML, after the query's own rows; the only one for DML and for
/// SHOWPLAN_XML), or an empty result when there is none (`SELECT 1` under
/// STATISTICS XML).
pub fn plan_result(sets: &[ResultSet]) -> QueryResult {
    sets.iter()
        .find(|s| s.columns.iter().any(|c| c == SHOWPLAN_COLUMN))
        .map(|s| QueryResult {
            columns: s.columns.clone(),
            rows: s.rows.clone(),
        })
        .unwrap_or(QueryResult {
            columns: vec![],
            rows: vec![],
        })
}

/// A plan result (TS `parseExplainResult`): the first text cell holding
/// `<ShowPlanXML`, read from its first `StmtSimple > QueryPlan > RelOp` (or
/// its first `RelOp`). Without a plan the root is a `Query Plan` node that
/// says so; XML that doesn't parse is a `Query Plan` node with the parser's
/// error as its filter. SQL Server reports no planning or execution time
/// here.
pub fn parse_explain(result: &QueryResult, analyze: bool) -> ExplainResult {
    let mut ids = Ids::new();
    let plan = match extract_xml(result) {
        None => ExplainPlanNode {
            id: ids.next(),
            filter: Some(NO_PLAN.to_string()),
            ..node("Query Plan")
        },
        Some(xml) => match roxmltree::Document::parse(xml) {
            Err(e) => ExplainPlanNode {
                id: ids.next(),
                filter: Some(e.to_string()),
                ..node("Query Plan")
            },
            Ok(doc) => match root_rel_op(&doc) {
                Some(rel_op) => convert_rel_op(rel_op, &mut ids),
                None => ExplainPlanNode {
                    id: ids.next(),
                    ..node("Query Plan")
                },
            },
        },
    };
    ExplainResult {
        plan,
        planning_time: 0.0,
        execution_time: None,
        is_analyze: analyze,
    }
}

/// TS `extractMssqlXml`: rows in order, cells in column order.
fn extract_xml(result: &QueryResult) -> Option<&str> {
    result.rows.iter().flatten().find_map(|cell| match cell {
        Value::Text(s) if s.contains("<ShowPlanXML") => Some(s.as_str()),
        _ => None,
    })
}

type Node<'a, 'input> = roxmltree::Node<'a, 'input>;

/// An element's local name (plans use a default namespace, which the CSS
/// selectors of the TS ignored).
fn is(n: &Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name
}

/// Element children named `name`.
fn children_named<'a, 'input: 'a>(
    n: Node<'a, 'input>,
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'input>> + 'a {
    n.children().filter(move |c| is(c, name))
}

/// `:scope > * > name`: grandchildren named `name`, in document order.
fn grandchildren_named<'a, 'input: 'a>(
    n: Node<'a, 'input>,
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'input>> + 'a {
    n.children()
        .filter(Node::is_element)
        .flat_map(move |c| children_named(c, name))
}

/// `StmtSimple > QueryPlan > RelOp`, else the first `RelOp`.
fn root_rel_op<'a, 'input>(doc: &'a roxmltree::Document<'input>) -> Option<Node<'a, 'input>> {
    let under_plan = |n: &Node| {
        n.parent_element().is_some_and(|p| {
            is(&p, "QueryPlan") && p.parent_element().is_some_and(|s| is(&s, "StmtSimple"))
        })
    };
    let mut rel_ops = doc.descendants().filter(|n| is(n, "RelOp"));
    doc.descendants()
        .find(|n| is(n, "RelOp") && under_plan(n))
        .or_else(|| rel_ops.next())
}

/// A present, non-empty attribute (JS truthiness of `getAttribute`).
fn attr<'a>(n: &Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attribute(name).filter(|v| !v.is_empty())
}

/// `Number(text)`, `None` where JSON would have had `null` (NaN, ±Infinity).
fn number(text: &str) -> Option<f64> {
    Some(js_string_to_number(text)).filter(|f| f.is_finite())
}

/// `[name]` without its brackets, `]]` un-escaped (TS `stripBrackets`, plus
/// fix 11: the TS left `]]` as it was, so `fx]odd name` showed as
/// `fx]]odd name`).
fn strip_brackets(id: &str) -> String {
    let id = id.strip_prefix('[').unwrap_or(id);
    let id = id.strip_suffix(']').unwrap_or(id);
    id.replace("]]", "]")
}

/// TS `convertMssqlRelOp`. Ids are assigned before the children's, in
/// document order.
fn convert_rel_op(rel_op: Node, ids: &mut Ids) -> ExplainPlanNode {
    let mut n = ExplainPlanNode {
        id: ids.next(),
        ..node(
            attr(&rel_op, "LogicalOp")
                .or_else(|| attr(&rel_op, "PhysicalOp"))
                .unwrap_or("RelOp"),
        )
    };
    n.total_cost = attr(&rel_op, "EstimatedTotalSubtreeCost").and_then(number);
    n.plan_rows = attr(&rel_op, "EstimateRows").and_then(number);
    // `EstimateRowSize ?? AvgRowSize`: an empty EstimateRowSize still wins.
    n.plan_width = rel_op
        .attribute("EstimateRowSize")
        .or_else(|| rel_op.attribute("AvgRowSize"))
        .filter(|v| !v.is_empty())
        .and_then(number)
        .map(|f| f as i64);

    if let Some(object) = grandchildren_named(rel_op, "Object").next() {
        n.relation_name = attr(&object, "Table").map(strip_brackets);
        n.index_name = attr(&object, "Index").map(strip_brackets);
    }
    let predicate = grandchildren_named(rel_op, "Predicate")
        .flat_map(|p| children_named(p, "ScalarOperator"))
        .next();
    if let Some(scalar) = predicate {
        n.filter = scalar.attribute("ScalarString").map(str::to_string);
    }

    // STATISTICS XML: summed over the threads, the time is the slowest's.
    let threads: Vec<Node> = children_named(rel_op, "RunTimeInformation")
        .flat_map(|r| children_named(r, "RunTimeCountersPerThread"))
        .collect();
    if !threads.is_empty() {
        let count = |t: &Node, name: &str| t.attribute(name).map_or(0.0, js_string_to_number);
        let rows: f64 = threads.iter().map(|t| count(t, "ActualRows")).sum();
        let ms = threads
            .iter()
            .map(|t| count(t, "ActualElapsedms"))
            .fold(0.0, f64::max);
        let execs: f64 = threads.iter().map(|t| count(t, "ActualExecutions")).sum();
        n.actual_rows = Some(rows);
        n.actual_total_time = (ms > 0.0).then_some(ms);
        n.actual_loops = (execs > 0.0).then_some(execs as i64);
    }

    n.children = grandchildren_named(rel_op, "RelOp")
        .map(|child| convert_rel_op(child, ids))
        .collect();
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declares_the_bound_types() {
        let params = [
            Value::Int(1),
            Value::from("x"),
            Value::Null,
            Value::Bool(true),
            Value::Float(1.5),
            Value::Bytes(vec![0; 9000]),
        ];
        assert_eq!(
            declare_params(&params).unwrap(),
            "DECLARE @P1 bigint, @P2 nvarchar(4000), @P3 nvarchar(4000), @P4 bit, @P5 float(53), @P6 varbinary(max);\n"
        );
        assert_eq!(declare_params(&[]).unwrap(), "");
        // Decimals as the RPC binds them: numeric(p,s), else text.
        assert_eq!(
            declare_params(&[
                Value::Decimal("-12.50".into()),
                Value::Decimal("0.5".into()),
                Value::Decimal("1e5".into()),
            ])
            .unwrap(),
            "DECLARE @P1 numeric(4,2), @P2 numeric(2,1), @P3 nvarchar(4000);\n"
        );
        let err = declare_params(&[Value::Array(vec![])]).unwrap_err();
        assert_eq!(err.code, "QUERY_ERROR");
    }

    #[test]
    fn brackets_are_stripped_once_and_unescaped() {
        assert_eq!(strip_brackets("[t]]x]"), "t]x");
        assert_eq!(strip_brackets("plain"), "plain");
        assert_eq!(strip_brackets("[a]]]]]"), "a]]");
    }

    #[test]
    fn the_first_plan_set_is_read() {
        let set = |cols: &[&str], cell: &str| ResultSet {
            columns: cols.iter().map(|c| c.to_string()).collect(),
            rows: vec![vec![Value::from(cell)]],
        };
        let sets = [
            set(&["id"], "<ShowPlanXML/>"),
            set(&[SHOWPLAN_COLUMN], "first"),
            set(&[SHOWPLAN_COLUMN], "second"),
        ];
        assert_eq!(plan_result(&sets).rows, vec![vec![Value::from("first")]]);
        assert!(plan_result(&sets[..1]).rows.is_empty());
    }
}
