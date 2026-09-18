//! WebSocket streaming endpoint. Mirrors the Tauri Channel-based streaming in
//! `src-tauri/src/db/commands.rs::db_query_stream`.
//!
//! Protocol:
//!   1. Client opens WS.
//!   2. Client sends one Text frame: `{"query_id","connection_id","sql","values"}`.
//!   3. Server sends a sequence of `{"type":"batch", ...StreamBatch fields}` frames.
//!      The final batch carries `"is_final": true`.
//!   4. Server sends a terminal `{"type":"done"}` on success, or
//!      `{"type":"error","message","code"}` on failure.
//!   5. Server closes the socket.
//!
//! Cancellation: the client closes the WS, or the server's writer fails on a
//! dropped peer. Either way the streaming loop breaks out and the underlying
//! sqlx fetch is dropped, releasing the DB connection.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures::StreamExt;
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use seaquel_db::{DbError, StreamBatch};

use crate::AppState;

#[derive(Debug, Deserialize)]
struct StreamRequest {
    query_id: String,
    connection_id: String,
    sql: String,
    #[serde(default)]
    values: Vec<serde_json::Value>,
}

/// Events sent to the client over the WebSocket.
///
/// Uses `#[serde(tag = "type", rename_all = "camelCase")]` to match the Tauri
/// `StreamEvent` wire format so frontend code can share a parser.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum StreamEvent {
    Batch(StreamBatch),
    Done,
    Error { message: String, code: String },
}

pub async fn stream(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    // Read the request frame.
    let req = match socket.recv().await {
        Some(Ok(Message::Text(t))) => match serde_json::from_str::<StreamRequest>(&t) {
            Ok(r) => r,
            Err(e) => {
                warn!("stream request parse failed: {e}");
                let _ = send_error(
                    &mut socket,
                    &DbError::query_error(format!("malformed request: {e}")),
                )
                .await;
                return;
            }
        },
        Some(Ok(other)) => {
            warn!("stream first frame was not text: {other:?}");
            let _ = send_error(
                &mut socket,
                &DbError::query_error(format!("expected Text frame, got {other:?}")),
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

    debug!(
        activity = "db.stream",
        query_id = req.query_id.as_str(),
        connection_id = req.connection_id.as_str(),
        sql_len = req.sql.len(),
        params = req.values.len();
        "Stream start"
    );

    // `get_driver` clones the Arc out of the connections map and releases
    // the read lock before we return. Holding the lock across the streaming
    // loop below would block every other caller — including disconnect —
    // until the stream finished.
    let driver = match state.connection_manager.get_driver(&req.connection_id).await {
        Ok(d) => d,
        Err(e) => {
            let _ = send_error(&mut socket, &e).await;
            return;
        }
    };

    let mut stream = driver.query_stream(req.sql, req.values);
    while let Some(batch_result) = stream.next().await {
        match batch_result {
            Ok(batch) => {
                let ev = StreamEvent::Batch(batch);
                match encode_event(&ev) {
                    Ok(json) => {
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            // Peer dropped — stop fetching.
                            return;
                        }
                    }
                    Err(e) => {
                        warn!("failed to serialize batch: {e}");
                        break;
                    }
                }
            }
            Err(e) => {
                let _ = send_error(&mut socket, &e).await;
                return;
            }
        }
    }

    if let Ok(json) = encode_event(&StreamEvent::Done) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
}

/// Serialize a `StreamEvent`. `Done` and `Error` serialize finite String
/// fields and can't fail in practice, but routing them through the same
/// helper as `Batch` (which can carry arbitrary row data from a driver)
/// keeps one recovery path instead of a mix of `match` + `unwrap`.
fn encode_event(ev: &StreamEvent) -> Result<String, serde_json::Error> {
    serde_json::to_string(ev)
}

async fn send_error(socket: &mut WebSocket, err: &DbError) -> Result<(), axum::Error> {
    let ev = StreamEvent::Error {
        message: err.message.clone(),
        code: err.code.clone(),
    };
    match encode_event(&ev) {
        Ok(json) => socket.send(Message::Text(json.into())).await,
        Err(e) => {
            warn!("failed to serialize stream error: {e}");
            Ok(())
        }
    }
}
