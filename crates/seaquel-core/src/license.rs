//! Licensing, for interfaces and `seaquel-rpc`, which may not depend on
//! `seaquel-license` directly.
//!
//! - `license-desktop`: [`desktop`], the activation client the Tauri app
//!   builds with its compile-time base URL.
//! - `license-server`: [`server`], the web build's license gate over
//!   `auth.db`, which `seaquel-server` serves as `/internal/license/*`.

#[cfg(feature = "license-desktop")]
pub use seaquel_license::desktop;

#[cfg(feature = "license-server")]
pub use seaquel_license::server;

#[cfg(feature = "license-desktop")]
impl From<seaquel_license::desktop::LicenseError> for crate::CoreError {
    fn from(e: seaquel_license::desktop::LicenseError) -> Self {
        Self::new(e.code, e.message)
    }
}

#[cfg(feature = "license-server")]
impl From<seaquel_license::server::ServerError> for crate::CoreError {
    fn from(e: seaquel_license::server::ServerError) -> Self {
        Self::new(e.code.as_str(), e.message)
    }
}
