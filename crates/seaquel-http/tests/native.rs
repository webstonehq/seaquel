#![allow(clippy::disallowed_methods, clippy::disallowed_types)] // the fake proxy and the panic check spawn on tokio
//! `NativeHttp` against the local mock provider: streaming a round,
//! cancel, timeouts, redirects, and the egress guard end to end (spike S3's
//! cases). Nothing here leaves the machine: the mock and the fake proxy
//! listen on 127.0.0.1, and names that would need DNS are answered by a
//! fixed resolver or end at the fake proxy.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use seaquel_ai::http::{read_body, HttpClient, HttpErrorKind, HttpRequest, Method};
use seaquel_ai::testing::{scripts, LoopbackOnly, MockProvider, Reply, SseEnd, TEST_KEY};
use seaquel_ai::wire::{
    check_status, models_request, round_request, Decoder, Message, Provider, ProviderKind, Round,
    RoundEvent, StopReason,
};
use seaquel_http::egress::PublicResolver;
use seaquel_http::{Egress, NativeHttp, NativeHttpOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn any() -> NativeHttp {
    NativeHttp::new(NativeHttpOptions::new(Egress::Any))
}

fn public() -> NativeHttp {
    NativeHttp::new(NativeHttpOptions::new(Egress::Public))
}

fn get(url: &str) -> HttpRequest {
    HttpRequest {
        method: Method::Get,
        url: url.to_string(),
        headers: vec![],
        body: vec![],
    }
}

fn round() -> Round {
    Round {
        system: "sys".into(),
        messages: vec![Message::User("How many rows in café?".into())],
        tools: vec![],
    }
}

fn anthropic_at(mock: &MockProvider) -> Provider {
    Provider {
        kind: ProviderKind::Anthropic,
        base_url: Some(mock.url().to_string()),
        model: "claude-test".into(),
    }
}

#[tokio::test]
async fn a_round_streams_from_the_mock_and_decodes() {
    let mock = MockProvider::start().await;
    mock.reply(Reply::sse(scripts::anthropic_s1_round()));
    let http = LoopbackOnly(any());
    let req = round_request(&anthropic_at(&mock), Some(TEST_KEY), &round(), false);
    let resp = http.send(req).await.unwrap();
    assert_eq!(resp.status, 200);
    let mut decoder = Decoder::new(ProviderKind::Anthropic);
    let mut events = Vec::new();
    let mut body = resp.body;
    while let Some(chunk) = body.next().await {
        decoder.feed(&chunk.unwrap(), &mut events).unwrap();
    }
    decoder.finish(&mut events).unwrap();
    let calls = events
        .iter()
        .filter(|e| matches!(e, RoundEvent::ToolCall(_)))
        .count();
    assert_eq!(calls, 2);
    assert!(matches!(
        events.last(),
        Some(RoundEvent::Stop(StopReason::ToolUse))
    ));

    let got = mock.requests();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].method, "POST");
    assert_eq!(got[0].path, "/v1/messages");
    assert_eq!(got[0].header("x-api-key"), Some(TEST_KEY));
    assert_eq!(got[0].header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(got[0].header("content-type"), Some("application/json"));
    assert_eq!(
        got[0].json()["messages"][0]["content"],
        "How many rows in café?"
    );
}

#[tokio::test]
async fn a_get_sends_no_body() {
    let mock = MockProvider::start().await;
    mock.reply(Reply::json(200, &serde_json::json!({"data":[{"id":"m1"}]})));
    let req = models_request(&anthropic_at(&mock), Some(TEST_KEY), false);
    let resp = LoopbackOnly(any()).send(req).await.unwrap();
    assert_eq!(resp.status, 200);
    let body = read_body(resp.body, 1 << 20).await.unwrap();
    assert_eq!(seaquel_ai::wire::decode_models(&body).unwrap(), ["m1"]);
    let got = &mock.requests()[0];
    assert_eq!(got.method, "GET");
    assert_eq!(got.path, "/v1/models");
    assert!(got.body.is_empty());
}

