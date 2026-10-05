//! `seaquel-cli schema <CONNECTION> [TABLE]`.
//!
//! Without `TABLE` it lists the connection's tables and views: as a JSON
//! array of `schema`, `name`, `kind` (Core's `table`, `view`, `materialized-view`)
//! and `rowCount` (the engine's estimate, `null` when it has none), or a
//! table of SCHEMA, NAME, KIND and ROWS.
//!
//! With `TABLE` it describes one: `TABLE` is a listed table's name, or
//! `schema.name` as listed. It isn't split on `.`, so DuckDB's
//! `catalog.schema` schemas work as they are listed. JSON is
//! `{"schema", "name", "columns", "indexes"}`, the columns and indexes as
//! Core reports them; the table output is the columns (NAME, TYPE, NULL,
//! DEFAULT, KEY), a blank line, then the indexes (INDEX, COLUMNS, UNIQUE).
//!
//! It connects as `conn test` does, asking for a missing password or an
//! unknown SSH host key when it can, and closes the connection again.

use std::process::ExitCode;

use seaquel_core::CoreError;
use seaquel_types::{DbError, SchemaColumn, SchemaIndex, SchemaTable, TableKind};
use serde::Serialize;

use crate::output::{self, Format};
use crate::session::{block_on, fail, until_stopped, Session};
use crate::{connect, prompt, resolve, SchemaArgs};

const COMMAND: &str = "schema";

pub const TABLE_NOT_FOUND: &str = "TABLE_NOT_FOUND";
pub const AMBIGUOUS_TABLE: &str = "AMBIGUOUS_TABLE";

pub fn run(args: SchemaArgs) -> ExitCode {
    let format = Format::pick(args.output.format);
    block_on(
        COMMAND,
        schema(args.connection, args.table, format, args.input.no_input),
    )
}

/// One table or view as the list prints it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Listed {
    schema: String,
    name: String,
    kind: TableKind,
    row_count: Option<i64>,
}

/// One table described.
#[derive(Debug, Serialize)]
struct Described {
    schema: String,
    name: String,
    columns: Vec<SchemaColumn>,
    indexes: Vec<SchemaIndex>,
}

enum Answer {
    Tables(Vec<Listed>),
    Table(Described),
}

async fn schema(
    connection: String,
    table: Option<String>,
    format: Format,
    no_input: bool,
) -> ExitCode {
    let s = match Session::open().await {
        Ok(s) => s,
        Err(e) => return fail(COMMAND, &e),
    };
    // Stopped: the calls are dropped, and `close` closes the connection.
    let result = until_stopped(COMMAND, s, async |s| {
        read(s, &connection, table.as_deref(), no_input).await
    })
    .await;
    match result {
        Ok(Ok(answer)) => {
            output::print(&match (answer, format) {
                (Answer::Tables(rows), Format::Json) => output::json(&rows),
                (Answer::Tables(rows), Format::Table) => tables_table(&rows),
                (Answer::Table(t), Format::Json) => output::json(&t),
                (Answer::Table(t), Format::Table) => described_table(&t),
            });
            ExitCode::SUCCESS
        }
        Ok(Err(e)) => fail(COMMAND, &e),
        Err(stopped) => stopped,
    }
}

fn db_error(e: DbError) -> CoreError {
    CoreError::new(e.code, e.message)
}

async fn read(
    s: &Session,
    wanted: &str,
    table: Option<&str>,
    no_input: bool,
) -> Result<Answer, CoreError> {
    let row = resolve::saved_connection(s, wanted).await?;
    let mut prompter = prompt::for_command(no_input);
    let id = connect::connect(s, &row, &mut prompter).await?;
    let handle = s.ws.engine(&s.core, &id).map_err(db_error)?;
    let tables = handle.schema_tables().await.map_err(db_error)?;
    let Some(arg) = table else {
        return Ok(Answer::Tables(tables.into_iter().map(listed).collect()));
    };
    let t = find_table(&tables, arg)?;
    let (columns, indexes) = handle
        .table_metadata(&t.schema, &t.name)
        .await
        .map_err(db_error)?;
    Ok(Answer::Table(Described {
        schema: t.schema.clone(),
        name: t.name.clone(),
        columns,
        indexes,
    }))
}

/// `kind` as its JSON spells it (Core's `TableKind`, kebab-case).
fn kind_text(kind: TableKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn listed(t: SchemaTable) -> Listed {
    Listed {
        kind: t.kind,
        row_count: t.row_count,
        schema: t.schema,
        name: t.name,
    }
}

/// The listed table whose name, or `schema.name`, is `arg`.
fn find_table<'a>(tables: &'a [SchemaTable], arg: &str) -> Result<&'a SchemaTable, CoreError> {
    let found: Vec<&SchemaTable> = tables
        .iter()
        .filter(|t| arg == t.name || arg == format!("{}.{}", t.schema, t.name))
        .collect();
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err(CoreError::new(
            TABLE_NOT_FOUND,
            format!("{arg:?}: no table or view has this name"),
        )),
        many => Err(CoreError::new(
            AMBIGUOUS_TABLE,
            format!(
                "{arg:?}: {} tables or views have this name: {}. Pass one of them",
                many.len(),
                many.iter()
                    .map(|t| format!("{:?}", format!("{}.{}", t.schema, t.name)))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
        )),
    }
}

