//! The query builder and tutorial `ParsedQuery` (Task 6). Port of `parseSql`
//! from `src/lib/tutorial/sql-parser.ts` (deleted in phase 2b), quirks
//! included (decision 2): unqualified columns fall back to the first table,
//! IN lists, IS NULL and BETWEEN filters are dropped, each filter's connector
//! is the one before it, a decimal keeps its text, and a window `SUM()` counts
//! as an aggregate.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use sqlparser::ast::{
    BinaryOperator, Expr, GroupByExpr, LimitClause, OrderByKind, OrderBySort, Query, Select,
    SelectItem, SelectItemQualifiedWildcardKind, Statement, TableFactor, Value,
};

use super::util::{
    aggregate, column_ref, first_select, flat_from, literal, parse, table_and_schema, unnest,
    AggArg, FromJoin,
};
use crate::js_ws::is_js_space;
use crate::SqlEngine;

/// Tutorial schema: table → column names, from `src/lib/tutorial/schema.ts`
/// through the wrapper. The TS expands `*` and `t.*` from it, and looks up
/// which table an unqualified column belongs to.
pub type TutorialSchema = HashMap<String, Vec<String>>;

/// `JoinType` in `$lib/types` (the builder's join types).
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "UPPERCASE")]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
}

/// `FilterOperator` in `$lib/types`. The parser only produces the comparison,
/// `LIKE`, `IN` and `BETWEEN` ones; the rest are the builder's own.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum FilterOperator {
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = "!=")]
    NotEq,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = ">=")]
    GtEq,
    #[serde(rename = "<=")]
    LtEq,
    #[serde(rename = "LIKE")]
    Like,
    #[serde(rename = "NOT LIKE")]
    NotLike,
    #[serde(rename = "IS NULL")]
    IsNull,
    #[serde(rename = "IS NOT NULL")]
    IsNotNull,
    #[serde(rename = "IS TRUE")]
    IsTrue,
    #[serde(rename = "IS FALSE")]
    IsFalse,
    #[serde(rename = "IS NOT TRUE")]
    IsNotTrue,
    #[serde(rename = "IS NOT FALSE")]
    IsNotFalse,
    #[serde(rename = "IN")]
    In,
    #[serde(rename = "NOT IN")]
    NotIn,
    #[serde(rename = "BETWEEN")]
    Between,
}

/// `HavingOperator` in `$lib/types`.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum HavingOperator {
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = "!=")]
    NotEq,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = ">=")]
    GtEq,
    #[serde(rename = "<=")]
    LtEq,
}

