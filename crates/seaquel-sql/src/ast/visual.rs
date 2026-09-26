//! The Visual tab's AST (Task 6). Port of `parseQueryForVisualization` and
//! `getParseError` from `src/lib/db/sql-ast-parser.ts` (deleted in phase 2b),
//! with fixes 1–8 and 15. The recorder's `visual-fixed.ts` is the spec for 1–4,
//! 6 and 15 (`docs/plans/artifacts/2026-09-27-sql-recorder-visual-fixed.ts.txt`).
//!
//! Everything else prints as the TS does (decision 2), including its odd
//! forms: `x BETWEEN (1, 5)`, `x IN ((subquery))`, `EXISTS((subquery))`, `!=`
//! and `<>` as written, no parentheses around nested expressions, and
//! `schema: null`/`alias: null` on INSERT and UPDATE sources.

use serde::Serialize;
use sqlparser::ast::{
    BinaryOperator, CastKind, DataType, Distinct, DuplicateTreatment, Expr, FromTable, FunctionArg,
    FunctionArgExpr, FunctionArguments, GroupByExpr, LimitClause, OrderByKind, OrderBySort, Query,
    SelectItem, SelectItemQualifiedWildcardKind, Spanned, Statement, TableFactor, TableObject,
    TableWithJoins, TopQuantity, UnaryOperator, Value,
};
use sqlparser::tokenizer::Location;

use super::tutorial::{Connector, SortDirection};
use super::util::{
    first_select, flat_from, parse, parse_noting_not_eq, table_and_schema, unnest, FromJoin,
};
use crate::SqlEngine;

/// What kind of statement the Visual tab shows.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum VisualQueryType {
    Select,
    Insert,
    Update,
    Delete,
    Other,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum QuerySourceType {
    Table,
    Subquery,
}

/// A join's type in the Visual tab (the builder's `JoinType` has no CROSS).
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "UPPERCASE")]
pub enum QueryJoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

/// Complete parsed query structure for visualization.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ParsedQueryVisual {
    /// Type of query (SELECT, INSERT, UPDATE, DELETE)
    #[serde(rename = "type")]
    pub kind: VisualQueryType,
    /// Tables and subqueries in FROM clause
    pub sources: Vec<QuerySource>,
    /// JOIN clauses
    pub joins: Vec<QueryJoin>,
    /// WHERE conditions
    pub filters: Vec<QueryFilter>,
    /// GROUP BY columns
    pub group_by: Option<Vec<String>>,
    /// HAVING clause filter
    pub having: Option<QueryFilter>,
    /// SELECT columns/expressions
    pub projections: Vec<QueryProjection>,
    /// ORDER BY clauses
    pub order_by: Vec<QueryOrderBy>,
    /// LIMIT/OFFSET
    pub limit: Option<Limit>,
    /// DISTINCT flag
    pub distinct: bool,
}

/// LIMIT/OFFSET, or SQL Server's TOP (fix 7).
#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Limit {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
    pub offset: Option<i64>,
}

/// A table or subquery source in FROM clause.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct QuerySource {
    /// Type of source
    #[serde(rename = "type")]
    pub kind: QuerySourceType,
    /// Schema name (if specified). `Some(None)` is a JSON `null`: INSERT and
    /// UPDATE sources always carry `schema` and `alias`, as the TS wrote them.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "string"))]
    pub schema: Option<Option<String>>,
    /// Table name or subquery alias
    pub name: String,
    /// Alias for the source (`Some(None)`: see `schema`)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "string"))]
    pub alias: Option<Option<String>>,
    /// For subqueries, the nested parsed query
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub subquery: Option<Box<ParsedQueryVisual>>,
}

/// A JOIN clause.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct QueryJoin {
    /// Type of join
    #[serde(rename = "type")]
    pub kind: QueryJoinType,
    /// The joined table/source
    pub source: QuerySource,
    /// ON condition as readable string
    pub condition: String,
}

