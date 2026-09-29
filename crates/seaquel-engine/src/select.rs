//! The data tab's SELECT (phase 5c, Decision 9): a pure builder
//! parameterized like [`crate::crud`], by the quote functions, the
//! placeholder and the text type filters compare as.
//!
//! `SELECT <columns> FROM <qs(schema)>.<qi(table)> [WHERE …] [ORDER BY …]`,
//! with no paging: Core pages it with `Dialect::paginate` and counts it with
//! `count_query`, as it does the editor's SELECTs. Each filter compares the
//! column cast to text (`CAST(col AS TEXT) = $1`), as the TypeScript data tab
//! did; `IS NULL` and `IS NOT NULL` compare the column itself. Values are
//! always bound, never interpolated.

use seaquel_types::{SqlWithBindings, Value};

use crate::crud::{PlaceholderFn, QuoteIdFn};

/// A comparison the data tab's filters make. The operator text is fixed
/// here, so nothing from the caller reaches the SQL but quoted names and
/// placeholders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
    Like,
    NotLike,
}

impl CompareOp {
    fn sql(self) -> &'static str {
        match self {
            CompareOp::Eq => "=",
            CompareOp::Ne => "!=",
            CompareOp::Gt => ">",
            CompareOp::Lt => "<",
            CompareOp::Ge => ">=",
            CompareOp::Le => "<=",
            CompareOp::Like => "LIKE",
            CompareOp::NotLike => "NOT LIKE",
        }
    }
}

/// One WHERE condition. It has no `Debug`, since it holds filter values;
/// callers log counts, never conditions.
#[derive(Clone, PartialEq)]
pub enum Condition<'a> {
    /// `CAST(col AS <text>) <op> <placeholder>`.
    Compare {
        column: &'a str,
        op: CompareOp,
        value: Value,
    },
    /// `CAST(col AS <text>) [NOT] IN (<placeholder>, …)`. At least one
    /// value (an empty list is the caller's refusal; here it would be
    /// `IN ()`).
    In {
        column: &'a str,
        negated: bool,
        values: Vec<Value>,
    },
    IsNull(&'a str),
    IsNotNull(&'a str),
}

/// One output column of a listed select: `CAST(col AS <text>) AS col` when
/// `cast`, else the column itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectColumn<'a> {
    pub name: &'a str,
    pub cast: bool,
}

/// What [`build_table_select`] builds.
#[derive(Clone, PartialEq)]
pub struct TableSelect<'a> {
    pub schema: &'a str,
    pub table: &'a str,
    /// `None` selects `*`.
    pub columns: Option<Vec<SelectColumn<'a>>>,
    pub conditions: Vec<Condition<'a>>,
    /// Join the conditions with `OR` instead of `AND`.
    pub or: bool,
    /// `(column, descending)`, in order.
    pub order: Vec<(&'a str, bool)>,
}

