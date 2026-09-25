use futures::StreamExt;
use seaquel_core::{Core, StreamEvent};
use seaquel_rpc::{EngineCall, EngineResponse};
use seaquel_types::{
    BatchStatement, ConnectConfig, ConnectResult, DbError, ExecuteResult, QueryResult, Value,
};
use tauri::{command, ipc::Channel, State};

#[command]
pub async fn db_connect(
    config: ConnectConfig,
    core: State<'_, Core>,
) -> Result<ConnectResult, DbError> {
    core.connect(&config).await
}

#[command]
pub async fn db_query(
    connection_id: String,
    sql: String,
    values: Vec<Value>,
    core: State<'_, Core>,
) -> Result<QueryResult, DbError> {
    core.query(&connection_id, &sql, values).await
}

/// Streams results through `on_event`: `batch` events, then `done` or
/// `error`. A stream the client cancelled just stops; one cut off by a
/// disconnect ends with a `CONNECTION_CLOSED` error. Errors arrive as events rather
/// than as a rejected invoke, so the frontend has one termination path.
#[command]
pub async fn db_query_stream(
    query_id: String,
    connection_id: String,
    sql: String,
    values: Vec<Value>,
    on_event: Channel<StreamEvent>,
    core: State<'_, Core>,
) -> Result<(), DbError> {
    let mut events = core.query_stream(query_id, connection_id, sql, values);
    while let Some(event) = events.next().await {
        // Err means the webview dropped the channel. Stop; dropping `events`
        // stops the driver's fetch and releases its connection.
        if on_event.send(event).is_err() {
            break;
        }
    }
    Ok(())
}

#[command]
pub async fn db_cancel_stream(query_id: String, core: State<'_, Core>) -> Result<(), DbError> {
    core.cancel_stream(&query_id);
    Ok(())
}

#[command]
pub async fn db_execute(
    connection_id: String,
    sql: String,
    values: Vec<Value>,
    core: State<'_, Core>,
) -> Result<ExecuteResult, DbError> {
    core.execute(&connection_id, &sql, values).await
}

#[command]
pub async fn db_transaction(
    connection_id: String,
    statements: Vec<BatchStatement>,
    core: State<'_, Core>,
) -> Result<(), DbError> {
    core.transaction(&connection_id, statements).await
}

/// Introspection, EXPLAIN and SQL generation through the connection's Rust
/// dialect. `NOT_SUPPORTED` means the engine's dialect is still in TypeScript.
#[command]
pub async fn db_engine(call: EngineCall, core: State<'_, Core>) -> Result<EngineResponse, DbError> {
    seaquel_rpc::dispatch(&core, call).await
}

#[command]
pub async fn db_disconnect(connection_id: String, core: State<'_, Core>) -> Result<(), DbError> {
    core.disconnect(&connection_id).await
}

#[command]
pub async fn db_test(config: ConnectConfig, core: State<'_, Core>) -> Result<(), DbError> {
    core.test(&config).await
}
