//! The DuckDB SQL dialect: a port of the pure half of the demo's `duckdb.ts`
//! (`src/lib/db/duckdb.ts`: quoting, pagination, CRUD and DDL generation,
//! column types, EXPLAIN text). Only the browser demo still uses that file.
//!
//! Output matches the TypeScript byte for byte (see `tests/dialect_parity.rs`)
//! except for these bug fixes (numbered as in `tests/fixtures/bugfixes.json`):
//! - 1: the DDL quote doubles `"`, as the CRUD quote already did.
//! - 4: DuckDB has no `ALTER TABLE … ADD FOREIGN KEY` and rejects constraints
//!   in `ADD COLUMN`, so every added foreign key is a comment line.
//! - 6 (DDL side): a schema as the tree lists it is split quote-aware
//!   ([`DuckdbDialect::quote_schema`]): `fx_aux.main` is `"fx_aux"."main"`.
//! - 8: `DROP INDEX "schema"."index"`; unqualified, DuckDB looks in `main`.
//! - 9: `ADD COLUMN` has no `NOT NULL`: a separate `SET NOT NULL` follows
//!   (or, without a default, a note).
//! - 10: removed indexes are dropped first, and statements DuckDB refuses on
//!   an indexed table or on a PRIMARY KEY/UNIQUE column are notes.
//! - 11: a foreign key in `CREATE TABLE` references the table's own schema
//!   (DuckDB has no cross-schema foreign keys); in an attached catalog it is
//!   a note.
//! - 12: no bare `ARRAY`, `LIST`, `MAP`, `STRUCT` or `UNION` column types.
//!
//! CRUD binds its values with `?` (decision 4 of the phase 2 plan) instead
//! of the adapter's inline literals. The cast map is ignored, as it is for
//! MySQL and MSSQL (the UI sends one for Postgres only): DuckDB casts a
//! bound text key to the column's type itself (DATE, TIMESTAMP, UUID, …).

use seaquel_engine::crud::{self, question_placeholder};
use seaquel_engine::ddl::{self, AlterTableOptions, AlterTableRules, UniqueChanges};
use seaquel_engine::{CastMap, Dialect, RowValues, SqlWithBindings, Value};
use seaquel_types::{ColumnCategory, ColumnTypeInfo, CreateTableColumn, CreateTableDefinition};

use crate::introspect;

pub struct DuckdbDialect;

/// `"id"`, with embedded `"` doubled.
pub(crate) fn qi(id: &str) -> String {
    format!("\"{}\"", id.replace('"', "\"\""))
}

/// The parts of a dotted name in DuckDB's identifier syntax: `"…"` parts
/// with `""` for `"`, bare parts up to the next dot. `None` when it doesn't
/// parse (an unclosed quote, a quoted part not followed by a dot, an empty
/// name or a trailing dot).
pub fn parse_dotted(name: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = name.chars().collect();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let mut part = String::new();
        if chars[i] == '"' {
            i += 1;
            loop {
                let c = *chars.get(i)?;
                if c == '"' {
                    if chars.get(i + 1) == Some(&'"') {
                        part.push('"');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                part.push(c);
                i += 1;
            }
        } else {
            while i < chars.len() && chars[i] != '.' {
                part.push(chars[i]);
                i += 1;
            }
        }
        parts.push(part);
        if i < chars.len() {
            if chars[i] != '.' {
                return None;
            }
            i += 1;
            if i == chars.len() {
                return None;
            }
        }
    }
    (!parts.is_empty()).then_some(parts)
}

/// Bug fix 6's DDL side: a schema as the tree lists it, quoted. `main` is
/// `"main"`, `"fx.a.b"` is `"fx.a.b"` and `fx_aux.main` is
/// `"fx_aux"."main"`. Anything that doesn't parse as one or two parts is
/// quoted whole.
pub fn quote_schema(schema: &str) -> String {
    match parse_dotted(schema) {
        Some(parts) if parts.len() <= 2 => {
            parts.iter().map(|p| qi(p)).collect::<Vec<_>>().join(".")
        }
        _ => qi(schema),
    }
}

/// `"schema"."table"` for a schema as the tree lists it (see
/// [`quote_schema`]). The TypeScript mirror is `duckdbQualifiedTable` in
/// `src/lib/engine/qualified-table.ts`.
pub fn qualified_table(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_schema(schema), qi(table))
}

/// The attached catalog a listed schema names (`fx_aux` for `fx_aux.main`),
/// or `None` for a schema of the default catalog.
fn catalog_of(schema: &str) -> Option<String> {
    parse_dotted(schema)
        .filter(|parts| parts.len() == 2)
        .map(|mut parts| parts.swap_remove(0))
}

const ALTER_OPTIONS: AlterTableOptions = AlterTableOptions {
    supports_drop_column: true,
    supports_alter_column: true,
    use_modify_column: false,
    qualify_drop_index: true,
    drop_index_on_table: false,
    alter_default_with_modify_column: false,
    qualify_index_name: false,
    supports_add_foreign_key: false,
    unsupported_notes: Some("DuckDB"),
    inline_foreign_keys_on_added_columns: false,
    constraint_index_prefix: None,
    drop_indexes_first: true,
    tsql: false,
};

