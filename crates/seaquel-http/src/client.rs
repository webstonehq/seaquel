//! Building the reqwest client without ever panicking. Moved from
//! `seaquel-license` (phase 6); the license clients' behaviour is unchanged.
//!
//! `reqwest::Client::new()` (and `Client::default()`) panic when the TLS
//! backend can't be set up; with rustls and native roots that happens when
//! the OS store yields no valid certificate and at least one invalid one.
//! The desktop builds its client in Tauri's `setup`, so a panic there would
//! take the whole app down. Instead:
//!
//! 1. build with the OS and the bundled webpki roots;
//! 2. on failure, log it and build again without the OS roots (webpki only);
//! 3. if that fails too, keep no client: [`LazyClient::get`] tries again on
//!    each use and the call fails with the caller's network error.
//!
//! Extra roots (the server's `NODE_EXTRA_CA_CERTS`, see [`load_extra_roots`])
//! are added on top in every attempt; each was checked on its own when it
//! was loaded, so one bad certificate can't fail the build.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How a client is configured before the roots are chosen.
#[derive(Clone, Default)]
pub struct ClientOptions {
    pub connect_timeout: Option<Duration>,
    /// The whole request, body included.
    pub timeout: Option<Duration>,
    /// Between two reads (an idle provider).
    pub read_timeout: Option<Duration>,
    /// Trusted in addition to the built-in roots.
    pub extra_roots: Vec<reqwest::Certificate>,
    /// Never follow a redirect (`Policy::none()`): the 3xx is the answer.
    /// Off keeps reqwest's default (up to 10).
    pub no_redirects: bool,
    /// Resolve names through the egress filter instead of the system.
    pub resolver: Option<Arc<crate::egress::PublicResolver>>,
    /// A proxy for every request instead of the `HTTP(S)_PROXY`/`NO_PROXY`
    /// environment (which reqwest reads by default).
    pub proxy: Option<reqwest::Proxy>,
    /// The `activity` of this module's log lines; `license.http` when
    /// empty, as before the move.
    pub activity: &'static str,
    /// Take proxies from the `HTTP(S)_PROXY`/`ALL_PROXY`/`NO_PROXY`
    /// environment only, never from the OS settings (`Egress::Public`):
    /// with reqwest's `system-proxy` feature on, which Cargo's
    /// feature unification can do in any build, its automatic proxies
    /// include the macOS/Windows system proxy, which the egress guard
    /// can't see. See [`proxy_plan`].
    pub env_proxies_only: bool,
}

/// How the client picks its proxies (see [`proxy_plan`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxyPlan {
    /// [`ClientOptions::proxy`]: every request through it.
    Given,
    /// reqwest's automatic proxies: the environment, and the OS settings
    /// with `system-proxy`.
    Auto,
    /// These, from the environment, each with `NO_PROXY`'s exceptions;
    /// automatic proxies off.
    Env(Vec<(ProxyScheme, String)>),
    /// No proxy at all; automatic proxies off.
    Off,
}

/// Which requests an [`ProxyPlan::Env`] proxy takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyScheme {
    Http,
    Https,
    All,
}

/// The proxy decision for `options`, over the environment `get`.
/// Whenever it says anything but [`ProxyPlan::Auto`], the client turns
/// reqwest's automatic proxies (and so the OS proxy) off.
///
/// The environment is read as reqwest reads it: each variable in upper
/// case, else lower case; empty values don't count; `HTTP_PROXY` is
/// ignored when `REQUEST_METHOD` is set (a CGI request, httpoxy).
pub fn proxy_plan(options: &ClientOptions, get: impl Fn(&str) -> Option<String>) -> ProxyPlan {
    if options.proxy.is_some() {
        return ProxyPlan::Given;
    }
    if !options.env_proxies_only {
        return ProxyPlan::Auto;
    }
    let var = |upper: &str| {
        [upper.to_string(), upper.to_ascii_lowercase()]
            .iter()
            .filter_map(|k| get(k))
            .map(|v| v.trim().to_string())
            .find(|v| !v.is_empty())
    };
    let cgi = get("REQUEST_METHOD").is_some();
    let mut proxies = Vec::new();
    for (scheme, name) in [
        (ProxyScheme::All, "ALL_PROXY"),
        (ProxyScheme::Http, "HTTP_PROXY"),
        (ProxyScheme::Https, "HTTPS_PROXY"),
    ] {
        if scheme == ProxyScheme::Http && cgi {
            continue;
        }
        if let Some(url) = var(name) {
            proxies.push((scheme, url));
        }
    }
    if proxies.is_empty() {
        ProxyPlan::Off
    } else {
        ProxyPlan::Env(proxies)
    }
}

