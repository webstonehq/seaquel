//! Generic DDL generation: a port of `src/lib/db/alter-table.ts`.
//!
//! Every function is parameterized the way the TypeScript was: by a quote
//! function and, for `ALTER TABLE`, by [`AlterTableOptions`]. There are no
//! per-dialect branches here. Output is byte-for-byte what the TypeScript
//! produced (statement order, spacing, newlines, `-- No changes detected`),
//! except where a dialect opts into [`AlterTableOptions::qualify_drop_index`]
//! (Postgres), [`AlterTableOptions::drop_index_on_table`] or
//! [`AlterTableOptions::alter_default_with_modify_column`] (MySQL).

use std::collections::{HashMap, HashSet};

use seaquel_types::{CreateTableColumn, CreateTableDefinition, CreateTableForeignKey};

/// Quotes one identifier (`name` → `"name"`, `` `name` ``, `[name]`, …).
pub type QuoteFn<'a> = &'a dyn Fn(&str) -> String;

/// The dialect switches `generateAlterTableSql` took in TypeScript, plus the
/// bug-fix switches `qualify_drop_index`, `drop_index_on_table` and
/// `alter_default_with_modify_column`, which default to the TypeScript behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlterTableOptions {
    /// Some DBs (SQLite) don't support `DROP COLUMN`.
    pub supports_drop_column: bool,
    /// Some DBs (SQLite) don't support `ALTER COLUMN`.
    pub supports_alter_column: bool,
    /// Some DBs use `MODIFY COLUMN` instead of `ALTER COLUMN` (MySQL).
    pub use_modify_column: bool,
    /// Emit `DROP INDEX schema.name;` instead of `DROP INDEX name;`. The
    /// schema is the original definition's: that's where the index lives,
    /// even if the same edit moves the table. Postgres turns this on (bug fix 3);
    /// the TypeScript never qualified it, so it defaults to off.
    pub qualify_drop_index: bool,
    /// Emit `DROP INDEX name ON schema.table;`, and `ALTER TABLE schema.table
    /// DROP PRIMARY KEY;` for an index named `PRIMARY` (MySQL bug fix 6: MySQL
    /// has no bare `DROP INDEX name`). The table is the original definition's,
    /// where the index lives. Takes precedence over `qualify_drop_index`.
    pub drop_index_on_table: bool,
    /// With `use_modify_column`, a change to only the default emits
    /// `ALTER COLUMN … SET DEFAULT …` / `DROP DEFAULT` (MySQL bug fix 7). The
    /// TypeScript dropped such edits: `MODIFY` fired only for a type or
    /// nullability change, and the `SET DEFAULT` branch was skipped under
    /// `use_modify_column`. `SET DEFAULT` leaves the rest of the column
    /// (`ON UPDATE`, `COMMENT`, collation) alone, which a `MODIFY` rebuilt
    /// from the editor's definition would drop. When `MODIFY` fires anyway, it
    /// carries the new default, as before.
    pub alter_default_with_modify_column: bool,
    /// Emit `CREATE INDEX schema.name ON table (…)` instead of
    /// `CREATE INDEX name ON schema.table (…)` for an added index (SQLite bug
    /// fix 3: SQLite rejects a schema-qualified table there and takes the
    /// schema on the index name). Uses the updated definition's schema.
    pub qualify_index_name: bool,
    /// The engine has `ALTER TABLE … ADD FOREIGN KEY`. When it doesn't
    /// (SQLite), an added foreign key is a note (see `unsupported_notes`)
    /// instead of invalid SQL (SQLite bug fix 4).
    pub supports_add_foreign_key: bool,
    /// The engine's name, for comment lines about edits it can't make with
    /// `ALTER TABLE`: with `!supports_alter_column`, a column whose type,
    /// nullability or default changed (SQLite bug fix 6; the TypeScript
    /// dropped such edits silently), and with `!supports_add_foreign_key`, an
    /// added foreign key. The notes follow every statement, one per line, so
    /// a client that splits the script on `;\n` and skips `--` lines still
    /// runs every statement. `None` keeps the TypeScript behaviour (the edit
    /// is dropped without a word).
    pub unsupported_notes: Option<&'static str>,
    /// With `!supports_add_foreign_key`, a foreign key on a column added in
    /// the same edit goes inline: `ADD COLUMN c … REFERENCES t (id)` (SQLite
    /// accepts that when the column's default is NULL, so only a column
    /// without a default gets it; others keep the note). The table is
    /// unqualified, as SQLite's REFERENCES clause requires.
    pub inline_foreign_keys_on_added_columns: bool,
    /// Indexes whose names start with this prefix belong to a constraint and
    /// can't be dropped (SQLite's `sqlite_autoindex_*`, behind UNIQUE and
    /// PRIMARY KEY; SQLite reserves the `sqlite_` prefix, so the name alone
    /// tells). Removing one from the definition is a note (see
    /// `unsupported_notes`) instead of a `DROP INDEX` the engine rejects.
    pub constraint_index_prefix: Option<&'static str>,
    /// Emit the dropped indexes first, before the renames and column changes
    /// (MSSQL bug fix 13: SQL Server refuses to alter or drop a column an
    /// index uses). Off keeps the TypeScript order, drops after the column
    /// changes.
    pub drop_indexes_first: bool,
    /// T-SQL statement forms (MSSQL bug fixes 7, 12, 13 and 15; see
    /// [`tsql`]): a rename is `EXEC sp_rename`, an added column is `ADD`
    /// with an explicit `NULL`/`NOT NULL`, a type or nullability change is
    /// one `ALTER COLUMN c <type> [COLLATE …] NULL|NOT NULL`, defaults are
    /// constraints found through the catalog (a default change and every
    /// `DROP COLUMN` drop the column's default constraint first), and a
    /// dropped index may be a PRIMARY KEY or UNIQUE constraint's. The quote
    /// fn must be T-SQL's `[…]`.
    pub tsql: bool,
}

impl Default for AlterTableOptions {
    fn default() -> Self {
        Self {
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
            drop_indexes_first: false,
            tsql: false,
        }
    }
}

/// More switches for [`generate_alter_table_sql_with`], each off by default
/// (the TypeScript behaviour). They turn statements the engine would reject
/// into notes (comment lines that carry the statement, worded with
/// [`AlterTableOptions::unsupported_notes`]'s engine name) so that the rest
/// of the script runs. DuckDB turns all three on (bug fixes 9 and 10).
///
/// Kept apart from [`AlterTableOptions`], which every engine builds as an
/// exhaustive `const`, so adding a switch here doesn't touch the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AlterTableRules {
    /// `ADD COLUMN` carries no `NOT NULL` (DuckDB rejects any constraint
    /// there). A NOT NULL column with a default is made NOT NULL by its own
    /// `ALTER COLUMN … SET NOT NULL` after the `ADD COLUMN` (the default fills
    /// the existing rows); without a default that fails on a table with rows,
    /// so the column stays nullable and a note says to fill it and run the
    /// `SET NOT NULL`.
    pub not_null_after_add_column: bool,
    /// While the table keeps indexes (those of the original definition that
    /// the updated one still has), `RENAME COLUMN`, `DROP COLUMN`,
    /// `ALTER COLUMN … TYPE` and `SET`/`DROP NOT NULL` are notes naming the
    /// indexes to drop first: DuckDB refuses them on a table with any index,
    /// on whatever column. A later statement on a column whose rename became
    /// a note (its default, a new index on it) is a note too.
    pub indexes_block_column_changes: bool,
    /// `DROP COLUMN` and `ALTER COLUMN … TYPE` of a column in a PRIMARY KEY or
    /// UNIQUE constraint (`is_primary_key`, `is_unique`, or
    /// `in_unique_constraint` for a composite one), and `DROP COLUMN` of
    /// a column that comes before one in the original column order, are
    /// notes: DuckDB backs those constraints with an index that depends on
    /// the column, or on a column after it.
    pub constraints_block_column_drops: bool,
    /// How a change to a column's own UNIQUE (`is_unique`, on a column that
    /// isn't in the primary key) is emitted. The TypeScript dropped it.
    pub unique_changes: UniqueChanges,
}

/// [`AlterTableRules::unique_changes`]: a column's UNIQUE checked (on an
/// existing or an added column) or unchecked. The checkbox stands for a
/// single-column UNIQUE constraint or unique index on that column (what the
/// drivers' `apply_unique_indexes` reports); composite ones
/// (`in_unique_constraint`) are edited as indexes. Unchecking drops every
/// one on the column, unless the same edit removes its index under Indexes
/// (that drop is the index section's). A dropped UNIQUE comes before the
/// column changes (SQL Server refuses to alter a column in one), an added
/// one after them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UniqueChanges {
    /// Not emitted (the TypeScript).
    #[default]
    Ignore,
    /// Notes both ways: the engine can't add or drop a UNIQUE constraint on
    /// an existing table (DuckDB).
    Notes,
    /// Adding is a note (the engine has no `ADD UNIQUE`); unchecking drops
    /// the column's unique index from the definition's indexes with
    /// `DROP INDEX schema.name`, and is a note when it is a constraint's
    /// ([`AlterTableOptions::constraint_index_prefix`]) or there is none
    /// (SQLite).
    DropIndex,
    /// `ALTER TABLE t ADD UNIQUE (c);`, and unchecking drops the column's
    /// unique index from the definition's indexes with `DROP INDEX name ON
    /// table` (a note when there is none) (MySQL: a UNIQUE constraint is
    /// its index).
    AddAndDropIndex,
    /// `ADD UNIQUE`, and a drop that finds the column's single-column
    /// unique indexes in `pg_index` and drops each as its constraint or as
    /// an index, in a `DO` block (Postgres).
    AddAndDropPostgres,
    /// `ADD UNIQUE`, and a drop that finds them in `sys.indexes` and drops
    /// each as its constraint or as an index with dynamic SQL, like the
    /// `@dfN` default lookup (SQL Server).
    AddAndDropTsql,
}

