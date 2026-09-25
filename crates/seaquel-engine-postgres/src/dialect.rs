//! The Postgres SQL dialect: a port of the pure half of
//! `src/lib/db/postgres.ts`, deleted in phase 1 (quoting, pagination, CRUD
//! and DDL generation, column types, EXPLAIN text).
//!
//! Output matches the TypeScript byte for byte (see `tests/dialect_parity.rs`)
//! except for agreed bug fixes: the DDL quote escapes embedded `"` (fix 2),
//! `ALTER TABLE` schema-qualifies `DROP INDEX` (fix 3), and CRUD casts
//! primary-key placeholders from the cast map (fix 6).

use seaquel_engine::crud::{self, dollar_placeholder};
use seaquel_engine::ddl::{self, AlterTableOptions, AlterTableRules, UniqueChanges};
use seaquel_engine::{CastMap, Dialect, RowValues, SqlWithBindings, Value};
use seaquel_types::{ColumnCategory, ColumnTypeInfo, CreateTableDefinition};

pub struct PostgresDialect;

/// `"id"`, with embedded `"` doubled. Used for CRUD and, since bug fix 2,
/// for DDL too (TypeScript's DDL quote didn't escape).
fn qi(id: &str) -> String {
    format!("\"{}\"", id.replace('"', "\"\""))
}

const ALTER_OPTIONS: AlterTableOptions = AlterTableOptions {
    supports_drop_column: true,
    supports_alter_column: true,
    use_modify_column: false,
    qualify_drop_index: true,
    drop_index_on_table: false,
    alter_default_with_modify_column: false,
    qualify_index_name: false,
    supports_add_foreign_key: true,
    unsupported_notes: None,
    inline_foreign_keys_on_added_columns: false,
    constraint_index_prefix: None,
    drop_indexes_first: false,
    tsql: false,
};

/// Postgres adds a UNIQUE constraint and drops one (or a unique index) by the
/// name the catalog gives (Task 18).
const ALTER_RULES: AlterTableRules = AlterTableRules {
    not_null_after_add_column: false,
    indexes_block_column_changes: false,
    constraints_block_column_drops: false,
    unique_changes: UniqueChanges::AddAndDropPostgres,
};

impl Dialect for PostgresDialect {
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
        casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_update(
            schema,
            table,
            column,
            value,
            pks,
            row,
            &qi,
            casts,
            &dollar_placeholder,
        )
    }

    fn build_set_default(
        &self,
        schema: &str,
        table: &str,
        column: &str,
        pks: &[String],
        row: &RowValues,
        casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_set_default(
            schema,
            table,
            column,
            pks,
            row,
            &qi,
            casts,
            &dollar_placeholder,
        )
    }

    fn build_insert(
        &self,
        schema: &str,
        table: &str,
        values: &[(String, Value)],
        casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_insert(schema, table, values, &qi, casts, &dollar_placeholder)
    }

    fn build_delete(
        &self,
        schema: &str,
        table: &str,
        pks: &[String],
        row: &RowValues,
        casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_delete(schema, table, pks, row, &qi, casts, &dollar_placeholder)
    }

    fn create_table(&self, def: &CreateTableDefinition) -> String {
        ddl::generate_create_table_ddl(def, &qi)
    }

    fn alter_table(&self, from: &CreateTableDefinition, to: &CreateTableDefinition) -> String {
        ddl::generate_alter_table_sql_with(from, to, &qi, &qi, ALTER_OPTIONS, ALTER_RULES)
    }

    fn column_types(&self) -> Vec<ColumnTypeInfo> {
        use ColumnCategory::*;
        // (name, category, has_length, has_precision)
        const TYPES: &[(&str, ColumnCategory, bool, bool)] = &[
            ("text", String, false, false),
            ("varchar", String, true, false),
            ("char", String, true, false),
            ("integer", Numeric, false, false),
            ("bigint", Numeric, false, false),
            ("smallint", Numeric, false, false),
            ("serial", Numeric, false, false),
            ("bigserial", Numeric, false, false),
            ("numeric", Numeric, false, true),
            ("real", Numeric, false, false),
            ("double precision", Numeric, false, false),
            ("money", Numeric, false, false),
            ("date", DateTime, false, false),
            ("time", DateTime, false, false),
            ("timestamp", DateTime, false, false),
            ("timestamptz", DateTime, false, false),
            ("interval", DateTime, false, false),
            ("boolean", Boolean, false, false),
            ("json", Json, false, false),
            ("jsonb", Json, false, false),
            ("bytea", Binary, false, false),
            ("uuid", Uuid, false, false),
            ("inet", Network, false, false),
            ("cidr", Network, false, false),
            ("macaddr", Network, false, false),
            ("point", Other, false, false),
            ("line", Other, false, false),
            ("polygon", Other, false, false),
            ("xml", Other, false, false),
            ("tsvector", Other, false, false),
            ("tsquery", Other, false, false),
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

    /// `EXPLAIN (FORMAT JSON)`, with `ANALYZE` when asked. One trailing `;` is
    /// stripped (TS: `query.replace(/;$/, "")`, which only matches at the very
    /// end); inner semicolons and trailing whitespace are kept.
    fn explain_sql(&self, sql: &str, analyze: bool) -> String {
        let base = sql.strip_suffix(';').unwrap_or(sql);
        if analyze {
            format!("EXPLAIN (ANALYZE, FORMAT JSON) {base}")
        } else {
            format!("EXPLAIN (FORMAT JSON) {base}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_doubles_embedded_quotes() {
        // Bug fix 2 at the unit level; the fixtures cover full statements.
        assert_eq!(qi(r#"a"b"#), r#""a""b""#);
    }

    #[test]
    fn explain_strips_only_a_final_semicolon() {
        let d = PostgresDialect;
        assert_eq!(
            d.explain_sql("SELECT 1;;", false),
            "EXPLAIN (FORMAT JSON) SELECT 1;"
        );
        assert_eq!(
            d.explain_sql("SELECT 1;\n", false),
            "EXPLAIN (FORMAT JSON) SELECT 1;\n"
        );
    }
}
