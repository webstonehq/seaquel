//! The newline-delimited JSON-RPC transport `seaquel-cli mcp` serves on
//! stdio.
//!
//! rmcp 3.4.1's own (`AsyncRwTransport`, what `serve(stdio())` builds) drops
//! a line that isn't JSON with a debug-level log and no reply (it follows the
//! other MCP SDKs there, rust-sdk issue #938). A request holding a lone UTF-16
//! surrogate escape (`"\ud800"`) is such a line, since serde_json rejects it,
//! so the host waited forever and nothing said why. rmcp has no hook into
//! that path (the codec's parse and its reply are private), so this transport
//! reads the lines itself:
//!
//! - a line that isn't JSON (or isn't UTF-8) gets a `-32700` parse error
//!   response with `"id": null` (none can be read), and a warning on stderr.
//!   JSON-RPC 2.0 requires the null id; MCP 2026-07-28 allows leaving it out,
//!   which is what rmcp's `JsonRpcError` does, but hosts on the older
//!   revisions expect it, so these replies add it themselves;
//! - JSON that isn't a JSON-RPC message gets `-32600` (as in rmcp), with the
//!   line's `id` when it has one (else `null`), and a warning;
//! - a message with no `id` that fails to parse is a notification, which
//!   JSON-RPC never answers: a debug log only (rmcp ignores unknown
//!   notifications too).
//!
//! The logs name the parse error (line and column), never the line itself,
//! which may hold SQL or values. Everything else matches rmcp: a UTF-8 BOM
//! and a trailing `\r` are stripped, blank lines skipped, and each message
//! goes out as one line, flushed.

use std::future::Future;
use std::io;
use std::sync::Arc;

use rmcp::model::{ErrorData, RequestId};
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;
use rmcp::RoleServer;
use serde_json::error::Category;
use serde_json::Value as Json;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// A server transport over a reader and a writer of JSON-RPC lines.
pub struct LineTransport<R, W> {
    read: BufReader<R>,
    /// The line being read. Kept across `receive` calls: rmcp polls
    /// `receive` inside a `select!`, and a cancelled `read_until` leaves its
    /// partial line here for the next call (as rmcp's transport does).
    line: Vec<u8>,
    write: Arc<Mutex<Option<W>>>,
}

/// The transport over this process's stdin and stdout.
pub fn stdio() -> LineTransport<tokio::io::Stdin, tokio::io::Stdout> {
    LineTransport::new(tokio::io::stdin(), tokio::io::stdout())
}

impl<R, W> LineTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    pub fn new(read: R, write: W) -> Self {
        Self {
            read: BufReader::new(read),
            line: Vec::new(),
            write: Arc::new(Mutex::new(Some(write))),
        }
    }

    /// What to do with a line that isn't a message the server takes: the
    /// error to reply with, or `None` for a notification.
    fn refusal(line: &[u8], e: &serde_json::Error) -> Option<Json> {
        let reply = Self::refusal_message(line, e)?;
        let mut reply = serde_json::to_value(&reply).ok()?;
        if let Some(fields) = reply.as_object_mut() {
            fields.entry("id").or_insert(Json::Null);
        }
        Some(reply)
    }

    fn refusal_message(line: &[u8], e: &serde_json::Error) -> Option<TxJsonRpcMessage<RoleServer>> {
        match e.classify() {
            Category::Syntax | Category::Eof => {
                log::warn!(
                    "Received a line that isn't valid JSON ({e}); answered with a parse error"
                );
                Some(TxJsonRpcMessage::<RoleServer>::error(
                    ErrorData::parse_error(format!("Parse error: {e}"), None),
                    None,
                ))
            }
            Category::Data | Category::Io => {
                let value: Option<Json> = serde_json::from_slice(line).ok();
                let id = value.as_ref().and_then(|v| v.get("id"));
                if id.is_none() && value.as_ref().is_some_and(|v| v.get("method").is_some()) {
                    log::debug!("Ignoring a notification that doesn't parse: {e}");
                    return None;
                }
                let id = id.and_then(|id| serde_json::from_value::<RequestId>(id.clone()).ok());
                log::warn!(
                    "Received JSON that isn't a valid JSON-RPC message ({e}); answered with an \
                     invalid request error"
                );
                Some(TxJsonRpcMessage::<RoleServer>::error(
                    ErrorData::invalid_request(format!("Invalid request: {e}"), None),
                    id,
                ))
            }
        }
    }
}