/// Drops the single-column UNIQUE constraints and unique indexes of `column`
/// on `table` (quoted `"s"."t"`) on Postgres, leaving partial and INCLUDE
/// indexes alone (they aren't the checkbox's): an anonymous `DO` block, since
/// their names come from the catalog. The dollar tag is one the body
/// doesn't contain.
pub fn pg_drop_unique(table: &str, column: &str) -> String {
    let lit = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let body = format!(
        "DECLARE r record; BEGIN FOR r IN SELECT x.indexrelid::regclass AS idx, k.conname FROM pg_index x \
         LEFT JOIN pg_constraint k ON k.conindid = x.indexrelid AND k.conrelid = x.indrelid \
         WHERE x.indrelid = {t}::regclass AND x.indisunique AND NOT x.indisprimary AND x.indnkeyatts = 1 AND x.indnatts = 1 AND x.indpred IS NULL \
         AND x.indkey[0] = (SELECT a.attnum FROM pg_attribute a WHERE a.attrelid = x.indrelid AND a.attname = {c}) \
         LOOP IF r.conname IS NULL THEN EXECUTE format('DROP INDEX %s', r.idx); \
         ELSE EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', {t}, r.conname); END IF; END LOOP; END",
        t = lit(table),
        c = lit(column),
    );
    let mut tag = "$sq$".to_string();
    let mut n = 0;
    while body.contains(&tag) {
        n += 1;
        tag = format!("$sq{n}$");
    }
    format!("DO {tag} {body} {tag};")
}

/// T-SQL pieces of [`generate_alter_table_sql`] with
/// [`AlterTableOptions::tsql`]. Each returned statement is one piece: it has
/// no `;\n` inside, so a client that splits the script on `;\n` keeps it
/// whole, and the `@dfN` variables are numbered so the whole script also
/// runs as one batch.
pub mod tsql {
    /// `N'text'`, with `'` doubled.
    pub fn literal(text: &str) -> String {
        format!("N'{}'", text.replace('\'', "''"))
    }

    /// Looks up the default constraint of `column` on `table` (quoted
    /// `[s].[t]`) and drops it with dynamic SQL, if there is one. `EXEC ()`
    /// can't take a function call, so the statement goes into `@df{n}`
    /// first.
    pub fn drop_default(n: usize, table: &str, column: &str) -> String {
        format!(
            "DECLARE @df{n} nvarchar(max) = (SELECT {} + QUOTENAME(d.name) \
             FROM sys.default_constraints d INNER JOIN sys.columns c ON c.object_id = d.parent_object_id AND c.column_id = d.parent_column_id \
             WHERE d.parent_object_id = OBJECT_ID({}) AND c.name = {}); \
             IF @df{n} IS NOT NULL EXEC (@df{n});",
            literal(&format!("ALTER TABLE {table} DROP CONSTRAINT ")),
            literal(table),
            literal(column),
        )
    }

    /// Drops the single-column UNIQUE constraints and unique indexes of
    /// `column` on `table` (quoted `[s].[t]`), if there are any, leaving
    /// filtered and INCLUDE indexes alone: their
    /// `DROP CONSTRAINT`s and `DROP INDEX`es go into `@uq{n}` and run with
    /// `EXEC ()`, as in [`drop_default`].
    pub fn drop_unique(n: usize, table: &str, column: &str) -> String {
        format!(
            "DECLARE @uq{n} nvarchar(max) = N''; \
             SELECT @uq{n} = @uq{n} + CASE WHEN i.is_unique_constraint = 1 THEN {} + QUOTENAME(i.name) + N'; ' \
             ELSE N'DROP INDEX ' + QUOTENAME(i.name) + {} END FROM sys.indexes i \
             WHERE i.object_id = OBJECT_ID({}) AND i.is_unique = 1 AND i.is_primary_key = 0 AND i.has_filter = 0 \
             AND (SELECT COUNT(*) FROM sys.index_columns ic WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id) = 1 \
             AND EXISTS (SELECT 1 FROM sys.index_columns ic INNER JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
             WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.is_included_column = 0 AND c.name = {}); \
             IF @uq{n} <> N'' EXEC (@uq{n});",
            literal(&format!("ALTER TABLE {table} DROP CONSTRAINT ")),
            literal(&format!(" ON {table}; ")),
            literal(table),
            literal(column),
        )
    }

    /// Drops index `name` (quoted as `quoted_name`) on `table`: as a
    /// constraint when it is a PRIMARY KEY or UNIQUE constraint's (which
    /// `DROP INDEX` refuses), else as an index. The definition can't tell
    /// which, so the catalog decides.
    pub fn drop_index(table: &str, name: &str, quoted_name: &str) -> String {
        format!(
            "IF EXISTS (SELECT 1 FROM sys.key_constraints WHERE parent_object_id = OBJECT_ID({}) AND name = {}) \
             ALTER TABLE {table} DROP CONSTRAINT {quoted_name} ELSE DROP INDEX {quoted_name} ON {table};",
            literal(table),
            literal(name),
        )
    }

    /// Built-in types without a collation. `COLLATE` is left off for these,
    /// so a column retyped from text to one of them doesn't carry its old
    /// collation into an invalid statement.
    const NO_COLLATION: &[&str] = &[
        "bigint",
        "int",
        "smallint",
        "tinyint",
        "bit",
        "decimal",
        "numeric",
        "money",
        "smallmoney",
        "float",
        "real",
        "date",
        "time",
        "datetime",
        "datetime2",
        "smalldatetime",
        "datetimeoffset",
        "binary",
        "varbinary",
        "image",
        "uniqueidentifier",
        "xml",
        "rowversion",
        "timestamp",
        "hierarchyid",
        "geography",
        "geometry",
        "sql_variant",
    ];

    /// ` COLLATE name` for a column of type `ty` with `collation`, or `""`.
    /// `ty` may carry its size inline (`decimal(10,2)`, `varchar (20)`).
    pub fn collate(ty: &str, collation: Option<&str>) -> String {
        let base = ty
            .split('(')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        match collation.filter(|c| !c.is_empty()) {
            Some(c) if !NO_COLLATION.contains(&base.as_str()) => format!(" COLLATE {c}"),
            _ => String::new(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::collate;

        #[test]
        fn collate_skips_types_without_a_collation() {
            let bin = Some("Latin1_General_BIN");
            assert_eq!(collate("varchar", bin), " COLLATE Latin1_General_BIN");
            assert_eq!(collate("NVARCHAR(max)", bin), " COLLATE Latin1_General_BIN");
            assert_eq!(collate("char (3)", bin), " COLLATE Latin1_General_BIN");
            for ty in [
                "INT",
                "decimal(10,2)",
                "DECIMAL (10, 2)",
                " datetime2(3) ",
                "varbinary(max)",
            ] {
                assert_eq!(collate(ty, bin), "", "{ty}");
            }
            assert_eq!(collate("varchar", None), "");
            assert_eq!(collate("varchar", Some("")), "");
        }
    }
}

/// A `-- …` line. Line breaks in `text` (from identifiers) become spaces, so
/// nothing after them can run as SQL.
pub fn note(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if matches!(c, '\n' | '\r') { ' ' } else { c })
        .collect();
    format!("-- {flat}")
}

/// JS truthiness of an optional string: `undefined` and `""` are falsy.
fn truthy(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

/// `String.prototype.trim`: ECMAScript WhiteSpace and LineTerminator. That is
/// Rust's `White_Space` minus U+0085 (NEL), plus U+FEFF (BOM).
fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{0085}') || c == '\u{FEFF}')
}

/// The full type expression for a column (e.g. `VARCHAR(255)`, `DECIMAL(10,2)`).
/// Precision wins over length when both are set.
pub fn build_column_type(col: &CreateTableColumn) -> String {
    if let Some(p) = truthy(&col.precision) {
        return format!("{}({})", col.ty, p);
    }
    if let Some(l) = truthy(&col.length) {
        return format!("{}({})", col.ty, l);
    }
    col.ty.clone()
}

/// Sanitize a user-provided DEFAULT value for interpolation into DDL: trims
/// it, and returns `""` when it contains `;` or `--`.
pub fn sanitize_default_value(value: &str) -> String {
    let trimmed = js_trim(value);
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.contains(';') || trimmed.contains("--") {
        return String::new();
    }
    trimmed.to_string()
}

/// ` NOT NULL` and ` DEFAULT …` as they follow a column's type.
fn push_constraints(out: &mut String, col: &CreateTableColumn) {
    if !col.nullable {
        out.push_str(" NOT NULL");
    }
    if !col.default_value.is_empty() {
        let safe = sanitize_default_value(&col.default_value);
        if !safe.is_empty() {
            out.push_str(" DEFAULT ");
            out.push_str(&safe);
        }
    }
}

/// `  "name" type[ NOT NULL][ DEFAULT …]`, one line of a `CREATE TABLE`.
pub fn build_column_line(col: &CreateTableColumn, q: QuoteFn) -> String {
    let mut line = format!("  {} {}", q(&col.name), build_column_type(col));
    push_constraints(&mut line, col);
    line
}

/// `CREATE TABLE` DDL, followed by one `CREATE INDEX` per index.
pub fn generate_create_table_ddl(def: &CreateTableDefinition, q: QuoteFn) -> String {
    let mut lines: Vec<String> = def
        .columns
        .iter()
        .map(|c| build_column_line(c, q))
        .collect();

    let pk_cols: Vec<String> = def
        .columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| q(&c.name))
        .collect();
    if !pk_cols.is_empty() {
        lines.push(format!("  PRIMARY KEY ({})", pk_cols.join(", ")));
    }

    for col in &def.columns {
        if col.is_unique && !col.is_primary_key {
            lines.push(format!("  UNIQUE ({})", q(&col.name)));
        }
    }

    for fk in &def.foreign_keys {
        lines.push(format!(
            "  FOREIGN KEY ({}) REFERENCES {}.{} ({})",
            q(&fk.column),
            q(&fk.referenced_schema),
            q(&fk.referenced_table),
            q(&fk.referenced_column),
        ));
    }

    let table = format!("{}.{}", q(&def.schema_name), q(&def.table_name));
    let mut sql = format!("CREATE TABLE {} (\n{}\n);", table, lines.join(",\n"));

    for idx in &def.indexes {
        let unique = if idx.unique { "UNIQUE " } else { "" };
        let cols: Vec<String> = idx.columns.iter().map(|c| q(c)).collect();
        sql.push_str(&format!(
            "\n\nCREATE {}INDEX {} ON {} ({});",
            unique,
            q(&idx.name),
            table,
            cols.join(", ")
        ));
    }

    sql
}

/// How [`generate_add_column_ddl`] writes its statement.
#[derive(Debug, Clone, Copy, Default)]
pub struct AddColumnOptions {
    /// `ADD COLUMN` rather than `ADD`.
    pub column_keyword: bool,
    /// The engine's name, when NOT NULL can't go in the `ADD COLUMN` (bug
    /// fix 9, DuckDB; the one-statement form of
    /// [`AlterTableRules::not_null_after_add_column`]). A NOT NULL column then
    /// gets its own `ALTER COLUMN … SET NOT NULL` after it when it has a
    /// default, else [`not_null_note`]. Unlike [`generate_alter_table_sql`]
    /// this doesn't know the table's indexes, so it always emits the
    /// `SET NOT NULL`.
    pub not_null_after: Option<&'static str>,
}

