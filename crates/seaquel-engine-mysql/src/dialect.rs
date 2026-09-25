//! The MySQL/MariaDB SQL dialect: a port of the pure half of
//! `src/lib/db/mysql.ts`, deleted in phase 2 (quoting, pagination, CRUD and
//! DDL generation, column types, EXPLAIN text). One dialect serves both
//! servers.
//!
//! Output matches the TypeScript byte for byte (see `tests/dialect_parity.rs`)
//! except for bug fixes 2 (the DDL quote doubles backticks, as the CRUD quote
//! already did), 6 (`DROP INDEX … ON schema.table`, `DROP PRIMARY KEY`) and 7
//! (`ALTER COLUMN … SET DEFAULT` / `DROP DEFAULT` for a default-only change).

use seaquel_engine::crud::{self, question_placeholder};
use seaquel_engine::ddl::{self, AlterTableOptions, AlterTableRules, UniqueChanges};
use seaquel_engine::{CastMap, Dialect, RowValues, SqlWithBindings, Value};

use crate::introspect::{self, Flavor};
use seaquel_types::{ColumnCategory, ColumnTypeInfo, CreateTableColumn, CreateTableDefinition};

pub struct MysqlDialect;

/// `` `id` ``, with embedded backticks doubled. Used for CRUD and, since bug
/// fix 2, for DDL too (TypeScript's DDL quote didn't escape).
fn qi(id: &str) -> String {
    format!("`{}`", id.replace('`', "``"))
}

const ALTER_OPTIONS: AlterTableOptions = AlterTableOptions {
    supports_drop_column: true,
    supports_alter_column: true,
    use_modify_column: true,
    qualify_drop_index: false,
    drop_index_on_table: true,
    alter_default_with_modify_column: true,
    qualify_index_name: false,
    supports_add_foreign_key: true,
    unsupported_notes: None,
    inline_foreign_keys_on_added_columns: false,
    constraint_index_prefix: None,
    drop_indexes_first: false,
    tsql: false,
};

/// MySQL adds a UNIQUE constraint and drops one as its index (Task 18).
const ALTER_RULES: AlterTableRules = AlterTableRules {
    not_null_after_add_column: false,
    indexes_block_column_changes: false,
    constraints_block_column_drops: false,
    unique_changes: UniqueChanges::AddAndDropIndex,
};

impl MysqlDialect {
    /// `ALTER TABLE … ADD COLUMN …;` (TS `generateAddColumnSql`). Not on the
    /// `Dialect` trait: no wire request uses it yet.
    pub fn add_column(&self, schema: &str, table: &str, column: &CreateTableColumn) -> String {
        let table = format!("{}.{}", qi(schema), qi(table));
        let opts = ddl::AddColumnOptions {
            column_keyword: true,
            ..Default::default()
        };
        ddl::generate_add_column_ddl(&table, column, &qi, opts)
    }
}

/// The TypeScript adapter never casts (MySQL's `CAST` only takes a few target
/// types, not column types like `tinyint(1)`), so every builder ignores the
/// cast map.
impl Dialect for MysqlDialect {
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
        crud::build_param_set_default(
            schema,
            table,
            column,
            pks,
            row,
            &qi,
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
        crud::build_param_insert(schema, table, values, &qi, None, &question_placeholder)
    }

    fn build_delete(
        &self,
        schema: &str,
        table: &str,
        pks: &[String],
        row: &RowValues,
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        crud::build_param_delete(schema, table, pks, row, &qi, None, &question_placeholder)
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
            ("VARCHAR", String, true, false),
            ("CHAR", String, true, false),
            ("TEXT", String, false, false),
            ("TINYTEXT", String, false, false),
            ("MEDIUMTEXT", String, false, false),
            ("LONGTEXT", String, false, false),
            ("ENUM", String, false, false),
            ("SET", String, false, false),
            ("INT", Numeric, false, false),
            ("TINYINT", Numeric, false, false),
            ("SMALLINT", Numeric, false, false),
            ("MEDIUMINT", Numeric, false, false),
            ("BIGINT", Numeric, false, false),
            ("FLOAT", Numeric, false, false),
            ("DOUBLE", Numeric, false, false),
            ("DECIMAL", Numeric, false, true),
            ("DATE", DateTime, false, false),
            ("DATETIME", DateTime, false, false),
            ("TIMESTAMP", DateTime, false, false),
            ("TIME", DateTime, false, false),
            ("YEAR", DateTime, false, false),
            ("BOOLEAN", Boolean, false, false),
            ("JSON", Json, false, false),
            ("BINARY", Binary, true, false),
            ("VARBINARY", Binary, true, false),
            ("BLOB", Binary, false, false),
            ("TINYBLOB", Binary, false, false),
            ("MEDIUMBLOB", Binary, false, false),
            ("LONGBLOB", Binary, false, false),
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

    /// MySQL's EXPLAIN: `EXPLAIN FORMAT=JSON`, or `EXPLAIN ANALYZE` (a text
    /// tree, MySQL 8.0.18+). One trailing `;` is stripped (TS
    /// `query.replace(/;$/, "")`).
    ///
    /// MySQL only: the dialect is shared by both servers and doesn't know
    /// which one a connection talks to, and MariaDB has no `EXPLAIN ANALYZE`.
    /// Drivers call [`crate::introspect::explain_sql`] with the connection's
    /// flavor (MariaDB analyzes with `ANALYZE FORMAT=JSON`); nothing else
    /// should run this for a MariaDB connection.
    fn explain_sql(&self, sql: &str, analyze: bool) -> String {
        introspect::explain_sql(sql, analyze, Flavor::Mysql)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_doubles_embedded_backticks() {
        assert_eq!(qi("a`b"), "`a``b`");
        assert_eq!(qi("``"), "``````");
    }
}
