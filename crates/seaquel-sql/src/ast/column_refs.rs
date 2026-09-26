//! Column sources for inline editing (Task 6, decision 9). Port of the SQL
//! half of `resolveColumnSources` (`src/lib/db/column-sources.ts`, deleted in
//! phase 2b): per output column, the table it reads and the column name. The
//! TS wrapper (`src/lib/sql/index.ts`) looks up the table in its schema cache
//! and reads the primary keys with `findTable`, so the cache never crosses the
//! boundary.

use std::collections::HashMap;

use serde::Serialize;
use sqlparser::ast::{SelectItem, Statement, TableFactor};

use super::util::{column_ref, first_select, flat_from, parse, table_and_schema};
use crate::SqlEngine;

/// The base-table column an output column reads, as the query names it.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ColumnRef {
    /// The table's schema, when the query qualifies the table. Without one
    /// the wrapper takes the first table with that name, as `findTable` does.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub schema: Option<String>,
    pub table: String,
    pub column: String,
}

struct FromEntry {
    schema: Option<String>,
    table: String,
}

/// One entry per output column (`None` where it isn't a column of a FROM
/// table the query names), or `None` for the whole query when its columns
/// can't be mapped: it doesn't parse, isn't a SELECT, has no base table in
/// FROM, or selects `*` or `t.*` (the columns would depend on the live schema).
pub fn column_refs(sql: &str, engine: SqlEngine) -> Option<Vec<Option<ColumnRef>>> {
    let stmts = parse(sql, engine).ok()?;
    let Some(Statement::Query(q)) = stmts.first() else {
        return None;
    };
    let sel = first_select(q)?;

    let mut from: Vec<FromEntry> = Vec::new();
    for item in flat_from(&sel.from) {
        // Base tables only; subqueries and table functions are skipped.
        let TableFactor::Table { name, .. } = item.factor else {
            continue;
        };
        let (table, schema) = table_and_schema(name);
        if table.is_empty() {
            continue;
        }
        from.push(FromEntry {
            schema: schema.filter(|s| !s.is_empty()),
            table,
        });
    }
    if from.is_empty() {
        return None;
    }
    // Each entry by its alias and by its table name; a later entry wins a
    // name both use, as in the TS.
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut idx = 0;
    for item in flat_from(&sel.from) {
        let TableFactor::Table { name, alias, .. } = item.factor else {
            continue;
        };
        if table_and_schema(name).0.is_empty() {
            continue;
        }
        if let Some(a) = alias.as_ref().filter(|a| !a.name.value.is_empty()) {
            by_name.insert(a.name.value.clone(), idx);
        }
        by_name.insert(from[idx].table.clone(), idx);
        idx += 1;
    }

    let mut out = Vec::with_capacity(sel.projection.len());
    for item in &sel.projection {
        let expr = match item {
            SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => e,
            // `*` / `t.*`: column positions depend on the live schema.
            _ => return None,
        };
        let Some((qualifier, column)) = column_ref(expr) else {
            // A function call, aggregate, literal, CASE, …
            out.push(None);
            continue;
        };
        let entry = match qualifier.filter(|q| !q.is_empty()) {
            Some(q) => by_name.get(&q).map(|i| &from[*i]),
            // An unqualified column is unambiguous only with one table.
            None if from.len() == 1 => Some(&from[0]),
            None => None,
        };
        out.push(entry.map(|e| ColumnRef {
            schema: e.schema.clone(),
            table: e.table.clone(),
            column,
        }));
    }
    Some(out)
}
