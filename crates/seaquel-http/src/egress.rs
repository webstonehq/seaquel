//! Where model calls may go.
//!
//! On web a user names an OpenAI-compatible base URL and `seaquel-server`
//! calls it, so without a guard any user could make the server reach its
//! own network (cloud metadata at `169.254.169.254`, a database next to
//! it, the Rust service's loopback routes). Spike S3 showed a DNS filter
//! alone isn't enough: an IP literal, in any spelling the URL parser
//! normalises (`127.0.0.1`, `2130706433`, `0x7f.1`, `[::ffff:127.0.0.1]`),
//! never goes through DNS, a redirect is followed to wherever it points,
//! and through a proxy nothing resolves locally. So under [`Egress::Public`]:
//!
//! 1. **The parsed URL's host** may not be an IP literal in a blocked range
//!    ([`blocked_ip`]), and the scheme must be `https`. This runs on the
//!    URL as reqwest parses it, before anything is sent, so it holds with a
//!    proxy too.
//! 2. **The resolver** ([`PublicResolver`]) drops every blocked address a
//!    name resolves to and fails when none is left, so a name pointing at
//!    a private address (or rebinding to one) can't be reached directly.
//!    A name with both public and private addresses connects to the public
//!    ones only.
//! 3. **No redirect is followed** (every egress mode): a 3xx is the answer,
//!    and the wire turns it into `PROVIDER_ERROR`.
//! 4. **With a proxy** (`NativeHttpOptions::proxy`, or `HTTP_PROXY`,
//!    `HTTPS_PROXY` or `ALL_PROXY` in either case), rule 1 still runs, and
//!    the target name is first resolved here, best effort
//!    ([`PublicResolver::check_name`]): a name whose local answers are all
//!    blocked is refused. A name that doesn't resolve here goes to the
//!    proxy, which resolves it: past that the operator's proxy decides
//!    (split-horizon DNS catches most internal names here). The proxy's own
//!    host is resolved unfiltered, since the operator chose it. The check
//!    runs whenever a proxy is configured, even for a URL `NO_PROXY`
//!    exempts (that one then also meets rule 2). The operator docs say so.
//!
//! [`Egress::Any`] (desktop, CLI, or an operator's `SEAQUEL_AI_EGRESS=any`
//! for an Ollama next to the server) allows private addresses and `http:`,
//! still without redirects. [`Egress::Off`] refuses every model call.

use std::collections::HashSet;
use std::fmt;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Where model calls may go. Core's `AiEgress` maps onto it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Egress {
    /// No model call at all (`AI_EGRESS_BLOCKED`).
    Off,
    /// Public addresses over `https` only (the web server's default).
    Public,
    /// Anything, `http:` included (desktop, CLI).
    Any,
}

impl Egress {
    /// `SEAQUEL_AI_EGRESS`'s values: `public`, `any`, `off` (ASCII case
    /// ignored, surrounding space trimmed). Anything else is `None`, so the
    /// server can refuse to start on a typo rather than guess.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "public" => Some(Egress::Public),
            "any" => Some(Egress::Any),
            "off" => Some(Egress::Off),
            _ => None,
        }
    }
}

/// Why a URL or a name was refused. Its `Display` names the rule, never
/// the URL's path or query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Egress is [`Egress::Off`].
    Off,
    /// The URL doesn't parse or has no host.
    InvalidUrl,
    /// A scheme other than `https` (or `http` under [`Egress::Any`]).
    Scheme,
    /// The host is an IP literal in a blocked range.
    PrivateAddress,
    /// Every address the name resolved to is in a blocked range.
    NoPublicAddress,
    /// The host is one of the configured proxies' (rule 4): the proxy
    /// itself is no model provider, and its name resolves unfiltered.
    ProxyHost,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refusal::Off => "model calls are turned off on this server",
            Refusal::InvalidUrl => "the provider URL isn't a valid URL",
            Refusal::Scheme => "the provider URL must use https on this server",
            Refusal::PrivateAddress => "the provider URL points at a private or local address, which this server doesn't call",
            Refusal::NoPublicAddress => "the provider's name resolves only to private or local addresses, which this server doesn't call",
            Refusal::ProxyHost => "the provider URL names this server's proxy, which this server doesn't call as a provider",
        })
    }
}

impl std::error::Error for Refusal {}

/// Whether `ip` is in a range a public model call may not reach: loopback,
/// private, link-local, unique-local, carrier-grade NAT, multicast,
/// unspecified, documentation, benchmarking and reserved ranges, and an
/// IPv6 address that embeds a blocked IPv4 one (IPv4-mapped, -compatible,
/// SIIT `::ffff:0:0/96`, NAT64 `64:ff9b::/96`, 6to4 `2002::/16`). The IETF
/// assignments `2001::/23` (Teredo, benchmarking, ORCHID), local NAT64
/// (`64:ff9b:1::/48`) and documentation (`2001:db8::/32`, `3fff::/20`)
/// are blocked whole.
pub fn blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v4_blocked(v),
        IpAddr::V6(v) => v6_blocked(v),
    }
}

