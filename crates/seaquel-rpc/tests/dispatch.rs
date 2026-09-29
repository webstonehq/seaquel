//! `dispatch` routes every `EngineRequest` to the right Core call, and each
//! request and response keeps its wire shape.
//!
//! The Core has two mock engines: one with no Rust dialect or introspection
//! (what an engine looks like before its dialect moves to Rust), registered
//! as "duckdb" only because `DriverType` is a closed enum and a made-up id
//! can't connect (the real DuckDB engine isn't in this Core, so its dialect
//! won't change the test), and a "postgres" engine whose dialect and driver
//! echo their arguments, so a test can see what reached them.

use std::sync::{Arc, Mutex};

use seaquel_core::Core;
use seaquel_engine::{
    CastMap, ConnectConfig, DatabaseStatistics, DbError, Dialect, Driver, Engine, ExecuteResult,
    ExplainResult, QueryResult, RowValues, SchemaColumn, SchemaIndex, SchemaTable, SqlWithBindings,
    Value,
};
use seaquel_rpc::{dispatch_on, EngineRequest, EngineResponse};
use seaquel_types::{ColumnTypeInfo, CreateTableDefinition};
use serde_json::{json, Value as Json};

// ── Mocks ──

fn row_text(row: &[(String, Value)]) -> String {
    row.iter()
        .map(|(k, v)| format!("{k}={}", serde_json::to_string(v).unwrap()))
        .collect::<Vec<_>>()
        .join(",")
}

fn casts_text(casts: Option<&CastMap>) -> String {
    match casts {
        None => "none".into(),
        Some(c) => {
            let mut pairs: Vec<_> = c.iter().map(|(k, v)| format!("{k}:{v}")).collect();
            pairs.sort();
            pairs.join(",")
        }
    }
}

/// Every output spells out its inputs, row order included.
struct EchoDialect;

impl Dialect for EchoDialect {
    fn quote_ident(&self, id: &str) -> String {
        format!("[{id}]")
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
        SqlWithBindings {
            sql: format!(
                "update {schema}.{table}.{column} pks={} row={} casts={}",
                pks.join(","),
                row_text(row),
                casts_text(casts)
            ),
            bind_values: Some(vec![value]),
        }
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
        SqlWithBindings {
            sql: format!(
                "default {schema}.{table}.{column} pks={} row={} casts={}",
                pks.join(","),
                row_text(row),
                casts_text(casts)
            ),
            bind_values: None,
        }
    }
    fn build_insert(
        &self,
        schema: &str,
        table: &str,
        values: &[(String, Value)],
        casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        SqlWithBindings {
            sql: format!(
                "insert {schema}.{table} values={} casts={}",
                row_text(values),
                casts_text(casts)
            ),
            bind_values: Some(values.iter().map(|(_, v)| v.clone()).collect()),
        }
    }
    fn build_delete(
        &self,
        schema: &str,
        table: &str,
        pks: &[String],
        row: &RowValues,
        casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        SqlWithBindings {
            sql: format!(
                "delete {schema}.{table} pks={} row={} casts={}",
                pks.join(","),
                row_text(row),
                casts_text(casts)
            ),
            bind_values: None,
        }
    }
    fn create_table(&self, def: &CreateTableDefinition) -> String {
        format!("create {}.{}", def.schema_name, def.table_name)
    }
    fn alter_table(&self, from: &CreateTableDefinition, to: &CreateTableDefinition) -> String {
        format!("alter {} -> {}", from.table_name, to.table_name)
    }
    fn column_types(&self) -> Vec<ColumnTypeInfo> {
        vec![serde_json::from_value(json!({
            "name": "integer", "category": "Numeric", "hasLength": false, "hasPrecision": false
        }))
        .unwrap()]
    }
    fn explain_sql(&self, sql: &str, analyze: bool) -> String {
        format!("EXPLAIN {analyze} {sql}")
    }
}

/// Implements introspection and records each call.
#[derive(Default)]
struct EchoDriver {
    calls: Mutex<Vec<String>>,
}

