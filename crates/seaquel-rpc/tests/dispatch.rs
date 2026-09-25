//! `dispatch` routes every `EngineRequest` to the right Core call, and each
//! request and response keeps its wire shape.
//!
//! The Core has two engines: the real SQLite engine (no Rust dialect, no
//! introspection yet) and a mock "postgres" engine whose dialect and driver
//! echo their arguments, so a test can see what reached them.

use std::sync::{Arc, Mutex};

use seaquel_core::Core;
use seaquel_engine::{
    CastMap, ConnectConfig, DatabaseStatistics, DbError, Dialect, Driver, Engine, ExecuteResult,
    ExplainResult, QueryResult, RowValues, SchemaColumn, SchemaIndex, SchemaTable, SqlWithBindings,
    Value,
};
use seaquel_rpc::{dispatch, EngineCall, EngineRequest, EngineResponse};
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
            "isPrimaryKey": true, "isForeignKey": false
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

struct Fixture {
    core: Core,
    driver: Arc<EchoDriver>,
    /// A connection on the mock "postgres" engine.
    pg: String,
    /// A real in-memory SQLite connection.
    lite: String,
}

async fn fixture() -> Fixture {
    let driver = Arc::new(EchoDriver::default());
    // Not `with_default_plugins()`: in a workspace build Cargo unifies
    // seaquel-core's features, so that could already register "postgres".
    let core = Core::builder()
        .engine(seaquel_engine_sqlite::engine())
        .engine(Arc::new(EchoEngine(driver.clone())))
        .build();
    let connect = |config: Json| {
        let core = &core;
        async move {
            let config: ConnectConfig = serde_json::from_value(config).unwrap();
            core.connect(&config).await.unwrap().connection_id
        }
    };
    let pg = connect(json!({ "driver": "postgres" })).await;
    let lite = connect(json!({ "driver": "sqlite", "connection_string": "sqlite::memory:" })).await;
    Fixture {
        core,
        driver,
        pg,
        lite,
    }
}