/// Rule 1: the URL's scheme and, for an IP literal, its address. reqwest
/// parses with the `url` crate, which turns every IPv4 spelling (decimal,
/// octal, hex, short forms) into [`url::Host::Ipv4`] for `http`/`https`,
/// so the literal is checked as the connection will use it.
pub fn check_url(url: &reqwest::Url, egress: Egress) -> Result<(), Refusal> {
    match (egress, url.scheme()) {
        (Egress::Off, _) => return Err(Refusal::Off),
        (_, "https") | (Egress::Any, "http") => {}
        _ => return Err(Refusal::Scheme),
    }
    let host = url.host().ok_or(Refusal::InvalidUrl)?;
    if egress == Egress::Public {
        let blocked = match host {
            url::Host::Ipv4(v) => v4_blocked(v),
            url::Host::Ipv6(v) => v6_blocked(v),
            url::Host::Domain(_) => false,
        };
        if blocked {
            return Err(Refusal::PrivateAddress);
        }
    }
    Ok(())
}

/// `addrs` without the blocked ones; `Err` when none is left.
pub fn filter_addrs(addrs: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, Refusal> {
    let kept: Vec<SocketAddr> = addrs.into_iter().filter(|a| !blocked_ip(a.ip())).collect();
    if kept.is_empty() {
        Err(Refusal::NoPublicAddress)
    } else {
        Ok(kept)
    }
}

type LookupFuture = Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send>>;
type Lookup = Arc<dyn Fn(String) -> LookupFuture + Send + Sync>;

/// Rule 2: a reqwest resolver that resolves through the system (or a
/// lookup a test passes) and keeps only addresses [`blocked_ip`] allows.
#[derive(Clone)]
pub struct PublicResolver {
    lookup: Lookup,
    /// Names resolved without the filter: the configured proxies' hosts,
    /// which the operator chose (a proxy on `localhost` must be reachable).
    unfiltered: Arc<HashSet<String>>,
}

impl PublicResolver {
    /// Resolves with the system's resolver (`getaddrinfo`, through tokio).
    pub fn system() -> Self {
        Self::with_lookup(|name| {
            Box::pin(
                async move { Ok(tokio::net::lookup_host((name.as_str(), 0)).await?.collect()) },
            )
        })
    }

    /// Resolves with `lookup` (tests: a name with chosen answers, no DNS).
    pub fn with_lookup(lookup: impl Fn(String) -> LookupFuture + Send + Sync + 'static) -> Self {
        Self {
            lookup: Arc::new(lookup),
            unfiltered: Arc::default(),
        }
    }

    /// The same resolver, letting `names` (the proxies' hosts, ASCII case
    /// ignored) resolve unfiltered.
    pub fn with_unfiltered(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.unfiltered = Arc::new(names.into_iter().map(|n| n.to_ascii_lowercase()).collect());
        self
    }

    /// Rule 4's local check through a proxy: `Err` when `name` resolves
    /// here and every answer is blocked. A name that doesn't resolve here
    /// (an error or no answer) passes: the proxy resolves it.
    pub async fn check_name(&self, name: &str) -> Result<(), Refusal> {
        match (self.lookup)(name.to_string()).await {
            Ok(found) if !found.is_empty() => filter_addrs(found).map(|_| ()),
            _ => Ok(()),
        }
    }
}

impl fmt::Debug for PublicResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PublicResolver")
    }
}

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let lookup = self.lookup.clone();
        let name = name.as_str().to_string();
        let unfiltered = self.unfiltered.contains(&name.to_ascii_lowercase());
        Box::pin(async move {
            let found = lookup(name).await?;
            if unfiltered {
                return Ok(addrs_of(found));
            }
            let kept = filter_addrs(found)?;
            Ok(addrs_of(kept))
        })
    }
}

fn v4_blocked(v: Ipv4Addr) -> bool {
    let [a, b, c, _] = v.octets();
    a == 0 // "this network", 0.0.0.0 included
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b)) // carrier-grade NAT
        || (a == 169 && b == 254) // link-local, cloud metadata
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2) // documentation
        || (a == 192 && b == 88 && c == 99) // 6to4 relay anycast
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || (a == 198 && b == 51 && c == 100) // documentation
        || (a == 203 && b == 0 && c == 113) // documentation
        || a >= 224 // multicast, reserved, broadcast
}

fn embedded_v4(hi: u16, lo: u16) -> Ipv4Addr {
    let [a, b] = hi.to_be_bytes();
    let [c, d] = lo.to_be_bytes();
    Ipv4Addr::new(a, b, c, d)
}