#[seaquel_runtime::async_trait]
impl Driver for EchoDriver {
    async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        unimplemented!()
    }
    async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        unimplemented!()
    }
    async fn close(&self) -> Result<(), DbError> {
        Ok(())
    }
    async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
        self.calls.lock().unwrap().push("list_schemas".into());
        Ok(vec!["public".into(), "sales".into()])
    }
    async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
        self.calls.lock().unwrap().push("schema_tables".into());
        Ok(vec![serde_json::from_value(json!({
            "name": "orders", "schema": "sales", "type": "table", "columns": [], "indexes": []
        }))
        .unwrap()])
    }
    async fn table_metadata(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("table_metadata {schema}.{table}"));
        let column = serde_json::from_value(json!({
            "name": "id", "type": "integer", "nullable": false,
            "isPrimaryKey": true, "isForeignKey": false,
            "isUnique": true, "inUniqueConstraint": true
        }))
        .unwrap();
        let index = serde_json::from_value(json!({
            "name": "orders_pkey", "columns": ["id"], "unique": true, "type": "btree"
        }))
        .unwrap();
        Ok((vec![column], vec![index]))
    }
    async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
        self.calls.lock().unwrap().push("statistics".into());
        Ok(serde_json::from_value(json!({
            "overview": { "databaseName": "mock", "totalSize": "0 B", "tableCount": 0, "indexCount": 0 },
            "tableSizes": [],
            "indexUsage": []
        }))
        .unwrap())
    }
    async fn explain(
        &self,
        sql: &str,
        params: Vec<Value>,
        analyze: bool,
    ) -> Result<ExplainResult, DbError> {
        self.calls.lock().unwrap().push(format!(
            "explain {sql} {} {analyze}",
            serde_json::to_string(&params).unwrap()
        ));
        Ok(serde_json::from_value(json!({
            "plan": { "id": "0", "nodeType": "Result", "children": [] },
            "planningTime": 0.5,
            "isAnalyze": analyze
        }))
        .unwrap())
    }
}

struct EchoEngine(Arc<EchoDriver>);

#[seaquel_runtime::async_trait]
impl Engine for EchoEngine {
    fn id(&self) -> &'static str {
        "postgres"
    }
    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(self.0.clone())
    }
    fn dialect(&self) -> Option<&dyn Dialect> {
        Some(&EchoDialect)
    }
}

/// A driver with only the required methods: every introspection call gets
/// the trait's `NOT_SUPPORTED` default.
struct BareDriver;

#[seaquel_runtime::async_trait]
impl Driver for BareDriver {
    async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
        })
    }
    async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        Ok(ExecuteResult {
            rows_affected: 0,
            last_insert_id: None,
        })
    }
    async fn close(&self) -> Result<(), DbError> {
        Ok(())
    }
}

/// An engine without a Rust dialect.
struct NoDialectEngine;

#[seaquel_runtime::async_trait]
impl Engine for NoDialectEngine {
    /// Any `DriverType` id works: a made-up one can't be connected, since
    /// `ConnectConfig.driver` only accepts the five real engines.
    fn id(&self) -> &'static str {
        "duckdb"
    }
    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(BareDriver))
    }
}

struct Fixture {
    core: Core,
    driver: Arc<EchoDriver>,
    /// A connection on the mock "postgres" engine.
    pg: String,
    /// A connection on the bare engine.
    bare: String,
}

async fn fixture() -> Fixture {
    let driver = Arc::new(EchoDriver::default());
    // Not `with_default_plugins()`: in a workspace build Cargo unifies
    // seaquel-core's features, so that could already register "postgres".
    let core = Core::builder()
        .engine(Arc::new(NoDialectEngine))
        .engine(Arc::new(EchoEngine(driver.clone())))
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    let connect = |config: Json| {
        let core = &core;
        async move {
            let config: ConnectConfig = serde_json::from_value(config).unwrap();
            core.connect(&config).await.unwrap().connection_id
        }
    };
    let pg = connect(json!({ "driver": "postgres" })).await;
    let bare = connect(json!({ "driver": "duckdb" })).await;
    Fixture {
        core,
        driver,
        pg,
        bare,
    }
}