/// `ALTER TABLE <table> ADD [COLUMN] …;`. `table` is quoted (and qualified)
/// already.
pub fn generate_add_column_ddl(
    table: &str,
    column: &CreateTableColumn,
    q: QuoteFn,
    opts: AddColumnOptions,
) -> String {
    let col_keyword = if opts.column_keyword { "COLUMN " } else { "" };
    let mut sql = format!(
        "ALTER TABLE {table} ADD {col_keyword}{} {}",
        q(&column.name),
        build_column_type(column)
    );
    let Some(engine) = opts.not_null_after else {
        push_constraints(&mut sql, column);
        sql.push(';');
        return sql;
    };
    let safe = sanitize_default_value(&column.default_value);
    if !safe.is_empty() {
        sql.push_str(&format!(" DEFAULT {safe}"));
    }
    sql.push(';');
    if !column.nullable {
        sql.push('\n');
        if safe.is_empty() {
            sql.push_str(&not_null_note(engine, table, q, &column.name));
        } else {
            sql.push_str(&format!(
                "ALTER TABLE {table} ALTER COLUMN {} SET NOT NULL;",
                q(&column.name)
            ));
        }
    }
    sql
}

/// JS `new Map(entries)`: a later duplicate key replaces an earlier one.
fn by_key<'a, T>(items: &'a [T], key: impl Fn(&'a T) -> &'a str) -> HashMap<&'a str, &'a T> {
    items.iter().map(|i| (key(i), i)).collect()
}

fn fk_key(fk: &CreateTableForeignKey) -> String {
    format!(
        "{}->{}.{}",
        fk.column, fk.referenced_table, fk.referenced_column
    )
}

/// The statements that turn `original` into `updated`, one per line, or
/// `-- No changes detected`.
///
/// Columns are matched by id (so renames are detected), indexes by name,
/// foreign keys by `column->table.column`. Dropped foreign keys are never
/// emitted: that needs constraint names.
pub fn generate_alter_table_sql(
    original: &CreateTableDefinition,
    updated: &CreateTableDefinition,
    q: QuoteFn,
    opts: AlterTableOptions,
) -> String {
    generate_alter_table_sql_with(original, updated, q, q, opts, AlterTableRules::default())
}

