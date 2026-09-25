//! The [`Dialect`] trait: an engine's pure SQL knowledge (quoting, pagination,
//! CRUD and DDL generation, column types, EXPLAIN text). No I/O, so it builds
//! for wasm32 and can run in the browser.

use std::collections::HashMap;

use seaquel_runtime::{MaybeSend, MaybeSync};
use seaquel_types::{ColumnTypeInfo, CreateTableDefinition, SqlWithBindings, Value};

/// Column → declared type, for wrapping a placeholder in `CAST(… AS type)`.
///
/// The types are trusted input: they are interpolated into the SQL as they
/// are, not quoted or bound. They must come from the engine's own catalog
/// (e.g. Postgres `castType`, which quotes identifiers with `quote_ident`),
/// never from user-typed text.
pub type CastMap = HashMap<String, String>;

/// A row as column → value pairs, in the order the TypeScript row object had.
pub type RowValues = Vec<(String, Value)>;

pub trait Dialect: MaybeSend + MaybeSync {
    fn quote_ident(&self, id: &str) -> String;

    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String;

    /// `casts` wraps the value placeholder and, since bug fix 6, the
    /// primary-key placeholders.
    #[allow(clippy::too_many_arguments)]
    fn build_update(
        &self,
        schema: &str,
        table: &str,
        column: &str,
        value: Value,
        pks: &[String],
        row: &RowValues,
        casts: Option<&CastMap>,
    ) -> SqlWithBindings;

    /// `casts` wraps the primary-key placeholders (bug fix 6).
    fn build_set_default(
        &self,
        schema: &str,
        table: &str,
        column: &str,
        pks: &[String],
        row: &RowValues,
        casts: Option<&CastMap>,
    ) -> SqlWithBindings;

    fn build_insert(
        &self,
        schema: &str,
        table: &str,
        values: &[(String, Value)],
        casts: Option<&CastMap>,
    ) -> SqlWithBindings;

    /// `casts` wraps the primary-key placeholders (bug fix 6).
    fn build_delete(
        &self,
        schema: &str,
        table: &str,
        pks: &[String],
        row: &RowValues,
        casts: Option<&CastMap>,
    ) -> SqlWithBindings;

    fn create_table(&self, def: &CreateTableDefinition) -> String;

    fn alter_table(&self, from: &CreateTableDefinition, to: &CreateTableDefinition) -> String;

    fn column_types(&self) -> Vec<ColumnTypeInfo>;

    fn explain_sql(&self, sql: &str, analyze: bool) -> String;
}