impl ClientOptions {
    fn activity(&self) -> &'static str {
        if self.activity.is_empty() {
            "license.http"
        } else {
            self.activity
        }
    }
}

/// Whether [`load_extra_roots_with`]'s warnings name the file. The server
/// and the license client name it (the operator set it); the terminal
/// binaries don't (a path under the user's home stays out of
/// their logs), and say only the error's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowPath {
    Yes,
    No,
}

/// The warning for a file that can't be read, its `kind` being the
/// `io::ErrorKind`.
pub fn unreadable_roots_message(path: &Path, show: ShowPath, kind: &str) -> String {
    match show {
        ShowPath::Yes => format!(
            "NODE_EXTRA_CA_CERTS {} can't be read ({kind}); ignoring it",
            path.display()
        ),
        ShowPath::No => format!("NODE_EXTRA_CA_CERTS can't be read ({kind}); ignoring it"),
    }
}

/// The certificates in the PEM file at `path`, for trusting a
/// TLS-inspecting proxy's CA the way Node's fetch did with
/// `NODE_EXTRA_CA_CERTS`. A file that can't be read or parsed, and each
/// certificate rustls won't take as a root, is skipped with a warning that
/// names the file.
// The server's control-plane client and model calls use it (the desktop
// trusts the OS store).
pub fn load_extra_roots(path: &Path, activity: &'static str) -> Vec<reqwest::Certificate> {
    load_extra_roots_with(path, activity, ShowPath::Yes)
}

/// [`load_extra_roots`], its warnings naming the file only with
/// [`ShowPath::Yes`].
pub fn load_extra_roots_with(
    path: &Path,
    activity: &'static str,
    show: ShowPath,
) -> Vec<reqwest::Certificate> {
    let shown = match show {
        ShowPath::Yes => format!(" {}", path.display()),
        ShowPath::No => String::new(),
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            let text = unreadable_roots_message(path, show, &format!("{:?}", e.kind()));
            log::warn!(activity = activity; "{text}");
            return Vec::new();
        }
    };
    let certs = match reqwest::Certificate::from_pem_bundle(&bytes) {
        Ok(c) => c,
        Err(e) => {
            match show {
                ShowPath::Yes => {
                    log::warn!(activity = activity; "NODE_EXTRA_CA_CERTS{shown} isn't a PEM bundle ({e}); ignoring it")
                }
                ShowPath::No => {
                    log::warn!(activity = activity; "NODE_EXTRA_CA_CERTS isn't a PEM bundle; ignoring it")
                }
            }
            return Vec::new();
        }
    };
    if certs.is_empty() {
        log::warn!(activity = activity; "NODE_EXTRA_CA_CERTS{shown} holds no certificate");
    }
    certs
        .into_iter()
        .enumerate()
        .filter(|(i, cert)| {
            // reqwest only checks a root when it builds, and one bad root
            // fails the whole build; try each alone.
            let ok = reqwest::Client::builder()
                .tls_built_in_root_certs(false)
                .add_root_certificate(cert.clone())
                .build();
            match (&ok, show) {
                (Err(e), ShowPath::Yes) => {
                    log::warn!(activity = activity; "NODE_EXTRA_CA_CERTS{shown}: certificate #{} rejected ({e}); skipping it", i + 1)
                }
                (Err(_), ShowPath::No) => {
                    log::warn!(activity = activity; "NODE_EXTRA_CA_CERTS: certificate #{} rejected; skipping it", i + 1)
                }
                (Ok(_), _) => {}
            }
            ok.is_ok()
        })
        .map(|(_, cert)| cert)
        .collect()
}

