//! The one door to the network. `seaquel-ai` does no I/O itself: Core is
//! handed an [`HttpClient`] (`seaquel-http`'s `NativeHttp` on desktop, the
//! server and the CLI; the demo's fetch bridge in the browser) and passes
//! the requests the wire builds through it.
//!
//! Nothing here prints a header value, a body or a URL's path or query:
//! [`HttpRequest`]'s `Debug` shows the method, the host and the header
//! names, and [`HttpError`] carries a kind and a detail that never holds the
//! URL (the native client strips it from reqwest's errors).

use std::fmt;

use seaquel_runtime::{BoxStream, MaybeSend, MaybeSync};

/// A value that must not be printed: a header value (an API key is one).
/// `Debug` shows `<redacted>`; there is no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// The value, for the one place that sends it.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl From<&str> for Redacted<String> {
    fn from(v: &str) -> Self {
        Self(v.to_string())
    }
}

impl From<String> for Redacted<String> {
    fn from(v: String) -> Self {
        Self(v)
    }
}

/// The two methods the wire uses: `POST` for a round and the inline
/// prompt, `GET` for the model list (`/models`), which is also the
/// provider test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// One request to a provider. Header names are lowercase.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, Redacted<String>)>,
    /// Empty for a `GET`.
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// The value of header `name` (lowercase), for tests and the clients.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.expose().as_str())
    }

    /// The URL's host, for logs and `Debug` (never the path or query,
    /// which an OpenAI-compatible base URL may use for a key).
    pub fn host(&self) -> &str {
        host_of(&self.url)
    }
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("host", &self.host())
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// The host of `url` (`scheme://[userinfo@]host[:port]/…`), or `""`. A
/// plain cut, enough for logs and the test client's loopback check; the
/// native client checks egress on the URL it parsed itself.
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    // A backslash ends the authority too: the URL parser reads it as `/`
    // for `http`/`https`, so `https://a.example\@127.0.0.1/` goes to
    // a.example.
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(v6) = host_port.strip_prefix('[') {
        return v6.split(']').next().unwrap_or("");
    }
    host_port.split(':').next().unwrap_or("")
}

/// A provider's answer: the status and the body as it arrives. Dropping
/// the body stream drops the connection, which is how a cancelled turn
/// stops the provider (spike S1).
pub struct HttpResponse {
    pub status: u16,
    pub body: BoxStream<'static, Result<Vec<u8>, HttpError>>,
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpErrorKind {
    /// The URL doesn't parse, or its scheme isn't `http`/`https`.
    InvalidUrl,
    /// Refused by the egress rules before anything was sent (or by the
    /// resolver: every address the name has is private).
    EgressBlocked,
    /// Connecting, TLS, or the connection dropping before the head.
    Connect,
    /// The connect, idle or per-round timeout passed.
    Timeout,
    /// The body broke off while it was being read.
    Body,
    /// Anything else the client reports.
    Other,
}

/// A failure below the provider's wire. `detail` is for logs and the
/// user: never a URL, header or body.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpError {
    pub kind: HttpErrorKind,
    pub detail: String,
}

impl HttpError {
    pub fn new(kind: HttpErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    /// The error code Core answers with.
    pub fn code(&self) -> &'static str {
        match self.kind {
            HttpErrorKind::InvalidUrl => "INVALID_ARGUMENT",
            HttpErrorKind::EgressBlocked => "AI_EGRESS_BLOCKED",
            HttpErrorKind::Timeout => "TIMEOUT",
            HttpErrorKind::Connect | HttpErrorKind::Body | HttpErrorKind::Other => "PROVIDER_ERROR",
        }
    }
}

impl fmt::Debug for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpError")
            .field("kind", &self.kind)
            .field("detail", &self.detail)
            .finish()
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for HttpError {}

/// Sends a request and hands back the response as it streams. No default:
/// Core is built with one or answers `NOT_SUPPORTED`.
#[seaquel_runtime::async_trait]
pub trait HttpClient: MaybeSend + MaybeSync {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;
}

/// The whole body, at most `max` bytes; past it the rest is dropped (and
/// the connection with it). For error bodies and the non-streaming calls.
pub async fn read_body(
    mut body: BoxStream<'static, Result<Vec<u8>, HttpError>>,
    max: usize,
) -> Result<Vec<u8>, HttpError> {
    use futures::StreamExt;
    let mut out = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        let room = max.saturating_sub(out.len());
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if out.len() >= max {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_cuts_the_authority() {
        assert_eq!(
            host_of("https://api.anthropic.com/v1/messages"),
            "api.anthropic.com"
        );
        assert_eq!(host_of("http://127.0.0.1:8080/v1?key=x"), "127.0.0.1");
        assert_eq!(host_of("http://u:p@localhost:1/x"), "localhost");
        assert_eq!(host_of("http://[::1]:1/x"), "::1");
        assert_eq!(host_of("https://h.example?q=1"), "h.example");
        assert_eq!(host_of("nonsense"), "nonsense");
    }

    /// Review fix 5: a `\` ends the authority, as the URL parser (and so
    /// reqwest) reads it for `http`/`https`; the userinfo trick doesn't
    /// make the test client think it's going to loopback.
    #[test]
    fn host_of_agrees_with_the_url_parser_on_backslashes() {
        assert_eq!(
            host_of("https://evil.example\\@127.0.0.1/v1"),
            "evil.example"
        );
        assert_eq!(host_of("http://127.0.0.1\\x"), "127.0.0.1");
    }

    #[test]
    fn debug_hides_header_values_path_and_body() {
        let req = HttpRequest {
            method: Method::Post,
            url: "https://h.example/v1/secret-path?key=test-key-not-real".into(),
            headers: vec![("x-api-key".into(), "test-key-not-real".into())],
            body: b"{\"messages\":\"MARKER\"}".to_vec(),
        };
        let shown = format!("{req:?}");
        assert!(!shown.contains("test-key-not-real"), "{shown}");
        assert!(!shown.contains("secret-path"), "{shown}");
        assert!(!shown.contains("MARKER"), "{shown}");
        assert!(
            shown.contains("x-api-key") && shown.contains("h.example"),
            "{shown}"
        );
        assert_eq!(req.header("x-api-key"), Some("test-key-not-real"));
    }
}
