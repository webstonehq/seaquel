//! WebSocket streaming endpoint. The Tauri counterpart is `db_query_stream`
//! in `src-tauri/src/db/commands.rs`; both forward `Core::query_stream`.
//!
//! Protocol:
//!   1. Client opens WS.
//!   2. Client sends one Text frame: `{"query_id","connection_id","sql","values"}`.
//!   3. Server sends `StreamEvent` frames: `{"type":"batch", ...StreamBatch fields}`
//!      (the last one has `"is_final": true`), then a terminal `{"type":"done"}`
//!      or `{"type":"error","message","code"}`.
//!   4. Server closes the socket.
//!
//! Cancellation: the client closes the WS. The next send fails, the event
//! stream is dropped, and Core stops the driver's fetch.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures::StreamExt;
use log::warn;
use seaquel_core::StreamEvent;
use seaquel_types::{DbError, Value};
use serde::Deserialize;

use crate::AppState;

#[derive(Debug, Deserialize)]
struct StreamRequest {
    query_id: String,
    connection_id: String,
    sql: String,
    #[serde(default)]
    values: Vec<Value>,
}

pub async fn stream(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let req = match socket.recv().await {
        Some(Ok(Message::Text(t))) => match serde_json::from_str::<StreamRequest>(&t) {
            Ok(r) => r,
            Err(e) => {
                warn!("stream request parse failed: {e}");
                send_error(&mut socket, DbError::query_error(format!("malformed request: {e}"))).await;
                return;
            }
        },
        Some(Ok(other)) => {
            warn!("stream first frame was not text: {other:?}");
            send_error(
                &mut socket,
                DbError::query_error(format!("expected Text frame, got {other:?}")),
            )
            .await;
            return;
        }
        Some(Err(e)) => {
            warn!("ws recv error before request: {e}");
            return;
        }
        None => return, // client closed immediately
    };

    let mut events = state
        .core
        .query_stream(req.query_id, req.connection_id, req.sql, req.values);
    while let Some(event) = events.next().await {
        // A batch carries arbitrary row data from a driver, so serializing it
        // can fail. Report that as the terminal error instead of skipping it.
        let json = match serde_json::to_string(&event) {
            Ok(json) => json,
            Err(e) => {
                warn!("failed to serialize stream event: {e}");
                send_error(&mut socket, DbError::query_error(format!("failed to serialize result: {e}"))).await;
                return;
            }
        };
        if socket.send(Message::Text(json.into())).await.is_err() {
            // Peer went away. Dropping `events` stops the fetch.
            return;
        }
    }
}

async fn send_error(socket: &mut WebSocket, err: DbError) {
    if let Ok(json) = serde_json::to_string(&StreamEvent::from(err)) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
}