/// S1: dropping the body mid-stream closes the connection, so the
/// provider's next write fails and it stops generating.
#[tokio::test]
async fn dropping_the_body_closes_the_connection() {
    let mock = MockProvider::start().await;
    mock.reply(Reply::Sse {
        events: scripts::anthropic_text("first")[..3].to_vec(),
        piece: 1 << 20,
        gap: Duration::ZERO,
        end: SseEnd::Repeat {
            event: scripts::anthropic_event(
                "content_block_delta",
                serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"tok "}}),
            ),
            every: Duration::from_millis(10),
        },
    });
    let req = round_request(&anthropic_at(&mock), Some(TEST_KEY), &round(), false);
    let resp = LoopbackOnly(any()).send(req).await.unwrap();
    let mut body = resp.body;
    body.next().await.unwrap().unwrap();
    drop(body);
    assert!(
        mock.client_gone(Duration::from_secs(5)).await,
        "the mock should see the connection close"
    );
}

#[tokio::test]
async fn a_provider_that_stops_sending_times_out() {
    let mock = MockProvider::start().await;
    mock.reply(Reply::Sse {
        events: scripts::anthropic_text("x")[..2].to_vec(),
        piece: 1 << 20,
        gap: Duration::ZERO,
        end: SseEnd::Stall,
    });
    let mut options = NativeHttpOptions::new(Egress::Any);
    options.idle_timeout = Duration::from_millis(300);
    let http = LoopbackOnly(NativeHttp::new(options));
    let req = round_request(&anthropic_at(&mock), Some(TEST_KEY), &round(), false);
    let resp = http.send(req).await.unwrap();
    let mut body = resp.body;
    let mut err = None;
    while let Some(chunk) = body.next().await {
        if let Err(e) = chunk {
            err = Some(e);
            break;
        }
    }
    let err = err.expect("the stall should end in an error");
    assert_eq!(err.kind, HttpErrorKind::Timeout, "{err:?}");
    assert_eq!(err.code(), "TIMEOUT");
}

#[tokio::test]
async fn a_provider_that_never_answers_times_out() {
    let mock = MockProvider::start().await;
    mock.reply(Reply::Hang);
    let mut options = NativeHttpOptions::new(Egress::Any);
    options.idle_timeout = Duration::from_millis(300);
    let err = LoopbackOnly(NativeHttp::new(options))
        .send(get(&format!("{}/v1/models", mock.url())))
        .await
        .unwrap_err();
    assert_eq!(err.kind, HttpErrorKind::Timeout, "{err:?}");
}

#[tokio::test]
async fn the_round_timeout_bounds_a_trickling_answer() {
    let mock = MockProvider::start().await;
    mock.reply(Reply::Sse {
        events: scripts::anthropic_text("x")[..2].to_vec(),
        piece: 1 << 20,
        gap: Duration::ZERO,
        end: SseEnd::Repeat {
            event: ": keep-alive\n\n".into(),
            every: Duration::from_millis(20),
        },
    });
    let mut options = NativeHttpOptions::new(Egress::Any);
    options.round_timeout = Duration::from_millis(400);
    let req = round_request(&anthropic_at(&mock), Some(TEST_KEY), &round(), false);
    let resp = LoopbackOnly(NativeHttp::new(options))
        .send(req)
        .await
        .unwrap();
    let mut body = resp.body;
    let mut err = None;
    while let Some(chunk) = body.next().await {
        if let Err(e) = chunk {
            err = Some(e);
            break;
        }
    }
    assert_eq!(err.expect("timed out").kind, HttpErrorKind::Timeout);
}

/// S3: a 302 is the answer, not followed, in every egress mode.
#[tokio::test]
async fn a_redirect_is_not_followed() {
    let target = MockProvider::start().await;
    let hop = MockProvider::start().await;
    hop.reply(Reply::redirect(302, &format!("{}/secret", target.url())));
    let resp = LoopbackOnly(any())
        .send(get(&format!("{}/v1/models", hop.url())))
        .await
        .unwrap();
    assert_eq!(resp.status, 302);
    let err = check_status(resp.status, &[]).unwrap_err();
    assert_eq!(err.code(), "PROVIDER_ERROR");
    assert_eq!(target.connections(), 0, "the redirect target was reached");
}

