//! Seaquel's secret storage: the `SecretStore` trait that Core's workspace
//! holds, and a store backed by the OS keychain (macOS Keychain, Windows
//! Credential Manager, the Secret Service on Linux). It keeps the service name
//! and entry layout `tauri-plugin-keyring` used, so passwords saved by earlier
//! releases read back unchanged.
//!
//! Every store checks keys with [`validate_key`] before touching storage, so
//! no interface can read or write an entry outside Seaquel's own key names.
//! Errors name the key and never the value, and no `Debug` output in this
//! crate includes a secret value.

mod keychain;
mod memory;
mod secret_wait;

pub use keychain::{KeychainStore, DESKTOP_SERVICE};
pub use memory::MemoryStore;
pub use secret_wait::{SecretWait, DEFAULT_SECRET_WAIT_LIMIT};

use seaquel_runtime::{MaybeSend, MaybeSync};

/// A place to keep secrets by key: connection passwords, SSH passwords and
/// key passphrases, the license key and AI API keys.
///
/// Keys must pass [`validate_key`]; every method refuses anything else with
/// [`SecretError::InvalidKey`] before touching storage.
#[seaquel_runtime::async_trait]
pub trait SecretStore: MaybeSend + MaybeSync {
    /// The secret stored under `key`, or `None` when there is none. Any other
    /// failure (a locked or unreachable keychain, say) is an error, not `None`.
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError>;

    /// Store `value` under `key`, replacing any existing value.
    async fn set(&self, key: &str, value: &str) -> Result<(), SecretError>;

    /// Remove the secret under `key`. Removing a key that has no secret is not
    /// an error.
    async fn delete(&self, key: &str) -> Result<(), SecretError>;
}

/// Which store operation failed, for error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretOp {
    Get,
    Set,
    Delete,
}

impl std::fmt::Display for SecretOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SecretOp::Get => "reading",
            SecretOp::Set => "saving",
            SecretOp::Delete => "deleting",
        })
    }
}

/// A secret store failure. It names the key, never the value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// The key isn't one of Seaquel's key names (see [`validate_key`]).
    /// `Display` echoes at most [`MAX_KEY_ECHO`] characters of the key.
    #[error("invalid secret key {:?}: {reason}", short_key(key))]
    InvalidKey { key: String, reason: &'static str },
    /// The store itself failed: the keychain is locked, unreachable, or
    /// returned something unexpected.
    #[error("{op} secret {key:?} failed: {message}")]
    Store {
        op: SecretOp,
        key: String,
        message: String,
    },
    /// There is no store to ask: no D-Bus session or Secret Service
    /// provider (a headless Linux host), no keychain the session can use
    /// (macOS over SSH). Unlike [`SecretError::Store`], nothing refused, so
    /// a connect can ask for the secret instead (phase 7a probe F4).
    #[error("{op} secret {key:?} failed: {message}")]
    Unavailable {
        op: SecretOp,
        key: String,
        message: String,
    },
}

impl SecretError {
    /// The wire error code: `INVALID_ARGUMENT` for a bad key,
    /// `SECRET_STORE_ERROR` for anything the store reports.
    pub fn code(&self) -> &'static str {
        match self {
            SecretError::InvalidKey { .. } => "INVALID_ARGUMENT",
            SecretError::Store { .. } | SecretError::Unavailable { .. } => "SECRET_STORE_ERROR",
        }
    }

    /// Whether the store isn't there at all (no Secret Service on a
    /// headless Linux host, no login keychain over SSH on macOS), as
    /// opposed to a store that refused. The wire code is the same.
    pub fn unavailable(&self) -> bool {
        matches!(self, SecretError::Unavailable { .. })
    }

    /// The key the failed call was for.
    pub fn key(&self) -> &str {
        match self {
            SecretError::InvalidKey { key, .. }
            | SecretError::Store { key, .. }
            | SecretError::Unavailable { key, .. } => key,
        }
    }
}

/// How many characters of a refused key an error message echoes. A refused
/// key can be anything a caller sent, so it isn't repeated in full.
pub const MAX_KEY_ECHO: usize = 64;

/// `key` cut to [`MAX_KEY_ECHO`] characters, with `…` when it was longer.
fn short_key(key: &str) -> String {
    match key.char_indices().nth(MAX_KEY_ECHO) {
        Some((cut, _)) => format!("{}…", &key[..cut]),
        None => key.to_string(),
    }
}

/// Key prefixes that take an id: `db:<id>`, `ssh:<id>`, `ssh-key:<id>` and
/// `ai-api-key:<id>`.
const ID_PREFIXES: [&str; 4] = ["db:", "ssh:", "ssh-key:", "ai-api-key:"];

/// Keys without an id.
const FIXED_KEYS: [&str; 1] = ["license-key"];

/// The longest id accepted, in bytes. Ids are usually `conn-<uuid>` or a UUID,
/// but imported SQLite connections use `conn-sqlite-<path>`, so this leaves
/// room for a Linux `PATH_MAX`.
pub const MAX_ID_LEN: usize = 4096;

