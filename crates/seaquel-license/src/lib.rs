//! Seaquel's licensing, in two parts that share little. The desktop part is
//! the activation client (activate, validate and deactivate a key). The server
//! part is the self-hosted web build's license gate: the install id, the
//! control-plane client with its soft and hard TTLs, member licenses and
//! air-gap bundles verified with Ed25519. Core exposes each behind its own
//! feature.

#[cfg(any(feature = "desktop", feature = "server"))]
mod http;

#[cfg(feature = "desktop")]
pub mod desktop;

#[cfg(feature = "server")]
pub mod server;