const ALTER_RULES: AlterTableRules = AlterTableRules {
    not_null_after_add_column: true,
    indexes_block_column_changes: true,
    constraints_block_column_drops: true,
    // DuckDB has no ALTER TABLE ADD/DROP CONSTRAINT (Task 18).
    unique_changes: UniqueChanges::Notes,
};

impl DuckdbDialect {
    /// `ALTER TABLE … ADD COLUMN …;` (TS `generateAddColumnSql`). Bug fix 9:
    /// no `NOT NULL` in `ADD COLUMN`; a NOT NULL column gets its own
    /// `SET NOT NULL` when it has a default, else a note (see
    /// [`ddl::AddColumnOptions::not_null_after`]). Not on the `Dialect`
    /// trait: no wire request uses it yet.
    pub fn add_column(&self, schema: &str, table: &str, column: &CreateTableColumn) -> String {
        let opts = ddl::AddColumnOptions {
            column_keyword: true,
            not_null_after: Some("DuckDB"),
        };
        ddl::generate_add_column_ddl(&qualified_table(schema, table), column, &qi, opts)
    }
}

/// TS `generateCreateTableDdl` with bug fixes 1, 6 and 11: a foreign key
/// references the table's own schema whatever the definition says (DuckDB
/// rejects cross-schema foreign keys, and `REFERENCES ""."t"` doesn't
/// parse), and in an attached catalog it is a note after the statements:
/// DuckDB can't create one there from the default catalog.
fn create_table(def: &CreateTableDefinition) -> String {
    let table = qualified_table(&def.schema_name, &def.table_name);
    let mut lines: Vec<String> = def
        .columns
        .iter()
        .map(|c| ddl::build_column_line(c, &qi))
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
    let catalog = catalog_of(&def.schema_name);
    let mut notes = Vec::new();
    for fk in &def.foreign_keys {
        let clause = format!(
            "({}) REFERENCES {} ({})",
            qi(&fk.column),
            qualified_table(&def.schema_name, &fk.referenced_table),
            qi(&fk.referenced_column)
        );
        match &catalog {
            Some(catalog) => notes.push(ddl::note(&format!(
                "DuckDB can't create a foreign key in attached catalog {} from here: {clause}; create the table after USE {} to add it",
                qi(catalog),
                qi(catalog)
            ))),
            None => lines.push(format!("  FOREIGN KEY {clause}")),
        }
    }
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
    for n in notes {
        sql.push('\n');
        sql.push_str(&n);
    }
    sql
}

impl Dialect for DuckdbDialect {
    fn quote_ident(&self, id: &str) -> String {
        qi(id)
    }

