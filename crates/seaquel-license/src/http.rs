//! Building the reqwest client without ever panicking.
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
pub(crate) struct ClientOptions {
    pub connect_timeout: Option<Duration>,
    pub timeout: Option<Duration>,
    /// Trusted in addition to the built-in roots.
    pub extra_roots: Vec<reqwest::Certificate>,
}

/// The certificates in the PEM file at `path`, for trusting a
/// TLS-inspecting proxy's CA the way Node's fetch did with
/// `NODE_EXTRA_CA_CERTS`. A file that can't be read or parsed, and each
/// certificate rustls won't take as a root, is skipped with a warning.
// Only the server's control-plane client uses it (the desktop trusts the OS
// store); the tests cover it in every build.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub(crate) fn load_extra_roots(path: &Path) -> Vec<reqwest::Certificate> {
    let shown = path.display();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            log::warn!(activity = "license.http"; "NODE_EXTRA_CA_CERTS {shown} can't be read ({e}); ignoring it");
            return Vec::new();
        }
    };
    let certs = match reqwest::Certificate::from_pem_bundle(&bytes) {
        Ok(c) => c,
        Err(e) => {
            log::warn!(activity = "license.http"; "NODE_EXTRA_CA_CERTS {shown} isn't a PEM bundle ({e}); ignoring it");
            return Vec::new();
        }
    };
    if certs.is_empty() {
        log::warn!(activity = "license.http"; "NODE_EXTRA_CA_CERTS {shown} holds no certificate");
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
            if let Err(e) = &ok {
                log::warn!(activity = "license.http"; "NODE_EXTRA_CA_CERTS {shown}: certificate #{} rejected ({e}); skipping it", i + 1);
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
    b.build()
}

/// Try `attempt(true)` (OS roots too), then `attempt(false)` (webpki only).
/// `None` when both fail. Split out so the fallback order is testable
/// without a broken certificate store.
pub(crate) fn with_fallback<C, E: std::fmt::Display>(
    what: &str,
    mut attempt: impl FnMut(bool) -> Result<C, E>,
) -> Option<C> {
    match attempt(true) {
        Ok(c) => return Some(c),
        Err(e) => {
            log::warn!(activity = "license.http"; "{what}: HTTP client with the OS certificate store failed ({e}); retrying with the bundled roots only")
        }
    }
    match attempt(false) {
        Ok(c) => Some(c),
        Err(e) => {
            log::error!(activity = "license.http"; "{what}: HTTP client couldn't be built ({e}); will retry on first use");
            None
        }
    }
}

/// A client built at construction when possible, else on first use.
#[derive(Clone)]
pub(crate) struct LazyClient {
    what: &'static str,
    options: ClientOptions,
    client: Arc<Mutex<Option<reqwest::Client>>>,
}

impl LazyClient {
    pub(crate) fn new(what: &'static str, options: ClientOptions) -> Self {
        let client = with_fallback(what, |native| build(&options, native));
        Self {
            what,
            options,
            client: Arc::new(Mutex::new(client)),
        }
    }

    /// The client, building it now if construction failed. `Err` carries a
    /// message for the caller's network error.
    pub(crate) fn get(&self) -> Result<reqwest::Client, String> {
        let mut slot = self.client.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = slot.as_ref() {
            return Ok(c.clone());
        }
        let built = with_fallback(self.what, |native| build(&self.options, native))
            .ok_or_else(|| "the HTTP client couldn't be set up (TLS)".to_string())?;
        *slot = Some(built.clone());
        Ok(built)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

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
        let roots = load_extra_roots(one.path());
        assert_eq!(roots.len(), 1);
        let two = temp_pem(&format!("{TEST_CA}\n{TEST_CA}"));
        assert_eq!(load_extra_roots(two.path()).len(), 2);
        let options = ClientOptions {
            extra_roots: roots,
            ..Default::default()
        };
        assert!(build(&options, false).is_ok());
        assert!(LazyClient::new("t", options).get().is_ok());
    }

    #[test]
    fn a_bad_extra_roots_file_is_ignored_not_fatal() {
        assert!(load_extra_roots(Path::new("/nonexistent/seaquel-ca.pem")).is_empty());
        let garbage =
            temp_pem("-----BEGIN CERTIFICATE-----\nnot base64!!\n-----END CERTIFICATE-----\n");
        assert!(load_extra_roots(garbage.path()).is_empty());
        let empty = temp_pem("no pem here");
        assert!(load_extra_roots(empty.path()).is_empty());
        // A valid PEM next to a certificate rustls rejects keeps the valid one.
        let mixed = temp_pem(&format!(
            "{TEST_CA}-----BEGIN CERTIFICATE-----\nMAMCAQA=\n-----END CERTIFICATE-----\n"
        ));
        assert_eq!(load_extra_roots(mixed.path()).len(), 1);
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
