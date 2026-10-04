//! A release server on `127.0.0.1` for tests (the `testing` feature): it
//! serves the release metadata and the assets a test publishes, records
//! every request, and can stall, cut or redirect a body. No test that
//! installs reaches GitHub or any other host: [`MockReleases::source`] is a
//! loopback [`ReleaseSource`].

#![allow(clippy::disallowed_methods)] // the server spawns its connections on tokio

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use super::ReleaseSource;

/// How one path is answered.
#[derive(Clone, Debug)]
pub enum Route {
    /// `status` with `body`. `length` is the `Content-Length` sent (`None`:
    /// chunked); `stall_after` writes that many body bytes and then holds
    /// the connection until the client goes; `close_after` writes that many
    /// and closes without the rest.
    Body {
        status: u16,
        body: Vec<u8>,
        length: Option<u64>,
        stall_after: Option<usize>,
        close_after: Option<usize>,
        /// Written in pieces of this many bytes with `gap` between them.
        piece: usize,
        gap: Duration,
    },
    /// A redirect to `location`.
    Redirect { status: u16, location: String },
}

impl Route {
    /// `200`, the whole body with its length.
    pub fn ok(body: Vec<u8>) -> Self {
        Route::Body {
            status: 200,
            length: Some(body.len() as u64),
            body,
            stall_after: None,
            close_after: None,
            piece: 16 * 1024,
            gap: Duration::ZERO,
        }
    }

    pub fn status(status: u16) -> Self {
        Route::Body {
            status,
            body: b"{\"message\":\"Not Found\"}".to_vec(),
            length: Some(23),
            stall_after: None,
            close_after: None,
            piece: 16 * 1024,
            gap: Duration::ZERO,
        }
    }

    /// The body sent chunked, so no length tells the client how much comes.
    pub fn chunked(body: Vec<u8>) -> Self {
        Route::Body {
            status: 200,
            body,
            length: None,
            stall_after: None,
            close_after: None,
            piece: 16 * 1024,
            gap: Duration::ZERO,
        }
    }
}

/// One request as the server got it. Header names are lowercase.
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Default)]
struct State {
    routes: HashMap<String, Route>,
    requests: Vec<RecordedRequest>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The running server. Dropping it stops the listener.
pub struct MockReleases {
    url: String,
    state: Arc<Mutex<State>>,
    task: JoinHandle<()>,
}

impl Drop for MockReleases {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The asset's metadata path (`/api/v<version>`).
pub fn release_path(version: &str) -> String {
    format!("/api/v{version}")
}

/// The asset's download path (`/download/v<version>/<name>`).
pub fn asset_path(version: &str, name: &str) -> String {
    format!("/download/v{version}/{name}")
}

/// `bytes` gzipped, as the release step makes the asset.
pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(bytes).expect("gzip in memory");
    gz.finish().expect("gzip in memory")
}

/// `sha256:<hex>`, as GitHub's `digest` field.
pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

impl MockReleases {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the release server");
        let url = format!("http://{}", listener.local_addr().expect("server address"));
        let state = Arc::new(Mutex::new(State::default()));
        let s = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let s = s.clone();
                tokio::spawn(async move { serve(s, sock).await });
            }
        });
        Self { url, state, task }
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The release metadata under `/api`, the assets under `/download`.
    pub fn source(&self) -> ReleaseSource {
        ReleaseSource::new(
            &format!("{}/api", self.url),
            &format!("{}/download", self.url),
        )
        .expect("a loopback source")
    }

    pub fn route(&self, path: &str, route: Route) {
        lock(&self.state).routes.insert(path.to_string(), route);
    }

    /// The release `v<version>` with `assets` as GitHub's API lists them:
    /// each `(name, size, digest)`, any of them wrong if the test wants.
    pub fn metadata(&self, version: &str, assets: &[(&str, u64, Option<&str>)]) {
        let list: Vec<serde_json::Value> = assets
            .iter()
            .map(|(name, size, digest)| {
                serde_json::json!({
                    "name": name,
                    "size": size,
                    "digest": digest,
                    "browser_download_url": format!("https://example.invalid/{name}"),
                })
            })
            .collect();
        let body = serde_json::json!({ "tag_name": format!("v{version}"), "assets": list });
        self.route(
            &release_path(version),
            Route::ok(body.to_string().into_bytes()),
        );
    }

    /// Publishes `asset` (the gzipped bytes) as `name` in release
    /// `v<version>`, with its true size and digest.
    pub fn publish(&self, version: &str, name: &str, asset: Vec<u8>) {
        let d = digest(&asset);
        self.metadata(version, &[(name, asset.len() as u64, Some(&d))]);
        self.route(&asset_path(version, name), Route::ok(asset));
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        lock(&self.state).requests.clone()
    }

    /// How many requests asked for `path`.
    pub fn hits(&self, path: &str) -> usize {
        lock(&self.state)
            .requests
            .iter()
            .filter(|r| r.path == path)
            .count()
    }
}

async fn serve(state: Arc<Mutex<State>>, mut sock: TcpStream) {
    let mut head = Vec::new();
    let mut buf = [0u8; 4096];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match sock.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
        if head.len() > 64 * 1024 {
            return;
        }
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or_default().to_string();
    let path = first.next().unwrap_or_default().to_string();
    let headers = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let route = {
        let mut s = lock(&state);
        s.requests.push(RecordedRequest {
            method,
            path: path.clone(),
            headers,
        });
        s.routes.get(&path).cloned()
    };
    let route = route.unwrap_or_else(|| Route::status(404));
    match route {
        Route::Redirect { status, location } => {
            let head = format!(
                "HTTP/1.1 {status} Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = sock.write_all(head.as_bytes()).await;
        }
        Route::Body {
            status,
            body,
            length,
            stall_after,
            close_after,
            piece,
            gap,
        } => {
            let framing = match length {
                Some(n) => format!("Content-Length: {n}"),
                None => "Transfer-Encoding: chunked".to_string(),
            };
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/octet-stream\r\n{framing}\r\nConnection: close\r\n\r\n"
            );
            if sock.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            let stop = stall_after
                .or(close_after)
                .unwrap_or(body.len())
                .min(body.len());
            for part in body[..stop].chunks(piece.max(1)) {
                let ok = if length.is_some() {
                    sock.write_all(part).await
                } else {
                    let mut chunk = format!("{:x}\r\n", part.len()).into_bytes();
                    chunk.extend_from_slice(part);
                    chunk.extend_from_slice(b"\r\n");
                    sock.write_all(&chunk).await
                };
                if ok.is_err() {
                    return;
                }
                let _ = sock.flush().await;
                if !gap.is_zero() {
                    tokio::time::sleep(gap).await;
                }
            }
            if stall_after.is_some() {
                // Held until the client goes (a read sees EOF or an error).
                let mut b = [0u8; 64];
                loop {
                    match sock.read(&mut b).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                }
            }
            if close_after.is_some() {
                return;
            }
            if length.is_none() {
                let _ = sock.write_all(b"0\r\n\r\n").await;
            }
            let _ = sock.flush().await;
        }
    }
}