    fn quote_schema(&self, schema: &str) -> String {
        quote_schema(schema)
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
        crud::build_param_update_qs(
            schema,
            table,
            column,
            value,
            pks,
            row,
            &qi,
            &quote_schema,
            None,
            &question_placeholder,
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
        crud::build_param_set_default_qs(
            schema,
            table,
            column,
            pks,
            row,
            &qi,
            &quote_schema,
            None,
            &question_placeholder,
        )
    }

    fn build_insert(
        &self,
        schema: &str,
        table: &str,
        values: &[(String, Value)],
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_insert_qs(
            schema,
            table,
            values,
            &qi,
            &quote_schema,
            None,
            &question_placeholder,
        )
    }

    fn build_delete(
        &self,
        schema: &str,
        table: &str,
        pks: &[String],
        row: &RowValues,
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_delete_qs(
            schema,
            table,
            pks,
            row,
            &qi,
            &quote_schema,
            None,
            &question_placeholder,
        )
    }

    fn create_table(&self, def: &CreateTableDefinition) -> String {
        create_table(def)
    }

    fn alter_table(&self, from: &CreateTableDefinition, to: &CreateTableDefinition) -> String {
        ddl::generate_alter_table_sql_with(from, to, &qi, &quote_schema, ALTER_OPTIONS, ALTER_RULES)
    }

    /// TS `getColumnTypes`, without the bare `ARRAY`, `LIST`, `MAP`,
    /// `STRUCT` and `UNION` (bug fix 12: DuckDB needs their type arguments;
    /// full types still come through the table editor's SQL pane).
    fn column_types(&self) -> Vec<ColumnTypeInfo> {
        use ColumnCategory::*;
        // (name, category, has_length, has_precision)
        const TYPES: &[(&str, ColumnCategory, bool, bool)] = &[
            ("VARCHAR", String, true, false),
            ("TEXT", String, false, false),
            ("INTEGER", Numeric, false, false),
            ("BIGINT", Numeric, false, false),
            ("HUGEINT", Numeric, false, false),
            ("SMALLINT", Numeric, false, false),
            ("TINYINT", Numeric, false, false),
            ("UINTEGER", Numeric, false, false),
            ("UBIGINT", Numeric, false, false),
            ("UHUGEINT", Numeric, false, false),
            ("USMALLINT", Numeric, false, false),
            ("UTINYINT", Numeric, false, false),
            ("DOUBLE", Numeric, false, false),
            ("FLOAT", Numeric, false, false),
            ("DECIMAL", Numeric, false, true),
            ("BIGNUM", Numeric, false, false),
            ("DATE", DateTime, false, false),
            ("TIME", DateTime, false, false),
            ("TIMESTAMP", DateTime, false, false),
            ("TIMESTAMP WITH TIME ZONE", DateTime, false, false),
            ("INTERVAL", DateTime, false, false),
            ("BOOLEAN", Boolean, false, false),
            ("JSON", Json, false, false),
            ("BLOB", Binary, false, false),
            ("BIT", Binary, false, false),
            ("UUID", Uuid, false, false),
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

    fn explain_sql(&self, sql: &str, analyze: bool) -> String {
        introspect::explain_sql(sql, analyze)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_names_parse_quote_aware() {
        let p = |s: &str| parse_dotted(s);
        let v = |parts: &[&str]| Some(parts.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(p("main"), v(&["main"]));
        assert_eq!(p("fx_aux.main"), v(&["fx_aux", "main"]));
        assert_eq!(p("\"fx.a.b\""), v(&["fx.a.b"]));
        assert_eq!(p("\"fx.we\"\"ird\".main"), v(&["fx.we\"ird", "main"]));
        assert_eq!(p("a.b.c"), v(&["a", "b", "c"]));
        assert_eq!(
            p("seaquel_test.fx_sales.\"fx.dotted \"\"t\"\"\""),
            v(&["seaquel_test", "fx_sales", "fx.dotted \"t\""])
        );
        assert_eq!(p(".a"), v(&["", "a"]));
        for bad in ["", "a.", "\"open", "\"a\"b", "\"a\".", "a..b."] {
            assert_eq!(p(bad), None, "{bad}");
        }
    }

    #[test]
    fn schemas_are_quoted_part_by_part() {
        assert_eq!(quote_schema("main"), "\"main\"");
        assert_eq!(quote_schema("\"a.b\""), "\"a.b\"");
        assert_eq!(quote_schema("fx_aux.main"), "\"fx_aux\".\"main\"");
        assert_eq!(
            quote_schema("\"fx.we\"\"ird\".main"),
            "\"fx.we\"\"ird\".\"main\""
        );
        // Three parts, or no parse: quoted whole.
        assert_eq!(quote_schema("a.b.c"), "\"a.b.c\"");
        assert_eq!(quote_schema("say \"hi"), "\"say \"\"hi\"");
        assert_eq!(
            qualified_table("fx_aux.main", "it's \"x\""),
            "\"fx_aux\".\"main\".\"it's \"\"x\"\"\""
        );
        assert_eq!(catalog_of("fx_aux.main").as_deref(), Some("fx_aux"));
        assert_eq!(catalog_of("\"fx_aux.main\""), None);
        assert_eq!(catalog_of("main"), None);
    }

    #[test]
    fn crud_binds_with_question_marks_in_attached_catalogs() {
        let d = DuckdbDialect;
        let row = vec![("id".to_string(), Value::Int(1))];
        let pks = vec!["id".to_string()];
        let out = d.build_update(
            "fx_aux.main",
            "users",
            "nickname",
            Value::from("x"),
            &pks,
            &row,
            None,
        );
        assert_eq!(
            out.sql,
            "UPDATE \"fx_aux\".\"main\".\"users\" SET \"nickname\" = ? WHERE \"id\" = ?"
        );
        assert_eq!(out.bind_values, Some(vec![Value::from("x"), Value::Int(1)]));
        assert_eq!(
            d.build_delete("\"fx.a.b\"", "t", &pks, &row, None).sql,
            "DELETE FROM \"fx.a.b\".\"t\" WHERE \"id\" = ?"
        );
        assert_eq!(
            d.build_set_default("main", "t", "c", &pks, &row, None).sql,
            "UPDATE \"main\".\"t\" SET \"c\" = DEFAULT WHERE \"id\" = ?"
        );
        assert_eq!(
            d.build_insert("main", "t", &row, None).sql,
            "INSERT INTO \"main\".\"t\" (\"id\") VALUES (?)"
        );
    }

    #[test]
    fn crud_ignores_casts() {
        let d = DuckdbDialect;
        let casts: CastMap = [("id".to_string(), "INTEGER".to_string())]
            .into_iter()
            .collect();
        let row = vec![("id".to_string(), Value::Int(1))];
        let pks = vec!["id".to_string()];
        for built in [
            d.build_update("s", "t", "id", Value::Int(2), &pks, &row, Some(&casts)),
            d.build_set_default("s", "t", "id", &pks, &row, Some(&casts)),
            d.build_delete("s", "t", &pks, &row, Some(&casts)),
            d.build_insert("s", "t", &row, Some(&casts)),
        ] {
            assert!(!built.sql.contains("CAST"), "{}", built.sql);
        }
    }
}
