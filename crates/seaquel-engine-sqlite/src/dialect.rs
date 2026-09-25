//! The SQLite SQL dialect: a port of the pure half of `src/lib/db/sqlite.ts`,
//! deleted in phase 2 (quoting, pagination, CRUD and DDL generation, column
//! types, EXPLAIN text).
//!
//! Output matches the TypeScript byte for byte (see `tests/dialect_parity.rs`)
//! except for bug fixes 2 (the DDL quote doubles `"`, as the CRUD quote
//! already did), 3 (`CREATE INDEX "main"."i" ON "t"`: SQLite rejects a
//! schema-qualified table there), 4 (SQLite has no `ADD FOREIGN KEY`: a
//! foreign key on a column added in the same edit goes inline,
//! `ADD COLUMN c … REFERENCES "t" ("id")`, one on an existing column is a
//! comment line), 6 (a type, nullability or default edit is a comment line
//! instead of being dropped silently) and 7 (removing a constraint's
//! `sqlite_autoindex_*`, which the table editor now sees, is a comment line:
//! SQLite can't drop it).
//!
//! CRUD numbers its placeholders `$1`, `$2`, … like the TypeScript; sqlx's
//! SQLite binder reads `$N` as the Nth argument. The cast map is ignored:
//! casting to a declared type corrupts values under SQLite's type affinity
//! (`CAST('2024-01-01 10:00' AS DATETIME)` is `2024`), which is why the UI
//! stopped sending casts for SQLite (phase 2, Task 1).

use seaquel_engine::crud::{self, dollar_placeholder};
use seaquel_engine::ddl::{self, AlterTableOptions, AlterTableRules, UniqueChanges};
use seaquel_engine::{CastMap, Dialect, RowValues, SqlWithBindings, Value};
use seaquel_types::{ColumnCategory, ColumnTypeInfo, CreateTableColumn, CreateTableDefinition};

use crate::introspect;

pub struct SqliteDialect;

/// `"id"`, with embedded `"` doubled. Used for CRUD and, since bug fix 2,
/// for DDL too (TypeScript's DDL quote didn't escape).
pub(crate) fn qi(id: &str) -> String {
    format!("\"{}\"", id.replace('"', "\"\""))
}

const ALTER_OPTIONS: AlterTableOptions = AlterTableOptions {
    supports_drop_column: true,
    supports_alter_column: false,
    use_modify_column: false,
    qualify_drop_index: false,
    drop_index_on_table: false,
    alter_default_with_modify_column: false,
    qualify_index_name: true,
    supports_add_foreign_key: false,
    unsupported_notes: Some("SQLite"),
    inline_foreign_keys_on_added_columns: true,
    constraint_index_prefix: Some("sqlite_autoindex_"),
    drop_indexes_first: false,
    tsql: false,
};

/// SQLite can't add a UNIQUE constraint to an existing table (a note); a
/// unique index it drops, a constraint's autoindex is a note (Task 18).
const ALTER_RULES: AlterTableRules = AlterTableRules {
    not_null_after_add_column: false,
    indexes_block_column_changes: false,
    constraints_block_column_drops: false,
    unique_changes: UniqueChanges::DropIndex,
};

impl SqliteDialect {
    /// `ALTER TABLE "main"."t" ADD COLUMN …;` (TS `generateAddColumnSql`,
    /// which ignored the schema). Not on the `Dialect` trait: no wire request
    /// uses it yet.
    pub fn add_column(&self, _schema: &str, table: &str, column: &CreateTableColumn) -> String {
        let table = format!("{}.{}", qi("main"), qi(table));
        let opts = ddl::AddColumnOptions {
            column_keyword: true,
            ..Default::default()
        };
        ddl::generate_add_column_ddl(&table, column, &qi, opts)
    }
}