/// Check that `key` is one of Seaquel's secret key names:
/// `db:<id>`, `ssh:<id>`, `ssh-key:<id>`, `license-key` or `ai-api-key:<id>`.
///
/// An `<id>` must be non-empty, at most [`MAX_ID_LEN`] bytes and free of
/// control characters. It is deliberately not limited to UUID characters:
/// today's ids include `conn-<uuid>`, provider UUIDs, and ids the DBeaver and
/// TablePlus imports build from a host and port (`conn-::1-5432`) or a SQLite
/// file path (`conn-sqlite-/Users/me/My DB.sqlite`). Existing keychain entries
/// use those ids, so a narrower charset would orphan them.
pub fn validate_key(key: &str) -> Result<(), SecretError> {
    let invalid = |reason| {
        Err(SecretError::InvalidKey {
            key: key.to_string(),
            reason,
        })
    };
    if FIXED_KEYS.contains(&key) {
        return Ok(());
    }
    let Some(id) = ID_PREFIXES.iter().find_map(|p| key.strip_prefix(p)) else {
        return invalid("expected db:<id>, ssh:<id>, ssh-key:<id>, license-key or ai-api-key:<id>");
    };
    if id.is_empty() {
        return invalid("the id is empty");
    }
    if id.len() > MAX_ID_LEN {
        return invalid("the id is too long");
    }
    if id.chars().any(char::is_control) {
        return invalid("the id contains a control character");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_every_key_form() {
        for key in [
            "db:conn-6f1c1d9e-5b0a-4f7e-9d38-0a8f2b7c1e44",
            "ssh:conn-6f1c1d9e-5b0a-4f7e-9d38-0a8f2b7c1e44",
            "ssh-key:conn-6f1c1d9e-5b0a-4f7e-9d38-0a8f2b7c1e44",
            "license-key",
            "ai-api-key:6f1c1d9e-5b0a-4f7e-9d38-0a8f2b7c1e44",
            "ai-api-key:anthropic",
            "db:demo-connection",
            // Ids the DBeaver/TablePlus imports build.
            "db:conn-localhost-5432",
            "db:conn-::1-5432",
            "db:conn-sqlite-/Users/me/My DB.sqlite",
            "db:conn-sqlite-C:\\data\\app.db",
            "db:x",
            "ssh:ünïcødé",
        ] {
            assert_eq!(validate_key(key), Ok(()), "{key}");
        }
    }

    #[test]
    fn refuses_unknown_forms() {
        for key in [
            "",
            "db",
            "license",
            "license-key:x",
            " license-key",
            "license-key ",
            // The legacy single AI key; its methods are being removed.
            "ai-api-key",
            "DB:x",
            "db;x",
            "postgres:x",
            "ssh-keys:x",
            ":x",
        ] {
            let err = validate_key(key).unwrap_err();
            assert_eq!(err.code(), "INVALID_ARGUMENT", "{key}");
            assert_eq!(err.key(), key);
        }
    }

    #[test]
    fn refuses_empty_ids() {
        for key in ["db:", "ssh:", "ssh-key:", "ai-api-key:"] {
            let err = validate_key(key).unwrap_err();
            assert!(err.to_string().contains("the id is empty"), "{err}");
        }
    }

    #[test]
    fn refuses_control_characters() {
        for key in [
            "db:a\0b",
            "db:a\nb",
            "ssh:\t",
            "ai-api-key:x\u{7f}",
            "db:\u{85}",
        ] {
            let err = validate_key(key).unwrap_err();
            assert!(err.to_string().contains("control character"), "{err}");
        }
    }

    #[test]
    fn caps_id_length() {
        let longest = format!("db:{}", "a".repeat(MAX_ID_LEN));
        assert_eq!(validate_key(&longest), Ok(()));
        let too_long = format!("db:{}", "a".repeat(MAX_ID_LEN + 1));
        assert!(validate_key(&too_long)
            .unwrap_err()
            .to_string()
            .contains("too long"));
    }

    #[test]
    fn invalid_key_message_truncates_long_keys() {
        let at_limit = format!("x{}", "é".repeat(MAX_KEY_ECHO - 1));
        let err = validate_key(&at_limit).unwrap_err();
        assert!(err.to_string().contains(&format!("{at_limit:?}")), "{err}");

        let long = format!("nope{}", "é".repeat(200));
        let err = validate_key(&long).unwrap_err();
        let shown: String = long.chars().take(MAX_KEY_ECHO).collect();
        assert!(err.to_string().contains(&format!("\"{shown}…\"")), "{err}");
        assert!(!err.to_string().contains(&long));
        // The error itself keeps the whole key.
        assert_eq!(err.key(), long);
    }

    #[test]
    fn error_message_escapes_the_key() {
        let err = validate_key("db:a\nb").unwrap_err();
        assert!(err.to_string().contains(r#""db:a\nb""#), "{err}");
    }
}