/// The SELECT for `select`, placeholders numbered from 1 in the order the
/// values bind.
pub fn build_table_select(
    select: &TableSelect,
    qi: QuoteIdFn,
    qs: QuoteIdFn,
    placeholder: PlaceholderFn,
    text_type: &str,
) -> SqlWithBindings {
    let columns = match &select.columns {
        None => "*".to_string(),
        Some(columns) => columns
            .iter()
            .map(|c| {
                if c.cast {
                    format!("CAST({} AS {text_type}) AS {}", qi(c.name), qi(c.name))
                } else {
                    qi(c.name)
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
    };
    let mut sql = format!(
        "SELECT {columns} FROM {}.{}",
        qs(select.schema),
        qi(select.table)
    );
    let mut binds: Vec<Value> = Vec::new();
    let mut bind = |value: &Value| {
        binds.push(value.clone());
        placeholder(binds.len())
    };
    if !select.conditions.is_empty() {
        let conditions: Vec<String> = select
            .conditions
            .iter()
            .map(|c| match c {
                Condition::Compare { column, op, value } => format!(
                    "CAST({} AS {text_type}) {} {}",
                    qi(column),
                    op.sql(),
                    bind(value)
                ),
                Condition::In {
                    column,
                    negated,
                    values,
                } => {
                    let list: Vec<String> = values.iter().map(&mut bind).collect();
                    format!(
                        "CAST({} AS {text_type}) {}IN ({})",
                        qi(column),
                        if *negated { "NOT " } else { "" },
                        list.join(", ")
                    )
                }
                Condition::IsNull(column) => format!("{} IS NULL", qi(column)),
                Condition::IsNotNull(column) => format!("{} IS NOT NULL", qi(column)),
            })
            .collect();
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(if select.or { " OR " } else { " AND " }));
    }
    if !select.order.is_empty() {
        let order: Vec<String> = select
            .order
            .iter()
            .map(|(column, desc)| format!("{} {}", qi(column), if *desc { "DESC" } else { "ASC" }))
            .collect();
        sql.push_str(" ORDER BY ");
        sql.push_str(&order.join(", "));
    }
    SqlWithBindings {
        sql,
        bind_values: Some(binds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crud::{dollar_placeholder, question_placeholder};

    fn dq(id: &str) -> String {
        format!("\"{}\"", id.replace('"', "\"\""))
    }

    fn bracket(id: &str) -> String {
        format!("[{}]", id.replace(']', "]]"))
    }

    fn at(i: usize) -> String {
        format!("@P{i}")
    }

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn filters() -> TableSelect<'static> {
        TableSelect {
            schema: "s",
            table: "t",
            columns: None,
            conditions: vec![
                Condition::Compare {
                    column: "a",
                    op: CompareOp::Like,
                    value: t("x%"),
                },
                Condition::In {
                    column: "b",
                    negated: false,
                    values: vec![t("1"), t("2")],
                },
                Condition::IsNull("c"),
                Condition::In {
                    column: "d",
                    negated: true,
                    values: vec![t("3")],
                },
                Condition::Compare {
                    column: "e",
                    op: CompareOp::Ge,
                    value: t("9"),
                },
            ],
            or: false,
            order: vec![("a", true), ("b", false)],
        }
    }

    #[test]
    fn dollar_placeholders_number_in_bind_order() {
        let got = build_table_select(&filters(), &dq, &dq, &dollar_placeholder, "TEXT");
        assert_eq!(
            got.sql,
            "SELECT * FROM \"s\".\"t\" WHERE CAST(\"a\" AS TEXT) LIKE $1 AND CAST(\"b\" AS TEXT) IN ($2, $3) \
             AND \"c\" IS NULL AND CAST(\"d\" AS TEXT) NOT IN ($4) AND CAST(\"e\" AS TEXT) >= $5 \
             ORDER BY \"a\" DESC, \"b\" ASC"
        );
        assert_eq!(
            got.bind_values,
            Some(vec![t("x%"), t("1"), t("2"), t("3"), t("9")])
        );
    }

    #[test]
    fn question_placeholders() {
        let mut s = filters();
        s.or = true;
        s.order.clear();
        let got = build_table_select(&s, &dq, &dq, &question_placeholder, "CHAR");
        assert_eq!(
            got.sql,
            "SELECT * FROM \"s\".\"t\" WHERE CAST(\"a\" AS CHAR) LIKE ? OR CAST(\"b\" AS CHAR) IN (?, ?) \
             OR \"c\" IS NULL OR CAST(\"d\" AS CHAR) NOT IN (?) OR CAST(\"e\" AS CHAR) >= ?"
        );
    }

    #[test]
    fn at_placeholders_and_listed_columns() {
        let s = TableSelect {
            schema: "dbo",
            table: "odd]",
            columns: Some(vec![
                SelectColumn {
                    name: "id",
                    cast: false,
                },
                SelectColumn {
                    name: "v",
                    cast: true,
                },
            ]),
            conditions: vec![Condition::Compare {
                column: "n",
                op: CompareOp::Eq,
                value: t("a"),
            }],
            or: false,
            order: vec![],
        };
        let got = build_table_select(&s, &bracket, &bracket, &at, "NVARCHAR(MAX)");
        assert_eq!(
            got.sql,
            "SELECT [id], CAST([v] AS NVARCHAR(MAX)) AS [v] FROM [dbo].[odd]]] \
             WHERE CAST([n] AS NVARCHAR(MAX)) = @P1"
        );
    }

    #[test]
    fn every_comparison_operator() {
        for (op, text) in [
            (CompareOp::Eq, "="),
            (CompareOp::Ne, "!="),
            (CompareOp::Gt, ">"),
            (CompareOp::Lt, "<"),
            (CompareOp::Ge, ">="),
            (CompareOp::Le, "<="),
            (CompareOp::Like, "LIKE"),
            (CompareOp::NotLike, "NOT LIKE"),
        ] {
            let s = TableSelect {
                schema: "s",
                table: "t",
                columns: None,
                conditions: vec![Condition::Compare {
                    column: "a",
                    op,
                    value: t("v"),
                }],
                or: false,
                order: vec![],
            };
            let got = build_table_select(&s, &dq, &dq, &dollar_placeholder, "TEXT");
            assert_eq!(
                got.sql,
                format!("SELECT * FROM \"s\".\"t\" WHERE CAST(\"a\" AS TEXT) {text} $1")
            );
        }
    }
}