#[tokio::test]
async fn errors_never_carry_the_url_path_or_query() {
    // Port 9 on loopback: nothing listens (connection refused).
    let err = LoopbackOnly(any())
        .send(get("http://127.0.0.1:9/secret-path?key=MARKER"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, HttpErrorKind::Connect, "{err:?}");
    let shown = format!("{err:?} {err}");
    assert!(
        !shown.contains("secret-path") && !shown.contains("MARKER"),
        "{shown}"
    );
}

#[tokio::test]
async fn an_invalid_url_is_refused() {
    let err = any().send(get("not a url")).await.unwrap_err();
    assert_eq!(err.kind, HttpErrorKind::InvalidUrl);
    assert_eq!(err.code(), "INVALID_ARGUMENT");
}

// --------------------------------------------------------------- egress --

/// S3's literals, each reaching the mock under `Any` and refused under
/// `Public` before any connection.
#[tokio::test]
async fn ip_literals_are_refused_under_public_before_connecting() {
    let mock = MockProvider::start().await;
    let port = mock.url().rsplit(':').next().unwrap().to_string();
    for host in [
        "127.0.0.1",
        "127.1",
        "2130706433",
        "0x7f.1",
        "0177.0.0.1",
        "[::ffff:127.0.0.1]",
        "[::1]",
        "0.0.0.0",
    ] {
        for scheme in ["http", "https"] {
            let url = format!("{scheme}://{host}:{port}/v1/models");
            let err = public().send(get(&url)).await.unwrap_err();
            assert_eq!(err.kind, HttpErrorKind::EgressBlocked, "{url}: {err:?}");
            assert_eq!(err.code(), "AI_EGRESS_BLOCKED");
        }
    }
    assert_eq!(mock.connections(), 0);
    // The same literals reach it under Any (http; the mock has no TLS).
    for host in ["127.0.0.1", "127.1", "2130706433", "[::ffff:127.0.0.1]"] {
        mock.reply(Reply::json(200, &serde_json::json!({"data":[]})));
        let url = format!("http://{host}:{port}/v1/models");
        let resp = any().send(get(&url)).await.unwrap();
        assert_eq!(resp.status, 200, "{url}");
    }
    assert_eq!(mock.connections(), 4);
}

#[tokio::test]
async fn metadata_and_cgnat_literals_are_refused_under_public() {
    for url in [
        "https://169.254.169.254/latest/meta-data/",
        "https://100.64.0.1/v1",
        "https://[fd00::1]/v1",
        "https://[fe80::1]/v1",
    ] {
        let err = public().send(get(url)).await.unwrap_err();
        assert_eq!(err.kind, HttpErrorKind::EgressBlocked, "{url}");
    }
}

#[tokio::test]
async fn http_is_refused_under_public() {
    let err = public()
        .send(get("http://api.openai.com/v1/models"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, HttpErrorKind::EgressBlocked);
}

#[tokio::test]
async fn off_refuses_every_call() {
    let mock = MockProvider::start().await;
    let http = NativeHttp::new(NativeHttpOptions::new(Egress::Off));
    let err = http
        .send(get(&format!("{}/v1/models", mock.url())))
        .await
        .unwrap_err();
    assert_eq!(err.kind, HttpErrorKind::EgressBlocked);
    assert_eq!(mock.connections(), 0);
}

/// S3: `localhost` goes through the resolver, which refuses it; the mock
/// sees nothing.
#[tokio::test]
async fn a_name_resolving_to_loopback_is_refused_by_the_resolver() {
    let mock = MockProvider::start().await;
    let port = mock.url().rsplit(':').next().unwrap().to_string();
    let err = public()
        .send(get(&format!("https://localhost:{port}/v1/models")))
        .await
        .unwrap_err();
    assert_eq!(err.kind, HttpErrorKind::EgressBlocked, "{err:?}");
    assert_eq!(mock.connections(), 0);
}

/// A name whose answers are all private (DNS pointing at the mock) is
/// refused; the resolver was asked.
#[tokio::test]
async fn a_name_resolving_only_to_private_addresses_is_refused() {
    let mock = MockProvider::start().await;
    let addr: SocketAddr = mock.url().trim_start_matches("http://").parse().unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let a = asked.clone();
    let mut options = NativeHttpOptions::new(Egress::Public);
    options.resolver = Some(PublicResolver::with_lookup(move |_| {
        a.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(vec![
                SocketAddr::new("10.0.0.7".parse().unwrap(), 0),
                SocketAddr::new(addr.ip(), 0),
            ])
        })
    }));
    let err = NativeHttp::new(options)
        .send(get(&format!(
            "https://rebind.example.invalid:{}/v1/models",
            addr.port()
        )))
        .await
        .unwrap_err();
    assert_eq!(err.kind, HttpErrorKind::EgressBlocked, "{err:?}");
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(mock.connections(), 0);
}