/// A filter condition (WHERE or HAVING).
#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct QueryFilter {
    /// The full condition as readable string
    pub expression: String,
    /// Operator used (AND, OR) at top level. `string`, as in the interface
    /// it replaces (`types/visualize.ts`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "string"))]
    pub operator: Option<Connector>,
    /// For compound conditions, nested filters
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub children: Option<Vec<QueryFilter>>,
}

/// A SELECT column/expression.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct QueryProjection {
    /// The expression or column name
    pub expression: String,
    /// Alias if specified
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub alias: Option<String>,
    /// Whether this is an aggregate function
    pub is_aggregate: bool,
    /// Aggregate function name if applicable
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub aggregate_function: Option<String>,
}

/// An ORDER BY clause.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct QueryOrderBy {
    /// Column or expression
    pub expression: String,
    /// Sort direction
    pub direction: SortDirection,
}

const AGGREGATE_FUNCTIONS: [&str; 21] = [
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "array_agg",
    "string_agg",
    "group_concat",
    "json_agg",
    "jsonb_agg",
    "bool_and",
    "bool_or",
    "every",
    "stddev",
    "variance",
    "first",
    "last",
    "median",
    "mode",
    "percentile_cont",
    "percentile_disc",
];

fn is_aggregate_name(lower: &str) -> bool {
    AGGREGATE_FUNCTIONS.contains(&lower)
}

/// The first statement's visual AST, `Ok(None)` for input with no statement,
/// or the parser's error message.
pub fn parse_visual(sql: &str, engine: SqlEngine) -> Result<Option<ParsedQueryVisual>, String> {
    let parsed = parse_noting_not_eq(sql, engine).map_err(|e| error_message(&e))?;
    let stmts = parsed.statements;
    let Some(stmt) = stmts.first() else {
        return Ok(None);
    };
    let p = Printer {
        not_eq: parsed.not_eq,
    };
    Ok(Some(p.statement(stmt)))
}

/// The parser's error message for `sql`, or `None` if it parses.
pub fn parse_error(sql: &str, engine: SqlEngine) -> Option<String> {
    parse(sql, engine).err().map(|e| error_message(&e))
}

