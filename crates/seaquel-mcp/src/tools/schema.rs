//! `list_schemas`, `list_tables` and `describe_table`: refused when the
//! connection doesn't share its schema.

use seaquel_types::SchemaTable;
use serde_json::{json, Value as Json};

use super::{require_schema, ConnectionArgs, DescribeTableArgs, ListTablesArgs};
use crate::error::ToolError;
use crate::server::Inner;

pub const TABLE_NOT_FOUND: &str = "TABLE_NOT_FOUND";
pub const AMBIGUOUS_TABLE: &str = "AMBIGUOUS_TABLE";

pub(crate) async fn list_schemas(inner: &Inner, args: ConnectionArgs) -> Result<Json, ToolError> {
    let c = inner.resolve(&args.connection)?;
    require_schema(inner, c).await?;
    let schemas = inner
        .timed(async {
            let id = inner.connection(c).await?;
            Ok(inner.core.list_schemas(&id).await?)
        })
        .await?;
    Ok(json!({ "schemas": schemas }))
}

fn kind(t: &SchemaTable) -> Json {
    serde_json::to_value(t.kind).unwrap_or(Json::Null)
}

pub(crate) async fn list_tables(inner: &Inner, args: ListTablesArgs) -> Result<Json, ToolError> {
    let c = inner.resolve(&args.connection)?;
    require_schema(inner, c).await?;
    let tables = inner
        .timed(async {
            let id = inner.connection(c).await?;
            Ok(inner.core.schema_tables(&id).await?)
        })
        .await?;
    let tables: Vec<Json> = tables
        .iter()
        .filter(|t| args.schema.as_ref().is_none_or(|s| *s == t.schema))
        .map(|t| {
            let mut entry = json!({ "schema": t.schema, "name": t.name, "type": kind(t) });
            if let Some(rows) = t.row_count {
                entry["approxRows"] = json!(rows);
            }
            entry
        })
        .collect();
    Ok(json!({ "tables": tables }))
}

pub(crate) async fn describe_table(
    inner: &Inner,
    args: DescribeTableArgs,
) -> Result<Json, ToolError> {
    let c = inner.resolve(&args.connection)?;
    require_schema(inner, c).await?;
    let (schema, kind, columns, indexes) = inner
        .timed(async {
            let id = inner.connection(c).await?;
            // The listed table gives the schema when none was passed, the
            // kind, and whether it exists at all.
            let tables = inner.core.schema_tables(&id).await?;
            let matches: Vec<&SchemaTable> = tables
                .iter()
                .filter(|t| {
                    t.name == args.table && args.schema.as_ref().is_none_or(|s| *s == t.schema)
                })
                .collect();
            let table = match matches.as_slice() {
                [t] => *t,
                [] => {
                    return Err(ToolError::new(
                        TABLE_NOT_FOUND,
                        match &args.schema {
                            Some(s) => format!("No table {:?} in schema {s:?}", args.table),
                            None => format!("No table {:?}", args.table),
                        },
                    ))
                }
                many => {
                    return Err(ToolError::new(
                        AMBIGUOUS_TABLE,
                        format!(
                            "Tables named {:?} exist in several schemas ({}); pass `schema`",
                            args.table,
                            many.iter()
                                .map(|t| format!("{:?}", t.schema))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ))
                }
            };
            let (columns, indexes) = inner
                .core
                .table_metadata(&id, &table.schema, &table.name)
                .await?;
            Ok((table.schema.clone(), kind(table), columns, indexes))
        })
        .await?;

    let foreign_keys: Vec<Json> = columns
        .iter()
        .filter_map(|col| {
            col.foreign_key_ref.as_ref().map(|r| {
                json!({
                    "column": col.name,
                    "referencedSchema": r.referenced_schema,
                    "referencedTable": r.referenced_table,
                    "referencedColumn": r.referenced_column,
                })
            })
        })
        .collect();
    let columns: Vec<Json> = columns
        .iter()
        .map(|col| {
            json!({
                "name": col.name,
                "type": col.ty,
                "nullable": col.nullable,
                "default": col.default_value,
                "primaryKey": col.is_primary_key,
            })
        })
        .collect();
    let indexes: Vec<Json> = indexes
        .iter()
        .map(|i| json!({ "name": i.name, "columns": i.columns, "unique": i.unique, "type": i.ty }))
        .collect();
    Ok(json!({
        "schema": schema,
        "table": args.table,
        "type": kind,
        "columns": columns,
        "indexes": indexes,
        "foreignKeys": foreign_keys,
    }))
}
