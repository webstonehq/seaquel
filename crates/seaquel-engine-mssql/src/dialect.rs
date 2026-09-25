//! The SQL Server dialect: a port of the pure half of `src/lib/db/mssql.ts`,
//! deleted in phase 2 (quoting, pagination, CRUD and DDL generation, column
//! types, EXPLAIN text).
//!
//! Output matches the TypeScript byte for byte (see `tests/dialect_parity.rs`)
//! except for the bug fixes in `tests/fixtures/bugfixes.json`: 2 (the DDL
//! quote doubles `]`, as the CRUD quote already did), 7 (ALTER TABLE is
//! T-SQL, and CREATE TABLE and ADD say `NULL` for a nullable column), 8
//! (pagination looks for a top-level ORDER BY only), 12 (a dropped index may
//! be a constraint's), 13 (dropped indexes go first; DROP COLUMN drops the
//! column's default constraint first), 14 (pagination drops a trailing `;`
//! and comments) and 15 (ALTER COLUMN keeps a non-default collation).
//!
//! CRUD binds its values as `@P1`, `@P2`, … (phase 2 decision 4); the
//! TypeScript inlined literals. The cast map is ignored: the TypeScript
//! never cast, and SQL Server converts a bound value on assignment.

use seaquel_engine::crud;
use seaquel_engine::ddl::{self, AlterTableOptions, AlterTableRules, UniqueChanges};
use seaquel_engine::{CastMap, Dialect, RowValues, SqlWithBindings, Value};
use seaquel_types::{ColumnCategory, ColumnTypeInfo, CreateTableColumn, CreateTableDefinition};

use crate::introspect;

pub struct MssqlDialect;

/// `[id]`, with embedded `]` doubled. Used for CRUD and, since bug fix 2,
/// for DDL too (TypeScript's DDL quote didn't escape).
pub(crate) fn qi(id: &str) -> String {
    format!("[{}]", id.replace(']', "]]"))
}

/// `@P{index}`: tiberius sends parameters to `sp_executesql` under these
/// names.
fn at_placeholder(index: usize) -> String {
    format!("@P{index}")
}

const ALTER_OPTIONS: AlterTableOptions = AlterTableOptions {
    supports_drop_column: true,
    supports_alter_column: true,
    use_modify_column: false,
    qualify_drop_index: false,
    drop_index_on_table: false,
    alter_default_with_modify_column: false,
    qualify_index_name: false,
    supports_add_foreign_key: true,
    unsupported_notes: None,
    inline_foreign_keys_on_added_columns: false,
    constraint_index_prefix: None,
    drop_indexes_first: true,
    tsql: true,
};

/// SQL Server adds a UNIQUE constraint and drops one (or a unique index) by
/// the name the catalog gives (Task 18).
const ALTER_RULES: AlterTableRules = AlterTableRules {
    not_null_after_add_column: false,
    indexes_block_column_changes: false,
    constraints_block_column_drops: false,
    unique_changes: UniqueChanges::AddAndDropTsql,
};

/// `<type>[ COLLATE …] NULL|NOT NULL[ DEFAULT …]`: bug fix 7 says `NULL` out
/// loud, since a bare column's nullability depends on the session's
/// ANSI_NULL_DFLT settings, and fix 15 keeps a column's collation.
fn column_spec(col: &CreateTableColumn) -> String {
    let mut out = format!(
        "{}{} {}",
        ddl::build_column_type(col),
        ddl::tsql::collate(&col.ty, col.collation.as_deref()),
        if col.nullable { "NULL" } else { "NOT NULL" }
    );
    let safe = ddl::sanitize_default_value(&col.default_value);
    if !safe.is_empty() {
        out.push_str(" DEFAULT ");
        out.push_str(&safe);
    }
    out
}

impl MssqlDialect {
    /// `ALTER TABLE [s].[t] ADD [c] <type> NULL|NOT NULL[ DEFAULT …];` (TS
    /// `generateAddColumnSql`, with fixes 2 and 7). Not on the `Dialect`
    /// trait: no wire request uses it yet.
    pub fn add_column(&self, schema: &str, table: &str, column: &CreateTableColumn) -> String {
        format!(
            "ALTER TABLE {}.{} ADD {} {};",
            qi(schema),
            qi(table),
            qi(&column.name),
            column_spec(column)
        )
    }
}