/// Parse a wire request, run it on `connection_id`, and return the response
/// as wire JSON.
async fn call(core: &Core, connection_id: &str, request: Json) -> Result<Json, DbError> {
    let request: EngineRequest = serde_json::from_value(request).expect("request must deserialize");
    let response = dispatch_on(&core.connection_handle(connection_id), request).await?;
    Ok(serde_json::to_value(response).unwrap())
}

fn definition(name: &str) -> Json {
    json!({ "tableName": name, "schemaName": "public", "columns": [], "indexes": [], "foreignKeys": [] })
}

// ── Introspection ──

#[tokio::test]
async fn list_schemas() {
    let f = fixture().await;
    let out = call(&f.core, &f.pg, json!({ "method": "listSchemas" }))
        .await
        .unwrap();
    assert_eq!(
        out,
        json!({ "kind": "schemas", "data": ["public", "sales"] })
    );
}

#[tokio::test]
async fn schema_tables() {
    let f = fixture().await;
    let out = call(&f.core, &f.pg, json!({ "method": "schemaTables" }))
        .await
        .unwrap();
    assert_eq!(out["kind"], "tables");
    assert_eq!(out["data"][0]["name"], "orders");
    assert_eq!(out["data"][0]["schema"], "sales");
}

#[tokio::test]
async fn table_metadata() {
    let f = fixture().await;
    let out = call(
        &f.core,
        &f.pg,
        json!({ "method": "tableMetadata", "params": { "schema": "sales", "table": "order items" } }),
    )
    .await
    .unwrap();
    assert_eq!(out["kind"], "tableMetadata");
    assert_eq!(out["data"]["columns"][0]["name"], "id");
    assert_eq!(out["data"]["indexes"][0]["name"], "orders_pkey");
    assert_eq!(out["data"]["columns"][0]["isUnique"], true);
    assert_eq!(out["data"]["columns"][0]["inUniqueConstraint"], true);
    assert_eq!(
        *f.driver.calls.lock().unwrap(),
        vec!["table_metadata sales.order items"]
    );
}

#[tokio::test]
async fn statistics() {
    let f = fixture().await;
    let out = call(&f.core, &f.pg, json!({ "method": "statistics" }))
        .await
        .unwrap();
    assert_eq!(out["kind"], "statistics");
    assert_eq!(out["data"]["overview"]["databaseName"], "mock");
}