/// sqlparser's message without its `sql parser error: ` prefix, which the
/// Visual tab's "Parse warning: …" toast would only repeat. It keeps sqlparser's
/// `Line: L, Column: C`, which `seaquel-wasm` turns into a UTF-16 column.
fn error_message(e: &sqlparser::parser::ParserError) -> String {
    let s = e.to_string();
    for prefix in ["sql parser error: ", "sql tokenizer error: "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    s
}

fn empty(kind: VisualQueryType) -> ParsedQueryVisual {
    ParsedQueryVisual {
        kind,
        sources: vec![],
        joins: vec![],
        filters: vec![],
        group_by: None,
        having: None,
        projections: vec![],
        order_by: vec![],
        limit: None,
        distinct: false,
    }
}

/// One link of a left-deep chain, applied to the text of what it wraps.
enum Step<'e> {
    Binary(&'e BinaryOperator, &'e Expr),
    Suffix(&'static str),
    InList(bool, &'e [Expr]),
    Between(bool, &'e Expr, &'e Expr),
    Like(&'static str, bool, &'e Expr),
    Cast(&'e CastKind, String),
    AtTimeZone(&'e Expr),
}

/// Prints a statement, knowing how each `<>`/`!=` was written.
struct Printer {
    /// From `util::Parsed::not_eq`: each `!=`/`<>` token's start, and whether it
    /// was `!=`.
    not_eq: Vec<(Location, bool)>,
}

impl Printer {
    /// `!=` or `<>`, as written before `right` (the TS kept the text): the
    /// last `!=`/`<>` token that starts before the right operand, which is
    /// this node's operator (only whitespace, comments and `(` can sit
    /// between).
    fn not_eq(&self, right: &Expr) -> &'static str {
        let r = right.span().start;
        let before = if r.line == 0 {
            // No span: the one form used, if there's only one.
            match self.not_eq.iter().all(|(_, bang)| *bang) {
                true if !self.not_eq.is_empty() => return "!=",
                _ => return "<>",
            }
        } else {
            self.not_eq.partition_point(|(loc, _)| *loc < r)
        };
        match before.checked_sub(1).and_then(|i| self.not_eq.get(i)) {
            Some((_, true)) => "!=",
            _ => "<>",
        }
    }

    fn statement(&self, s: &Statement) -> ParsedQueryVisual {
        match s {
            Statement::Query(q) => self.query(q),
            Statement::Insert(ins) => {
                let mut v = empty(VisualQueryType::Insert);
                if let TableObject::TableName(n) = &ins.table {
                    let (name, schema) = table_and_schema(n);
                    v.sources.push(QuerySource {
                        kind: QuerySourceType::Table,
                        schema: Some(schema),
                        name: non_empty_or_unknown(name),
                        alias: Some(ins.table_alias.as_ref().map(|a| a.alias.value.clone())),
                        subquery: None,
                    });
                }
                v
            }
            Statement::Update(u) => {
                let mut v = empty(VisualQueryType::Update);
                // node-sql-parser lists the target and the tables it joins,
                // not the FROM tables, and none of their join conditions.
                v.sources = flat_from(std::slice::from_ref(&u.table))
                    .iter()
                    .map(|i| {
                        let (name, schema, alias) = match i.factor {
                            TableFactor::Table { name, alias, .. } => {
                                let (n, s) = table_and_schema(name);
                                (n, s, alias.as_ref().map(|a| a.name.value.clone()))
                            }
                            _ => (String::new(), None, None),
                        };
                        QuerySource {
                            kind: QuerySourceType::Table,
                            schema: Some(schema),
                            name: non_empty_or_unknown(name),
                            alias: Some(alias),
                            subquery: None,
                        }
                    })
                    .collect();
                if let Some(w) = &u.selection {
                    v.filters = vec![self.filter(w)];
                }
                v
            }
            Statement::Delete(d) => {
                let mut v = empty(VisualQueryType::Delete);
                let from = match &d.from {
                    FromTable::WithFromKeyword(t) | FromTable::WithoutKeyword(t) => t,
                };
                v.sources = self.sources(from);
                if let Some(w) = &d.selection {
                    v.filters = vec![self.filter(w)];
                }
                v
            }
            _ => empty(VisualQueryType::Other),
        }
    }

    fn query(&self, q: &Query) -> ParsedQueryVisual {
        let Some(sel) = first_select(q) else {
            return empty(VisualQueryType::Other);
        };
        let mut v = empty(VisualQueryType::Select);
        // FIX 4: DISTINCT is detected.
        v.distinct = matches!(
            sel.distinct,
            Some(Distinct::Distinct) | Some(Distinct::On(_))
        );
        v.sources = self.sources(&sel.from);
        v.joins = self.joins(&sel.from);
        if let Some(w) = &sel.selection {
            v.filters = vec![self.filter(w)];
        }
        match &sel.group_by {
            GroupByExpr::Expressions(e, _) if !e.is_empty() => {
                // FIX 15: a `::` cast is kept.
                v.group_by = Some(e.iter().map(|x| self.expr(x)).collect());
            }
            GroupByExpr::All(_) => v.group_by = Some(vec!["ALL".into()]),
            _ => {}
        }
        if let Some(h) = &sel.having {
            v.having = Some(self.filter(h));
        }
        v.projections = sel.projection.iter().map(|p| self.projection(p)).collect();
        if let Some(ob) = &q.order_by {
            if let OrderByKind::Expressions(list) = &ob.kind {
                v.order_by = list
                    .iter()
                    .map(|o| QueryOrderBy {
                        expression: self.expr(&o.expr),
                        direction: match o.options.sort {
                            Some(OrderBySort::Desc) => SortDirection::Desc,
                            _ => SortDirection::Asc,
                        },
                    })
                    .collect();
            }
        }
        // FIX 2: no LIMIT node without a LIMIT. FIX 5: MySQL's `LIMIT 5, 10`
        // is offset 5, count 10.
        v.limit = match &q.limit_clause {
            Some(LimitClause::LimitOffset {
                limit: Some(l),
                offset,
                ..
            }) => int(l).map(|count| Limit {
                count,
                offset: offset.as_ref().and_then(|o| int(&o.value)),
            }),
            Some(LimitClause::OffsetCommaLimit { offset, limit }) => {
                int(limit).map(|count| Limit {
                    count,
                    offset: int(offset),
                })
            }
            _ => None,
        };
        // FIX 7: SQL Server's TOP n is the LIMIT node (not TOP n PERCENT,
        // which isn't a row count).
        if v.limit.is_none() {
            if let Some(top) = sel.top.as_ref().filter(|t| !t.percent) {
                let count = match &top.quantity {
                    Some(TopQuantity::Constant(n)) => i64::try_from(*n).ok(),
                    Some(TopQuantity::Expr(e)) => int(unnest(e)),
                    None => None,
                };
                v.limit = count.map(|count| Limit {
                    count,
                    offset: None,
                });
            }
        }
        v
    }

    fn source(&self, f: &TableFactor) -> Option<QuerySource> {
        match f {
            TableFactor::Table { name, alias, .. } => {
                let (name, schema) = table_and_schema(name);
                if name.is_empty() {
                    return None;
                }
                Some(QuerySource {
                    kind: QuerySourceType::Table,
                    schema: schema.filter(|s| !s.is_empty()).map(Some),
                    name,
                    alias: alias
                        .as_ref()
                        .map(|a| a.name.value.clone())
                        .filter(|a| !a.is_empty())
                        .map(Some),
                    subquery: None,
                })
            }
            TableFactor::Derived {
                subquery, alias, ..
            } => {
                let a = alias
                    .as_ref()
                    .map(|a| a.name.value.clone())
                    .filter(|a| !a.is_empty());
                Some(QuerySource {
                    kind: QuerySourceType::Subquery,
                    name: a.clone().unwrap_or_else(|| "subquery".into()),
                    schema: None,
                    alias: a.map(Some),
                    subquery: Some(Box::new(self.query(subquery))),
                })
            }
            _ => None,
        }
    }

    fn sources(&self, from: &[TableWithJoins]) -> Vec<QuerySource> {
        flat_from(from)
            .iter()
            .filter_map(|i| self.source(i.factor))
            .collect()
    }

    fn joins(&self, from: &[TableWithJoins]) -> Vec<QueryJoin> {
        flat_from(from)
            .iter()
            .filter_map(|i| {
                let kind = match i.join? {
                    FromJoin::Left => QueryJoinType::Left,
                    FromJoin::Right => QueryJoinType::Right,
                    FromJoin::Full => QueryJoinType::Full,
                    FromJoin::Cross => QueryJoinType::Cross,
                    FromJoin::Inner | FromJoin::Other => QueryJoinType::Inner,
                };
                Some(QueryJoin {
                    kind,
                    source: self.source(i.factor).unwrap_or(QuerySource {
                        kind: QuerySourceType::Table,
                        schema: None,
                        name: "unknown".into(),
                        alias: None,
                        subquery: None,
                    }),
                    condition: i.on.map(|e| self.expr(e)).unwrap_or_default(),
                })
            })
            .collect()
    }

    fn filter(&self, e: &Expr) -> QueryFilter {
        let inner = unnest(e);
        if let Expr::BinaryOp { left, op, right } = inner {
            let o = match op {
                BinaryOperator::And => Some(Connector::And),
                BinaryOperator::Or => Some(Connector::Or),
                _ => None,
            };
            if let Some(o) = o {
                return QueryFilter {
                    expression: self.expr(inner),
                    operator: Some(o),
                    children: Some(vec![self.filter(left), self.filter(right)]),
                };
            }
        }
        QueryFilter {
            expression: self.expr(e),
            operator: None,
            children: None,
        }
    }

    fn projection(&self, item: &SelectItem) -> QueryProjection {
        let plain = |expression: String| QueryProjection {
            expression,
            alias: None,
            is_aggregate: false,
            aggregate_function: None,
        };
        let (expr, alias) = match item {
            SelectItem::UnnamedExpr(e) => (e, None),
            SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias.value.clone())),
            // `expr AS (a, b)`: not something node-sql-parser read.
            SelectItem::ExprWithAliases { expr, .. } => (expr, None),
            SelectItem::Wildcard(_) => return plain("*".into()),
            // FIX 1: the table's name, not `[object Object]`.
            SelectItem::QualifiedWildcard(SelectItemQualifiedWildcardKind::ObjectName(n), _) => {
                return plain(format!("{}.*", table_and_schema(n).0))
            }
            SelectItem::QualifiedWildcard(SelectItemQualifiedWildcardKind::Expr(e), _) => {
                return plain(format!("{}.*", self.expr(e)))
            }
        };
        let agg = detect_aggregate(expr);
        QueryProjection {
            expression: self.expr(expr),
            alias: alias.filter(|a| !a.is_empty()),
            is_aggregate: agg.is_some(),
            aggregate_function: agg,
        }
    }

    /// `expressionToString`, with fixes 1, 3, 6 and 15.
    ///
    /// Operator chains are left-deep trees as long as `parse` allows
    /// (`a AND b AND …`, `x::int::int …`, `a IS NULL IS NULL …`), so the
    /// left spine (binary operators, casts, `AT TIME ZONE`, the
    /// IS/IN/BETWEEN/LIKE tests and parentheses) is walked with a loop: its length costs heap, not stack.
    /// Everything else recurses, bounded by sqlparser's recursion limit.
    fn expr(&self, e: &Expr) -> String {
        let mut steps: Vec<Step<'_>> = Vec::new();
        let mut cur = e;
        loop {
            cur = match cur {
                // The TS never sees parentheses (node-sql-parser only flags them).
                Expr::Nested(x) => x,
                Expr::BinaryOp { left, op, right } => {
                    steps.push(Step::Binary(op, right));
                    left
                }
                Expr::IsNull(x) => {
                    steps.push(Step::Suffix(" IS NULL"));
                    x
                }
                Expr::IsNotNull(x) => {
                    steps.push(Step::Suffix(" IS NOT NULL"));
                    x
                }
                Expr::IsTrue(x) => {
                    steps.push(Step::Suffix(" IS TRUE"));
                    x
                }
                Expr::IsNotTrue(x) => {
                    steps.push(Step::Suffix(" IS NOT TRUE"));
                    x
                }
                Expr::IsFalse(x) => {
                    steps.push(Step::Suffix(" IS FALSE"));
                    x
                }
                Expr::IsNotFalse(x) => {
                    steps.push(Step::Suffix(" IS NOT FALSE"));
                    x
                }
                Expr::InList {
                    expr,
                    list,
                    negated,
                } => {
                    steps.push(Step::InList(*negated, list));
                    expr
                }
                // The TS form: an expression list holding the subquery.
                Expr::InSubquery { expr, negated, .. } => {
                    steps.push(Step::Suffix(if *negated {
                        " NOT IN ((subquery))"
                    } else {
                        " IN ((subquery))"
                    }));
                    expr
                }
                Expr::Between {
                    expr,
                    negated,
                    low,
                    high,
                } => {
                    steps.push(Step::Between(*negated, low, high));
                    expr
                }
                Expr::Like {
                    negated,
                    expr,
                    pattern,
                    ..
                } => {
                    steps.push(Step::Like("LIKE", *negated, pattern));
                    expr
                }
                Expr::ILike {
                    negated,
                    expr,
                    pattern,
                    ..
                } => {
                    steps.push(Step::Like("ILIKE", *negated, pattern));
                    expr
                }
                // FIX 15: the cast's type, as written and upper-cased;
                // `x::type` stays `::`.
                Expr::Cast {
                    expr,
                    data_type,
                    kind,
                    ..
                } => {
                    steps.push(Step::Cast(kind, cast_type(data_type)));
                    expr
                }
                Expr::AtTimeZone {
                    timestamp,
                    time_zone,
                } => {
                    steps.push(Step::AtTimeZone(time_zone));
                    timestamp
                }
                _ => break,
            };
        }
        let mut s = self.expr_leaf(cur);
        while let Some(step) = steps.pop() {
            match step {
                Step::Binary(op, right) => {
                    let op_text = match op {
                        BinaryOperator::NotEq => self.not_eq(right).to_string(),
                        other => other.to_string(),
                    };
                    s = format!("{s} {op_text} {}", self.expr(right));
                }
                Step::Suffix(t) => s.push_str(t),
                Step::InList(negated, list) => {
                    let items = list
                        .iter()
                        .map(|x| self.expr(x))
                        .collect::<Vec<_>>()
                        .join(", ");
                    s = format!("{s} {}IN ({items})", not(negated));
                }
                // The TS form: BETWEEN takes an expression list.
                Step::Between(negated, low, high) => {
                    s = format!(
                        "{s} {}BETWEEN ({}, {})",
                        not(negated),
                        self.expr(low),
                        self.expr(high)
                    );
                }
                Step::Like(word, negated, pattern) => {
                    s = format!("{s} {}{word} {}", not(negated), self.expr(pattern));
                }
                Step::AtTimeZone(tz) => {
                    s = format!("{s} AT TIME ZONE {}", self.expr(tz));
                }
                Step::Cast(kind, t) => {
                    s = match kind {
                        CastKind::DoubleColon => format!("{s}::{t}"),
                        CastKind::Cast => format!("CAST({s} AS {t})"),
                        CastKind::TryCast => format!("TRY_CAST({s} AS {t})"),
                        CastKind::SafeCast => format!("SAFE_CAST({s} AS {t})"),
                    };
                }
            }
        }
        s
    }

    /// An expression that isn't a step of a chain (see [`Printer::expr`]).
    fn expr_leaf(&self, e: &Expr) -> String {
        match e {
            // FIX 1: names print as their text.
            Expr::Identifier(i) => i.value.clone(),
            Expr::CompoundIdentifier(p) => {
                let n = p.len();
                p[n.saturating_sub(2)..]
                    .iter()
                    .map(|i| i.value.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
            }
            Expr::Value(v) => match &v.value {
                Value::SingleQuotedString(s)
                | Value::EscapedStringLiteral(s)
                | Value::UnicodeStringLiteral(s) => format!("'{s}'"),
                // node-sql-parser's `double_quote_string` and `natural_string`
                // aren't matched by the TS, which printed the bare value.
                Value::DoubleQuotedString(s) | Value::NationalStringLiteral(s) => s.clone(),
                Value::Number(n, _) => n.clone(),
                Value::Null => "NULL".into(),
                Value::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.into(),
                // FIX 6: placeholders and dollar-quoted strings print as
                // written (`$1`, `:name`, `@v`, `$$…$$`, `$q$…$q$`).
                Value::Placeholder(p) => p.clone(),
                other => other.to_string(),
            },
            Expr::Wildcard(_) => "*".into(),
            Expr::QualifiedWildcard(n, _) => format!("{}.*", table_and_schema(n).0),
            Expr::UnaryOp { op, expr } => match (op, expr.as_ref()) {
                // node-sql-parser reads `-5` as the number -5.
                (UnaryOperator::Minus, Expr::Value(v)) if matches!(v.value, Value::Number(..)) => {
                    format!("-{}", self.expr(expr))
                }
                _ => format!("{op} {}", self.expr(expr)),
            },
            Expr::Function(f) => {
                // FIX 3: every function prints (window functions without their
                // OVER, as the TS printed aggregates). node-sql-parser
                // upper-cases aggregate names.
                let mut name = f
                    .name
                    .0
                    .iter()
                    .filter_map(|p| p.as_ident().map(|i| i.value.as_str()))
                    .collect::<Vec<_>>()
                    .join(".");
                if is_aggregate_name(&name.to_lowercase()) {
                    name = name.to_uppercase();
                }
                let args = match &f.args {
                    FunctionArguments::None => String::new(),
                    FunctionArguments::Subquery(_) => "(subquery)".into(),
                    FunctionArguments::List(l) => {
                        let a = l
                            .args
                            .iter()
                            .map(|a| match a {
                                FunctionArg::Unnamed(FunctionArgExpr::Expr(x)) => self.expr(x),
                                FunctionArg::Unnamed(FunctionArgExpr::Wildcard) => "*".into(),
                                other => other.to_string(),
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        match l.duplicate_treatment {
                            Some(DuplicateTreatment::Distinct) => format!("DISTINCT {a}"),
                            _ => a,
                        }
                    }
                };
                format!("{name}({args})")
            }
            Expr::Case {
                operand,
                conditions,
                else_result,
                ..
            } => {
                let mut s = "CASE".to_string();
                if let Some(o) = operand {
                    s.push(' ');
                    s.push_str(&self.expr(o));
                }
                for c in conditions {
                    s.push_str(" WHEN ");
                    s.push_str(&self.expr(&c.condition));
                    s.push_str(" THEN ");
                    s.push_str(&self.expr(&c.result));
                }
                if let Some(e) = else_result {
                    s.push_str(" ELSE ");
                    s.push_str(&self.expr(e));
                }
                s + " END"
            }
            Expr::Subquery(_) => "(subquery)".into(),
            // The TS form: EXISTS is a function of an expression list.
            Expr::Exists { negated, .. } => format!("{}EXISTS((subquery))", not(*negated)),
            other => other.to_string(),
        }
    }
}

fn not(negated: bool) -> &'static str {
    if negated {
        "NOT "
    } else {
        ""
    }
}

fn non_empty_or_unknown(name: String) -> String {
    if name.is_empty() {
        "unknown".into()
    } else {
        name
    }
}

/// An integer literal.
fn int(e: &Expr) -> Option<i64> {
    match e {
        Expr::Value(v) => match &v.value {
            Value::Number(n, _) => n.parse::<i64>().ok(),
            _ => None,
        },
        _ => None,
    }
}

/// FIX 15: a cast's type as node-sql-parser gives it, upper-cased with its
/// length and scale: `INTEGER`, `VARCHAR(10)`, `NUMERIC(10,2)`, `TEXT[]`. A
/// user-defined type keeps its name as written.
fn cast_type(t: &DataType) -> String {
    match t {
        DataType::Custom(..) => t.to_string(),
        _ => t.to_string().to_uppercase(),
    }
}

/// `detectAggregate`: an aggregate call, or one inside the operands of an
/// operator. As in the TS, a call inside another function's arguments, a
/// CASE, a cast or a unary operator isn't looked at.
fn detect_aggregate(e: &Expr) -> Option<String> {
    // Iterative, like `Printer::expr`: operands to look at, leftmost first.
    let mut todo: Vec<&Expr> = vec![e];
    while let Some(e) = todo.pop() {
        match e {
            Expr::Function(f) => {
                let name = f
                    .name
                    .0
                    .iter()
                    .filter_map(|p| p.as_ident().map(|i| i.value.as_str()))
                    .collect::<Vec<_>>()
                    .join(".")
                    .to_lowercase();
                if is_aggregate_name(&name) {
                    return Some(name.to_uppercase());
                }
            }
            // node-sql-parser's `binary_expr`: the TS looks at `left`, then
            // `right`.
            Expr::BinaryOp { left, right, .. } => {
                todo.push(right);
                todo.push(left);
            }
            Expr::Like { expr, pattern, .. } | Expr::ILike { expr, pattern, .. } => {
                todo.push(pattern);
                todo.push(expr);
            }
            Expr::InList { expr, .. }
            | Expr::InSubquery { expr, .. }
            | Expr::Between { expr, .. }
            | Expr::IsNull(expr)
            | Expr::IsNotNull(expr)
            | Expr::IsTrue(expr)
            | Expr::IsNotTrue(expr)
            | Expr::IsFalse(expr)
            | Expr::IsNotFalse(expr) => todo.push(expr),
            Expr::Nested(x) => todo.push(x),
            _ => {}
        }
    }
    None
}
