//! Test support (the `testing` feature, native only): a mock provider on
//! `127.0.0.1` and an [`HttpClient`] wrapper that refuses to reach any
//! other host. Every test that builds an AI-capable Core wraps its client
//! in [`LoopbackOnly`], so no test can call a real provider (phase 6's
//! ground rule).

#![allow(clippy::disallowed_methods)] // the mock spawns its connections on tokio

mod mock;
pub mod scripts;

pub use mock::{MockProvider, RecordedRequest, Reply, SseEnd};

use crate::http::{host_of, HttpClient, HttpError, HttpRequest, HttpResponse};

/// The key every test uses. Never a real one.
pub const TEST_KEY: &str = "test-key-not-real";

/// Passes requests to `C` only when they go to `127.0.0.1`, `localhost`
/// or `::1`; panics on any other host.
pub struct LoopbackOnly<C>(pub C);

pub fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

#[seaquel_runtime::async_trait]
impl<C: HttpClient> HttpClient for LoopbackOnly<C> {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let host = host_of(&req.url);
        assert!(
            is_loopback_host(host),
            "a test tried to reach {host}: tests talk to the local mock only"
        );
        self.0.send(req).await
    }
}
