//! `seaquel_ai::http::HttpClient` over reqwest, for model calls.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use seaquel_ai::http::{HttpClient, HttpError, HttpErrorKind, HttpRequest, HttpResponse, Method};

use crate::client::{load_extra_roots, ClientOptions, LazyClient};
use crate::egress::{check_url, Egress, PublicResolver, Refusal};

/// How [`NativeHttp`] is set up. [`NativeHttpOptions::new`] has Decision
/// 8's timeouts.
#[derive(Clone)]
pub struct NativeHttpOptions {
    pub egress: Egress,
    /// Up to the connection being open (TLS included).
    pub connect_timeout: Duration,
    /// Between two reads, the response head included: a provider that
    /// stops sending.
    pub idle_timeout: Duration,
    /// One request from start to the end of its body: a round.
    pub round_timeout: Duration,
    /// A PEM bundle trusted on top of the built-in roots (the server's
    /// `NODE_EXTRA_CA_CERTS`).
    pub extra_ca_file: Option<PathBuf>,
    /// Every request through this proxy instead of `HTTP(S)_PROXY`.
    pub proxy: Option<String>,
    /// [`Egress::Public`]'s resolver; the system's when `None`. Tests pass
    /// one with fixed answers.
    pub resolver: Option<PublicResolver>,
}

impl NativeHttpOptions {
    /// 10 s to connect, 120 s idle, 10 minutes per round.
    pub fn new(egress: Egress) -> Self {
        Self {
            egress,
            connect_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(120),
            round_timeout: Duration::from_secs(600),
            extra_ca_file: None,
            proxy: None,
            resolver: None,
        }
    }
}

impl std::fmt::Debug for NativeHttpOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The proxy URL may carry credentials, the CA path names the host's
        // layout: only whether they are set.
        f.debug_struct("NativeHttpOptions")
            .field("egress", &self.egress)
            .field("connect_timeout", &self.connect_timeout)
            .field("idle_timeout", &self.idle_timeout)
            .field("round_timeout", &self.round_timeout)
            .field("extra_ca_file", &self.extra_ca_file.is_some())
            .field("proxy", &self.proxy.is_some())
            .field("resolver", &self.resolver.is_some())
            .finish()
    }
}

/// The proxy URLs reqwest takes from the environment, as it reads them.
const PROXY_ENV: [&str; 6] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
];

/// A host as the proxy-host rule compares it: lowercase, without a
/// trailing dot or IPv6 brackets.
fn normalized_host(h: &str) -> String {
    h.trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// The hosts of the configured proxies (the option, else the
/// environment), and whether there is any.
fn proxy_hosts(option: Option<&str>) -> (bool, Vec<String>) {
    proxy_hosts_from(option, |k| std::env::var(k).ok())
}

/// [`proxy_hosts`] over any environment: the option wins over every
/// variable; empty values don't count; a value without a scheme is read as
/// `http://`, as reqwest does.
fn proxy_hosts_from(
    option: Option<&str>,
    get: impl Fn(&str) -> Option<String>,
) -> (bool, Vec<String>) {
    let urls: Vec<String> = match option {
        Some(p) => vec![p.to_string()],
        None => PROXY_ENV
            .iter()
            .filter_map(|k| get(k))
            .filter(|v| !v.trim().is_empty())
            .collect(),
    };
    let mut hosts: Vec<String> = urls
        .iter()
        .filter_map(|u| {
            let u = u.trim();
            let u = if u.contains("://") {
                u.to_string()
            } else {
                format!("http://{u}")
            };
            reqwest::Url::parse(&u)
                .ok()
                .and_then(|u| u.host_str().map(normalized_host))
        })
        .collect();
    hosts.dedup();
    (!urls.is_empty(), hosts)
}

/// Model calls over reqwest (rustls; the OS and webpki roots with the
/// fallback; proxies from the environment), never following a redirect,
/// under the egress rules. Never panics, also not when the TLS roots are
/// broken: the request fails instead.
pub struct NativeHttp {
    egress: Egress,
    client: LazyClient,
    /// `Public` only: the resolver, for rule 4's check through a proxy.
    resolver: Option<Arc<PublicResolver>>,
    proxied: bool,
    proxy_hosts: Vec<String>,
    connect_timeout: Duration,
}

impl NativeHttp {
    pub fn new(options: NativeHttpOptions) -> Self {
        let extra_roots = options
            .extra_ca_file
            .as_deref()
            .map(|p| load_extra_roots(p, "ai.http"))
            .unwrap_or_default();
        let proxy = options.proxy.as_deref().and_then(|p| match reqwest::Proxy::all(p) {
            Ok(p) => Some(p),
            Err(_) => {
                // The value may hold credentials: never logged.
                log::warn!(activity = "ai.http"; "the model client's proxy isn't a valid URL; ignoring it");
                None
            }
        });
        let (proxied, hosts) = proxy_hosts(options.proxy.as_deref());
        let resolver = (options.egress == Egress::Public).then(|| {
            Arc::new(
                options
                    .resolver
                    .unwrap_or_else(PublicResolver::system)
                    .with_unfiltered(hosts.clone()),
            )
        });
        let client = LazyClient::new(
            "model client",
            ClientOptions {
                connect_timeout: Some(options.connect_timeout),
                timeout: Some(options.round_timeout),
                read_timeout: Some(options.idle_timeout),
                extra_roots,
                no_redirects: true,
                resolver: resolver.clone(),
                proxy,
                activity: "ai.http",
                // Under `Public` the guard knows only the
                // environment's proxies, so the client takes no others.
                env_proxies_only: options.egress == Egress::Public,
            },
        );
        Self {
            egress: options.egress,
            client,
            resolver,
            proxied,
            proxy_hosts: hosts,
            connect_timeout: options.connect_timeout,
        }
    }
}

fn refused(r: Refusal, egress: Egress) -> HttpError {
    let kind = match r {
        Refusal::InvalidUrl => HttpErrorKind::InvalidUrl,
        // `ftp:`, `file:`, …: not a URL a provider can have, whatever the
        // egress; `http:` under `Public` is the egress rule.
        Refusal::Scheme if egress == Egress::Any => HttpErrorKind::InvalidUrl,
        _ => HttpErrorKind::EgressBlocked,
    };
    HttpError::new(kind, r.to_string())
}

/// reqwest's error without its URL, and the egress refusal when the
/// resolver made it.
fn from_reqwest(e: reqwest::Error) -> HttpError {
    let e = e.without_url();
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&e);
    let mut detail = e.to_string();
    while let Some(s) = source {
        if let Some(r) = s.downcast_ref::<Refusal>() {
            return HttpError::new(HttpErrorKind::EgressBlocked, r.to_string());
        }
        detail.push_str(": ");
        detail.push_str(&s.to_string());
        source = s.source();
    }
    let kind = if e.is_timeout() {
        HttpErrorKind::Timeout
    } else if e.is_connect() {
        HttpErrorKind::Connect
    } else if e.is_body() || e.is_decode() {
        HttpErrorKind::Body
    } else {
        HttpErrorKind::Other
    };
    HttpError::new(kind, detail)
}

