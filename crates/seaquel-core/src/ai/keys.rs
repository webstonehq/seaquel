//! A provider's API key (Decision 7): the one the call supplies (web,
//! demo), else the workspace's keychain entry `ai-api-key:<providerId>`
//! (desktop), read by Core so the key never reaches the page. It is held
//! for the call only, and never logged, put in an error or stored.

use seaquel_workspace::ai::SuppliedSecret;
use seaquel_workspace::state::AI_API_KEY_PREFIX;

use crate::{CoreError, Workspace};

/// A key, redacted in `Debug`.
#[derive(Clone)]
pub(crate) struct ApiKey(String);

impl ApiKey {
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// The key for `provider_id`: `supplied` when it isn't empty, else the
/// keychain's (`None` without a store or entry). A store that refuses the
/// read (a denied keychain prompt) is `SECRET_UNREADABLE`.
pub(crate) async fn api_key(
    ws: &Workspace,
    provider_id: &str,
    supplied: Option<SuppliedSecret>,
) -> Result<Option<ApiKey>, CoreError> {
    if let Some(key) = supplied.filter(|k| !k.expose().is_empty()) {
        return Ok(Some(ApiKey(key.expose().to_string())));
    }
    #[cfg(feature = "secrets")]
    if let Some(store) = ws.secrets() {
        let entry = format!("{AI_API_KEY_PREFIX}{provider_id}");
        return match store.get(&entry).await {
            Ok(key) => Ok(key.filter(|k| !k.is_empty()).map(ApiKey)),
            Err(e) => {
                log::warn!(activity = "ai.key", code = e.code(); "Reading an AI key failed");
                Err(CoreError::new(
                    crate::SECRET_UNREADABLE,
                    format!(
                        "Seaquel couldn't read the provider's API key from the keychain ({}). \
                         Allow Seaquel to access the keychain when the system asks.",
                        e.code()
                    ),
                ))
            }
        };
    }
    let _ = (ws, provider_id, AI_API_KEY_PREFIX);
    Ok(None)
}