async fn write_line<W: AsyncWrite + Unpin>(
    write: &Mutex<Option<W>>,
    item: &impl serde::Serialize,
) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(item).map_err(io::Error::other)?;
    bytes.push(b'\n');
    let mut write = write.lock().await;
    let Some(w) = write.as_mut() else {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "Transport is closed",
        ));
    };
    w.write_all(&bytes).await?;
    w.flush().await
}

impl<R, W> Transport<RoleServer> for LineTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let write = self.write.clone();
        async move { write_line(&write, &item).await }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            match self.read.read_until(b'\n', &mut self.line).await {
                // EOF; bytes left in `line` are an unterminated last line.
                Ok(0) => return None,
                Ok(_) => {}
                Err(e) => {
                    log::error!("Reading the MCP input failed: {e}");
                    return None;
                }
            }
            let line = std::mem::take(&mut self.line);
            let mut text = line.strip_suffix(b"\n").unwrap_or(&line);
            text = text.strip_suffix(b"\r").unwrap_or(text);
            text = text.strip_prefix(UTF8_BOM).unwrap_or(text);
            if text.is_empty() {
                continue;
            }
            let reply = match serde_json::from_slice::<RxJsonRpcMessage<RoleServer>>(text) {
                Ok(message) => return Some(message),
                Err(e) => Self::refusal(text, &e),
            };
            if let Some(reply) = reply {
                if let Err(e) = write_line(&self.write, &reply).await {
                    log::error!("Writing the MCP output failed: {e}");
                    return None;
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        let writer = self.write.lock().await.take();
        if let Some(mut w) = writer {
            w.flush().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    async fn exchange(input: &[u8]) -> (Option<String>, Vec<Json>) {
        let (mut client_in, server_in) = tokio::io::duplex(1 << 16);
        let (server_out, client_out) = tokio::io::duplex(1 << 16);
        let mut transport = LineTransport::new(server_in, server_out);
        client_in.write_all(input).await.unwrap();
        client_in
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        let received = transport.receive().await.map(|m| {
            serde_json::to_value(&m).unwrap()["method"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        });
        transport.close().await.unwrap();
        drop(transport);
        let mut lines = tokio::io::BufReader::new(client_out).lines();
        let mut replies = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            replies.push(serde_json::from_str(&line).unwrap());
        }
        (received, replies)
    }

    #[tokio::test]
    async fn a_line_that_is_not_json_gets_a_parse_error() {
        let bad = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"x\\ud800\"}}\n";
        let (received, replies) = exchange(bad).await;
        assert_eq!(
            received.as_deref(),
            Some("ping"),
            "the next line still arrives"
        );
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(replies[0]["error"]["code"], -32700);
        assert_eq!(replies[0]["jsonrpc"], "2.0");
        assert_eq!(replies[0].get("id"), Some(&Json::Null), "{replies:?}");

        let (_, replies) = exchange(b"not json\n\xff\xfe\n").await;
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert!(replies.iter().all(|r| r["error"]["code"] == -32700));
        assert!(replies.iter().all(|r| r.get("id") == Some(&Json::Null)));
    }

    #[tokio::test]
    async fn json_that_is_not_a_message_gets_invalid_request_with_its_id() {
        let (received, replies) = exchange(b"{\"id\":7,\"hello\":1}\n[1,2]\n").await;
        assert_eq!(received.as_deref(), Some("ping"));
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert_eq!(replies[0]["error"]["code"], -32600);
        assert_eq!(replies[0]["id"], 7);
        assert_eq!(replies[1]["error"]["code"], -32600);
        assert_eq!(replies[1].get("id"), Some(&Json::Null), "{replies:?}");
    }

    #[tokio::test]
    async fn blank_lines_bom_and_bad_notifications_get_no_reply() {
        let input = b"\n\r\n\xEF\xBB\xBF{\"jsonrpc\":\"2.0\",\"method\":5}\n";
        let (received, replies) = exchange(input).await;
        assert_eq!(received.as_deref(), Some("ping"));
        assert!(replies.is_empty(), "{replies:?}");
    }
}