fn build(options: &ClientOptions, native_roots: bool) -> Result<reqwest::Client, reqwest::Error> {
    let mut b = reqwest::Client::builder().tls_built_in_native_certs(native_roots);
    for cert in &options.extra_roots {
        b = b.add_root_certificate(cert.clone());
    }
    if let Some(t) = options.connect_timeout {
        b = b.connect_timeout(t);
    }
    if let Some(t) = options.timeout {
        b = b.timeout(t);
    }
    if let Some(t) = options.read_timeout {
        b = b.read_timeout(t);
    }
    if options.no_redirects {
        b = b.redirect(reqwest::redirect::Policy::none());
    }
    if let Some(r) = &options.resolver {
        b = b.dns_resolver(r.clone());
    }
    match proxy_plan(options, |k| std::env::var(k).ok()) {
        ProxyPlan::Auto => {}
        ProxyPlan::Given => {
            if let Some(p) = &options.proxy {
                b = b.proxy(p.clone());
            }
        }
        // `no_proxy()` turns reqwest's automatic proxies off, the OS's
        // included; the environment's are then added one by one.
        ProxyPlan::Off => b = b.no_proxy(),
        ProxyPlan::Env(proxies) => {
            b = b.no_proxy();
            for (scheme, url) in proxies {
                let proxy = match scheme {
                    ProxyScheme::All => reqwest::Proxy::all(&url),
                    ProxyScheme::Http => reqwest::Proxy::http(&url),
                    ProxyScheme::Https => reqwest::Proxy::https(&url),
                };
                match proxy {
                    Ok(p) => b = b.proxy(p.no_proxy(reqwest::NoProxy::from_env())),
                    // The value may hold credentials: never logged.
                    Err(_) => {
                        log::warn!(activity = options.activity(); "a proxy variable isn't a valid URL; ignoring it")
                    }
                }
            }
        }
    }
    b.build()
}

/// Try `attempt(true)` (OS roots too), then `attempt(false)` (webpki only).
/// `None` when both fail. Split out so the fallback order is testable
/// without a broken certificate store.
pub fn with_fallback<C, E: std::fmt::Display>(
    what: &str,
    attempt: impl FnMut(bool) -> Result<C, E>,
) -> Option<C> {
    with_fallback_logged("license.http", what, attempt)
}

fn with_fallback_logged<C, E: std::fmt::Display>(
    activity: &'static str,
    what: &str,
    mut attempt: impl FnMut(bool) -> Result<C, E>,
) -> Option<C> {
    match attempt(true) {
        Ok(c) => return Some(c),
        Err(e) => {
            log::warn!(activity = activity; "{what}: HTTP client with the OS certificate store failed ({e}); retrying with the bundled roots only")
        }
    }
    match attempt(false) {
        Ok(c) => Some(c),
        Err(e) => {
            log::error!(activity = activity; "{what}: HTTP client couldn't be built ({e}); will retry on first use");
            None
        }
    }
}

/// A client built at construction when possible, else on first use.
#[derive(Clone)]
pub struct LazyClient {
    what: &'static str,
    options: ClientOptions,
    client: Arc<Mutex<Option<reqwest::Client>>>,
}

impl LazyClient {
    pub fn new(what: &'static str, options: ClientOptions) -> Self {
        let client =
            with_fallback_logged(options.activity(), what, |native| build(&options, native));
        Self {
            what,
            options,
            client: Arc::new(Mutex::new(client)),
        }
    }

