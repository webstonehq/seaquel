//! The engine RPC: one request/response pair for every dialect-dependent call
//! the frontend makes (introspection, EXPLAIN, and SQL generation), and a
//! dispatcher onto [`Core`].
//!
//! The Tauri app exposes [`dispatch`] as the `db_engine` command and
//! `seaquel-server` as `POST /api/db/engine`. Both take an [`EngineCall`],
//! whose top-level `connection_id` lets the web app's Node proxy scope and
//! strip it like every other `/api/db/*` body.
//!
//! Wire shape:
//!
//! ```json
//! {"connection_id":"…","request":{"method":"tableMetadata","params":{"schema":"public","table":"t"}}}
//! {"kind":"tableMetadata","data":{"columns":[…],"indexes":[…]}}
//! ```
//!
//! Values use the tagged `Value` wire format from `seaquel-types`. A row
//! ([`RowValues`]) is an array of `[column, value]` pairs, so its column order
//! survives JSON (serde_json objects don't keep key order).

use seaquel_core::Core;
use seaquel_engine::{CastMap, RowValues};
use seaquel_types::{
    ColumnTypeInfo, CreateTableDefinition, DatabaseStatistics, DbError, ExplainResult,
    SchemaColumn, SchemaIndex, SchemaTable, SqlWithBindings, Value,
};
use serde::{Deserialize, Serialize};

/// One engine call on one connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EngineCall {
    pub connection_id: String,
    pub request: EngineRequest,
}

/// What to run. `{"method": <camelCase variant>, "params": {…}}`; variants
/// without fields have no `params`. Field names are the Rust ones
/// (`primary_keys`), like `connection_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum EngineRequest {
    ListSchemas,
    SchemaTables,
    TableMetadata {
        schema: String,
        table: String,
    },
    Statistics,
    Explain {
        sql: String,
        #[cfg_attr(feature = "ts", ts(type = "unknown[]"))]
        params: Vec<Value>,
        analyze: bool,
    },
    ColumnTypes,
    Paginate {
        sql: String,
        // ts-rs maps u64 to `bigint`, but serde_json reads plain numbers.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        limit: u64,
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        offset: u64,
    },
    BuildUpdate {
        schema: String,
        table: String,
        column: String,
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        value: Value,
        primary_keys: Vec<String>,
        #[cfg_attr(feature = "ts", ts(type = "Array<[string, unknown]>"))]
        row: RowValues,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        casts: Option<CastMap>,
    },
    BuildSetDefault {
        schema: String,
        table: String,
        column: String,
        primary_keys: Vec<String>,
        #[cfg_attr(feature = "ts", ts(type = "Array<[string, unknown]>"))]
        row: RowValues,
        /// Casts for the primary-key placeholders (bug fix 6).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        casts: Option<CastMap>,
    },
    BuildInsert {
        schema: String,
        table: String,
        #[cfg_attr(feature = "ts", ts(type = "Array<[string, unknown]>"))]
        values: RowValues,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        casts: Option<CastMap>,
    },
    BuildDelete {
        schema: String,
        table: String,
        primary_keys: Vec<String>,
        #[cfg_attr(feature = "ts", ts(type = "Array<[string, unknown]>"))]
        row: RowValues,
        /// Casts for the primary-key placeholders (bug fix 6).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        casts: Option<CastMap>,
    },
    CreateTable {
        definition: CreateTableDefinition,
    },
    AlterTable {
        from: CreateTableDefinition,
        to: CreateTableDefinition,
    },
}

/// The result, as `{"kind": <camelCase variant>, "data": …}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum EngineResponse {
    Schemas(Vec<String>),
    Tables(Vec<SchemaTable>),
    TableMetadata {
        columns: Vec<SchemaColumn>,
        indexes: Vec<SchemaIndex>,
    },
    Statistics(DatabaseStatistics),
    // Boxed: an ExplainResult is several times larger than the other variants.
    Explain(Box<ExplainResult>),
    ColumnTypes(Vec<ColumnTypeInfo>),
    Sql(String),
    SqlWithBindings(SqlWithBindings),
}

/// Run one call. Errors are Core's: `CONNECTION_NOT_FOUND` for an unknown
/// connection, `NOT_SUPPORTED` when the connection's engine has no Rust
/// dialect or introspection yet, and the driver's own errors otherwise.
pub async fn dispatch(core: &Core, call: EngineCall) -> Result<EngineResponse, DbError> {
    use EngineRequest as Req;
    use EngineResponse as Res;

    let id = call.connection_id.as_str();
    match call.request {
        Req::ListSchemas => core.list_schemas(id).await.map(Res::Schemas),
        Req::SchemaTables => core.schema_tables(id).await.map(Res::Tables),
        Req::TableMetadata { schema, table } => core
            .table_metadata(id, &schema, &table)
            .await
            .map(|(columns, indexes)| Res::TableMetadata { columns, indexes }),
        Req::Statistics => core.statistics(id).await.map(Res::Statistics),
        Req::Explain {
            sql,
            params,
            analyze,
        } => core
            .explain(id, &sql, params, analyze)
            .await
            .map(|plan| Res::Explain(Box::new(plan))),
        Req::ColumnTypes => core.with_dialect(id, |d| Res::ColumnTypes(d.column_types())),
        Req::Paginate { sql, limit, offset } => {
            core.with_dialect(id, |d| Res::Sql(d.paginate(&sql, limit, offset)))
        }
        Req::BuildUpdate {
            schema,
            table,
            column,
            value,
            primary_keys,
            row,
            casts,
        } => core.with_dialect(id, |d| {
            Res::SqlWithBindings(d.build_update(
                &schema,
                &table,
                &column,
                value,
                &primary_keys,
                &row,
                casts.as_ref(),
            ))
        }),
        Req::BuildSetDefault {
            schema,
            table,
            column,
            primary_keys,
            row,
            casts,
        } => core.with_dialect(id, |d| {
            Res::SqlWithBindings(d.build_set_default(
                &schema,
                &table,
                &column,
                &primary_keys,
                &row,
                casts.as_ref(),
            ))
        }),
        Req::BuildInsert {
            schema,
            table,
            values,
            casts,
        } => core.with_dialect(id, |d| {
            Res::SqlWithBindings(d.build_insert(&schema, &table, &values, casts.as_ref()))
        }),
        Req::BuildDelete {
            schema,
            table,
            primary_keys,
            row,
            casts,
        } => core.with_dialect(id, |d| {
            Res::SqlWithBindings(d.build_delete(
                &schema,
                &table,
                &primary_keys,
                &row,
                casts.as_ref(),
            ))
        }),
        Req::CreateTable { definition } => {
            core.with_dialect(id, |d| Res::Sql(d.create_table(&definition)))
        }
        Req::AlterTable { from, to } => {
            core.with_dialect(id, |d| Res::Sql(d.alter_table(&from, &to)))
        }
    }
}