/// A fake HTTP proxy on loopback: records the first request line it gets,
/// answers 502 and closes.
async fn fake_proxy() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    #[allow(clippy::disallowed_methods)]
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let line = String::from_utf8_lossy(&buf[..n])
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            s.lock().unwrap().push(line);
            let _ = sock
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await;
        }
    });
    (url, seen)
}

/// A resolver for the proxy tests: fixed answers per name, an error for
/// any other name (as an internal name the server can't resolve), and a
/// count of what it was asked.
fn proxy_test_resolver(asked: Arc<Mutex<Vec<String>>>) -> PublicResolver {
    PublicResolver::with_lookup(move |name| {
        asked.lock().unwrap().push(name.clone());
        Box::pin(async move {
            let ip = match name.as_str() {
                "localhost" => "127.0.0.1",
                "public.test" => "8.8.8.8",
                "private.test" => "10.0.0.7",
                "metadata.test" => "169.254.169.254",
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "no such name",
                    ))
                }
            };
            Ok(vec![SocketAddr::new(ip.parse().unwrap(), 0)])
        })
    })
}

/// Rule 4: through a proxy the URL check still runs (a private literal
/// never reaches the proxy), and the target name is resolved here first,
/// best effort (review fix 3): only private answers are refused; a name
/// that doesn't resolve here goes to the proxy, which resolves it.
#[tokio::test]
async fn through_a_proxy_the_host_check_still_runs_and_private_names_are_refused() {
    let (proxy, seen) = fake_proxy().await;
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut options = NativeHttpOptions::new(Egress::Public);
    options.proxy = Some(proxy);
    options.resolver = Some(proxy_test_resolver(asked.clone()));
    let http = NativeHttp::new(options);

    // A private literal: refused here, the proxy sees nothing.
    let err = http
        .send(get("https://169.254.169.254/latest/meta-data/"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, HttpErrorKind::EgressBlocked);
    // Names that resolve here only to private addresses: refused too.
    for url in [
        "https://private.test/v1/models",
        "https://metadata.test/latest",
    ] {
        let err = http.send(get(url)).await.unwrap_err();
        assert_eq!(err.kind, HttpErrorKind::EgressBlocked, "{url}");
        assert_eq!(err.code(), "AI_EGRESS_BLOCKED");
    }
    assert!(seen.lock().unwrap().is_empty());

    // A public name reaches the proxy as a CONNECT; so does a name that
    // doesn't resolve here. The fake proxy refuses the tunnel, so the
    // calls fail after that.
    let _ = http.send(get("https://public.test/v1/models")).await;
    let _ = http
        .send(get("https://internal.example.invalid/v1/models"))
        .await;
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        [
            "CONNECT public.test:443 HTTP/1.1",
            "CONNECT internal.example.invalid:443 HTTP/1.1"
        ]
    );
    let asked = asked.lock().unwrap().clone();
    assert!(
        asked.contains(&"internal.example.invalid".to_string()),
        "{asked:?}"
    );
}

/// Review fix 3: a proxy named by host (`localhost`) is reached under
/// `Public`: the resolver lets the proxy's own host through unfiltered.
#[tokio::test]
async fn a_proxy_named_by_host_is_reached_under_public() {
    let (proxy, seen) = fake_proxy().await;
    let port = proxy.rsplit(':').next().unwrap().to_string();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut options = NativeHttpOptions::new(Egress::Public);
    options.proxy = Some(format!("http://localhost:{port}"));
    options.resolver = Some(proxy_test_resolver(asked.clone()));
    let http = NativeHttp::new(options);
    let err = http
        .send(get("https://public.test/v1/models"))
        .await
        .unwrap_err();
    assert_ne!(err.kind, HttpErrorKind::EgressBlocked, "{err:?}");
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["CONNECT public.test:443 HTTP/1.1"]
    );
    // The target's own name still may not be private.
    let err = http
        .send(get("https://private.test/v1/models"))
        .await
        .unwrap_err();
    assert_eq!(err.kind, HttpErrorKind::EgressBlocked);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