/// `AggregateFunction` in `$lib/types`.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "UPPERCASE")]
pub enum AggregateFunction {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl AggregateFunction {
    /// From an upper-cased function name.
    fn from_upper(name: &str) -> Option<Self> {
        Some(match name {
            "COUNT" => Self::Count,
            "SUM" => Self::Sum,
            "AVG" => Self::Avg,
            "MIN" => Self::Min,
            "MAX" => Self::Max,
            _ => return None,
        })
    }
}

/// How a filter or HAVING condition joins the one before it.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "UPPERCASE")]
pub enum Connector {
    And,
    Or,
}

/// `SortDirection` in `$lib/types`.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "UPPERCASE")]
pub enum SortDirection {
    Asc,
    Desc,
}

/// Where a subquery sits. The parser writes `where` and `from`; `select` is
/// the builder's own.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum SubqueryRole {
    Where,
    From,
    Select,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ParsedTable {
    pub table_name: String,
    pub alias: Option<String>,
    pub selected_columns: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub is_cte_reference: Option<bool>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ParsedJoin {
    pub source_table: String,
    pub source_column: String,
    pub target_table: String,
    pub target_column: String,
    pub join_type: JoinType,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ParsedFilter {
    pub column: String,
    pub operator: FilterOperator,
    pub value: String,
    pub connector: Connector,
    /// Index into `subqueries` when the value is a subquery.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
    pub subquery_index: Option<usize>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ParsedOrderBy {
    pub column: String,
    pub direction: SortDirection,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ParsedGroupBy {
    pub column: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ParsedHaving {
    pub aggregate_function: AggregateFunction,
    pub column: String,
    pub operator: HavingOperator,
    pub value: String,
    pub connector: Connector,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ParsedSelectAggregate {
    pub function: AggregateFunction,
    pub expression: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub alias: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ParsedColumnAggregate {
    pub table_name: String,
    pub column: String,
    pub function: AggregateFunction,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub alias: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ParsedSubquery {
    pub id: String,
    pub role: SubqueryRole,
    /// Index of the filter that uses this subquery.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
    pub linked_filter_index: Option<usize>,
    pub inner_query: ParsedQuery,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "ParsedCTE"))]
#[serde(rename_all = "camelCase")]
pub struct ParsedCte {
    pub id: String,
    pub name: String,
    pub inner_query: ParsedQuery,
}

#[derive(Serialize, Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ParsedQuery {
    pub tables: Vec<ParsedTable>,
    pub joins: Vec<ParsedJoin>,
    pub filters: Vec<ParsedFilter>,
    pub group_by: Vec<ParsedGroupBy>,
    pub having: Vec<ParsedHaving>,
    pub order_by: Vec<ParsedOrderBy>,
    /// The number as written (`10`, or `10.5`, which the TS kept as a number
    /// too).
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub limit: Option<serde_json::Number>,
    pub select_aggregates: Vec<ParsedSelectAggregate>,
    pub column_aggregates: Vec<ParsedColumnAggregate>,
    pub subqueries: Vec<ParsedSubquery>,
    /// Always there on the top-level query; absent on subqueries and CTEs.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ctes: Option<Vec<ParsedCte>>,
}

/// `parseSql(sql, { validTableNames })` in the engine's dialect (fix 9); the
/// tutorial passes `Postgres`. `valid_tables: None` accepts every table (the
/// TS `null`). The TS `undefined` (the tutorial tables) never crosses the
/// boundary: the `src/lib/sql` wrapper resolves it to `getTableNames()`, so
/// only `string[] | null` arrives.
///
/// `None` when the SQL doesn't parse or its first statement isn't a query
/// with a SELECT (the TS `null`).
pub fn parse_builder_query(
    sql: &str,
    engine: SqlEngine,
    tutorial_schema: &TutorialSchema,
    valid_tables: Option<&[String]>,
) -> Option<ParsedQuery> {
    // JS `trim()`: sqlparser skips the same whitespace, so an input of only
    // JS whitespace would parse to no statements and give `None` otherwise.
    if sql.chars().all(is_js_space) {
        return Some(ParsedQuery {
            ctes: Some(vec![]),
            ..Default::default()
        });
    }
    let stmts = parse(sql, engine).ok()?;
    let Some(Statement::Query(q)) = stmts.first() else {
        return None;
    };
    let ctx = Ctx {
        schema: tutorial_schema,
        valid: valid_tables,
    };
    ctx.top(q)
}

const WHERE_FILTER_OPERATORS: [(&str, FilterOperator); 12] = [
    ("=", FilterOperator::Eq),
    ("!=", FilterOperator::NotEq),
    ("<>", FilterOperator::NotEq),
    (">", FilterOperator::Gt),
    ("<", FilterOperator::Lt),
    (">=", FilterOperator::GtEq),
    ("<=", FilterOperator::LtEq),
    ("LIKE", FilterOperator::Like),
    ("NOT LIKE", FilterOperator::NotLike),
    ("IN", FilterOperator::In),
    ("NOT IN", FilterOperator::NotIn),
    ("BETWEEN", FilterOperator::Between),
];

struct Ctx<'a> {
    schema: &'a TutorialSchema,
    valid: Option<&'a [String]>,
}

/// What a WHERE or HAVING walk writes into.
struct Out<'o> {
    filters: &'o mut Vec<ParsedFilter>,
    subqueries: &'o mut Vec<ParsedSubquery>,
}

impl Ctx<'_> {
    /// `findTableForColumn`: the first table whose schema has the column, else
    /// the first table.
    fn find_table_for_column<'t>(
        &self,
        tables: &'t [ParsedTable],
        col: &str,
    ) -> Option<&'t ParsedTable> {
        tables
            .iter()
            .find(|t| {
                self.schema
                    .get(&t.table_name)
                    .is_some_and(|c| c.iter().any(|c| c == col))
            })
            .or(tables.first())
    }

    /// `table.column` for a column reference, the way GROUP BY, ORDER BY and
    /// WHERE name it.
    fn full_column(
        &self,
        qualifier: Option<String>,
        col: String,
        alias_map: &HashMap<String, String>,
        tables: &[ParsedTable],
    ) -> String {
        match qualifier {
            Some(t) => format!("{}.{col}", resolve(&t, alias_map)),
            None => match self.find_table_for_column(tables, &col) {
                Some(tb) => format!("{}.{col}", tb.table_name),
                None => col,
            },
        }
    }

    fn top(&self, q: &Query) -> Option<ParsedQuery> {
        let select = first_select(q)?;
        let mut alias_map: HashMap<String, String> = HashMap::new();
        let mut ctes = Vec::new();
        let mut cte_names: HashSet<String> = HashSet::new();

        if let Some(with) = &q.with {
            for cte in &with.cte_tables {
                let name = cte.alias.name.value.clone();
                if name.is_empty() {
                    continue;
                }
                if let Some(inner) = self.sub(&cte.query, None) {
                    ctes.push(ParsedCte {
                        id: format!("cte-{}", ctes.len()),
                        name: name.clone(),
                        inner_query: inner,
                    });
                    cte_names.insert(name.clone());
                    alias_map.insert(name.clone(), name);
                }
            }
        }

        let mut out = self.body(q, select, Some(&cte_names), alias_map, true);
        out.ctes = Some(ctes);
        Some(out)
    }

    /// `parseSubqueryAst`.
    fn sub(&self, q: &Query, cte_names: Option<&HashSet<String>>) -> Option<ParsedQuery> {
        let select = first_select(q)?;
        Some(self.body(q, select, cte_names, HashMap::new(), false))
    }

    fn body(
        &self,
        q: &Query,
        select: &Select,
        cte_names: Option<&HashSet<String>>,
        mut alias_map: HashMap<String, String>,
        top: bool,
    ) -> ParsedQuery {
        let mut out = ParsedQuery::default();
        let is_cte = |n: &str| cte_names.is_some_and(|s| s.contains(n));

        // FROM
        for item in flat_from(&select.from) {
            match item.factor {
                TableFactor::Derived {
                    subquery, alias, ..
                } => {
                    if let Some(inner) = self.sub(subquery, cte_names) {
                        let idx = out.subqueries.len();
                        out.subqueries.push(ParsedSubquery {
                            id: format!("subquery-{idx}"),
                            role: SubqueryRole::From,
                            linked_filter_index: None,
                            inner_query: inner,
                        });
                        if let Some(a) = alias {
                            alias_map.insert(a.name.value.clone(), a.name.value.clone());
                        }
                    }
                }
                TableFactor::Table { name, alias, .. } => {
                    let (table, _schema) = table_and_schema(name);
                    if table.is_empty() {
                        continue;
                    }
                    if let Some(v) = self.valid {
                        if !v.contains(&table) && !is_cte(&table) {
                            continue;
                        }
                    }
                    let alias = alias
                        .as_ref()
                        .map(|a| a.name.value.clone())
                        .filter(|a| !a.is_empty());
                    if let Some(a) = &alias {
                        alias_map.insert(a.clone(), table.clone());
                    }
                    alias_map.insert(table.clone(), table.clone());
                    if let (Some(join), Some(on)) = (item.join, item.on) {
                        if let Some(info) = join_condition(on, &alias_map) {
                            let swap = info.left_table == table;
                            out.joins.push(ParsedJoin {
                                source_table: if swap {
                                    info.right_table.clone()
                                } else {
                                    info.left_table.clone()
                                },
                                source_column: if swap {
                                    info.right_column.clone()
                                } else {
                                    info.left_column.clone()
                                },
                                target_column: if swap {
                                    info.left_column
                                } else {
                                    info.right_column
                                },
                                join_type: normalize_join(join),
                                target_table: table.clone(),
                            });
                        }
                    }
                    out.tables.push(ParsedTable {
                        table_name: table.clone(),
                        alias,
                        selected_columns: vec![],
                        is_cte_reference: if top {
                            None
                        } else {
                            cte_names.map(|s| s.contains(&table))
                        },
                    });
                }
                _ => {}
            }
        }

        // SELECT list
        for item in &select.projection {
            let (expr, alias) = match item {
                SelectItem::Wildcard(_) => {
                    for t in out.tables.iter_mut() {
                        if let Some(cols) = self.schema.get(&t.table_name) {
                            t.selected_columns = cols.clone();
                        } else if top && is_cte(&t.table_name) {
                            t.selected_columns = vec!["*".into()];
                        }
                    }
                    continue;
                }
                SelectItem::QualifiedWildcard(
                    SelectItemQualifiedWildcardKind::ObjectName(n),
                    _,
                ) => {
                    let (q, _) = table_and_schema(n);
                    let name = resolve(&q, &alias_map);
                    if let Some(t) = out.tables.iter_mut().find(|t| t.table_name == name) {
                        if let Some(cols) = self.schema.get(&name) {
                            t.selected_columns = cols.clone();
                        }
                    }
                    continue;
                }
                SelectItem::UnnamedExpr(e) => (e, None),
                SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias.value.clone())),
                _ => continue,
            };
            if let Some((t, col)) = column_ref(expr) {
                let target = match t {
                    Some(t) => {
                        let name = resolve(&t, &alias_map);
                        out.tables.iter().position(|x| x.table_name == name)
                    }
                    None => self
                        .find_table_for_column(&out.tables, &col)
                        .map(|x| x.table_name.clone())
                        .and_then(|n| out.tables.iter().position(|x| x.table_name == n)),
                };
                if let Some(i) = target {
                    if !out.tables[i].selected_columns.contains(&col) {
                        out.tables[i].selected_columns.push(col);
                    }
                }
            } else if let Some((func, arg)) = aggregate(expr) {
                let Some(func) = AggregateFunction::from_upper(&func) else {
                    continue;
                };
                // An empty alias (`AS ""`) is no alias to the TS (`col.as || undefined`).
                let alias = alias.filter(|a| !a.is_empty());
                match arg {
                    None => {}
                    Some(AggArg::Star) | Some(AggArg::Other) => {
                        out.select_aggregates.push(ParsedSelectAggregate {
                            function: func,
                            expression: "*".into(),
                            alias,
                        })
                    }
                    Some(AggArg::Column(t, col)) => {
                        let table = match t {
                            Some(t) => Some(resolve(&t, &alias_map)),
                            None => self
                                .find_table_for_column(&out.tables, &col)
                                .map(|x| x.table_name.clone()),
                        };
                        if let Some(table) = table.filter(|t| !t.is_empty()) {
                            out.column_aggregates.push(ParsedColumnAggregate {
                                table_name: table.clone(),
                                column: col.clone(),
                                function: func,
                                alias,
                            });
                            if let Some(t) = out.tables.iter_mut().find(|t| t.table_name == table) {
                                if !t.selected_columns.contains(&col) {
                                    t.selected_columns.push(col);
                                }
                            }
                        }
                    }
                }
            }
        }