    /// The client, building it now if construction failed. `Err` carries a
    /// message for the caller's network error.
    pub fn get(&self) -> Result<reqwest::Client, String> {
        let mut slot = self.client.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = slot.as_ref() {
            return Ok(c.clone());
        }
        let built = with_fallback_logged(self.options.activity(), self.what, |native| {
            build(&self.options, native)
        })
        .ok_or_else(|| "the HTTP client couldn't be set up (TLS)".to_string())?;
        *slot = Some(built.clone());
        Ok(built)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    /// Under `Public` the OS proxy can never be picked, whatever
    /// features Cargo unified: the plan is never `Auto`.
    #[test]
    fn env_only_never_leaves_proxies_to_reqwest() {
        let only = ClientOptions {
            env_proxies_only: true,
            ..Default::default()
        };
        assert_eq!(proxy_plan(&only, env(&[])), ProxyPlan::Off);
        assert_eq!(
            proxy_plan(&only, env(&[("HTTPS_PROXY", "http://p:3128"), ("", "")])),
            ProxyPlan::Env(vec![(ProxyScheme::Https, "http://p:3128".into())])
        );
        assert_eq!(
            proxy_plan(
                &only,
                env(&[
                    ("all_proxy", "socks5://a:1"),
                    ("http_proxy", "http://h:1"),
                    ("HTTPS_PROXY", " "),
                ])
            ),
            ProxyPlan::Env(vec![
                (ProxyScheme::All, "socks5://a:1".into()),
                (ProxyScheme::Http, "http://h:1".into()),
            ])
        );
        // Upper case wins, as reqwest reads them.
        assert_eq!(
            proxy_plan(
                &only,
                env(&[
                    ("HTTP_PROXY", "http://up:1"),
                    ("http_proxy", "http://low:1")
                ])
            ),
            ProxyPlan::Env(vec![(ProxyScheme::Http, "http://up:1".into())])
        );
        // A CGI request (`REQUEST_METHOD` set) can't pick HTTP_PROXY, as
        // reqwest ignores it there (httpoxy).
        assert_eq!(
            proxy_plan(
                &only,
                env(&[("HTTP_PROXY", "http://evil:1"), ("REQUEST_METHOD", "GET")])
            ),
            ProxyPlan::Off
        );
        // A given proxy is the decision either way.
        let given = ClientOptions {
            env_proxies_only: true,
            proxy: Some(reqwest::Proxy::all("http://g:1").unwrap()),
            ..Default::default()
        };
        assert_eq!(
            proxy_plan(&given, env(&[("HTTPS_PROXY", "http://p:1")])),
            ProxyPlan::Given
        );
        // The license client keeps reqwest's own behaviour.
        assert_eq!(
            proxy_plan(
                &ClientOptions::default(),
                env(&[("HTTPS_PROXY", "http://p:1")])
            ),
            ProxyPlan::Auto
        );
    }

    #[test]
    fn native_roots_first() {
        let tried = RefCell::new(Vec::new());
        let got = with_fallback("t", |native| {
            tried.borrow_mut().push(native);
            Ok::<_, String>(native)
        });
        assert_eq!(got, Some(true));
        assert_eq!(*tried.borrow(), [true]);
    }

    #[test]
    fn falls_back_to_bundled_roots_when_native_fail() {
        let tried = RefCell::new(Vec::new());
        let got = with_fallback("t", |native| {
            tried.borrow_mut().push(native);
            if native {
                Err("bad native cert".to_string())
            } else {
                Ok("webpki")
            }
        });
        assert_eq!(got, Some("webpki"));
        assert_eq!(*tried.borrow(), [true, false]);
    }

    #[test]
    fn none_when_both_fail() {
        let got: Option<()> = with_fallback("t", |_| Err::<(), _>("nope"));
        assert_eq!(got, None);
    }

    /// A self-signed RSA CA, made for this test with openssl.
    const TEST_CA: &str = "-----BEGIN CERTIFICATE-----
MIICsjCCAZoCCQDxAXFu1Q9JgTANBgkqhkiG9w0BAQsFADAaMRgwFgYDVQQDDA9T
ZWFxdWVsIFRlc3QgQ0EwIBcNMjYwOTI2MTU0MTM5WhgPMjEyNjA5MDIxNTQxMzla
MBoxGDAWBgNVBAMMD1NlYXF1ZWwgVGVzdCBDQTCCASIwDQYJKoZIhvcNAQEBBQAD
ggEPADCCAQoCggEBAKzktw/iJd0PtxnMOWbcgpkJikpdRudDni1kwZO+mFXATqa8
L5UNOraFmg9IJpeahxgqN6sUW5Y3Z+oKVs0j9uBKW2HPsK2rEmIEyvnnJ0iSqJ5V
53HxfEMXew+dKY6L4WdX6flhmkC+NzBf/CWbc13jRisiXjoQ5RA+xMNaFpADL43m
ZHMMggYnl/8MGQ/05kaoar3TfoiKEMW65zFE3BoRjagCv28KkiMuhBAhWvWgGNLe
sbEkgooXWMiSEScuI2i4Ai43j6+Jt7lflsdi9i4nVsstP2uHVQEuCDcOy6cblP6j
M5Ustr9Dk21GsuW3wMFFNZ/McWL2Y/AVsWIKYsMCAwEAATANBgkqhkiG9w0BAQsF
AAOCAQEAARqhYCSUWJVrKLyKPPgLY9SkkCj+yiRiuuor4jnxGXOV/oL6Ttekdzyg
X6F8WWa1qwQwGfdxk1dcTl8UtYICTHhetPkazFm3Y3kRn3U3W5oJJq042zoz70cY
ZYsI1aHgIkLKqKucFxHuJzBR8V0RIa2RYTiqwTiShEaeVFXZEcoRhwMyhc8AKZ5L
NzNfoODCU1ViJp2D4E254DL91W8GDG6lNk51lwTZnD4AcOSCoWB8LDHK/I9vrAOA
56H/XO5ZmhVI+VkTTZl1gQ3UnU+qgKRYWwvL4rGs42OEgYtGKED7cppjInB81KCv
W1RIbAgTWVo5Sz2/Uy0giXv/lerWGg==
-----END CERTIFICATE-----
";

    fn temp_pem(contents: &str) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f
    }

