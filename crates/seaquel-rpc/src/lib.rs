//! The RPCs the GUIs call Core through.
//!
//! - The workspace RPC ([`Request`], [`Response`], [`dispatch_workspace`]):
//!   metadata storage, secrets and the `db` group (connect, queries, engine
//!   calls on the workspace's own connections), served as the `core_call`
//!   Tauri command and `POST /rpc`. See the `workspace` and `db` modules.
//! - Query streams ([`dispatch_stream`]) and workspace events
//!   ([`workspace_events`]) as [`CoreEvent`]s, for the desktop's
//!   `core_stream`/`core_events` and the web's `/rpc/stream`.
//! - The engine RPC, below.
//!
//! The engine RPC: one request/response pair for every dialect-dependent call
//! the frontend makes (introspection, EXPLAIN, and SQL generation), and a
//! dispatcher onto Core.
//!
//! The GUIs reach it as `db.engine`: an [`EngineRequest`] on one of the
//! workspace's connections, run through [`dispatch_on`].
//!
//! Wire shape (the request, then the response):
//!
//! ```json
//! {"method":"tableMetadata","params":{"schema":"public","table":"t"}}
//! {"kind":"tableMetadata","data":{"columns":[…],"indexes":[…]}}
//! ```
//!
//! Values use the tagged `Value` wire format from `seaquel-types`. A row
//! ([`RowValues`]) is an array of `[column, value]` pairs, so its column order
//! survives JSON (serde_json objects don't keep key order).

mod db;
mod git;
mod license;
mod ssh;
mod workspace;
pub use db::{
    dispatch_stream, workspace_events, ConnectParams, ConnectTargetParams, Connected, CoreEvent,
    DbRequest, DbResponse, QueryStreamParams, CONNECTION_CLOSED, TUNNEL_CLOSED, WORKSPACE_EVICTED,
};
#[cfg(feature = "git")]
pub use git::dispatch_git;
pub use git::{GitRequest, GitResponse};
#[cfg(feature = "license-desktop")]
pub use license::dispatch_license;
pub use license::{DesktopLicenseRequest, DesktopLicenseResponse, LicenseResponse};
pub use ssh::{dispatch_ssh, SshRequest, SshResponse, TunnelConfig, TunnelInfo};
#[cfg(feature = "secrets")]
pub use workspace::dispatch_secret;
pub use workspace::{
    dispatch_workspace, parse_request, Request, Response, RpcError, SecretRequest, SecretResponse,
    StorageRequest, StorageResponse, INVALID_ARGUMENT, NOT_SUPPORTED,
};

use seaquel_core::ConnectionHandle;
use seaquel_engine::{CastMap, RowValues};
use seaquel_types::{
    ColumnTypeInfo, CreateTableDefinition, DatabaseStatistics, DbError, ExplainResult,
    SchemaColumn, SchemaIndex, SchemaTable, SqlWithBindings, Value,
};
use serde::{Deserialize, Serialize};

/// What to run. `{"method": <camelCase variant>, "params": {…}}`; variants
/// without fields have no `params`. Field names are the Rust ones
/// (`primary_keys`).
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
        /// The column's default expression from its metadata (`defaultValue`,
        /// or `NULL` when it has none). SQLite, which has no `DEFAULT` in
        /// `UPDATE`, assigns it; other engines ignore it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        column_default: Option<String>,
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

/// Run one call on the connection `handle` names. From `Workspace::engine`,
/// a connection the workspace doesn't own is `CONNECTION_NOT_FOUND`.
pub async fn dispatch_on(
    handle: &ConnectionHandle<'_>,
    request: EngineRequest,
) -> Result<EngineResponse, DbError> {
    use EngineRequest as Req;
    use EngineResponse as Res;

    let core = handle;
    match request {
        Req::ListSchemas => core.list_schemas().await.map(Res::Schemas),
        Req::SchemaTables => core.schema_tables().await.map(Res::Tables),
        Req::TableMetadata { schema, table } => core
            .table_metadata(&schema, &table)
            .await
            .map(|(columns, indexes)| Res::TableMetadata { columns, indexes }),
        Req::Statistics => core.statistics().await.map(Res::Statistics),
        Req::Explain {
            sql,
            params,
            analyze,
        } => core
            .explain(&sql, params, analyze)
            .await
            .map(|plan| Res::Explain(Box::new(plan))),
        Req::ColumnTypes => core.with_dialect(|d| Res::ColumnTypes(d.column_types())),
        Req::Paginate { sql, limit, offset } => {
            core.with_dialect(|d| Res::Sql(d.paginate(&sql, limit, offset)))
        }
        Req::BuildUpdate {
            schema,
            table,
            column,
            value,
            primary_keys,
            row,
            casts,
        } => core.with_dialect(|d| {
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
            column_default,
            primary_keys,
            row,
            casts,
        } => core.with_dialect(|d| {
            Res::SqlWithBindings(d.build_set_default_expr(
                &schema,
                &table,
                &column,
                column_default.as_deref(),
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
        } => core.with_dialect(|d| {
            Res::SqlWithBindings(d.build_insert(&schema, &table, &values, casts.as_ref()))
        }),
        Req::BuildDelete {
            schema,
            table,
            primary_keys,
            row,
            casts,
        } => core.with_dialect(|d| {
            Res::SqlWithBindings(d.build_delete(
                &schema,
                &table,
                &primary_keys,
                &row,
                casts.as_ref(),
            ))
        }),
        Req::CreateTable { definition } => {
            core.with_dialect(|d| Res::Sql(d.create_table(&definition)))
        }
        Req::AlterTable { from, to } => core.with_dialect(|d| Res::Sql(d.alter_table(&from, &to))),
    }
}