        // WHERE
        if let Some(w) = &select.selection {
            let mut filters = Vec::new();
            let mut o = Out {
                filters: &mut filters,
                subqueries: &mut out.subqueries,
            };
            self.where_rec(
                w,
                &alias_map,
                &out.tables,
                &mut o,
                Connector::And,
                cte_names,
            );
            out.filters = filters;
        }

        // GROUP BY
        if let GroupByExpr::Expressions(exprs, _) = &select.group_by {
            for g in exprs {
                if let Some((t, col)) = column_ref(g) {
                    let column = self.full_column(t, col, &alias_map, &out.tables);
                    out.group_by.push(ParsedGroupBy { column });
                }
            }
        }

        // HAVING
        if let Some(hv) = &select.having {
            self.having_rec(
                hv,
                &mut out.having,
                Connector::And,
                &mut out.subqueries,
                cte_names,
            );
        }

        // ORDER BY
        if let Some(ob) = &q.order_by {
            if let OrderByKind::Expressions(list) = &ob.kind {
                for o in list {
                    if let Some((t, col)) = column_ref(&o.expr) {
                        let column = self.full_column(t, col, &alias_map, &out.tables);
                        out.order_by.push(ParsedOrderBy {
                            column,
                            direction: match o.options.sort {
                                Some(OrderBySort::Desc) => SortDirection::Desc,
                                _ => SortDirection::Asc,
                            },
                        });
                    }
                }
            }
        }