/// [`generate_alter_table_sql`] with a separate quote for the schema part of
/// qualified names (`quote_schema`, see `Dialect::quote_schema`: DuckDB's
/// listed `catalog.schema` is two identifiers) and the [`AlterTableRules`].
/// With `quote_schema == q` and the default rules the output is
/// [`generate_alter_table_sql`]'s.
pub fn generate_alter_table_sql_with(
    original: &CreateTableDefinition,
    updated: &CreateTableDefinition,
    q: QuoteFn,
    quote_schema: QuoteFn,
    opts: AlterTableOptions,
    rules: AlterTableRules,
) -> String {
    let qs = quote_schema;
    let table = format!("{}.{}", qs(&updated.schema_name), q(&updated.table_name));
    let mut stmts: Vec<String> = Vec::new();
    // Comment lines, after every statement, in the order they came up.
    let mut notes: Vec<String> = Vec::new();
    let engine = opts.unsupported_notes.unwrap_or("This database");
    /// A statement without its trailing `;`, for a note that carries it.
    fn bare(stmt: &str) -> &str {
        stmt.strip_suffix(';').unwrap_or(stmt)
    }

    // ── Columns ──
    let orig_cols = by_key(&original.columns, |c| c.id.as_str());
    let new_cols = by_key(&updated.columns, |c| c.id.as_str());
    let orig_idx = by_key(&original.indexes, |i| i.name.as_str());
    let new_idx = by_key(&updated.indexes, |i| i.name.as_str());

    // The indexes the table keeps through this edit (rules: they block
    // column changes), as `"schema"."index"`.
    let kept: Vec<String> = if rules.indexes_block_column_changes {
        original
            .indexes
            .iter()
            .filter(|i| !i.name.is_empty() && new_idx.contains_key(i.name.as_str()))
            .map(|i| format!("{}.{}", qs(&original.schema_name), q(&i.name)))
            .collect()
    } else {
        Vec::new()
    };
    // A statement the engine rejects while the table has indexes.
    let blocked = |stmt: String, stmts: &mut Vec<String>, notes: &mut Vec<String>| {
        if kept.is_empty() {
            stmts.push(stmt);
        } else {
            notes.push(note(&format!(
                "{engine} can't run this while the table has indexes; drop {} first and recreate {} afterwards: {}",
                kept.join(", "),
                if kept.len() > 1 { "them" } else { "it" },
                bare(&stmt)
            )));
        }
    };
    // DROP COLUMN or TYPE of `col`: a note when `col` is in a PRIMARY KEY or
    // UNIQUE constraint (rules), else as `blocked`.
    let constrained = |col: &CreateTableColumn,
                       stmt: String,
                       stmts: &mut Vec<String>,
                       notes: &mut Vec<String>| {
        if rules.constraints_block_column_drops && in_constraint(col) {
            notes.push(note(&format!(
                    "{engine} can't drop or change the type of a column in a PRIMARY KEY or UNIQUE constraint; recreate the table to change it: {}",
                    bare(&stmt)
                )));
        } else {
            blocked(stmt, stmts, notes);
        }
    };
    // New names of columns whose rename became a note: a later statement
    // naming one of them can't run either.
    let mut unrenamed: HashSet<String> = HashSet::new();
    let after_rename = |names: &[&str],
                        stmt: String,
                        unrenamed: &HashSet<String>,
                        stmts: &mut Vec<String>,
                        notes: &mut Vec<String>| {
        if names.iter().any(|n| unrenamed.contains(*n)) {
            notes.push(note(&format!(
                "{engine} can't run this before the rename above: {}",
                bare(&stmt)
            )));
        } else {
            stmts.push(stmt);
        }
    };

    // Renamed
    for new_col in &updated.columns {
        if let Some(orig) = orig_cols.get(new_col.id.as_str()) {
            if orig.name != new_col.name {
                if opts.tsql {
                    stmts.push(format!(
                        "EXEC sp_rename {}, {}, N'COLUMN';",
                        tsql::literal(&format!("{table}.{}", q(&orig.name))),
                        tsql::literal(&new_col.name)
                    ));
                    continue;
                }
                blocked(
                    format!(
                        "ALTER TABLE {} RENAME COLUMN {} TO {};",
                        table,
                        q(&orig.name),
                        q(&new_col.name)
                    ),
                    &mut stmts,
                    &mut notes,
                );
                if !kept.is_empty() {
                    unrenamed.insert(new_col.name.clone());
                }
            }
        }
    }

    // Added
    let orig_fks: HashSet<String> = original.foreign_keys.iter().map(fk_key).collect();
    // Indexes into `updated.foreign_keys` emitted inline with their column.
    let mut inlined_fks: HashSet<usize> = HashSet::new();
    for col in &updated.columns {
        if !orig_cols.contains_key(col.id.as_str()) {
            if opts.tsql {
                // ADD without COLUMN, and NULL said out loud: a bare
                // column's nullability depends on ANSI_NULL_DFLT settings.
                let mut stmt = format!(
                    "ALTER TABLE {} ADD {} {}{} {}",
                    table,
                    q(&col.name),
                    build_column_type(col),
                    tsql::collate(&col.ty, col.collation.as_deref()),
                    if col.nullable { "NULL" } else { "NOT NULL" }
                );
                let safe = sanitize_default_value(&col.default_value);
                if !safe.is_empty() {
                    stmt.push_str(&format!(" DEFAULT {safe}"));
                }
                stmt.push(';');
                stmts.push(stmt);
                continue;
            }
            let mut stmt = format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                table,
                q(&col.name),
                build_column_type(col)
            );
            let safe = sanitize_default_value(&col.default_value);
            if rules.not_null_after_add_column {
                if !safe.is_empty() {
                    stmt.push_str(&format!(" DEFAULT {safe}"));
                }
            } else {
                push_constraints(&mut stmt, col);
            }
            let inline = opts.inline_foreign_keys_on_added_columns
                && !opts.supports_add_foreign_key
                && safe.is_empty();
            if inline {
                let fk = updated.foreign_keys.iter().enumerate().find(|(i, fk)| {
                    fk.column == col.name
                        && !orig_fks.contains(&fk_key(fk))
                        && !inlined_fks.contains(i)
                });
                if let Some((i, fk)) = fk {
                    stmt.push_str(&format!(
                        " REFERENCES {} ({})",
                        q(&fk.referenced_table),
                        q(&fk.referenced_column)
                    ));
                    inlined_fks.insert(i);
                }
            }
            stmt.push(';');
            stmts.push(stmt);
            if rules.not_null_after_add_column && !col.nullable {
                if safe.is_empty() {
                    notes.push(not_null_note(engine, &table, q, &col.name));
                } else {
                    blocked(
                        format!(
                            "ALTER TABLE {} ALTER COLUMN {} SET NOT NULL;",
                            table,
                            q(&col.name)
                        ),
                        &mut stmts,
                        &mut notes,
                    );
                }
            }
        }
    }

    // `@dfN` variables of the T-SQL default lookups, numbered in order.
    let mut default_vars = 0usize;
    let mut next_var = || {
        default_vars += 1;
        default_vars
    };

    // Dropped
    if opts.supports_drop_column {
        for (position, col) in original.columns.iter().enumerate() {
            if !new_cols.contains_key(col.id.as_str()) {
                if opts.tsql {
                    // The default constraint first, whatever the definition
                    // says about a default: it would block the drop.
                    stmts.push(format!(
                        "{} ALTER TABLE {} DROP COLUMN {};",
                        tsql::drop_default(next_var(), &table, &col.name),
                        table,
                        q(&col.name)
                    ));
                    continue;
                }
                let stmt = format!("ALTER TABLE {} DROP COLUMN {};", table, q(&col.name));
                let before_constraint = rules.constraints_block_column_drops
                    && !in_constraint(col)
                    && original.columns[position + 1..].iter().any(in_constraint);
                if before_constraint {
                    notes.push(note(&format!(
                        "{engine} can't drop a column that comes before a PRIMARY KEY or UNIQUE column; recreate the table to change it: {}",
                        bare(&stmt)
                    )));
                    continue;
                }
                constrained(col, stmt, &mut stmts, &mut notes);
            }
        }
    }

    // UNIQUE checked or unchecked (rules). Drops go here, before the column
    // changes; adds after them.
    let own_unique = |c: &CreateTableColumn| c.is_unique && !c.is_primary_key;
    let mut unique_adds: Vec<&str> = Vec::new();
    let mut unique_vars = 0usize;
    if rules.unique_changes != UniqueChanges::Ignore {
        for new_col in &updated.columns {
            match orig_cols.get(new_col.id.as_str()) {
                Some(orig) if own_unique(orig) && !own_unique(new_col) => {
                    // The column's own unique indexes in the definition.
                    let candidates: Vec<_> = original
                        .indexes
                        .iter()
                        .filter(|i| {
                            i.unique
                                && !i.name.is_empty()
                                && i.columns.len() == 1
                                && i.columns[0] == orig.name
                        })
                        .collect();
                    if candidates
                        .iter()
                        .any(|i| !new_idx.contains_key(i.name.as_str()))
                    {
                        continue; // removed under Indexes: dropped there
                    }
                    // Only one is surely the checkbox's: with more (SQLite's
                    // partial ones look the same here), a note says to drop
                    // it under Indexes.
                    let own_index = match candidates.as_slice() {
                        [one] => Some(*one),
                        _ => None,
                    };
                    let name = new_col.name.as_str();
                    let on = format!("{}.{}", qs(&original.schema_name), q(&original.table_name));
                    let drop_note = || {
                        note(&format!(
                            "{engine} can't drop the UNIQUE constraint on {} from an existing table; recreate the table to drop it",
                            q(name)
                        ))
                    };
                    match (rules.unique_changes, own_index) {
                        (UniqueChanges::AddAndDropPostgres, _) => {
                            stmts.push(pg_drop_unique(&table, name));
                        }
                        (UniqueChanges::AddAndDropTsql, _) => {
                            unique_vars += 1;
                            stmts.push(tsql::drop_unique(unique_vars, &table, name));
                        }
                        (UniqueChanges::AddAndDropIndex, Some(i)) => {
                            stmts.push(format!("DROP INDEX {} ON {on};", q(&i.name)));
                        }
                        (UniqueChanges::DropIndex, Some(i))
                            if !opts
                                .constraint_index_prefix
                                .is_some_and(|p| i.name.starts_with(p)) =>
                        {
                            stmts.push(format!(
                                "DROP INDEX {}.{};",
                                qs(&original.schema_name),
                                q(&i.name)
                            ));
                        }
                        (UniqueChanges::AddAndDropIndex | UniqueChanges::DropIndex, None)
                            if candidates.len() > 1 =>
                        {
                            notes.push(note(&format!(
                                "{engine} can't tell which unique index on {} is its UNIQUE: drop it under Indexes",
                                q(name)
                            )));
                        }
                        _ => notes.push(drop_note()),
                    }
                }
                Some(orig) if !own_unique(orig) && own_unique(new_col) => {
                    unique_adds.push(&new_col.name);
                }
                None if own_unique(new_col) => unique_adds.push(&new_col.name),
                _ => {}
            }
        }
    }

    // Modified (the new name is used; any rename was emitted above)
    if !opts.supports_alter_column {
        if let Some(engine) = opts.unsupported_notes {
            for new_col in &updated.columns {
                let Some(orig) = orig_cols.get(new_col.id.as_str()) else {
                    continue;
                };
                let mut changed = Vec::new();
                if build_column_type(orig) != build_column_type(new_col) {
                    changed.push("type");
                }
                if orig.nullable != new_col.nullable {
                    changed.push("NOT NULL");
                }
                if orig.default_value != new_col.default_value {
                    changed.push("default");
                }
                if !changed.is_empty() {
                    notes.push(note(&format!(
                        "{engine} can't alter column {} ({}); recreate the table to change it",
                        q(&new_col.name),
                        changed.join(", ")
                    )));
                }
            }
        }
    }
    if opts.supports_alter_column {
        for new_col in &updated.columns {
            let Some(orig) = orig_cols.get(new_col.id.as_str()) else {
                continue;
            };
            let col_name = q(&new_col.name);
            let orig_type = build_column_type(orig);
            let new_type = build_column_type(new_col);

            let default_changed = orig.default_value != new_col.default_value;
            let modified = orig_type != new_type || orig.nullable != new_col.nullable;
            if opts.tsql {
                if modified {
                    stmts.push(format!(
                        "ALTER TABLE {} ALTER COLUMN {} {}{} {};",
                        table,
                        col_name,
                        new_type,
                        tsql::collate(&new_col.ty, new_col.collation.as_deref()),
                        if new_col.nullable { "NULL" } else { "NOT NULL" }
                    ));
                }
                if default_changed {
                    let mut stmt = tsql::drop_default(next_var(), &table, &new_col.name);
                    let safe = sanitize_default_value(&new_col.default_value);
                    if !safe.is_empty() {
                        stmt.push_str(&format!(
                            " ALTER TABLE {table} ADD DEFAULT {safe} FOR {col_name};"
                        ));
                    }
                    stmts.push(stmt);
                }
                continue;
            }
            if modified {
                if opts.use_modify_column {
                    let mut stmt = format!(
                        "ALTER TABLE {} MODIFY COLUMN {} {}",
                        table, col_name, new_type
                    );
                    push_constraints(&mut stmt, new_col);
                    stmt.push(';');
                    stmts.push(stmt);
                } else {
                    if orig_type != new_type {
                        constrained(
                            orig,
                            format!(
                                "ALTER TABLE {} ALTER COLUMN {} TYPE {};",
                                table, col_name, new_type
                            ),
                            &mut stmts,
                            &mut notes,
                        );
                    }
                    if orig.nullable && !new_col.nullable {
                        blocked(
                            format!(
                                "ALTER TABLE {} ALTER COLUMN {} SET NOT NULL;",
                                table, col_name
                            ),
                            &mut stmts,
                            &mut notes,
                        );
                    } else if !orig.nullable && new_col.nullable {
                        blocked(
                            format!(
                                "ALTER TABLE {} ALTER COLUMN {} DROP NOT NULL;",
                                table, col_name
                            ),
                            &mut stmts,
                            &mut notes,
                        );
                    }
                }
            }

            let alter_default =
                !opts.use_modify_column || (opts.alter_default_with_modify_column && !modified);
            if default_changed && alter_default {
                let safe = sanitize_default_value(&new_col.default_value);
                let stmt = if safe.is_empty() {
                    format!(
                        "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT;",
                        table, col_name
                    )
                } else {
                    format!(
                        "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {};",
                        table, col_name, safe
                    )
                };
                after_rename(
                    &[new_col.name.as_str()],
                    stmt,
                    &unrenamed,
                    &mut stmts,
                    &mut notes,
                );
            }
        }
    }

    for name in unique_adds {
        let stmt = format!("ALTER TABLE {} ADD UNIQUE ({});", table, q(name));
        if matches!(
            rules.unique_changes,
            UniqueChanges::Notes | UniqueChanges::DropIndex
        ) {
            notes.push(note(&format!(
                "{engine} can't add a UNIQUE constraint to an existing table: {}; create a unique index instead, or recreate the table",
                bare(&stmt)
            )));
        } else {
            stmts.push(stmt);
        }
    }

    // ── Indexes ──

    // Constraint indexes that were removed from the definition but stay (a
    // note says so), by columns: an added unique index on the same columns
    // is that index renamed in the editor, and creating it would duplicate
    // it. A non-unique one is a new index and is created.
    let mut kept_constraint_indexes: Vec<&[String]> = Vec::new();
    let mut dropped_indexes: Vec<String> = Vec::new();
    for idx in &original.indexes {
        if !idx.name.is_empty() && !new_idx.contains_key(idx.name.as_str()) {
            if let (Some(prefix), Some(engine)) =
                (opts.constraint_index_prefix, opts.unsupported_notes)
            {
                if idx.name.starts_with(prefix) {
                    kept_constraint_indexes.push(&idx.columns);
                    notes.push(note(&format!(
                        "{engine} can't drop or rename index {}: it belongs to a UNIQUE or PRIMARY KEY constraint; recreate the table to change it",
                        q(&idx.name)
                    )));
                    continue;
                }
            }
            if opts.tsql {
                let on = format!("{}.{}", qs(&original.schema_name), q(&original.table_name));
                dropped_indexes.push(tsql::drop_index(&on, &idx.name, &q(&idx.name)));
                continue;
            }
            if opts.drop_index_on_table {
                let on = format!("{}.{}", qs(&original.schema_name), q(&original.table_name));
                if idx.name == "PRIMARY" {
                    dropped_indexes.push(format!("ALTER TABLE {on} DROP PRIMARY KEY;"));
                } else {
                    dropped_indexes.push(format!("DROP INDEX {} ON {on};", q(&idx.name)));
                }
                continue;
            }
            let name = if opts.qualify_drop_index {
                format!("{}.{}", qs(&original.schema_name), q(&idx.name))
            } else {
                q(&idx.name)
            };
            dropped_indexes.push(format!("DROP INDEX {};", name));
        }
    }
    if opts.drop_indexes_first {
        stmts.splice(0..0, dropped_indexes);
    } else {
        stmts.extend(dropped_indexes);
    }

    for idx in &updated.indexes {
        if !idx.name.is_empty() && !orig_idx.contains_key(idx.name.as_str()) {
            if let Some(i) = kept_constraint_indexes
                .iter()
                .position(|cols| idx.unique && *cols == idx.columns.as_slice())
            {
                kept_constraint_indexes.swap_remove(i);
                continue;
            }
            let unique = if idx.unique { "UNIQUE " } else { "" };
            let cols: Vec<String> = idx.columns.iter().map(|c| q(c)).collect();
            let (name, on) = if opts.qualify_index_name {
                (
                    format!("{}.{}", qs(&updated.schema_name), q(&idx.name)),
                    q(&updated.table_name),
                )
            } else {
                (q(&idx.name), table.clone())
            };
            let names: Vec<&str> = idx.columns.iter().map(String::as_str).collect();
            after_rename(
                &names,
                format!(
                    "CREATE {}INDEX {} ON {} ({});",
                    unique,
                    name,
                    on,
                    cols.join(", ")
                ),
                &unrenamed,
                &mut stmts,
                &mut notes,
            );
        }
    }

    // ── Foreign keys (added only) ──
    for (i, fk) in updated.foreign_keys.iter().enumerate() {
        if !orig_fks.contains(&fk_key(fk)) && !inlined_fks.contains(&i) {
            let ref_table = if fk.referenced_schema.is_empty() {
                q(&fk.referenced_table)
            } else {
                format!("{}.{}", q(&fk.referenced_schema), q(&fk.referenced_table))
            };
            if !opts.supports_add_foreign_key {
                if let Some(engine) = opts.unsupported_notes {
                    notes.push(note(&format!(
                        "{engine} can't add a foreign key to an existing table: ({}) REFERENCES {} ({}); recreate the table to add it",
                        q(&fk.column),
                        ref_table,
                        q(&fk.referenced_column)
                    )));
                }
                continue;
            }
            stmts.push(format!(
                "ALTER TABLE {} ADD FOREIGN KEY ({}) REFERENCES {} ({});",
                table,
                q(&fk.column),
                ref_table,
                q(&fk.referenced_column)
            ));
        }
    }

    stmts.extend(notes);
    if stmts.is_empty() {
        return "-- No changes detected".to_string();
    }
    stmts.join("\n")
}