/// TS `generateCreateTableSql`: unlike the generic builder, a single primary
/// key is inline (`INTEGER` ones get `AUTOINCREMENT`), `UNIQUE` is inline,
/// and the table is never schema-qualified.
fn create_table(def: &CreateTableDefinition) -> String {
    let pk_count = def.columns.iter().filter(|c| c.is_primary_key).count();
    let mut lines: Vec<String> = Vec::new();

    for col in &def.columns {
        let mut line = format!("  {} {}", qi(&col.name), ddl::build_column_type(col));
        if col.is_primary_key && pk_count == 1 {
            line.push_str(" PRIMARY KEY");
            if col.ty.to_uppercase() == "INTEGER" {
                line.push_str(" AUTOINCREMENT");
            }
        }
        if !col.nullable {
            line.push_str(" NOT NULL");
        }
        if !col.default_value.is_empty() {
            let safe = ddl::sanitize_default_value(&col.default_value);
            if !safe.is_empty() {
                line.push_str(" DEFAULT ");
                line.push_str(&safe);
            }
        }
        if col.is_unique && !col.is_primary_key {
            line.push_str(" UNIQUE");
        }
        lines.push(line);
    }

    if pk_count > 1 {
        let pks: Vec<String> = def
            .columns
            .iter()
            .filter(|c| c.is_primary_key)
            .map(|c| qi(&c.name))
            .collect();
        lines.push(format!("  PRIMARY KEY ({})", pks.join(", ")));
    }

    for fk in &def.foreign_keys {
        lines.push(format!(
            "  FOREIGN KEY ({}) REFERENCES {} ({})",
            qi(&fk.column),
            qi(&fk.referenced_table),
            qi(&fk.referenced_column)
        ));
    }

    let table = qi(&def.table_name);
    let mut sql = format!("CREATE TABLE {table} (\n{}\n);", lines.join(",\n"));
    for idx in &def.indexes {
        let unique = if idx.unique { "UNIQUE " } else { "" };
        let cols: Vec<String> = idx.columns.iter().map(|c| qi(c)).collect();
        sql.push_str(&format!(
            "\n\nCREATE {unique}INDEX {} ON {table} ({});",
            qi(&idx.name),
            cols.join(", ")
        ));
    }
    sql
}

/// `'text'`, with `'` doubled.
fn string_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Whether SQLite reads `s` as one identifier token: a letter, `_` or
/// non-ASCII character, then letters, digits, `_`, `$` or non-ASCII.
fn is_bare_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || !c.is_ascii())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$' || !c.is_ascii())
}

/// The inside of `s` when it's one token quoted by `open`…`close` whose
/// embedded `close` characters are doubled (`escaped`), undoubled.
fn quoted_token(s: &str, open: char, close: char, escaped: bool) -> Option<String> {
    let inner = s.strip_prefix(open)?.strip_suffix(close)?;
    if !escaped {
        return (!inner.contains(close)).then(|| inner.to_string());
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == close && chars.next() != Some(close) {
            return None; // a lone quote: more than one token
        }
        out.push(c);
    }
    Some(out)
}

/// A column's `dflt_value` as an expression that evaluates to the value an
/// INSERT would store. SQLite's DEFAULT clause takes a signed number
/// (`+1`, `-2.5`, `0x10`), a literal (`'s'`, `x'00ff'`, `NULL`, `TRUE`,
/// `CURRENT_TIMESTAMP`, …) or `(expr)`, which `dflt_value` shows without its
/// parentheses; all of those are valid expressions as they are. It also
/// takes a bare identifier or a quoted name (`DEFAULT active`,
/// `DEFAULT "dq"`, `` `bt` ``, `[br]`) and stores it as a string; as an
/// expression those would read a column, so they become string literals.
/// A default can't reference a column, so no expression is lost that way.
pub(crate) fn default_operand(dflt_value: &str) -> String {
    const KEYWORDS: [&str; 6] = [
        "true",
        "false",
        "null",
        "current_time",
        "current_date",
        "current_timestamp",
    ];
    let v = dflt_value.trim();
    if v.is_empty() {
        return "NULL".to_string();
    }
    if is_bare_identifier(v) {
        return if KEYWORDS.iter().any(|k| v.eq_ignore_ascii_case(k)) {
            v.to_string()
        } else {
            string_literal(v)
        };
    }
    let name = quoted_token(v, '"', '"', true)
        .or_else(|| quoted_token(v, '`', '`', true))
        .or_else(|| quoted_token(v, '[', ']', false));
    match name {
        Some(text) => string_literal(&text),
        None => v.to_string(),
    }
}

impl Dialect for SqliteDialect {
    fn quote_ident(&self, id: &str) -> String {
        qi(id)
    }

    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String {
        format!("{sql} LIMIT {limit} OFFSET {offset}")
    }

    fn build_update(
        &self,
        schema: &str,
        table: &str,
        column: &str,
        value: Value,
        pks: &[String],
        row: &RowValues,
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_update(
            schema,
            table,
            column,
            value,
            pks,
            row,
            &qi,
            None,
            &dollar_placeholder,
        )
    }

    /// `SET col = DEFAULT`, as the TypeScript wrote it. SQLite has no
    /// `DEFAULT` in `UPDATE`, so the statement fails: only for callers that
    /// don't know the column's default (see `build_set_default_expr`).
    fn build_set_default(
        &self,
        schema: &str,
        table: &str,
        column: &str,
        pks: &[String],
        row: &RowValues,
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_set_default(
            schema,
            table,
            column,
            pks,
            row,
            &qi,
            None,
            &dollar_placeholder,
        )
    }

