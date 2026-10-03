//! Seaquel's outbound HTTP, native only.
//!
//! - [`client`]: building a reqwest client without ever panicking (the OS
//!   and webpki roots, the webpki-only fallback, extra roots from
//!   `NODE_EXTRA_CA_CERTS`, proxies from the environment). Moved here from
//!   `seaquel-license` in phase 6; the license clients use it unchanged.
//! - [`NativeHttp`]: `seaquel_ai::http::HttpClient` over that client, with
//!   the model calls' timeouts, no redirects and the egress rules.
//! - [`egress`]: the web server's guard against model calls to private
//!   addresses (phase 6 Decision 9).

pub mod client;
pub mod egress;
mod native;

pub use egress::Egress;
pub use native::{NativeHttp, NativeHttpOptions};