#[seaquel_runtime::async_trait]
impl HttpClient for NativeHttp {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        if self.egress == Egress::Off {
            return Err(refused(Refusal::Off, self.egress));
        }
        let url =
            reqwest::Url::parse(&req.url).map_err(|_| refused(Refusal::InvalidUrl, self.egress))?;
        check_url(&url, self.egress).map_err(|r| refused(r, self.egress))?;
        if self.egress == Egress::Public {
            if let Some(host) = url.host_str() {
                // Rule 4: the proxy itself is never a target (its name
                // resolves unfiltered, and `NO_PROXY` could send it direct).
                if self.proxy_hosts.contains(&normalized_host(host)) {
                    return Err(refused(Refusal::ProxyHost, self.egress));
                }
            }
        }
        if let (true, Some(resolver), Some(url::Host::Domain(name))) =
            (self.proxied, &self.resolver, url.host())
        {
            // Rule 4: through a proxy nothing below resolves the name here.
            // Best effort and bounded: a lookup that doesn't answer within
            // the connect timeout counts as "doesn't resolve here", and the
            // proxy decides.
            match tokio::time::timeout(self.connect_timeout, resolver.check_name(name)).await {
                Ok(checked) => checked.map_err(|r| refused(r, self.egress))?,
                Err(_) => {
                    log::warn!(activity = "ai.http"; "the local name check before the proxy timed out; leaving it to the proxy");
                }
            }
        }
        let client = self
            .client
            .get()
            .map_err(|e| HttpError::new(HttpErrorKind::Connect, e))?;
        let mut builder = match req.method {
            Method::Get => client.get(url),
            Method::Post => client.post(url).body(req.body),
        };
        for (name, value) in &req.headers {
            builder = builder.header(name.as_str(), value.expose().as_str());
        }
        let resp = builder.send().await.map_err(from_reqwest)?;
        let status = resp.status().as_u16();
        // `resp` lives in the stream: dropping the stream drops the
        // connection (S1's cancel).
        let body = futures::stream::unfold(Some(resp), |state| async move {
            let mut resp = state?;
            match resp.chunk().await {
                Ok(Some(bytes)) => Some((Ok(bytes.to_vec()), Some(resp))),
                Ok(None) => None,
                Err(e) => Some((Err(from_reqwest(e)), None)),
            }
        })
        .boxed();
        Ok(HttpResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn proxy_hosts_read_every_variable_in_both_cases() {
        for name in PROXY_ENV {
            let pairs: &'static [(&'static str, &'static str)] =
                Box::leak(vec![(name, "http://Corp-Proxy.example:3128")].into_boxed_slice());
            let (proxied, hosts) = proxy_hosts_from(None, env(pairs));
            assert!(proxied, "{name}");
            assert_eq!(hosts, ["corp-proxy.example"], "{name}");
        }
    }

    #[test]
    fn proxy_hosts_without_a_scheme_ipv6_and_empty_values() {
        let (proxied, mut hosts) = proxy_hosts_from(
            None,
            env(&[
                ("HTTPS_PROXY", "squid.internal:3128"),
                ("http_proxy", "http://[fd00::1]:8080"),
                ("ALL_PROXY", "  "),
            ]),
        );
        hosts.sort();
        assert!(proxied);
        assert_eq!(hosts, ["fd00::1", "squid.internal"]);
        assert_eq!(
            proxy_hosts_from(None, env(&[("HTTP_PROXY", "")])),
            (false, vec![])
        );
        assert_eq!(proxy_hosts_from(None, env(&[])), (false, vec![]));
    }

    #[test]
    fn the_option_overrides_the_environment() {
        let (proxied, hosts) = proxy_hosts_from(
            Some("http://chosen.example:1"),
            env(&[
                ("HTTPS_PROXY", "http://env.example:2"),
                ("ALL_PROXY", "http://all.example:3"),
            ]),
        );
        assert!(proxied);
        assert_eq!(hosts, ["chosen.example"]);
    }
}