        // LIMIT: node-sql-parser's first LIMIT number, which for MySQL's
        // `LIMIT 5, 10` is the offset (a quirk the builder keeps).
        if let Some(lc) = &q.limit_clause {
            let lim = match lc {
                LimitClause::LimitOffset { limit, .. } => limit.as_ref(),
                LimitClause::OffsetCommaLimit { offset, .. } => Some(offset),
            };
            if let Some(Expr::Value(v)) = lim {
                if let Value::Number(n, _) = &v.value {
                    out.limit = n.parse::<serde_json::Number>().ok();
                }
            }
        }

        out
    }

    fn where_rec(
        &self,
        e: &Expr,
        alias_map: &HashMap<String, String>,
        tables: &[ParsedTable],
        out: &mut Out<'_>,
        connector: Connector,
        cte_names: Option<&HashSet<String>>,
    ) {
        // Walk an AND/OR chain's left spine with a loop, so a long flat chain
        // (`a = 1 AND b = 2 AND …`) doesn't recurse once per term. The right
        // side of each node is the next filter, with that node's connector.
        let mut rights: Vec<(&Expr, Connector)> = Vec::new();
        let mut cur = unnest(e);
        let mut first_connector = connector;
        loop {
            if let Expr::BinaryOp { left, op, right } = cur {
                let conn = match op {
                    BinaryOperator::And => Some(Connector::And),
                    BinaryOperator::Or => Some(Connector::Or),
                    _ => None,
                };
                if let Some(c) = conn {
                    rights.push((right, c));
                    cur = unnest(left);
                    continue;
                }
            }
            break;
        }
        // `cur` is the leftmost leaf; it keeps the connector we were given.
        self.where_leaf(cur, alias_map, tables, out, first_connector, cte_names);
        while let Some((r, c)) = rights.pop() {
            first_connector = c;
            self.where_rec(r, alias_map, tables, out, first_connector, cte_names);
        }
    }

    fn where_leaf(
        &self,
        e: &Expr,
        alias_map: &HashMap<String, String>,
        tables: &[ParsedTable],
        out: &mut Out<'_>,
        connector: Connector,
        cte_names: Option<&HashSet<String>>,
    ) {
        let Some(cmp) = comparison(e) else { return };
        let Some((t, col)) = column_ref(unnest(cmp.left)) else {
            return;
        };
        let column = self.full_column(t, col, alias_map, tables);
        let operator = map_operator(&cmp.op);
        match cmp.right {
            Right::Subquery(q) => {
                let parsed = self.sub(q, cte_names);
                if let (Some(p), Some(op)) = (parsed, operator) {
                    let idx = out.subqueries.len();
                    out.subqueries.push(ParsedSubquery {
                        id: format!("subquery-{idx}"),
                        role: SubqueryRole::Where,
                        linked_filter_index: Some(out.filters.len()),
                        inner_query: p,
                    });
                    out.filters.push(ParsedFilter {
                        column,
                        operator: op,
                        value: String::new(),
                        connector,
                        subquery_index: Some(idx),
                    });
                }
            }
            Right::Expr(r) => {
                if let (Some(op), Some(v)) = (operator, r.and_then(literal)) {
                    out.filters.push(ParsedFilter {
                        column,
                        operator: op,
                        value: v,
                        connector,
                        subquery_index: None,
                    });
                }
            }
        }
    }

    fn having_rec(
        &self,
        e: &Expr,
        having: &mut Vec<ParsedHaving>,
        connector: Connector,
        subqueries: &mut Vec<ParsedSubquery>,
        cte_names: Option<&HashSet<String>>,
    ) {
        // Like `where_rec`: the left spine of an AND/OR chain with a loop, so
        // its length doesn't cost stack. The leftmost leaf keeps `connector`;
        // each right side gets the connector of the node it hangs from.
        let mut rights: Vec<(&Expr, Connector)> = Vec::new();
        let mut cur = unnest(e);
        while let Expr::BinaryOp { left, op, right } = cur {
            let c = match op {
                BinaryOperator::And => Connector::And,
                BinaryOperator::Or => Connector::Or,
                _ => break,
            };
            rights.push((right, c));
            cur = unnest(left);
        }
        self.having_leaf(cur, having, connector, subqueries, cte_names);
        while let Some((r, c)) = rights.pop() {
            self.having_rec(r, having, c, subqueries, cte_names);
        }
    }

    fn having_leaf(
        &self,
        e: &Expr,
        having: &mut Vec<ParsedHaving>,
        connector: Connector,
        subqueries: &mut Vec<ParsedSubquery>,
        cte_names: Option<&HashSet<String>>,
    ) {
        let Expr::BinaryOp { left, op, right } = e else {
            return;
        };
        let Some((func, arg)) = aggregate(unnest(left)) else {
            return;
        };
        let Some(func) = AggregateFunction::from_upper(&func) else {
            return;
        };
        let operator = map_having_operator(op);
        let column = match &arg {
            Some(AggArg::Column(_, c)) => c.clone(),
            Some(_) => String::new(),
            None => return,
        };
        if let Expr::Subquery(q) = unnest(right) {
            if let (Some(p), Some(op)) = (self.sub(q, cte_names), operator) {
                let idx = subqueries.len();
                subqueries.push(ParsedSubquery {
                    id: format!("subquery-{idx}"),
                    role: SubqueryRole::Where,
                    linked_filter_index: None,
                    inner_query: p,
                });
                having.push(ParsedHaving {
                    aggregate_function: func,
                    column,
                    operator: op,
                    value: "(subquery)".into(),
                    connector,
                });
            }
            return;
        }
        if let (Some(op), Some(v)) = (operator, literal(right)) {
            having.push(ParsedHaving {
                aggregate_function: func,
                column,
                operator: op,
                value: v,
                connector,
            });
        }
    }
}