/// Re-review 2: the local name check through a proxy is bounded by the
/// connect timeout; a lookup that hangs counts as "doesn't resolve here",
/// so the request goes on to the proxy.
#[tokio::test]
async fn a_hung_local_lookup_through_a_proxy_times_out_and_goes_to_the_proxy() {
    let (proxy, seen) = fake_proxy().await;
    let mut options = NativeHttpOptions::new(Egress::Public);
    options.proxy = Some(proxy);
    options.connect_timeout = Duration::from_millis(300);
    options.round_timeout = Duration::from_secs(3);
    options.resolver = Some(PublicResolver::with_lookup(|_| {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(vec![])
        })
    }));
    let http = NativeHttp::new(options);
    let started = std::time::Instant::now();
    let _ = http.send(get("https://slow.test/v1/models")).await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["CONNECT slow.test:443 HTTP/1.1"]
    );
}

/// Re-review 3: under `Public` the proxy's own host is never a target
/// (case and a trailing dot ignored), even when the name doesn't resolve
/// here and `NO_PROXY` would send it direct.
#[tokio::test]
async fn the_proxy_host_itself_is_refused_as_a_target() {
    let (proxy, seen) = fake_proxy().await;
    let port = proxy.rsplit(':').next().unwrap().to_string();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut options = NativeHttpOptions::new(Egress::Public);
    // `proxy.test` doesn't resolve in the test resolver: the local check
    // fails open, so only the proxy-host rule stops these.
    options.proxy = Some(format!("http://Proxy.Test:{port}"));
    options.resolver = Some(proxy_test_resolver(asked));
    let http = NativeHttp::new(options);
    for url in [
        "https://proxy.test/v1",
        "https://PROXY.TEST./v1",
        "https://proxy.test:8443/admin",
    ] {
        let err = http.send(get(url)).await.unwrap_err();
        assert_eq!(err.kind, HttpErrorKind::EgressBlocked, "{url}: {err:?}");
    }
    assert!(seen.lock().unwrap().is_empty());
    // Under Any the rule doesn't apply.
    let mut options = NativeHttpOptions::new(Egress::Any);
    options.proxy = Some(format!("http://127.0.0.1:{port}"));
    let err = NativeHttp::new(options)
        .send(get(&format!("http://127.0.0.1:{port}/x")))
        .await;
    assert!(err.map(|_| ()).map_err(|e| e.kind) != Err(HttpErrorKind::EgressBlocked));
}

/// Review fix 6: the options' `Debug` shows whether a proxy is set, not
/// the proxy (it may carry credentials).
#[test]
fn options_debug_hides_the_proxy() {
    let mut options = NativeHttpOptions::new(Egress::Public);
    options.proxy = Some("http://user:SECRETPW@proxy.internal:3128".into());
    let shown = format!("{options:?}");
    assert!(
        !shown.contains("SECRETPW") && !shown.contains("proxy.internal"),
        "{shown}"
    );
    assert!(shown.contains("proxy: true"), "{shown}");
}

/// Review fix 9: an OpenRouter-sized model list (~1 MB) read through the
/// client under the models cap parses.
#[tokio::test]
async fn a_large_model_list_is_read_whole() {
    use seaquel_ai::wire::{decode_models, MAX_MODELS_BODY_BYTES};
    let mock = MockProvider::start().await;
    let models: Vec<serde_json::Value> = (0..4000)
        .map(|i| serde_json::json!({"id": format!("vendor/model-{i}"), "description": "d".repeat(200)}))
        .collect();
    mock.reply(Reply::json(200, &serde_json::json!({"data": models})));
    let resp = LoopbackOnly(any())
        .send(get(&format!("{}/v1/models", mock.url())))
        .await
        .unwrap();
    let body = read_body(resp.body, MAX_MODELS_BODY_BYTES).await.unwrap();
    assert!(body.len() > 900_000);
    assert_eq!(decode_models(&body).unwrap().len(), 4000);
}

#[tokio::test]
async fn the_loopback_only_wrapper_lets_the_mock_through_and_nothing_else() {
    let result = tokio::spawn(async {
        LoopbackOnly(any())
            .send(get("https://api.anthropic.com/v1/models"))
            .await
            .map(|_| ())
    })
    .await;
    let panic = result.unwrap_err();
    assert!(panic.is_panic());
}