fn v6_blocked(v: Ipv6Addr) -> bool {
    if v.is_unspecified() || v.is_loopback() {
        return true;
    }
    if let Some(m) = v.to_ipv4_mapped() {
        return v4_blocked(m); // ::ffff:a.b.c.d
    }
    let s = v.segments();
    if s[..4] == [0; 4] && s[4] == 0xffff && s[5] == 0 {
        return v4_blocked(embedded_v4(s[6], s[7])); // SIIT ::ffff:0:a.b.c.d
    }
    if s[..6] == [0; 6] {
        return v4_blocked(embedded_v4(s[6], s[7])); // ::a.b.c.d (IPv4-compatible)
    }
    if s[0] == 0x64 && s[1] == 0xff9b {
        if s[2..6] == [0; 4] {
            return v4_blocked(embedded_v4(s[6], s[7])); // NAT64 64:ff9b::/96
        }
        return s[2] == 1; // local-use NAT64 64:ff9b:1::/48
    }
    if s[0] == 0x2002 {
        return v4_blocked(embedded_v4(s[1], s[2])); // 6to4
    }
    (s[0] == 0x100 && s[1..4] == [0; 3]) // discard 100::/64
        || (s[0] == 0x2001 && s[1] < 0x200) // IETF assignments 2001::/23 (Teredo, benchmarking, ORCHID)
        || (s[0] == 0x2001 && s[1] == 0xdb8) // documentation
        || (s[0] == 0x3fff && s[1] < 0x1000) // documentation 3fff::/20
        || (s[0] & 0xfe00) == 0xfc00 // unique-local fc00::/7
        || (s[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        || (s[0] & 0xffc0) == 0xfec0 // site-local fec0::/10 (deprecated)
        || (s[0] & 0xff00) == 0xff00 // multicast
}

fn addrs_of(v: Vec<SocketAddr>) -> Addrs {
    Box::new(v.into_iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> reqwest::Url {
        reqwest::Url::parse(s).unwrap()
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn parses_the_env_values() {
        assert_eq!(Egress::parse("public"), Some(Egress::Public));
        assert_eq!(Egress::parse(" ANY "), Some(Egress::Any));
        assert_eq!(Egress::parse("off"), Some(Egress::Off));
        assert_eq!(Egress::parse(""), None);
        assert_eq!(Egress::parse("private"), None);
    }

    #[test]
    fn blocked_v4_ranges() {
        for s in [
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.254",
            "127.0.0.1",
            "127.255.255.255",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(blocked_ip(ip(s)), "{s} should be blocked");
        }
        for s in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "192.169.0.1",
            "198.17.255.255",
            "198.20.0.0",
            "223.255.255.255",
        ] {
            assert!(!blocked_ip(ip(s)), "{s} should be allowed");
        }
    }

    #[test]
    fn blocked_v6_ranges() {
        for s in [
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::1",
            "100::1",
            "2001::1",
            "2001:db8::1",
            "2002:7f00:1::",
            "2002:a9fe:a9fe::1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "febf::1",
            "fec0::1",
            "ff02::1",
        ] {
            assert!(blocked_ip(ip(s)), "{s} should be blocked");
        }
        for s in [
            "2606:4700:4700::1111",
            "2a00:1450:4001::200e",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2002:808:808::1",
        ] {
            assert!(!blocked_ip(ip(s)), "{s} should be allowed");
        }
    }

    /// SIIT `::ffff:0:a.b.c.d` (embedded IPv4 checked),
    /// the IETF assignments `2001::/23` (benchmarking `2001:2::/48`,
    /// ORCHID) and the new documentation range `3fff::/20`.
    #[test]
    fn siit_benchmarking_orchid_and_new_documentation_ranges() {
        for s in [
            "::ffff:0:a00:1",
            "::ffff:0:7f00:1",
            "::ffff:0:a9fe:a9fe",
            "2001:2::1",
            "2001:2:0:ffff::1",
            "2001:10::1",
            "2001:20::1",
            "2001:1ff::1",
            "3fff::1",
            "3fff:fff:ffff::1",
        ] {
            assert!(blocked_ip(ip(s)), "{s} should be blocked");
        }
        for s in [
            "::ffff:0:808:808",
            "2001:200::1",
            "3fff:1000::1",
            "2001:4860:4860::8888",
        ] {
            assert!(!blocked_ip(ip(s)), "{s} should be allowed");
        }
    }

    /// Every spelling of a loopback or private literal that the URL
    /// parser normalises is caught on the parsed host, without DNS.
    #[test]
    fn ip_literals_in_any_spelling_are_refused_under_public() {
        for s in [
            "https://127.0.0.1/v1",
            "https://127.1/v1",
            "https://2130706433/v1",
            "https://0x7f000001/v1",
            "https://0x7f.1/v1",
            "https://0177.0.0.1/v1",
            "https://017700000001/v1",
            "https://0/v1",
            "https://0.0.0.0/v1",
            "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.1/v1",
            "https://10.1.2.3:8443/v1",
            "https://[::1]/v1",
            "https://[::ffff:127.0.0.1]/v1",
            "https://[::ffff:7f00:1]/v1",
            "https://[0:0:0:0:0:ffff:127.0.0.1]/v1",
            "https://[::]/v1",
            "https://[fe80::1]/v1",
            "https://[fd00::1]/v1",
            "https://[64:ff9b::a9fe:a9fe]/v1",
        ] {
            assert_eq!(
                check_url(&url(s), Egress::Public),
                Err(Refusal::PrivateAddress),
                "{s}"
            );
        }
    }

    /// A zone id (`%25eth0`) doesn't parse as a URL host at all, so it can't
    /// smuggle a link-local address past the check.
    #[test]
    fn an_ipv6_zone_id_does_not_parse() {
        assert!(reqwest::Url::parse("https://[fe80::1%25eth0]/v1").is_err());
        assert!(reqwest::Url::parse("https://[fe80::1%eth0]/v1").is_err());
    }

    #[test]
    fn public_literals_and_names_pass_the_url_check() {
        for s in [
            "https://api.openai.com/v1",
            "https://8.8.8.8/v1",
            "https://[2606:4700:4700::1111]/v1",
            "https://localhost/v1", // a name: the resolver decides
        ] {
            assert_eq!(check_url(&url(s), Egress::Public), Ok(()), "{s}");
        }
    }

    #[test]
    fn http_only_under_any() {
        assert_eq!(
            check_url(&url("http://api.openai.com/v1"), Egress::Public),
            Err(Refusal::Scheme)
        );
        assert_eq!(
            check_url(&url("http://127.0.0.1:11434/v1"), Egress::Any),
            Ok(())
        );
        assert_eq!(
            check_url(&url("ftp://example.com/x"), Egress::Any),
            Err(Refusal::Scheme)
        );
        assert_eq!(
            check_url(&url("file:///etc/passwd"), Egress::Any),
            Err(Refusal::Scheme)
        );
    }

    #[test]
    fn any_allows_private_literals_and_off_refuses_everything() {
        for s in [
            "https://127.0.0.1/v1",
            "https://169.254.169.254/",
            "https://[::1]/",
        ] {
            assert_eq!(check_url(&url(s), Egress::Any), Ok(()), "{s}");
        }
        assert_eq!(
            check_url(&url("https://api.anthropic.com/v1"), Egress::Off),
            Err(Refusal::Off)
        );
    }

    fn sa(s: &str) -> SocketAddr {
        SocketAddr::new(ip(s), 0)
    }

    #[test]
    fn a_name_with_public_and_private_addresses_keeps_only_the_public() {
        assert_eq!(
            filter_addrs(vec![
                sa("127.0.0.1"),
                sa("8.8.8.8"),
                sa("::1"),
                sa("2606:4700:4700::1111")
            ]),
            Ok(vec![sa("8.8.8.8"), sa("2606:4700:4700::1111")])
        );
        assert_eq!(
            filter_addrs(vec![sa("10.0.0.1"), sa("169.254.169.254")]),
            Err(Refusal::NoPublicAddress)
        );
        assert_eq!(filter_addrs(vec![]), Err(Refusal::NoPublicAddress));
    }

    fn fixed(answers: Vec<SocketAddr>) -> PublicResolver {
        PublicResolver::with_lookup(move |_| {
            let a = answers.clone();
            Box::pin(async move { Ok(a) })
        })
    }

    async fn resolve(r: &PublicResolver, name: &str) -> Result<Vec<SocketAddr>, String> {
        match r.resolve(name.parse().unwrap()).await {
            Ok(addrs) => Ok(addrs.collect()),
            Err(e) => Err(e.to_string()),
        }
    }

    #[tokio::test]
    async fn the_resolver_filters_its_answers() {
        let mixed = fixed(vec![sa("192.168.0.10"), sa("8.8.4.4")]);
        assert_eq!(resolve(&mixed, "mixed.test").await, Ok(vec![sa("8.8.4.4")]));
        let private = fixed(vec![sa("127.0.0.1"), sa("::ffff:10.0.0.1")]);
        let err = resolve(&private, "private.test").await.unwrap_err();
        assert!(err.contains("private or local"), "{err}");
    }

    #[tokio::test]
    async fn the_system_resolver_refuses_localhost() {
        let err = resolve(&PublicResolver::system(), "localhost")
            .await
            .unwrap_err();
        assert!(err.contains("private or local"), "{err}");
    }
}
