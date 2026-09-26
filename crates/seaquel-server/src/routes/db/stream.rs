//! WebSocket streaming endpoint. The Tauri counterpart is `db_query_stream`
//! in `src-tauri/src/db/commands.rs`; both forward `Core::query_stream`.
//!
//! Protocol:
//!   1. Client opens WS.
//!   2. Client sends one Text frame:
//!      `{"query_id","connection_id","sql","values","read_only"}`.
//!      `values` and `read_only` are optional (`[]` and `false`). With
//!      `"read_only": true` Core runs the AI's token check and then the
//!      engine's read-only query (the AI's `run_query` and dashboard widgets);
//!      that result arrives as one final batch.
//!   3. Server sends `StreamEvent` frames: `{"type":"batch", ...StreamBatch fields}`
//!      (the last one has `"is_final": true`), then a terminal `{"type":"done"}`
//!      or `{"type":"error","message","code"}`.
//!   4. Server closes the socket.
//!
//! Cancellation: the client closes the WS. The handler watches the socket
//! while it waits for the next event, so a close (or a dropped connection)
//! drops the event stream at once, and Core drops the driver's fetch. That
//! matters for a read-only query, which sends nothing until it finishes.
//! Frames the client sends after the first are ignored.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures::StreamExt;
use log::warn;
use seaquel_core::{QueryOptions, StreamEvent};
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
    /// Missing means read-write: only `selectReadOnly` sets it.
    #[serde(default)]
    read_only: bool,
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
                send_error(
                    &mut socket,
                    DbError::query_error(format!("malformed request: {e}")),
                )
                .await;
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

    let mut events = state.core.query_stream(
        req.query_id,
        req.connection_id,
        req.sql,
        req.values,
        QueryOptions::default().with_read_only(req.read_only),
    );
    loop {
        let event = tokio::select! {
            event = events.next() => event,
            frame = socket.recv() => match frame {
                // The client closed the socket or went away: returning drops
                // `events`, which cancels the query.
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => continue,
            },
        };
        let Some(event) = event else { return };
        // A batch carries arbitrary row data from a driver, so serializing it
        // can fail. Report that as the terminal error instead of skipping it.
        let json = match serde_json::to_string(&event) {
            Ok(json) => json,
            Err(e) => {
                warn!("failed to serialize stream event: {e}");
                send_error(
                    &mut socket,
                    DbError::query_error(format!("failed to serialize result: {e}")),
                )
                .await;
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

#[cfg(test)]
mod tests {
    use super::StreamRequest;

    #[test]
    fn read_only_defaults_to_false() {
        let req: StreamRequest =
            serde_json::from_str(r#"{"query_id":"q","connection_id":"c","sql":"SELECT 1"}"#)
                .unwrap();
        assert!(!req.read_only);
        assert!(req.values.is_empty());
    }

    #[test]
    fn read_only_is_parsed() {
        for flag in [true, false] {
            let req: StreamRequest = serde_json::from_str(&format!(
                r#"{{"query_id":"q","connection_id":"c","sql":"SELECT 1","values":[],"read_only":{flag}}}"#
            ))
            .unwrap();
            assert_eq!(req.read_only, flag);
        }
    }

    #[test]
    fn read_only_must_be_a_boolean() {
        // A string would otherwise be a way to send a flag the client didn't mean.
        assert!(serde_json::from_str::<StreamRequest>(
            r#"{"query_id":"q","connection_id":"c","sql":"SELECT 1","read_only":"true"}"#
        )
        .is_err());
    }
}