    #[test]
    fn extra_roots_load_from_a_pem_file_and_build() {
        let one = temp_pem(TEST_CA);
        let roots = load_extra_roots(one.path(), "t");
        assert_eq!(roots.len(), 1);
        let two = temp_pem(&format!("{TEST_CA}\n{TEST_CA}"));
        assert_eq!(load_extra_roots(two.path(), "t").len(), 2);
        let options = ClientOptions {
            extra_roots: roots,
            ..Default::default()
        };
        assert!(build(&options, false).is_ok());
        assert!(LazyClient::new("t", options).get().is_ok());
    }

    #[test]
    fn a_bad_extra_roots_file_is_ignored_not_fatal() {
        assert!(load_extra_roots(Path::new("/nonexistent/seaquel-ca.pem"), "t").is_empty());
        let garbage =
            temp_pem("-----BEGIN CERTIFICATE-----\nnot base64!!\n-----END CERTIFICATE-----\n");
        assert!(load_extra_roots(garbage.path(), "t").is_empty());
        let empty = temp_pem("no pem here");
        assert!(load_extra_roots(empty.path(), "t").is_empty());
        // A valid PEM next to a certificate rustls rejects keeps the valid one.
        let mixed = temp_pem(&format!(
            "{TEST_CA}-----BEGIN CERTIFICATE-----\nMAMCAQA=\n-----END CERTIFICATE-----\n"
        ));
        assert_eq!(load_extra_roots(mixed.path(), "t").len(), 1);
    }

    #[test]
    fn a_missing_client_is_built_on_first_use() {
        let lazy = LazyClient {
            what: "t",
            options: ClientOptions::default(),
            client: Arc::new(Mutex::new(None)),
        };
        assert!(lazy.get().is_ok());
        assert!(lazy.client.lock().unwrap().is_some());
    }
}