/// TS `generateCreateTableDdl` with fixes 2 and 7.
fn create_table(def: &CreateTableDefinition) -> String {
    let mut lines: Vec<String> = def
        .columns
        .iter()
        .map(|c| format!("  {} {}", qi(&c.name), column_spec(c)))
        .collect();
    let pks: Vec<String> = def
        .columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| qi(&c.name))
        .collect();
    if !pks.is_empty() {
        lines.push(format!("  PRIMARY KEY ({})", pks.join(", ")));
    }
    for col in &def.columns {
        if col.is_unique && !col.is_primary_key {
            lines.push(format!("  UNIQUE ({})", qi(&col.name)));
        }
    }
    for fk in &def.foreign_keys {
        lines.push(format!(
            "  FOREIGN KEY ({}) REFERENCES {}.{} ({})",
            qi(&fk.column),
            qi(&fk.referenced_schema),
            qi(&fk.referenced_table),
            qi(&fk.referenced_column),
        ));
    }
    let table = format!("{}.{}", qi(&def.schema_name), qi(&def.table_name));
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

/// Characters of a T-SQL word for [`paginate`]: an identifier, keyword,
/// number or `@variable`.
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '#' | '$')
}

/// `OFFSET … ROWS FETCH NEXT … ROWS ONLY` after `sql`, with
/// `ORDER BY (SELECT NULL)` first unless the query has its own ORDER BY.
///
/// A small T-SQL tokenizer (bug fix 8): ORDER BY counts only at the top
/// level, outside parentheses (`OVER()`, subqueries, CTEs), string literals
/// (`''` escaped, `N'…'`), `[bracketed]` (`]]` escaped) and `"quoted"` (`""`
/// escaped) names, and `--` and nested `/* */` comments; a comment between
/// ORDER and BY still counts. Bug fix 14: whatever follows the last token
/// that isn't `;` (whitespace, semicolons, comments) is dropped before the
/// clause is appended, so a trailing `;` isn't a syntax error and a trailing
/// `--` comment doesn't comment the pagination out.
pub(crate) fn paginate(sql: &str, limit: u64, offset: u64) -> String {
    let s: Vec<char> = sql.chars().collect();
    let n = s.len();
    let starts = |i: usize, pat: &str| {
        let pat: Vec<char> = pat.chars().collect();
        s.get(i..i + pat.len()) == Some(&pat[..])
    };
    let (mut i, mut depth) = (0usize, 0i64);
    // Top-level words, upper-cased; `None` for any other top-level token.
    let mut words: Vec<Option<String>> = Vec::new();
    // End (in chars) of the last significant token that isn't `;`.
    let mut cut = 0usize;
    while i < n {
        let ch = s[i];
        if ch.is_whitespace() {
            i += 1;
        } else if starts(i, "--") {
            i = match s[i..].iter().position(|&c| c == '\n') {
                Some(j) => i + j + 1,
                None => n,
            };
        } else if starts(i, "/*") {
            let mut level = 1;
            i += 2;
            while i < n && level > 0 {
                if starts(i, "/*") {
                    level += 1;
                    i += 2;
                } else if starts(i, "*/") {
                    level -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if matches!(ch, '\'' | '"' | '[') {
            let close = if ch == '[' { ']' } else { ch };
            i += 1;
            while i < n {
                if s[i] == close {
                    if i + 1 < n && s[i + 1] == close {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            cut = i;
            if depth == 0 {
                words.push(None);
            }
        } else if is_word_char(ch) {
            let mut j = i;
            while j < n && is_word_char(s[j]) {
                j += 1;
            }
            if depth == 0 {
                words.push(Some(
                    s[i..j].iter().collect::<String>().to_ascii_uppercase(),
                ));
            }
            i = j;
            cut = j;
        } else {
            if ch == '(' {
                depth += 1;
            } else if ch == ')' {
                depth -= 1;
            }
            if ch != ';' {
                cut = i + 1;
            }
            if depth == 0 && ch != '(' && ch != ')' {
                words.push(None);
            }
            i += 1;
        }
    }
    let has_order = words
        .windows(2)
        .any(|w| w[0].as_deref() == Some("ORDER") && w[1].as_deref() == Some("BY"));
    let body: String = s[..cut].iter().collect();
    let order = if has_order {
        ""
    } else {
        " ORDER BY (SELECT NULL)"
    };
    format!("{body}{order} OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY")
}

impl Dialect for MssqlDialect {
    fn quote_ident(&self, id: &str) -> String {
        qi(id)
    }

    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String {
        paginate(sql, limit, offset)
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
            &at_placeholder,
        )
    }

    fn build_set_default(
        &self,
        schema: &str,
        table: &str,
        column: &str,
        pks: &[String],
        row: &RowValues,
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_set_default(schema, table, column, pks, row, &qi, None, &at_placeholder)
    }

    fn build_insert(
        &self,
        schema: &str,
        table: &str,
        values: &[(String, Value)],
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_insert(schema, table, values, &qi, None, &at_placeholder)
    }

    fn build_delete(
        &self,
        schema: &str,
        table: &str,
        pks: &[String],
        row: &RowValues,
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_delete(schema, table, pks, row, &qi, None, &at_placeholder)
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
            ("VARCHAR", String, true, false),
            ("NVARCHAR", String, true, false),
            ("CHAR", String, true, false),
            ("NCHAR", String, true, false),
            ("TEXT", String, false, false),
            ("NTEXT", String, false, false),
            ("INT", Numeric, false, false),
            ("BIGINT", Numeric, false, false),
            ("SMALLINT", Numeric, false, false),
            ("TINYINT", Numeric, false, false),
            ("DECIMAL", Numeric, false, true),
            ("NUMERIC", Numeric, false, true),
            ("MONEY", Numeric, false, false),
            ("SMALLMONEY", Numeric, false, false),
            ("FLOAT", Numeric, false, false),
            ("REAL", Numeric, false, false),
            ("BIT", Numeric, false, false),
            ("DATE", DateTime, false, false),
            ("DATETIME", DateTime, false, false),
            ("DATETIME2", DateTime, false, false),
            ("SMALLDATETIME", DateTime, false, false),
            ("TIME", DateTime, false, false),
            ("DATETIMEOFFSET", DateTime, false, false),
            ("BINARY", Binary, true, false),
            ("VARBINARY", Binary, true, false),
            ("IMAGE", Binary, false, false),
            ("UNIQUEIDENTIFIER", Uuid, false, false),
            ("XML", Other, false, false),
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

    /// The batches the driver's EXPLAIN runs (bug fix 9), as one script with
    /// `GO` between them: SQL Server takes `SET SHOWPLAN_XML` and
    /// `SET STATISTICS XML` only alone in their batch, so this text doesn't
    /// run as one batch. The driver runs [`introspect::explain_batches`]
    /// one by one on its held connection.
    fn explain_sql(&self, sql: &str, analyze: bool) -> String {
        introspect::explain_batches(sql, analyze).join("\nGO\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_doubles_closing_brackets() {
        assert_eq!(qi("a]b"), "[a]]b]");
        assert_eq!(qi("[x"), "[[x]");
    }

    #[test]
    fn pagination_counts_characters_not_bytes() {
        assert_eq!(
            paginate("SELECT [名前] FROM t; -- é", 5, 0),
            "SELECT [名前] FROM t ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 5 ROWS ONLY"
        );
        assert_eq!(
            paginate("SELECT 名 FROM t ORDER BY 名", 5, 10),
            "SELECT 名 FROM t ORDER BY 名 OFFSET 10 ROWS FETCH NEXT 5 ROWS ONLY"
        );
        // Characters outside the BMP; Unicode whitespace (the TS copy matches).
        assert_eq!(
            paginate("SELECT '🚀' AS [x🚀]\u{85};\u{a0}", 1, 0),
            "SELECT '🚀' AS [x🚀] ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 1 ROWS ONLY"
        );
        // An unclosed string or comment runs to the end.
        assert_eq!(
            paginate("SELECT 'ORDER BY", 1, 0),
            "SELECT 'ORDER BY ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 1 ROWS ONLY"
        );
        assert_eq!(
            paginate("SELECT 1 /* ORDER BY", 1, 0),
            "SELECT 1 ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 1 ROWS ONLY"
        );
    }

    #[test]
    fn collation_is_left_off_for_types_without_one() {
        let base = CreateTableDefinition {
            table_name: "t".into(),
            schema_name: "dbo".into(),
            columns: vec![CreateTableColumn {
                id: "c1".into(),
                name: "c".into(),
                ty: "varchar".into(),
                length: Some("20".into()),
                precision: None,
                nullable: true,
                default_value: String::new(),
                is_primary_key: false,
                is_unique: false,
                collation: Some("Latin1_General_BIN".into()),
                in_unique_constraint: false,
            }],
            indexes: vec![],
            foreign_keys: vec![],
        };
        let mut to = base.clone();
        to.columns[0].ty = "INT".into();
        to.columns[0].length = None;
        assert_eq!(
            MssqlDialect.alter_table(&base, &to),
            "ALTER TABLE [dbo].[t] ALTER COLUMN [c] INT NULL;"
        );
        to.columns[0].ty = "nvarchar".into();
        to.columns[0].length = Some("max".into());
        assert_eq!(
            MssqlDialect.alter_table(&base, &to),
            "ALTER TABLE [dbo].[t] ALTER COLUMN [c] nvarchar(max) COLLATE Latin1_General_BIN NULL;"
        );
    }
}
