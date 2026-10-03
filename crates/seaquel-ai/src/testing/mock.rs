//! A mock provider: a small HTTP/1.1 server on `127.0.0.1` that answers
//! each request with the next scripted [`Reply`] and records what it got.
//! It splits bodies into pieces of any size (so lines, JSON and multi-byte
//! characters straddle reads), stalls, closes early, answers errors and
//! 3xx, and notices when the client goes away (spike S1's cancel check).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// How an SSE reply ends after its events.
#[derive(Clone, Debug)]
pub enum SseEnd {
    /// The last chunk, then the connection closes.
    Finish,
    /// Nothing more, connection held open until the client goes.
    Stall,
    /// The connection drops without the last chunk.
    Close,
    /// `event` again every `every` until a write fails (the client went).
    Repeat { event: String, every: Duration },
}

#[derive(Clone, Debug)]
pub enum Reply {
    /// `200 text/event-stream`, chunked: `events` written in pieces of
    /// `piece` bytes with `gap` between pieces.
    Sse {
        events: Vec<String>,
        piece: usize,
        gap: Duration,
        end: SseEnd,
    },
    /// Any status, headers and body (errors, 3xx, JSON answers).
    Raw {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// The head is never sent: the request hangs until the client goes.
    Hang,
}

impl Reply {
    /// `events` in 5-byte pieces with no gap, then the last chunk.
    pub fn sse(events: Vec<String>) -> Self {
        Reply::Sse {
            events,
            piece: 5,
            gap: Duration::ZERO,
            end: SseEnd::Finish,
        }
    }

    pub fn json(status: u16, body: &Value) -> Self {
        Reply::Raw {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string().into_bytes(),
        }
    }

    pub fn redirect(status: u16, location: &str) -> Self {
        Reply::Raw {
            status,
            headers: vec![("location".into(), location.into())],
            body: Vec::new(),
        }
    }
}

/// One request as the mock got it. Header names are lowercase.
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

#[derive(Default)]
struct State {
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<RecordedRequest>>,
    connections: AtomicUsize,
    client_gone: AtomicBool,
    gone: Notify,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The running mock. Dropping it leaves the listener task running until
/// the test's runtime ends.
#[derive(Clone)]
pub struct MockProvider {
    url: String,
    state: Arc<State>,
}

impl MockProvider {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the mock");
        let url = format!("http://{}", listener.local_addr().expect("mock address"));
        let state = Arc::new(State::default());
        let s = state.clone();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                s.connections.fetch_add(1, Ordering::SeqCst);
                let s = s.clone();
                tokio::spawn(async move { serve(s, sock).await });
            }
        });
        Self { url, state }
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Queues the answer to the next request. With nothing queued the
    /// mock answers 500.
    pub fn reply(&self, reply: Reply) -> &Self {
        lock(&self.state.replies).push_back(reply);
        self
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        lock(&self.state.requests).clone()
    }

    /// TCP connections accepted so far.
    pub fn connections(&self) -> usize {
        self.state.connections.load(Ordering::SeqCst)
    }

    /// Whether a client went away while a reply was being written (a
    /// `Stall`, `Repeat` or `Hang` reply notices), waiting up to `within`.
    pub async fn client_gone(&self, within: Duration) -> bool {
        let wait = self.state.gone.notified();
        if self.state.client_gone.load(Ordering::SeqCst) {
            return true;
        }
        tokio::time::timeout(within, wait).await.is_ok()
            || self.state.client_gone.load(Ordering::SeqCst)
    }
}

fn mark_gone(state: &State) {
    state.client_gone.store(true, Ordering::SeqCst);
    state.gone.notify_waiters();
}

async fn read_request(sock: &mut TcpStream) -> Option<RecordedRequest> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Some(RecordedRequest {
        method,
        path,
        headers,
        body,
    })
}

async fn chunk(sock: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    sock.write_all(format!("{:x}\r\n", bytes.len()).as_bytes())
        .await?;
    sock.write_all(bytes).await?;
    sock.write_all(b"\r\n").await?;
    sock.flush().await
}

/// Waits until the peer closes (a read returns 0 or fails).
async fn until_closed(sock: &mut TcpStream) {
    let mut tmp = [0u8; 256];
    loop {
        match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        301 => "Moved Permanently",
        302 => "Found",
        307 => "Temporary Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        529 => "Overloaded",
        _ => "Status",
    }
}

async fn serve(state: Arc<State>, mut sock: TcpStream) {
    let Some(req) = read_request(&mut sock).await else {
        return;
    };
    lock(&state.requests).push(req);
    let reply = lock(&state.replies).pop_front().unwrap_or(Reply::Raw {
        status: 500,
        headers: vec![("content-type".into(), "application/json".into())],
        body: br#"{"error":{"type":"mock_error","message":"no reply scripted"}}"#.to_vec(),
    });
    match reply {
        Reply::Raw {
            status,
            headers,
            body,
        } => {
            let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
            for (k, v) in headers {
                head.push_str(&format!("{k}: {v}\r\n"));
            }
            head.push_str(&format!(
                "content-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            ));
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.write_all(&body).await;
            let _ = sock.flush().await;
        }
        Reply::Hang => {
            until_closed(&mut sock).await;
            mark_gone(&state);
        }
        Reply::Sse {
            events,
            piece,
            gap,
            end,
        } => {
            let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n";
            if sock.write_all(head.as_bytes()).await.is_err() {
                mark_gone(&state);
                return;
            }
            let bytes = events.concat().into_bytes();
            for p in bytes.chunks(piece.max(1)) {
                if chunk(&mut sock, p).await.is_err() {
                    mark_gone(&state);
                    return;
                }
                if !gap.is_zero() {
                    tokio::time::sleep(gap).await;
                }
            }
            match end {
                SseEnd::Finish => {
                    let _ = sock.write_all(b"0\r\n\r\n").await;
                    let _ = sock.flush().await;
                }
                SseEnd::Close => {}
                SseEnd::Stall => {
                    until_closed(&mut sock).await;
                    mark_gone(&state);
                }
                SseEnd::Repeat { event, every } => loop {
                    if chunk(&mut sock, event.as_bytes()).await.is_err() {
                        mark_gone(&state);
                        return;
                    }
                    tokio::time::sleep(every).await;
                },
            }
        }
    }
}