fn tables_table(rows: &[Listed]) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.schema.clone(),
                r.name.clone(),
                kind_text(r.kind),
                r.row_count.map(|n| n.to_string()).unwrap_or_default(),
            ]
        })
        .collect();
    output::table(&["SCHEMA", "NAME", "KIND", "ROWS"], &cells)
}

/// `PK`, `FK → table.column`, both, or nothing.
fn key(c: &SchemaColumn) -> String {
    let mut keys = Vec::new();
    if c.is_primary_key {
        keys.push("PK".to_string());
    }
    if c.is_foreign_key {
        keys.push(match &c.foreign_key_ref {
            Some(r) => format!("FK → {}.{}", r.referenced_table, r.referenced_column),
            None => "FK".to_string(),
        });
    }
    keys.join(", ")
}

fn yes_no(b: bool) -> String {
    if b { "yes" } else { "no" }.to_string()
}

fn described_table(t: &Described) -> String {
    let columns: Vec<Vec<String>> = t
        .columns
        .iter()
        .map(|c| {
            vec![
                c.name.clone(),
                c.ty.clone(),
                yes_no(c.nullable),
                c.default_value.clone().unwrap_or_default(),
                key(c),
            ]
        })
        .collect();
    let indexes: Vec<Vec<String>> = t
        .indexes
        .iter()
        .map(|i| vec![i.name.clone(), i.columns.join(", "), yes_no(i.unique)])
        .collect();
    let mut text = output::table(&["NAME", "TYPE", "NULL", "DEFAULT", "KEY"], &columns);
    text.push('\n');
    text.push_str(&output::table(&["INDEX", "COLUMNS", "UNIQUE"], &indexes));
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn table(schema: &str, name: &str) -> SchemaTable {
        serde_json::from_value(json!({
            "schema": schema, "name": name, "type": "table", "columns": [], "indexes": [],
        }))
        .unwrap()
    }

    #[test]
    fn a_table_by_name_or_by_schema_and_name() {
        let tables = [
            table("public", "users"),
            table("audit", "users"),
            table("public", "orders"),
            // DuckDB: an attached catalog's schema is `catalog.schema`.
            table("lake.main", "events"),
        ];
        assert_eq!(find_table(&tables, "orders").unwrap().schema, "public");
        assert_eq!(find_table(&tables, "audit.users").unwrap().schema, "audit");
        assert_eq!(
            find_table(&tables, "lake.main.events").unwrap().name,
            "events"
        );
        assert_eq!(
            find_table(&tables, "main.events").unwrap_err().code,
            TABLE_NOT_FOUND
        );
        let e = find_table(&tables, "users").unwrap_err();
        assert_eq!(e.code, AMBIGUOUS_TABLE);
        assert!(
            e.message.contains("\"public.users\"") && e.message.contains("\"audit.users\""),
            "{}",
            e.message
        );
    }

    #[test]
    fn the_list_names_each_kind_as_core_spells_it() {
        let mut view = table("public", "v");
        view.kind = TableKind::MaterializedView;
        view.row_count = Some(3);
        let text = output::json(&[listed(view)]);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            v,
            json!([{"schema": "public", "name": "v", "kind": "materialized-view", "rowCount": 3}])
        );
        let text = output::json(&[listed(table("public", "t"))]);
        assert!(text.contains("\"rowCount\": null"), "{text}");
        // The table's KIND column spells it the same way.
        assert_eq!(kind_text(TableKind::MaterializedView), "materialized-view");
        assert_eq!(kind_text(TableKind::Table), "table");
    }

    #[test]
    fn a_described_table_shows_keys_then_indexes() {
        let t: Described = Described {
            schema: "public".into(),
            name: "orders".into(),
            columns: serde_json::from_value(json!([
                {"name": "id", "type": "integer", "nullable": false,
                 "isPrimaryKey": true, "isForeignKey": false},
                {"name": "user_id", "type": "integer", "nullable": true,
                 "defaultValue": "0", "isPrimaryKey": false, "isForeignKey": true,
                 "foreignKeyRef": {"referencedSchema": "public", "referencedTable": "users",
                                   "referencedColumn": "id"}},
            ]))
            .unwrap(),
            indexes: serde_json::from_value(json!([
                {"name": "orders_pkey", "columns": ["id"], "unique": true, "type": "btree"},
            ]))
            .unwrap(),
        };
        let text = described_table(&t);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("NAME"), "{text}");
        assert!(lines[2].contains("PK") && lines[2].contains("no"), "{text}");
        assert!(lines[3].ends_with("FK → users.id"), "{text}");
        assert_eq!(lines[4], "", "{text}");
        assert!(lines[5].starts_with("INDEX"), "{text}");
        assert!(
            lines[7].starts_with("orders_pkey") && lines[7].ends_with("yes"),
            "{text}"
        );
    }
}