/// Whether `col` is in a PRIMARY KEY or UNIQUE constraint: its own UNIQUE
/// (`is_unique`) or a composite one (`in_unique_constraint`).
fn in_constraint(col: &CreateTableColumn) -> bool {
    col.is_primary_key || col.is_unique || col.in_unique_constraint
}

/// The note for a NOT NULL column added without a default
/// ([`AlterTableRules::not_null_after_add_column`]): it stays nullable, since
/// `SET NOT NULL` fails on a table with rows. `table` is quoted already.
pub fn not_null_note(engine: &str, table: &str, q: QuoteFn, column: &str) -> String {
    note(&format!(
        "{engine} can't add column {} as NOT NULL without a default; fill it, then run ALTER TABLE {table} ALTER COLUMN {} SET NOT NULL",
        q(column),
        q(column)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use seaquel_types::CreateTableIndex;

    /// The TypeScript Postgres DDL quote: no escaping.
    fn pg_ddl(name: &str) -> String {
        format!("\"{name}\"")
    }

    /// Postgres with bug fix 2 (escaping), as Task 5 will wire it.
    fn pg_escaping(name: &str) -> String {
        format!("\"{}\"", name.replace('"', "\"\""))
    }

    fn mysql(name: &str) -> String {
        format!("`{name}`")
    }

    fn col(id: &str, name: &str, ty: &str) -> CreateTableColumn {
        CreateTableColumn {
            id: id.into(),
            name: name.into(),
            ty: ty.into(),
            length: None,
            precision: None,
            nullable: true,
            default_value: String::new(),
            is_primary_key: false,
            is_unique: false,
            collation: None,
            in_unique_constraint: false,
        }
    }

    fn not_null(mut c: CreateTableColumn) -> CreateTableColumn {
        c.nullable = false;
        c
    }

    fn default(mut c: CreateTableColumn, d: &str) -> CreateTableColumn {
        c.default_value = d.into();
        c
    }

    fn pk(mut c: CreateTableColumn) -> CreateTableColumn {
        c.is_primary_key = true;
        c
    }

    fn length(mut c: CreateTableColumn, l: &str) -> CreateTableColumn {
        c.length = Some(l.into());
        c
    }

    fn precision(mut c: CreateTableColumn, p: &str) -> CreateTableColumn {
        c.precision = Some(p.into());
        c
    }

    fn index(id: &str, name: &str, cols: &[&str], unique: bool) -> CreateTableIndex {
        CreateTableIndex {
            id: id.into(),
            name: name.into(),
            columns: cols.iter().map(|c| c.to_string()).collect(),
            unique,
            ty: "btree".into(),
        }
    }

    fn fk(id: &str, column: &str, schema: &str, table: &str, rcol: &str) -> CreateTableForeignKey {
        CreateTableForeignKey {
            id: id.into(),
            column: column.into(),
            referenced_schema: schema.into(),
            referenced_table: table.into(),
            referenced_column: rcol.into(),
        }
    }

    fn table(
        schema: &str,
        name: &str,
        columns: Vec<CreateTableColumn>,
        indexes: Vec<CreateTableIndex>,
        foreign_keys: Vec<CreateTableForeignKey>,
    ) -> CreateTableDefinition {
        CreateTableDefinition {
            table_name: name.into(),
            schema_name: schema.into(),
            columns,
            indexes,
            foreign_keys,
        }
    }

    /// The `crm.customers` table the ddl-alter fixtures start from.
    fn customers() -> CreateTableDefinition {
        table(
            "crm",
            "customers",
            vec![
                pk(not_null(col("c1", "id", "integer"))),
                length(not_null(col("c2", "name", "varchar")), "100"),
                col("c3", "email", "text"),
                default(col("c4", "status", "text"), "'active'"),
                default(
                    precision(not_null(col("c5", "balance", "numeric")), "10,2"),
                    "0",
                ),
            ],
            vec![index("i1", "customers_email_idx", &["email"], false)],
            vec![fk("f1", "id", "auth", "users", "id")],
        )
    }

    /// Fixture "statement order: every branch but DROP INDEX" (ddl-alter.json).
    fn customers_every_branch() -> CreateTableDefinition {
        table(
            "crm",
            "customers",
            vec![
                pk(not_null(col("c1", "id", "integer"))),
                length(not_null(col("c2", "display_name", "varchar")), "150"),
                default(col("c4", "status", "text"), "'new'"),
                default(precision(col("c5", "balance", "numeric"), "10,2"), "0"),
                default(not_null(col("c6", "tier", "smallint")), "1"),
            ],
            vec![
                index("i1", "customers_email_idx", &["email"], false),
                index("i2", "customers_tier_idx", &["tier"], false),
            ],
            vec![
                fk("f1", "id", "auth", "users", "id"),
                fk("f2", "tier", "crm", "tiers", "id"),
            ],
        )
    }

    // ── helpers ──

    #[test]
    fn column_type_prefers_precision_then_length() {
        let c = col("c", "n", "numeric");
        assert_eq!(build_column_type(&c), "numeric");
        assert_eq!(build_column_type(&length(c.clone(), "5")), "numeric(5)");
        assert_eq!(
            build_column_type(&precision(length(c.clone(), "5"), "10,2")),
            "numeric(10,2)"
        );
        // "" is falsy in JS
        assert_eq!(
            build_column_type(&precision(length(c, "7"), "")),
            "numeric(7)"
        );
    }

    #[test]
    fn sanitize_default_value_rules() {
        assert_eq!(sanitize_default_value("  42  "), "42");
        assert_eq!(sanitize_default_value("   "), "");
        assert_eq!(sanitize_default_value(""), "");
        assert_eq!(sanitize_default_value("'x'; DROP TABLE users"), "");
        assert_eq!(sanitize_default_value("1 -- sneaky"), "");
        assert_eq!(sanitize_default_value("'a-b'"), "'a-b'");
        assert_eq!(sanitize_default_value("now()"), "now()");
        // JS trim: BOM is whitespace, NEL is not.
        assert_eq!(sanitize_default_value("\u{FEFF}\t1\n"), "1");
        assert_eq!(sanitize_default_value("\u{0085}1"), "\u{0085}1");
    }

    // ── CREATE TABLE (ddl-create.json cases) ──

    #[test]
    fn create_table_everything_together_fixture() {
        let def = table(
            "sales",
            "line_items",
            vec![
                pk(not_null(col("c1", "order_id", "integer"))),
                pk(not_null(col("c2", "line_no", "smallint"))),
                not_null(col("c3", "product_id", "integer")),
                {
                    let mut c = length(col("c4", "sku", "varchar"), "64");
                    c.is_unique = true;
                    c
                },
                default(
                    precision(not_null(col("c5", "unit_price", "numeric")), "12,2"),
                    "0",
                ),
                default(col("c6", "meta", "jsonb"), "'{}'::jsonb"),
            ],
            vec![index(
                "i1",
                "line_items_product_idx",
                &["product_id"],
                false,
            )],
            vec![
                fk("f1", "order_id", "sales", "orders", "id"),
                fk("f2", "product_id", "catalog", "products", "id"),
            ],
        );
        assert_eq!(
            generate_create_table_ddl(&def, &pg_ddl),
            "CREATE TABLE \"sales\".\"line_items\" (\n  \"order_id\" integer NOT NULL,\n  \"line_no\" smallint NOT NULL,\n  \"product_id\" integer NOT NULL,\n  \"sku\" varchar(64),\n  \"unit_price\" numeric(12,2) NOT NULL DEFAULT 0,\n  \"meta\" jsonb DEFAULT '{}'::jsonb,\n  PRIMARY KEY (\"order_id\", \"line_no\"),\n  UNIQUE (\"sku\"),\n  FOREIGN KEY (\"order_id\") REFERENCES \"sales\".\"orders\" (\"id\"),\n  FOREIGN KEY (\"product_id\") REFERENCES \"catalog\".\"products\" (\"id\")\n);\n\nCREATE INDEX \"line_items_product_idx\" ON \"sales\".\"line_items\" (\"product_id\");"
        );
    }

    #[test]
    fn create_table_defaults_fixture() {
        let def = table(
            "public",
            "defaults_demo",
            vec![
                default(not_null(col("c1", "created_at", "timestamptz")), "now()"),
                default(col("c2", "status", "text"), "'pending'"),
                default(col("c3", "padded", "integer"), "  42  "),
                default(col("c4", "injected", "text"), "'x'; DROP TABLE users"),
                default(col("c5", "commented", "text"), "1 -- sneaky"),
                default(col("c6", "blank", "text"), "   "),
                col("c7", "empty", "text"),
                default(not_null(col("c8", "flag", "boolean")), "FALSE"),
            ],
            vec![],
            vec![],
        );
        assert_eq!(
            generate_create_table_ddl(&def, &pg_ddl),
            "CREATE TABLE \"public\".\"defaults_demo\" (\n  \"created_at\" timestamptz NOT NULL DEFAULT now(),\n  \"status\" text DEFAULT 'pending',\n  \"padded\" integer DEFAULT 42,\n  \"injected\" text,\n  \"commented\" text,\n  \"blank\" text,\n  \"empty\" text,\n  \"flag\" boolean NOT NULL DEFAULT FALSE\n);"
        );
    }

    #[test]
    fn create_table_degenerate_and_unique_index() {
        let def = table(
            "s",
            "t",
            vec![],
            vec![index("i", "ix", &["a", "b"], true)],
            vec![],
        );
        assert_eq!(
            generate_create_table_ddl(&def, &pg_ddl),
            "CREATE TABLE \"s\".\"t\" (\n\n);\n\nCREATE UNIQUE INDEX \"ix\" ON \"s\".\"t\" (\"a\", \"b\");"
        );
    }

    #[test]
    fn create_table_uses_the_quote_fn() {
        let def = table(
            "s",
            "t",
            vec![pk(not_null(col("c", "id", "int")))],
            vec![],
            vec![],
        );
        assert_eq!(
            generate_create_table_ddl(&def, &mysql),
            "CREATE TABLE `s`.`t` (\n  `id` int NOT NULL,\n  PRIMARY KEY (`id`)\n);"
        );
    }

    #[test]
    fn add_column_with_and_without_keyword() {
        let c = default(length(not_null(col("c", "code", "varchar")), "8"), " 'x' ");
        let keyword = AddColumnOptions {
            column_keyword: true,
            ..Default::default()
        };
        assert_eq!(
            generate_add_column_ddl("\"s\".\"t\"", &c, &pg_ddl, keyword),
            "ALTER TABLE \"s\".\"t\" ADD COLUMN \"code\" varchar(8) NOT NULL DEFAULT 'x';"
        );
        assert_eq!(
            generate_add_column_ddl("`s`.`t`", &col("c", "n", "int"), &mysql, Default::default()),
            "ALTER TABLE `s`.`t` ADD `n` int;"
        );
    }

    #[test]
    fn add_column_with_not_null_after() {
        let opts = AddColumnOptions {
            column_keyword: true,
            not_null_after: Some("DuckDB"),
        };
        let with_default = default(not_null(col("c", "n", "int")), "0");
        assert_eq!(
            generate_add_column_ddl("\"s\".\"t\"", &with_default, &pg_ddl, opts),
            "ALTER TABLE \"s\".\"t\" ADD COLUMN \"n\" int DEFAULT 0;\n\
             ALTER TABLE \"s\".\"t\" ALTER COLUMN \"n\" SET NOT NULL;"
        );
        let without = generate_add_column_ddl(
            "\"s\".\"t\"",
            &not_null(col("c", "n", "int")),
            &pg_ddl,
            opts,
        );
        assert!(
            without.starts_with(
                "ALTER TABLE \"s\".\"t\" ADD COLUMN \"n\" int;\n-- DuckDB can't add column"
            ),
            "{without}"
        );
        assert_eq!(
            generate_add_column_ddl("\"s\".\"t\"", &col("c", "n", "int"), &pg_ddl, opts),
            "ALTER TABLE \"s\".\"t\" ADD COLUMN \"n\" int;"
        );
    }

    // ── ALTER TABLE ──

    #[test]
    fn alter_no_changes() {
        assert_eq!(
            generate_alter_table_sql(&customers(), &customers(), &pg_ddl, Default::default()),
            "-- No changes detected"
        );
    }

    #[test]
    fn alter_statement_order_fixture() {
        assert_eq!(
            generate_alter_table_sql(&customers(), &customers_every_branch(), &pg_ddl, Default::default()),
            "ALTER TABLE \"crm\".\"customers\" RENAME COLUMN \"name\" TO \"display_name\";\nALTER TABLE \"crm\".\"customers\" ADD COLUMN \"tier\" smallint NOT NULL DEFAULT 1;\nALTER TABLE \"crm\".\"customers\" DROP COLUMN \"email\";\nALTER TABLE \"crm\".\"customers\" ALTER COLUMN \"display_name\" TYPE varchar(150);\nALTER TABLE \"crm\".\"customers\" ALTER COLUMN \"status\" SET DEFAULT 'new';\nALTER TABLE \"crm\".\"customers\" ALTER COLUMN \"balance\" DROP NOT NULL;\nCREATE INDEX \"customers_tier_idx\" ON \"crm\".\"customers\" (\"tier\");\nALTER TABLE \"crm\".\"customers\" ADD FOREIGN KEY (\"tier\") REFERENCES \"crm\".\"tiers\" (\"id\");"
        );
    }

    #[test]
    fn alter_unsafe_default_becomes_drop_default_fixture() {
        let mut to = customers();
        to.columns[3].default_value = "'x' -- comment".into();
        assert_eq!(
            generate_alter_table_sql(&customers(), &to, &pg_ddl, Default::default()),
            "ALTER TABLE \"crm\".\"customers\" ALTER COLUMN \"status\" DROP DEFAULT;"
        );
    }

    #[test]
    fn alter_set_not_null_and_foreign_key_without_schema() {
        let mut to = customers();
        to.columns[2].nullable = false;
        to.foreign_keys
            .push(fk("f9", "email", "", "emails", "addr"));
        assert_eq!(
            generate_alter_table_sql(&customers(), &to, &pg_ddl, Default::default()),
            "ALTER TABLE \"crm\".\"customers\" ALTER COLUMN \"email\" SET NOT NULL;\nALTER TABLE \"crm\".\"customers\" ADD FOREIGN KEY (\"email\") REFERENCES \"emails\" (\"addr\");"
        );
    }

    #[test]
    fn alter_drop_index_is_unqualified_by_default() {
        let mut to = customers();
        to.indexes.clear();
        to.indexes.push(index("i2", "", &["name"], false)); // unnamed: skipped
        assert_eq!(
            generate_alter_table_sql(&customers(), &to, &pg_ddl, Default::default()),
            "DROP INDEX \"customers_email_idx\";"
        );
    }

    /// bugfixes.json "fix 3: statement order with every branch, including DROP INDEX".
    #[test]
    fn alter_qualify_drop_index_fixture() {
        let mut from = customers();
        from.foreign_keys.clear();
        let mut to = customers_every_branch();
        to.indexes.remove(0);
        to.foreign_keys = vec![fk("f1", "tier", "crm", "tiers", "id")];
        let opts = AlterTableOptions {
            qualify_drop_index: true,
            ..Default::default()
        };
        assert_eq!(
            generate_alter_table_sql(&from, &to, &pg_escaping, opts),
            "ALTER TABLE \"crm\".\"customers\" RENAME COLUMN \"name\" TO \"display_name\";\nALTER TABLE \"crm\".\"customers\" ADD COLUMN \"tier\" smallint NOT NULL DEFAULT 1;\nALTER TABLE \"crm\".\"customers\" DROP COLUMN \"email\";\nALTER TABLE \"crm\".\"customers\" ALTER COLUMN \"display_name\" TYPE varchar(150);\nALTER TABLE \"crm\".\"customers\" ALTER COLUMN \"status\" SET DEFAULT 'new';\nALTER TABLE \"crm\".\"customers\" ALTER COLUMN \"balance\" DROP NOT NULL;\nDROP INDEX \"crm\".\"customers_email_idx\";\nCREATE INDEX \"customers_tier_idx\" ON \"crm\".\"customers\" (\"tier\");\nALTER TABLE \"crm\".\"customers\" ADD FOREIGN KEY (\"tier\") REFERENCES \"crm\".\"tiers\" (\"id\");"
        );
    }

    /// bugfixes.json "fix 3 + 2: qualified DROP INDEX escapes both parts".
    #[test]
    fn alter_qualified_drop_index_escapes_both_parts_fixture() {
        let from = table(
            "s\"1",
            "t",
            vec![col("c1", "a", "text")],
            vec![index("i1", "ix\"a", &["a"], false)],
            vec![],
        );
        let mut to = from.clone();
        to.indexes.clear();
        let opts = AlterTableOptions {
            qualify_drop_index: true,
            ..Default::default()
        };
        assert_eq!(
            generate_alter_table_sql(&from, &to, &pg_escaping, opts),
            "DROP INDEX \"s\"\"1\".\"ix\"\"a\";"
        );
    }

    /// A schema change and a dropped index in one edit: the index is dropped
    /// from the schema it lives in, not the new one.
    #[test]
    fn qualified_drop_index_uses_the_original_schema() {
        let from = table(
            "old",
            "t",
            vec![col("c1", "a", "text")],
            vec![index("i1", "ix", &["a"], false)],
            vec![],
        );
        let mut to = from.clone();
        to.schema_name = "new".into();
        to.indexes.clear();
        let opts = AlterTableOptions {
            qualify_drop_index: true,
            ..Default::default()
        };
        assert_eq!(
            generate_alter_table_sql(&from, &to, &pg_escaping, opts),
            "DROP INDEX \"old\".\"ix\";"
        );
    }

    #[test]
    fn alter_use_modify_column() {
        // MySQL: one MODIFY COLUMN carries type, NOT NULL and default; a
        // default-only change emits nothing.
        let mut to = customers();
        to.columns[1] = default(
            length(not_null(col("c2", "name", "varchar")), "200"),
            " 'anon' ",
        );
        to.columns[2].nullable = false;
        to.columns[3].default_value = "'new'".into();
        let opts = AlterTableOptions {
            use_modify_column: true,
            ..Default::default()
        };
        assert_eq!(
            generate_alter_table_sql(&customers(), &to, &mysql, opts),
            "ALTER TABLE `crm`.`customers` MODIFY COLUMN `name` varchar(200) NOT NULL DEFAULT 'anon';\nALTER TABLE `crm`.`customers` MODIFY COLUMN `email` text NOT NULL;"
        );
    }

    /// MySQL bug fix 7: a default-only change is `SET DEFAULT` / `DROP
    /// DEFAULT`; with a type or nullability change, `MODIFY` carries it.
    #[test]
    fn alter_default_with_modify_column() {
        let opts = AlterTableOptions {
            use_modify_column: true,
            alter_default_with_modify_column: true,
            ..Default::default()
        };
        let mut to = customers();
        to.columns[3].default_value = "'new'".into();
        to.columns[4].default_value = String::new();
        to.columns[2].nullable = false;
        to.columns[2].default_value = "'x'".into();
        assert_eq!(
            generate_alter_table_sql(&customers(), &to, &mysql, opts),
            "ALTER TABLE `crm`.`customers` MODIFY COLUMN `email` text NOT NULL DEFAULT 'x';\nALTER TABLE `crm`.`customers` ALTER COLUMN `status` SET DEFAULT 'new';\nALTER TABLE `crm`.`customers` ALTER COLUMN `balance` DROP DEFAULT;"
        );
        // Unchanged defaults still emit nothing, and the option does nothing
        // without `use_modify_column`.
        assert_eq!(
            generate_alter_table_sql(&customers(), &customers(), &mysql, opts),
            "-- No changes detected"
        );
        let mut to = customers();
        to.columns[3].default_value = "'new'".into();
        let pg = AlterTableOptions {
            alter_default_with_modify_column: true,
            ..Default::default()
        };
        assert_eq!(
            generate_alter_table_sql(&customers(), &to, &pg_ddl, pg),
            "ALTER TABLE \"crm\".\"customers\" ALTER COLUMN \"status\" SET DEFAULT 'new';"
        );
    }

    /// MySQL bug fix 6: `DROP INDEX name ON table`, from the table the index
    /// lives on, and `DROP PRIMARY KEY` for the `PRIMARY` index.
    #[test]
    fn alter_drop_index_on_table() {
        let opts = AlterTableOptions {
            use_modify_column: true,
            drop_index_on_table: true,
            qualify_drop_index: true,
            ..Default::default()
        };
        let mut from = customers();
        from.indexes.push(index("i0", "PRIMARY", &["id"], true));
        let mut to = from.clone();
        to.schema_name = "billing".into();
        to.indexes.clear();
        assert_eq!(
            generate_alter_table_sql(&from, &to, &mysql, opts),
            "DROP INDEX `customers_email_idx` ON `crm`.`customers`;\nALTER TABLE `crm`.`customers` DROP PRIMARY KEY;"
        );
    }

    #[test]
    fn alter_without_drop_or_alter_column_support() {
        // SQLite: dropped and modified columns are silently skipped; renames,
        // adds and indexes still go through.
        let opts = AlterTableOptions {
            supports_drop_column: false,
            supports_alter_column: false,
            ..Default::default()
        };
        let mut to = customers_every_branch();
        to.foreign_keys.truncate(1);
        assert_eq!(
            generate_alter_table_sql(&customers(), &to, &pg_ddl, opts),
            "ALTER TABLE \"crm\".\"customers\" RENAME COLUMN \"name\" TO \"display_name\";\nALTER TABLE \"crm\".\"customers\" ADD COLUMN \"tier\" smallint NOT NULL DEFAULT 1;\nCREATE INDEX \"customers_tier_idx\" ON \"crm\".\"customers\" (\"tier\");"
        );

        let mut dropped = customers();
        dropped.columns.remove(2);
        assert_eq!(
            generate_alter_table_sql(&customers(), &dropped, &pg_ddl, opts),
            "-- No changes detected"
        );
    }

    /// SQLite fixes 3, 4 and 6: the index name carries the schema, and the
    /// edits SQLite can't make are comment lines after every statement.
    #[test]
    fn alter_with_sqlite_options() {
        let opts = AlterTableOptions {
            supports_alter_column: false,
            qualify_index_name: true,
            supports_add_foreign_key: false,
            unsupported_notes: Some("SQLite"),
            ..Default::default()
        };
        let to = customers_every_branch();
        assert_eq!(
            generate_alter_table_sql(&customers(), &to, &pg_escaping, opts),
            "ALTER TABLE \"crm\".\"customers\" RENAME COLUMN \"name\" TO \"display_name\";\n\
             ALTER TABLE \"crm\".\"customers\" ADD COLUMN \"tier\" smallint NOT NULL DEFAULT 1;\n\
             ALTER TABLE \"crm\".\"customers\" DROP COLUMN \"email\";\n\
             CREATE INDEX \"crm\".\"customers_tier_idx\" ON \"customers\" (\"tier\");\n\
             -- SQLite can't alter column \"display_name\" (type); recreate the table to change it\n\
             -- SQLite can't alter column \"status\" (default); recreate the table to change it\n\
             -- SQLite can't alter column \"balance\" (NOT NULL); recreate the table to change it\n\
             -- SQLite can't add a foreign key to an existing table: (\"tier\") REFERENCES \"crm\".\"tiers\" (\"id\"); recreate the table to add it"
        );

        // A default-only change is a note, not "no changes".
        let mut to = customers();
        to.columns[3].default_value = "'new'".into();
        assert_eq!(
            generate_alter_table_sql(&customers(), &to, &pg_escaping, opts),
            "-- SQLite can't alter column \"status\" (default); recreate the table to change it"
        );
        // Every kind of change at once, and a line break in a name can't end the comment.
        let mut to = customers();
        to.columns[3].name = "st\natus".into();
        to.columns[3].ty = "varchar".into();
        to.columns[3].nullable = false;
        to.columns[3].default_value = String::new();
        let sql = generate_alter_table_sql(&customers(), &to, &pg_escaping, opts);
        assert_eq!(
            sql.lines().last(),
            Some("-- SQLite can't alter column \"st atus\" (type, NOT NULL, default); recreate the table to change it")
        );
        // Unchanged: nothing to say.
        assert_eq!(
            generate_alter_table_sql(&customers(), &customers(), &pg_escaping, opts),
            "-- No changes detected"
        );
    }

    /// SQLite fixes 4 (inline REFERENCES on a new column without a default)
    /// and 7 (a constraint's index can't be dropped).
    #[test]
    fn alter_inline_foreign_keys_and_constraint_indexes() {
        let opts = AlterTableOptions {
            supports_alter_column: false,
            supports_add_foreign_key: false,
            unsupported_notes: Some("SQLite"),
            inline_foreign_keys_on_added_columns: true,
            constraint_index_prefix: Some("sqlite_autoindex_"),
            ..Default::default()
        };
        let mut from = customers();
        from.indexes.push(index(
            "i9",
            "sqlite_autoindex_customers_1",
            &["email"],
            true,
        ));
        let mut to = customers();
        to.columns.push(col("c6", "owner", "integer"));
        to.columns.push(default(col("c7", "tier", "integer"), "1"));
        to.foreign_keys
            .push(fk("f2", "owner", "crm", "users", "id"));
        to.foreign_keys.push(fk("f3", "tier", "crm", "tiers", "id"));
        assert_eq!(
            generate_alter_table_sql(&from, &to, &pg_escaping, opts),
            "ALTER TABLE \"crm\".\"customers\" ADD COLUMN \"owner\" integer REFERENCES \"users\" (\"id\");\n\
             ALTER TABLE \"crm\".\"customers\" ADD COLUMN \"tier\" integer DEFAULT 1;\n\
             -- SQLite can't drop or rename index \"sqlite_autoindex_customers_1\": it belongs to a UNIQUE or PRIMARY KEY constraint; recreate the table to change it\n\
             -- SQLite can't add a foreign key to an existing table: (\"tier\") REFERENCES \"crm\".\"tiers\" (\"id\"); recreate the table to add it"
        );
    }

    /// Fix 7: renaming a constraint's index in the editor is a removal plus
    /// an addition on the same columns. The removal stays a note and the
    /// CREATE is skipped: it would duplicate the index that stays. An added
    /// index on other columns is still created.
    #[test]
    fn alter_renaming_a_constraint_index_creates_no_duplicate() {
        let opts = AlterTableOptions {
            qualify_index_name: true,
            unsupported_notes: Some("SQLite"),
            constraint_index_prefix: Some("sqlite_autoindex_"),
            ..Default::default()
        };
        let mut from = customers();
        from.indexes.push(index(
            "i9",
            "sqlite_autoindex_customers_1",
            &["email"],
            true,
        ));
        let mut to = from.clone();
        to.indexes[1].name = "customers_email_key".into();
        let note = "-- SQLite can't drop or rename index \"sqlite_autoindex_customers_1\": it belongs to a UNIQUE or PRIMARY KEY constraint; recreate the table to change it";
        assert_eq!(
            generate_alter_table_sql(&from, &to, &pg_escaping, opts),
            note
        );

        // Same columns, but a second added unique index: only one is the
        // rename.
        to.indexes
            .push(index("i10", "email_again", &["email"], true));
        to.indexes.push(index("i11", "by_name", &["name"], false));
        assert_eq!(
            generate_alter_table_sql(&from, &to, &pg_escaping, opts),
            format!(
                "CREATE UNIQUE INDEX \"crm\".\"email_again\" ON \"customers\" (\"email\");\n\
                 CREATE INDEX \"crm\".\"by_name\" ON \"customers\" (\"name\");\n{note}"
            )
        );

        // Removing the constraint's index and adding a non-unique index on
        // its columns is a new index: it is created.
        let mut to = from.clone();
        to.indexes.remove(1);
        to.indexes.push(index("i12", "by_email", &["email"], false));
        assert_eq!(
            generate_alter_table_sql(&from, &to, &pg_escaping, opts),
            format!("CREATE INDEX \"crm\".\"by_email\" ON \"customers\" (\"email\");\n{note}")
        );

        // Without the option (other engines) a rename is DROP + CREATE.
        let mut plain = from.clone();
        plain.indexes[1].name = "old_email".into();
        let mut renamed = plain.clone();
        renamed.indexes[1].name = "new_email".into();
        assert_eq!(
            generate_alter_table_sql(&plain, &renamed, &pg_escaping, Default::default()),
            "DROP INDEX \"old_email\";\nCREATE UNIQUE INDEX \"new_email\" ON \"crm\".\"customers\" (\"email\");"
        );
    }

    /// DuckDB's rules with a composite UNIQUE (the Task 17 review's cases):
    /// a TYPE change of a column in `UNIQUE (a, b)` and a DROP COLUMN before
    /// one are notes; the same columns without `in_unique_constraint` are
    /// statements.
    #[test]
    fn composite_unique_blocks_type_and_earlier_drops() {
        let opts = AlterTableOptions {
            unsupported_notes: Some("DuckDB"),
            ..Default::default()
        };
        let rules = AlterTableRules {
            constraints_block_column_drops: true,
            ..Default::default()
        };
        let in_unique = |mut c: CreateTableColumn| {
            c.in_unique_constraint = true;
            c
        };
        let t = table(
            "main",
            "t",
            vec![
                pk(not_null(col("c1", "id", "INTEGER"))),
                in_unique(col("c2", "a", "INTEGER")),
                in_unique(col("c3", "b", "INTEGER")),
                col("c4", "z", "INTEGER"),
            ],
            vec![],
            vec![],
        );
        let mut to = t.clone();
        to.columns[1].ty = "BIGINT".into();
        to.columns[3].ty = "BIGINT".into();
        assert_eq!(
            generate_alter_table_sql_with(&t, &to, &pg_escaping, &pg_escaping, opts, rules),
            "ALTER TABLE \"main\".\"t\" ALTER COLUMN \"z\" TYPE BIGINT;\n\
             -- DuckDB can't drop or change the type of a column in a PRIMARY KEY or UNIQUE constraint; recreate the table to change it: ALTER TABLE \"main\".\"t\" ALTER COLUMN \"a\" TYPE BIGINT"
        );

        let u = table(
            "main",
            "u",
            vec![
                col("c1", "x", "INTEGER"),
                in_unique(col("c2", "a", "INTEGER")),
                in_unique(col("c3", "b", "INTEGER")),
            ],
            vec![],
            vec![],
        );
        let mut to = u.clone();
        to.columns.remove(0);
        assert_eq!(
            generate_alter_table_sql_with(&u, &to, &pg_escaping, &pg_escaping, opts, rules),
            "-- DuckDB can't drop a column that comes before a PRIMARY KEY or UNIQUE column; recreate the table to change it: ALTER TABLE \"main\".\"u\" DROP COLUMN \"x\""
        );
        // Without the flag (or the rules) it is a statement.
        let mut plain = u.clone();
        for c in &mut plain.columns {
            c.in_unique_constraint = false;
        }
        let mut to = plain.clone();
        to.columns.remove(0);
        assert_eq!(
            generate_alter_table_sql_with(&plain, &to, &pg_escaping, &pg_escaping, opts, rules),
            "ALTER TABLE \"main\".\"u\" DROP COLUMN \"x\";"
        );
    }

    #[test]
    fn alter_uses_the_new_schema_and_table() {
        let mut to = customers();
        to.schema_name = "billing".into();
        to.table_name = "clients".into();
        to.columns[2].nullable = false;
        assert_eq!(
            generate_alter_table_sql(&customers(), &to, &pg_ddl, Default::default()),
            "ALTER TABLE \"billing\".\"clients\" ALTER COLUMN \"email\" SET NOT NULL;"
        );
    }

    fn unique(mut c: CreateTableColumn) -> CreateTableColumn {
        c.is_unique = true;
        c
    }

    fn rules(unique_changes: UniqueChanges) -> AlterTableRules {
        AlterTableRules {
            unique_changes,
            ..Default::default()
        }
    }

    fn brackets(name: &str) -> String {
        format!("[{}]", name.replace(']', "]]"))
    }

    /// Task 18: a column's UNIQUE checked or unchecked in edit mode. The
    /// default rules keep the TypeScript behaviour (no statement).
    #[test]
    fn unique_checked_and_unchecked() {
        let from = customers();
        let mut checked = customers();
        checked.columns[2] = unique(checked.columns[2].clone());
        let alter = |from: &CreateTableDefinition, to: &CreateTableDefinition, u, q: QuoteFn| {
            generate_alter_table_sql_with(from, to, q, q, AlterTableOptions::default(), rules(u))
        };

        assert_eq!(
            generate_alter_table_sql(&from, &checked, &pg_escaping, Default::default()),
            "-- No changes detected"
        );
        for u in [
            UniqueChanges::AddAndDropIndex,
            UniqueChanges::AddAndDropPostgres,
        ] {
            assert_eq!(
                alter(&from, &checked, u, &pg_escaping),
                "ALTER TABLE \"crm\".\"customers\" ADD UNIQUE (\"email\");"
            );
        }
        assert_eq!(
            alter(&from, &checked, UniqueChanges::AddAndDropTsql, &brackets),
            "ALTER TABLE [crm].[customers] ADD UNIQUE ([email]);"
        );
        for u in [UniqueChanges::Notes, UniqueChanges::DropIndex] {
            assert_eq!(
                alter(&from, &checked, u, &pg_escaping),
                "-- This database can't add a UNIQUE constraint to an existing table: ALTER TABLE \"crm\".\"customers\" ADD UNIQUE (\"email\"); create a unique index instead, or recreate the table"
            );
        }
        // Re-ticking a column that is UNIQUE already emits nothing.
        for u in [
            UniqueChanges::AddAndDropPostgres,
            UniqueChanges::AddAndDropTsql,
        ] {
            assert_eq!(
                alter(&checked, &checked, u, &pg_escaping),
                "-- No changes detected"
            );
        }

        // Unchecked.
        assert_eq!(
            alter(&checked, &from, UniqueChanges::AddAndDropPostgres, &pg_escaping),
            "DO $sq$ DECLARE r record; BEGIN FOR r IN SELECT x.indexrelid::regclass AS idx, k.conname FROM pg_index x \
             LEFT JOIN pg_constraint k ON k.conindid = x.indexrelid AND k.conrelid = x.indrelid \
             WHERE x.indrelid = '\"crm\".\"customers\"'::regclass AND x.indisunique AND NOT x.indisprimary AND x.indnkeyatts = 1 AND x.indnatts = 1 AND x.indpred IS NULL \
             AND x.indkey[0] = (SELECT a.attnum FROM pg_attribute a WHERE a.attrelid = x.indrelid AND a.attname = 'email') \
             LOOP IF r.conname IS NULL THEN EXECUTE format('DROP INDEX %s', r.idx); \
             ELSE EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', '\"crm\".\"customers\"', r.conname); END IF; END LOOP; END $sq$;"
        );
        let tsql = alter(&checked, &from, UniqueChanges::AddAndDropTsql, &brackets);
        assert!(tsql.starts_with("DECLARE @uq1 nvarchar(max) = N''; SELECT @uq1 = @uq1 + CASE WHEN i.is_unique_constraint = 1 THEN N'ALTER TABLE [crm].[customers] DROP CONSTRAINT ' + QUOTENAME(i.name) + N'; ' ELSE N'DROP INDEX ' + QUOTENAME(i.name) + N' ON [crm].[customers]; ' END FROM sys.indexes i"), "{tsql}");
        assert!(
            tsql.ends_with("AND c.name = N'email'); IF @uq1 <> N'' EXEC (@uq1);"),
            "{tsql}"
        );
        // Without an index for it in the definition: a note.
        for u in [
            UniqueChanges::AddAndDropIndex,
            UniqueChanges::DropIndex,
            UniqueChanges::Notes,
        ] {
            assert_eq!(
                alter(&checked, &from, u, &pg_escaping),
                "-- This database can't drop the UNIQUE constraint on \"email\" from an existing table; recreate the table to drop it"
            );
        }
        assert_eq!(
            alter(&checked, &from, UniqueChanges::Ignore, &pg_escaping),
            "-- No changes detected"
        );
    }

    /// Task 18: unchecking drops the column's unique index from the
    /// definition (MySQL, SQLite), a constraint's SQLite autoindex is a
    /// note, and nothing is emitted when the same edit removes the index
    /// under Indexes (the index section drops it).
    #[test]
    fn unique_unchecked_drops_its_index() {
        let with_index = |name: &str| {
            let mut t = customers();
            t.columns[2] = unique(t.columns[2].clone());
            t.indexes.push(index("i9", name, &["email"], true));
            t
        };
        let unchecked = |t: &CreateTableDefinition| {
            let mut t = t.clone();
            t.columns[2].is_unique = false;
            t
        };
        let mysql_opts = AlterTableOptions {
            drop_index_on_table: true,
            ..Default::default()
        };
        let from = with_index("email");
        let r = rules(UniqueChanges::AddAndDropIndex);
        assert_eq!(
            generate_alter_table_sql_with(&from, &unchecked(&from), &mysql, &mysql, mysql_opts, r),
            "DROP INDEX `email` ON `crm`.`customers`;"
        );
        // Removed under Indexes too: dropped once, by the index section.
        let mut both = unchecked(&from);
        both.indexes.pop();
        assert_eq!(
            generate_alter_table_sql_with(&from, &both, &mysql, &mysql, mysql_opts, r),
            "DROP INDEX `email` ON `crm`.`customers`;"
        );

        let sqlite_opts = AlterTableOptions {
            unsupported_notes: Some("SQLite"),
            constraint_index_prefix: Some("sqlite_autoindex_"),
            ..Default::default()
        };
        let r = rules(UniqueChanges::DropIndex);
        let from = with_index("customers_email_uq");
        assert_eq!(
            generate_alter_table_sql_with(
                &from,
                &unchecked(&from),
                &pg_escaping,
                &pg_escaping,
                sqlite_opts,
                r
            ),
            "DROP INDEX \"crm\".\"customers_email_uq\";"
        );
        let from = with_index("sqlite_autoindex_customers_1");
        assert_eq!(
            generate_alter_table_sql_with(&from, &unchecked(&from), &pg_escaping, &pg_escaping, sqlite_opts, r),
            "-- SQLite can't drop the UNIQUE constraint on \"email\" from an existing table; recreate the table to drop it"
        );
        // Postgres: the catalog lookup, or nothing when removed under Indexes.
        let from = with_index("customers_email_key");
        let mut both = unchecked(&from);
        both.indexes.pop();
        let pg = AlterTableOptions {
            qualify_drop_index: true,
            ..Default::default()
        };
        assert_eq!(
            generate_alter_table_sql_with(
                &from,
                &both,
                &pg_escaping,
                &pg_escaping,
                pg,
                rules(UniqueChanges::AddAndDropPostgres)
            ),
            "DROP INDEX \"crm\".\"customers_email_key\";"
        );
    }

    /// Task 18: an added UNIQUE column gets its constraint after the ADD
    /// COLUMN; a primary key column's UNIQUE is ignored; a dropped UNIQUE
    /// comes before a type change of its column, an added one after it.
    #[test]
    fn unique_statement_order() {
        let u = rules(UniqueChanges::AddAndDropTsql);
        let opts = AlterTableOptions {
            tsql: true,
            ..Default::default()
        };
        let base = table(
            "dbo",
            "t",
            vec![
                pk(not_null(col("c1", "id", "int"))),
                unique(length(col("c2", "code", "nvarchar"), "10")),
                length(col("c3", "tag", "nvarchar"), "10"),
            ],
            vec![],
            vec![],
        );
        let mut to = base.clone();
        to.columns[0] = unique(to.columns[0].clone());
        to.columns[1].is_unique = false;
        to.columns[1].length = Some("20".into());
        to.columns[2] = unique(to.columns[2].clone());
        to.columns[2].length = Some("20".into());
        to.columns.push(unique(col("c4", "ref", "int")));
        let sql = generate_alter_table_sql_with(&base, &to, &brackets, &brackets, opts, u);
        let lines: Vec<&str> = sql.lines().collect();
        assert_eq!(lines.len(), 6, "{sql}");
        assert_eq!(lines[0], "ALTER TABLE [dbo].[t] ADD [ref] int NULL;");
        assert!(lines[1].starts_with("DECLARE @uq1 nvarchar(max)"), "{sql}");
        assert!(lines[1].contains("c.name = N'code'"), "{sql}");
        assert_eq!(
            lines[2],
            "ALTER TABLE [dbo].[t] ALTER COLUMN [code] nvarchar(20) NULL;"
        );
        assert_eq!(
            lines[3],
            "ALTER TABLE [dbo].[t] ALTER COLUMN [tag] nvarchar(20) NULL;"
        );
        assert_eq!(lines[4], "ALTER TABLE [dbo].[t] ADD UNIQUE ([tag]);");
        assert_eq!(lines[5], "ALTER TABLE [dbo].[t] ADD UNIQUE ([ref]);");
        // Each statement is one `;\n` piece, as the table editor splits it.
        assert_eq!(sql.split(";\n").count(), 6);
    }

    /// The `DO` block picks a dollar tag its body doesn't hold, and quotes
    /// names as literals.
    #[test]
    fn pg_drop_unique_escapes() {
        let sql = pg_drop_unique("\"s\".\"it's $sq$\"", "o'k");
        assert!(sql.starts_with("DO $sq1$ "), "{sql}");
        assert!(sql.ends_with(" $sq1$;"), "{sql}");
        assert!(sql.contains("'\"s\".\"it''s $sq$\"'::regclass"), "{sql}");
        assert!(sql.contains("a.attname = 'o''k'"), "{sql}");
    }
}