fn resolve(name: &str, alias_map: &HashMap<String, String>) -> String {
    alias_map
        .get(name)
        .filter(|t| !t.is_empty())
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

fn normalize_join(j: FromJoin) -> JoinType {
    match j {
        FromJoin::Left => JoinType::Left,
        FromJoin::Right => JoinType::Right,
        FromJoin::Full => JoinType::Full,
        FromJoin::Inner | FromJoin::Cross | FromJoin::Other => JoinType::Inner,
    }
}

struct JoinInfo {
    left_table: String,
    left_column: String,
    right_table: String,
    right_column: String,
}

/// The two sides of `a.x = b.y`.
fn join_condition(on: &Expr, alias_map: &HashMap<String, String>) -> Option<JoinInfo> {
    let Expr::BinaryOp {
        left,
        op: BinaryOperator::Eq,
        right,
    } = unnest(on)
    else {
        return None;
    };
    let (lt, lc) = column_ref(left)?;
    let (rt, rc) = column_ref(right)?;
    Some(JoinInfo {
        left_table: resolve(&lt?, alias_map),
        left_column: lc,
        right_table: resolve(&rt?, alias_map),
        right_column: rc,
    })
}

enum Right<'a> {
    Subquery(&'a Query),
    /// `None` where node-sql-parser's right side isn't a literal (IN lists,
    /// BETWEEN, IS NULL).
    Expr(Option<&'a Expr>),
}

struct Cmp<'a> {
    left: &'a Expr,
    /// node-sql-parser's operator string.
    op: String,
    right: Right<'a>,
}

/// View a predicate the way node-sql-parser's `binary_expr` does.
fn comparison(e: &Expr) -> Option<Cmp<'_>> {
    Some(match e {
        Expr::BinaryOp { left, op, right } => Cmp {
            left,
            op: op.to_string(),
            right: match unnest(right) {
                Expr::Subquery(q) => Right::Subquery(q),
                r => Right::Expr(Some(r)),
            },
        },
        Expr::Like {
            negated,
            expr,
            pattern,
            ..
        } => Cmp {
            left: expr,
            op: if *negated { "NOT LIKE" } else { "LIKE" }.into(),
            right: Right::Expr(Some(pattern)),
        },
        Expr::ILike {
            negated,
            expr,
            pattern,
            ..
        } => Cmp {
            left: expr,
            op: if *negated { "NOT ILIKE" } else { "ILIKE" }.into(),
            right: Right::Expr(Some(pattern)),
        },
        Expr::InList { expr, negated, .. } => Cmp {
            left: expr,
            op: if *negated { "NOT IN" } else { "IN" }.into(),
            right: Right::Expr(None),
        },
        Expr::InSubquery {
            expr,
            subquery,
            negated,
        } => Cmp {
            left: expr,
            op: if *negated { "NOT IN" } else { "IN" }.into(),
            right: Right::Subquery(subquery),
        },
        Expr::Between { expr, negated, .. } => Cmp {
            left: expr,
            op: if *negated { "NOT BETWEEN" } else { "BETWEEN" }.into(),
            right: Right::Expr(None),
        },
        Expr::IsNull(x) | Expr::IsNotNull(x) => Cmp {
            left: x,
            op: "IS".into(),
            right: Right::Expr(None),
        },
        _ => return None,
    })
}

fn map_operator(op: &str) -> Option<FilterOperator> {
    let up = op.to_uppercase();
    WHERE_FILTER_OPERATORS
        .iter()
        .find(|(k, _)| *k == up)
        .map(|(_, v)| *v)
}

fn map_having_operator(op: &BinaryOperator) -> Option<HavingOperator> {
    Some(match op {
        BinaryOperator::Eq => HavingOperator::Eq,
        BinaryOperator::NotEq => HavingOperator::NotEq,
        BinaryOperator::Gt => HavingOperator::Gt,
        BinaryOperator::Lt => HavingOperator::Lt,
        BinaryOperator::GtEq => HavingOperator::GtEq,
        BinaryOperator::LtEq => HavingOperator::LtEq,
        _ => return None,
    })
}
