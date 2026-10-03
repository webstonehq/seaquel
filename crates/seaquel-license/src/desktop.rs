//! The desktop activation client: `activate`, `validate` and `deactivate` a
//! key against the license server's `/api/licenses/*`.
//!
//! The base URL is passed in; the desktop app picks it at compile time. The
//! license state and its 12-hour revalidation stay in the TypeScript store
//! (the desktop license is honour-system; see the source/binary license
//! split doc).
//!
//! Errors carry today's codes: `NETWORK_ERROR` when the server can't be
//! reached, `ACTIVATION_ERROR`, `VALIDATION_ERROR` or `DEACTIVATION_ERROR`
//! for a non-2xx answer (with its status and body), and `PARSE_ERROR` for a
//! 2xx body that isn't a license. Keys never appear in a log line, an error
//! or `Debug`.

use std::fmt;

use log::{error, info, warn};
pub use seaquel_types::license::LicenseResponse;
use serde_json::json;

/// A failed license call. `message` is shown to the user as is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct LicenseError {
    pub code: String,
    pub message: String,
}

impl LicenseError {
    fn new(code: &str, message: String) -> Self {
        Self {
            code: code.to_string(),
            message,
        }
    }
}

/// The activation client. Cheap to clone; keep one.
#[derive(Clone)]
pub struct DesktopClient {
    base_url: String,
    http: seaquel_http::client::LazyClient,
}

impl fmt::Debug for DesktopClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DesktopClient")
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// One of the three calls: its path, its wording and its error code.
#[derive(Clone, Copy)]
enum Op {
    Activate,
    Validate,
    Deactivate,
}

impl Op {
    fn path(self) -> &'static str {
        match self {
            Op::Activate => "activate",
            Op::Validate => "validate",
            Op::Deactivate => "deactivate",
        }
    }

    fn activity(self) -> &'static str {
        match self {
            Op::Activate => "license.activate",
            Op::Validate => "license.validate",
            Op::Deactivate => "license.deactivate",
        }
    }

    /// "activation", as in "License activation failed".
    fn noun(self) -> &'static str {
        match self {
            Op::Activate => "activation",
            Op::Validate => "validation",
            Op::Deactivate => "deactivation",
        }
    }

    fn error_code(self) -> &'static str {
        match self {
            Op::Activate => "ACTIVATION_ERROR",
            Op::Validate => "VALIDATION_ERROR",
            Op::Deactivate => "DEACTIVATION_ERROR",
        }
    }
}

impl DesktopClient {
    /// A client for the license server at `base_url` (`https://seaquel.app`).
    /// A trailing `/` is dropped.
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut base_url = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        Self {
            base_url,
            // Never panics: see `seaquel_http::client`.
            http: seaquel_http::client::LazyClient::new(
                "license client",
                seaquel_http::client::ClientOptions::default(),
            ),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Activate `key` for this machine, named `instance_name`.
    pub async fn activate(
        &self,
        key: &str,
        instance_name: &str,
    ) -> Result<LicenseResponse, LicenseError> {
        self.call(
            Op::Activate,
            json!({ "key": key, "instance_name": instance_name }),
        )
        .await
    }

    /// Check that `key` is still valid for the instance `instance_id`.
    pub async fn validate(
        &self,
        key: &str,
        instance_id: &str,
    ) -> Result<LicenseResponse, LicenseError> {
        self.call(
            Op::Validate,
            json!({ "key": key, "instance_id": instance_id }),
        )
        .await
    }

    /// Release the instance `instance_id` of `key`.
    pub async fn deactivate(
        &self,
        key: &str,
        instance_id: &str,
    ) -> Result<LicenseResponse, LicenseError> {
        self.call(
            Op::Deactivate,
            json!({ "key": key, "instance_id": instance_id }),
        )
        .await
    }

    /// POST `body` as JSON to `/api/licenses/<op>` and parse the license.
    async fn call(&self, op: Op, body: serde_json::Value) -> Result<LicenseResponse, LicenseError> {
        let activity = op.activity();
        info!(activity = activity; "License {} requested", op.noun());
        let url = format!("{}/api/licenses/{}", self.base_url, op.path());

        let http = self.http.get().map_err(|e| {
            error!(activity = activity, error_code = "NETWORK_ERROR"; "License HTTP client unavailable");
            LicenseError::new(
                "NETWORK_ERROR",
                format!("Failed to connect to license server: {e}"),
            )
        })?;
        let response = http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                error!(activity = activity, error_code = "NETWORK_ERROR"; "Failed to connect to license server");
                // reqwest's message names the URL, never the body.
                LicenseError::new(
                    "NETWORK_ERROR",
                    format!("Failed to connect to license server: {e}"),
                )
            })?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            warn!(activity = activity, status = status.as_u16(), error_code = op.error_code(); "License API returned error");
            return Err(LicenseError::new(
                op.error_code(),
                format!("License {} failed ({status}): {text}", op.noun()),
            ));
        }

        let license = response.json::<LicenseResponse>().await.map_err(|e| {
            error!(activity = activity, error_code = "PARSE_ERROR"; "Invalid response from license server");
            LicenseError::new(
                "PARSE_ERROR",
                format!("Invalid response from license server: {e}"),
            )
        })?;
        info!(activity = activity; "License {} successful", op.noun());
        Ok(license)
    }
}