    /// Bug fix 11: `SET col = (<default>)`, since SQLite has no `DEFAULT` in
    /// `UPDATE`. The default is `pragma_table_info`'s `dflt_value`, the raw
    /// text of the column's DEFAULT clause, made into an expression by
    /// [`default_operand`]: `CURRENT_TIMESTAMP` and `(expr)` defaults are
    /// evaluated for the row as an INSERT would, a bare-word or quoted-name
    /// default is its text, and a column without one gets `NULL`. Without
    /// the default the TypeScript statement is kept.
    fn build_set_default_expr(
        &self,
        schema: &str,
        table: &str,
        column: &str,
        column_default: Option<&str>,
        pks: &[String],
        row: &RowValues,
        casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        match column_default {
            Some(expr) => crud::build_param_set_expr(
                schema,
                table,
                column,
                &default_operand(expr),
                pks,
                row,
                &qi,
                None,
                &dollar_placeholder,
            ),
            None => self.build_set_default(schema, table, column, pks, row, casts),
        }
    }

    fn build_insert(
        &self,
        schema: &str,
        table: &str,
        values: &[(String, Value)],
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_insert(schema, table, values, &qi, None, &dollar_placeholder)
    }

    fn build_delete(
        &self,
        schema: &str,
        table: &str,
        pks: &[String],
        row: &RowValues,
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_delete(schema, table, pks, row, &qi, None, &dollar_placeholder)
    }

    fn create_table(&self, def: &CreateTableDefinition) -> String {
        create_table(def)
    }

    fn alter_table(&self, from: &CreateTableDefinition, to: &CreateTableDefinition) -> String {
        ddl::generate_alter_table_sql_with(from, to, &qi, &qi, ALTER_OPTIONS, ALTER_RULES)
    }

    fn column_types(&self) -> Vec<ColumnTypeInfo> {
        use ColumnCategory::*;
        // (name, category, has_length, has_precision)
        const TYPES: &[(&str, ColumnCategory, bool, bool)] = &[
            ("TEXT", String, false, false),
            ("VARCHAR", String, true, false),
            ("INTEGER", Numeric, false, false),
            ("REAL", Numeric, false, false),
            ("NUMERIC", Numeric, false, true),
            ("BLOB", Binary, false, false),
            ("BOOLEAN", Boolean, false, false),
            ("DATE", DateTime, false, false),
            ("DATETIME", DateTime, false, false),
            ("TIMESTAMP", DateTime, false, false),
        ];
        TYPES
            .iter()
            .map(
                |&(name, category, has_length, has_precision)| ColumnTypeInfo {
                    name: name.to_string(),
                    category,
                    has_length: has_length.then_some(true),
                    has_precision: has_precision.then_some(true),
                },
            )
            .collect()
    }

    /// `EXPLAIN QUERY PLAN …` with or without ANALYZE: SQLite has no
    /// analyzing EXPLAIN. The driver runs and times the statement itself.
    fn explain_sql(&self, sql: &str, _analyze: bool) -> String {
        introspect::explain_sql(sql)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_operands() {
        for (dflt, expected) in [
            ("active", "'active'"),
            ("Active_1", "'Active_1'"),
            ("café", "'café'"),
            ("a1$b", "'a1$b'"),
            ("it", "'it'"),
            ("\"dq\"", "'dq'"),
            ("\"it's \"\"x\"\"\"", "'it''s \"x\"'"),
            ("\"\"", "''"),
            ("`bt`", "'bt'"),
            ("[br]", "'br'"),
            ("TRUE", "TRUE"),
            ("false", "false"),
            ("NULL", "NULL"),
            ("Current_Timestamp", "Current_Timestamp"),
            ("current_date", "current_date"),
            ("'lit'", "'lit'"),
            ("+1", "+1"),
            ("-2.5", "-2.5"),
            ("1e3", "1e3"),
            ("0x10", "0x10"),
            ("-0x10", "-0x10"),
            ("x'00ff'", "x'00ff'"),
            ("1+2", "1+2"),
            ("-(1)", "-(1)"),
            ("lower('A') || 'b'", "lower('A') || 'b'"),
            ("\"a\" || \"b\"", "\"a\" || \"b\""),
            ("", "NULL"),
        ] {
            assert_eq!(default_operand(dflt), expected, "{dflt}");
        }
    }

    #[test]
    fn quote_doubles_embedded_double_quotes() {
        assert_eq!(qi("a\"b"), "\"a\"\"b\"");
        assert_eq!(qi("\"\""), "\"\"\"\"\"\"");
    }
}
