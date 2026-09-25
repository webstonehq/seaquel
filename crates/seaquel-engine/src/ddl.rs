//! Generic DDL generation: a port of `src/lib/db/alter-table.ts`.
//!
//! Every function is parameterized the way the TypeScript was: by a quote
//! function and, for `ALTER TABLE`, by [`AlterTableOptions`]. There are no
//! per-dialect branches here. Output is byte-for-byte what the TypeScript
//! produced (statement order, spacing, newlines, `-- No changes detected`),
//! except where a dialect opts into [`AlterTableOptions::qualify_drop_index`].

use std::collections::{HashMap, HashSet};

use seaquel_types::{CreateTableColumn, CreateTableDefinition, CreateTableForeignKey};

/// Quotes one identifier (`name` → `"name"`, `` `name` ``, `[name]`, …).
pub type QuoteFn<'a> = &'a dyn Fn(&str) -> String;

/// The dialect switches `generateAlterTableSql` took in TypeScript, plus
/// `qualify_drop_index`.
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
}

impl Default for AlterTableOptions {
    fn default() -> Self {
        Self {
            supports_drop_column: true,
            supports_alter_column: true,
            use_modify_column: false,
            qualify_drop_index: false,
        }
    }
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

fn build_column_line(col: &CreateTableColumn, q: QuoteFn) -> String {
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

/// `ALTER TABLE … ADD [COLUMN] …;`
pub fn generate_add_column_ddl(
    schema: &str,
    table: &str,
    column: &CreateTableColumn,
    q: QuoteFn,
    include_column_keyword: bool,
) -> String {
    let col_keyword = if include_column_keyword {
        "COLUMN "
    } else {
        ""
    };
    let mut sql = format!(
        "ALTER TABLE {}.{} ADD {}{} {}",
        q(schema),
        q(table),
        col_keyword,
        q(&column.name),
        build_column_type(column)
    );
    push_constraints(&mut sql, column);
    sql.push(';');
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
    let table = format!("{}.{}", q(&updated.schema_name), q(&updated.table_name));
    let mut stmts: Vec<String> = Vec::new();

    // ── Columns ──
    let orig_cols = by_key(&original.columns, |c| c.id.as_str());
    let new_cols = by_key(&updated.columns, |c| c.id.as_str());

    // Renamed
    for new_col in &updated.columns {
        if let Some(orig) = orig_cols.get(new_col.id.as_str()) {
            if orig.name != new_col.name {
                stmts.push(format!(
                    "ALTER TABLE {} RENAME COLUMN {} TO {};",
                    table,
                    q(&orig.name),
                    q(&new_col.name)
                ));
            }
        }
    }

    // Added
    for col in &updated.columns {
        if !orig_cols.contains_key(col.id.as_str()) {
            let mut stmt = format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                table,
                q(&col.name),
                build_column_type(col)
            );
            push_constraints(&mut stmt, col);
            stmt.push(';');
            stmts.push(stmt);
        }
    }

    // Dropped
    if opts.supports_drop_column {
        for col in &original.columns {
            if !new_cols.contains_key(col.id.as_str()) {
                stmts.push(format!(
                    "ALTER TABLE {} DROP COLUMN {};",
                    table,
                    q(&col.name)
                ));
            }
        }
    }

    // Modified (the new name is used; any rename was emitted above)
    if opts.supports_alter_column {
        for new_col in &updated.columns {
            let Some(orig) = orig_cols.get(new_col.id.as_str()) else {
                continue;
            };
            let col_name = q(&new_col.name);
            let orig_type = build_column_type(orig);
            let new_type = build_column_type(new_col);

            if orig_type != new_type || orig.nullable != new_col.nullable {
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
                        stmts.push(format!(
                            "ALTER TABLE {} ALTER COLUMN {} TYPE {};",
                            table, col_name, new_type
                        ));
                    }
                    if orig.nullable && !new_col.nullable {
                        stmts.push(format!(
                            "ALTER TABLE {} ALTER COLUMN {} SET NOT NULL;",
                            table, col_name
                        ));
                    } else if !orig.nullable && new_col.nullable {
                        stmts.push(format!(
                            "ALTER TABLE {} ALTER COLUMN {} DROP NOT NULL;",
                            table, col_name
                        ));
                    }
                }
            }

            if orig.default_value != new_col.default_value && !opts.use_modify_column {
                let safe = sanitize_default_value(&new_col.default_value);
                if safe.is_empty() {
                    stmts.push(format!(
                        "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT;",
                        table, col_name
                    ));
                } else {
                    stmts.push(format!(
                        "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {};",
                        table, col_name, safe
                    ));
                }
            }
        }
    }

    // ── Indexes ──
    let orig_idx = by_key(&original.indexes, |i| i.name.as_str());
    let new_idx = by_key(&updated.indexes, |i| i.name.as_str());

    for idx in &original.indexes {
        if !idx.name.is_empty() && !new_idx.contains_key(idx.name.as_str()) {
            let name = if opts.qualify_drop_index {
                format!("{}.{}", q(&original.schema_name), q(&idx.name))
            } else {
                q(&idx.name)
            };
            stmts.push(format!("DROP INDEX {};", name));
        }
    }

    for idx in &updated.indexes {
        if !idx.name.is_empty() && !orig_idx.contains_key(idx.name.as_str()) {
            let unique = if idx.unique { "UNIQUE " } else { "" };
            let cols: Vec<String> = idx.columns.iter().map(|c| q(c)).collect();
            stmts.push(format!(
                "CREATE {}INDEX {} ON {} ({});",
                unique,
                q(&idx.name),
                table,
                cols.join(", ")
            ));
        }
    }

    // ── Foreign keys (added only) ──
    let orig_fks: HashSet<String> = original.foreign_keys.iter().map(fk_key).collect();
    for fk in &updated.foreign_keys {
        if !orig_fks.contains(&fk_key(fk)) {
            let ref_table = if fk.referenced_schema.is_empty() {
                q(&fk.referenced_table)
            } else {
                format!("{}.{}", q(&fk.referenced_schema), q(&fk.referenced_table))
            };
            stmts.push(format!(
                "ALTER TABLE {} ADD FOREIGN KEY ({}) REFERENCES {} ({});",
                table,
                q(&fk.column),
                ref_table,
                q(&fk.referenced_column)
            ));
        }
    }

    if stmts.is_empty() {
        return "-- No changes detected".to_string();
    }
    stmts.join("\n")
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
        assert_eq!(
            generate_add_column_ddl("s", "t", &c, &pg_ddl, true),
            "ALTER TABLE \"s\".\"t\" ADD COLUMN \"code\" varchar(8) NOT NULL DEFAULT 'x';"
        );
        assert_eq!(
            generate_add_column_ddl("s", "t", &col("c", "n", "int"), &mysql, false),
            "ALTER TABLE `s`.`t` ADD `n` int;"
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
}