/// Parse a wire call, dispatch it, and return the response as wire JSON.
async fn call(core: &Core, connection_id: &str, request: Json) -> Result<Json, DbError> {
    let call: EngineCall =
        serde_json::from_value(json!({ "connection_id": connection_id, "request": request }))
            .expect("request must deserialize");
    let response = dispatch(core, call).await?;
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
async fn paginate() {
    let f = fixture().await;
    let out = call(
        &f.core,
        &f.pg,
        json!({ "method": "paginate", "params": { "sql": "SELECT 1", "limit": 50, "offset": 100 } }),
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        json!({ "kind": "sql", "data": "SELECT 1 LIMIT 50 OFFSET 100" })
    );
}

#[tokio::test]
async fn build_update_keeps_row_order_and_values() {
    let f = fixture().await;
    let out = call(
        &f.core,
        &f.pg,
        json!({ "method": "buildUpdate", "params": {
            "schema": "public", "table": "t", "column": "price",
            "value": { "$sq": "decimal", "v": "12.50" },
            "primary_keys": ["id", "b"],
            // Not alphabetical: pairs must keep their order.
            "row": [["z", 1], ["id", 2], ["b", { "$sq": "bytes", "v": "AQI=" }]],
            "casts": { "price": "numeric" }
        } }),
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        json!({ "kind": "sqlWithBindings", "data": {
            "sql": r#"update public.t.price pks=id,b row=z=1,id=2,b={"$sq":"bytes","v":"AQI="} casts=price:numeric"#,
            "bindValues": [{ "$sq": "decimal", "v": "12.50" }]
        } })
    );
}

#[tokio::test]
async fn build_update_without_casts() {
    let f = fixture().await;
    for casts in [json!(null), json!("omitted")] {
        let mut params = json!({
            "schema": "public", "table": "t", "column": "c", "value": null,
            "primary_keys": ["id"], "row": [["id", 1]]
        });
        if casts.is_null() {
            params["casts"] = Json::Null;
        }
        let out = call(
            &f.core,
            &f.pg,
            json!({ "method": "buildUpdate", "params": params }),
        )
        .await
        .unwrap();
        assert!(
            out["data"]["sql"].as_str().unwrap().ends_with("casts=none"),
            "{out}"
        );
    }
}

#[tokio::test]
async fn build_set_default() {
    let f = fixture().await;
    let out = call(
        &f.core,
        &f.pg,
        json!({ "method": "buildSetDefault", "params": {
            "schema": "public", "table": "t", "column": "c",
            "primary_keys": ["id"], "row": [["id", 7], ["c", "x"]],
            "casts": { "id": "uuid" }
        } }),
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        json!({ "kind": "sqlWithBindings", "data": {
            "sql": r#"default public.t.c pks=id row=id=7,c="x" casts=id:uuid"#
        } })
    );
}

#[tokio::test]
async fn build_insert() {
    let f = fixture().await;
    let out = call(
        &f.core,
        &f.pg,
        json!({ "method": "buildInsert", "params": {
            "schema": "public", "table": "t",
            "values": [["name", "a"], ["age", 3], ["meta", { "$sq": "json", "v": { "k": 1 } }]],
            "casts": null
        } }),
    )
    .await
    .unwrap();
    assert_eq!(
        out["data"]["sql"],
        r#"insert public.t values=name="a",age=3,meta={"$sq":"json","v":{"k":1}} casts=none"#
    );
    assert_eq!(
        out["data"]["bindValues"],
        json!(["a", 3, { "$sq": "json", "v": { "k": 1 } }])
    );
}

#[tokio::test]
async fn build_delete() {
    let f = fixture().await;
    let out = call(
        &f.core,
        &f.pg,
        json!({ "method": "buildDelete", "params": {
            "schema": "public", "table": "t", "primary_keys": ["id"], "row": [["id", 9]]
        } }),
    )
    .await
    .unwrap();
    // `casts` is optional.
    assert_eq!(out["data"]["sql"], "delete public.t pks=id row=id=9 casts=none");
    let out = call(
        &f.core,
        &f.pg,
        json!({ "method": "buildDelete", "params": {
            "schema": "public", "table": "t", "primary_keys": ["id"], "row": [["id", 9]],
            "casts": { "id": "uuid" }
        } }),
    )
    .await
    .unwrap();
    assert_eq!(out["data"]["sql"], "delete public.t pks=id row=id=9 casts=id:uuid");
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
    let row = json!([["id", 1]]);
    vec![
        json!({ "method": "listSchemas" }),
        json!({ "method": "schemaTables" }),
        json!({ "method": "tableMetadata", "params": { "schema": "s", "table": "t" } }),
        json!({ "method": "statistics" }),
        json!({ "method": "explain", "params": { "sql": "SELECT 1", "params": [], "analyze": false } }),
        json!({ "method": "columnTypes" }),
        json!({ "method": "paginate", "params": { "sql": "SELECT 1", "limit": 1, "offset": 0 } }),
        json!({ "method": "buildUpdate", "params": { "schema": "s", "table": "t", "column": "c", "value": 1, "primary_keys": ["id"], "row": row } }),
        json!({ "method": "buildSetDefault", "params": { "schema": "s", "table": "t", "column": "c", "primary_keys": ["id"], "row": row } }),
        json!({ "method": "buildInsert", "params": { "schema": "s", "table": "t", "values": row } }),
        json!({ "method": "buildDelete", "params": { "schema": "s", "table": "t", "primary_keys": ["id"], "row": row } }),
        json!({ "method": "createTable", "params": { "definition": definition("t") } }),
        json!({ "method": "alterTable", "params": { "from": definition("t"), "to": definition("t") } }),
    ]
}

#[tokio::test]
async fn every_variant_on_sqlite_is_not_supported() {
    let f = fixture().await;
    let requests = every_request();
    assert_eq!(requests.len(), 13, "one per EngineRequest variant");
    for request in requests {
        let err = call(&f.core, &f.lite, request.clone()).await.unwrap_err();
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
    let parse = |j: Json| serde_json::from_value::<EngineCall>(j);
    // Unknown method.
    assert!(
        parse(json!({ "connection_id": "c", "request": { "method": "dropEverything" } })).is_err()
    );
    // Missing params.
    assert!(parse(json!({ "connection_id": "c", "request": { "method": "paginate" } })).is_err());
    // A row must be pairs, not an object.
    assert!(parse(
        json!({ "connection_id": "c", "request": { "method": "buildDelete", "params": {
        "schema": "s", "table": "t", "primary_keys": [], "row": { "id": 1 }
    } } })
    )
    .is_err());
    // Bad value tag.
    assert!(parse(
        json!({ "connection_id": "c", "request": { "method": "explain", "params": {
        "sql": "x", "params": [{ "$sq": "nope", "v": 1 }], "analyze": false
    } } })
    )
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
    let out = serde_json::to_value(EngineResponse::SqlWithBindings(SqlWithBindings {
        sql: "x".into(),
        bind_values: Some(vec![Value::Int(i64::MAX)]),
    }))
    .unwrap();
    assert_eq!(
        out,
        json!({ "kind": "sqlWithBindings", "data": {
            "sql": "x", "bindValues": [{ "$sq": "bigint", "v": "9223372036854775807" }]
        } })
    );
}