#[tokio::test]
async fn explain_decodes_tagged_params() {
    let f = fixture().await;
    let out = call(
        &f.core,
        &f.pg,
        json!({ "method": "explain", "params": {
            "sql": "SELECT $1, $2",
            "params": [{ "$sq": "bigint", "v": "9007199254740993" }, "x"],
            "analyze": true
        } }),
    )
    .await
    .unwrap();
    assert_eq!(out["kind"], "explain");
    assert_eq!(out["data"]["isAnalyze"], true);
    assert_eq!(
        *f.driver.calls.lock().unwrap(),
        vec![r#"explain SELECT $1, $2 [{"$sq":"bigint","v":"9007199254740993"},"x"] true"#]
    );
}

// ── Dialect ──

#[tokio::test]
async fn column_types() {
    let f = fixture().await;
    let out = call(&f.core, &f.pg, json!({ "method": "columnTypes" }))
        .await
        .unwrap();
    assert_eq!(out["kind"], "columnTypes");
    assert_eq!(out["data"][0]["name"], "integer");
}

#[tokio::test]
async fn create_table() {
    let f = fixture().await;
    let out = call(
        &f.core,
        &f.pg,
        json!({ "method": "createTable", "params": { "definition": definition("people") } }),
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        json!({ "kind": "sql", "data": "create public.people" })
    );
}

#[tokio::test]
async fn alter_table() {
    let f = fixture().await;
    let out = call(
        &f.core,
        &f.pg,
        json!({ "method": "alterTable", "params": { "from": definition("a"), "to": definition("b") } }),
    )
    .await
    .unwrap();
    assert_eq!(out, json!({ "kind": "sql", "data": "alter a -> b" }));
}

// ── Errors ──

/// One request per variant, for the error tests.
fn every_request() -> Vec<Json> {
    vec![
        json!({ "method": "listSchemas" }),
        json!({ "method": "schemaTables" }),
        json!({ "method": "tableMetadata", "params": { "schema": "s", "table": "t" } }),
        json!({ "method": "statistics" }),
        json!({ "method": "explain", "params": { "sql": "SELECT 1", "params": [], "analyze": false } }),
        json!({ "method": "columnTypes" }),
        json!({ "method": "createTable", "params": { "definition": definition("t") } }),
        json!({ "method": "alterTable", "params": { "from": definition("t"), "to": definition("t") } }),
    ]
}

#[tokio::test]
async fn every_variant_without_a_rust_dialect_is_not_supported() {
    let f = fixture().await;
    let requests = every_request();
    assert_eq!(requests.len(), 8, "one per EngineRequest variant");
    for request in requests {
        let err = call(&f.core, &f.bare, request.clone()).await.unwrap_err();
        assert_eq!(err.code, "NOT_SUPPORTED", "{request}: {}", err.message);
    }
}

#[tokio::test]
async fn every_variant_on_an_unknown_connection_is_not_found() {
    let f = fixture().await;
    for request in every_request() {
        let err = call(&f.core, "nope", request.clone()).await.unwrap_err();
        assert_eq!(err.code, "CONNECTION_NOT_FOUND", "{request}");
    }
}

#[test]
fn malformed_requests_are_rejected() {
    let parse = |j: Json| serde_json::from_value::<EngineRequest>(j);
    // Unknown method.
    assert!(parse(json!({ "method": "dropEverything" })).is_err());
    // Missing params.
    assert!(parse(json!({ "method": "tableMetadata" })).is_err());
    // Bad value tag.
    assert!(parse(json!({ "method": "explain", "params": {
        "sql": "x", "params": [{ "$sq": "nope", "v": 1 }], "analyze": false
    } }))
    .is_err());
}

#[test]
fn requests_round_trip_through_serde() {
    for request in every_request() {
        let parsed: EngineRequest = serde_json::from_value(request.clone()).unwrap();
        let back = serde_json::to_value(&parsed).unwrap();
        let reparsed: EngineRequest = serde_json::from_value(back).unwrap();
        assert_eq!(parsed, reparsed, "{request}");
    }
}

#[test]
fn responses_serialize_with_kind_and_data() {
    let out = serde_json::to_value(EngineResponse::Sql("CREATE TABLE t ()".into())).unwrap();
    assert_eq!(out, json!({ "kind": "sql", "data": "CREATE TABLE t ()" }));
}

/// Paging and the CRUD builders are Core's now (phase 5c, Decision 14):
/// `db.tablePage`, `db.planEdits` and `db.applyChanges` build with the
/// dialect. The engine RPC doesn't know those methods any more.
#[test]
fn removed_engine_calls_are_unknown_methods() {
    let row = json!([["id", 1]]);
    for request in [
        json!({ "method": "paginate", "params": { "sql": "SELECT 1", "limit": 1, "offset": 0 } }),
        json!({ "method": "buildUpdate", "params": { "schema": "s", "table": "t", "column": "c", "value": 1, "primary_keys": ["id"], "row": row } }),
        json!({ "method": "buildSetDefault", "params": { "schema": "s", "table": "t", "column": "c", "primary_keys": ["id"], "row": row } }),
        json!({ "method": "buildInsert", "params": { "schema": "s", "table": "t", "values": row } }),
        json!({ "method": "buildDelete", "params": { "schema": "s", "table": "t", "primary_keys": ["id"], "row": row } }),
    ] {
        let err = serde_json::from_value::<EngineRequest>(request.clone()).unwrap_err();
        assert!(
            err.to_string().contains("unknown variant"),
            "{request}: {err}"
        );
        // And through the workspace RPC: a bad request, before anything runs.
        let body = json!({"method": "db", "params": {"method": "engine", "params": {
            "connectionId": "c", "request": request}}});
        let err = seaquel_rpc::parse_request(body.to_string().as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{request}");
    }
}
