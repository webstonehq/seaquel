//! The OS keychain store.

use keyring::Entry;

use crate::{validate_key, SecretError, SecretOp, SecretStore};

/// The service every desktop build uses, dev builds included. It is what
/// `keyring.ts` hard-coded; changing it would orphan every saved password.
pub const DESKTOP_SERVICE: &str = "app.seaquel.desktop";

/// Secrets in the OS keychain, one entry per key: `Entry::new(service, key)`,
/// the layout `tauri-plugin-keyring` used.
///
/// Each call runs on tokio's blocking pool, since a keychain prompt can
/// block. Called outside a tokio runtime, it fails with a `SecretError`
/// instead of panicking. Dropping a call's future doesn't stop the keychain
/// call: a `set` or `delete` already handed to the blocking pool still
/// completes, and its result is discarded.
#[derive(Debug, Clone)]
pub struct KeychainStore {
    service: String,
}

impl KeychainStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    /// Validate `key`, then run `f` on the entry on the blocking pool.
    async fn with_entry<T, F>(&self, op: SecretOp, key: &str, f: F) -> Result<T, SecretError>
    where
        T: Send + 'static,
        F: FnOnce(Entry) -> Result<T, keyring::Error> + Send + 'static,
    {
        validate_key(key)?;
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| SecretError::Store {
            op,
            key: key.to_string(),
            message: "the keychain store needs a tokio runtime".to_string(),
        })?;
        let service = self.service.clone();
        let user = key.to_string();
        let joined = runtime
            .spawn_blocking(move || f(Entry::new(&service, &user)?))
            .await;
        match joined {
            Ok(result) => result.map_err(|err| store_error(op, key, &err)),
            Err(join) => Err(SecretError::Store {
                op,
                key: key.to_string(),
                message: if join.is_panic() {
                    "the keychain call panicked".to_string()
                } else {
                    "the keychain call was cancelled".to_string()
                },
            }),
        }
    }
}

#[seaquel_runtime::async_trait]
impl SecretStore for KeychainStore {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        self.with_entry(SecretOp::Get, key, |entry| match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(err),
        })
        .await
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        let value = value.to_string();
        self.with_entry(SecretOp::Set, key, move |entry| entry.set_password(&value))
            .await
    }

    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.with_entry(SecretOp::Delete, key, |entry| {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(err),
            }
        })
        .await
    }
}

/// Turn a keyring error into a `SecretError` without keeping the original.
/// `keyring::Error::BadEncoding` holds the stored bytes and its `Debug` prints
/// them, so the message is written here from the variant, never from `Debug`.
fn store_error(op: SecretOp, key: &str, err: &keyring::Error) -> SecretError {
    use keyring::Error as E;
    let message = match err {
        E::PlatformFailure(inner) => format!("keychain failure: {inner}"),
        E::NoStorageAccess(inner) => format!("couldn't access the keychain: {inner}"),
        E::NoEntry => "no entry in the keychain".to_string(),
        E::BadEncoding(_) => "the stored value is not UTF-8".to_string(),
        E::TooLong(attr, limit) => {
            format!("the {attr} is longer than the keychain's limit of {limit}")
        }
        E::Invalid(attr, reason) => format!("the {attr} is invalid: {reason}"),
        E::Ambiguous(items) => format!("the key matches {} keychain entries", items.len()),
        _ => "unexpected keychain error".to_string(),
    };
    SecretError::Store {
        op,
        key: key.to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_name_the_key_and_drop_stored_bytes() {
        let err = store_error(
            SecretOp::Get,
            "db:conn-1",
            &keyring::Error::BadEncoding(b"hunter2\xff".to_vec()),
        );
        let (display, debug) = (err.to_string(), format!("{err:?}"));
        assert!(display.contains("\"db:conn-1\""), "{display}");
        assert!(display.contains("reading"), "{display}");
        for text in [&display, &debug] {
            assert!(!text.contains("hunter2"), "{text}");
            assert!(!text.contains("104, 117"), "{text}"); // "hu" as bytes
        }
        assert_eq!(err.code(), "SECRET_STORE_ERROR");
    }

    #[test]
    fn debug_shows_only_the_service() {
        let store = KeychainStore::new("app.seaquel.test.x");
        assert_eq!(
            format!("{store:?}"),
            r#"KeychainStore { service: "app.seaquel.test.x" }"#
        );
    }

    #[test]
    fn fails_without_a_runtime_instead_of_panicking() {
        // A valid key, so the call gets as far as the runtime check; with no
        // runtime it never reaches the keychain.
        let store = KeychainStore::new("app.seaquel.test.never-used");
        let err = poll_once(store.get("db:conn-1")).unwrap_err();
        assert_eq!(err.code(), "SECRET_STORE_ERROR");
        assert!(err.to_string().contains("tokio runtime"), "{err}");
    }

    /// Poll a future on this thread without a tokio runtime. The future under
    /// test resolves on its first poll.
    fn poll_once<F: std::future::Future>(fut: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut fut = std::pin::pin!(fut);
        match fut.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(out) => out,
            Poll::Pending => panic!("expected the call to fail immediately"),
        }
    }

    #[tokio::test]
    async fn refuses_invalid_keys_without_touching_the_keychain() {
        // A bad key must fail before spawn_blocking; this never reaches the OS.
        let store = KeychainStore::new("app.seaquel.test.never-used");
        for err in [
            store.get("nope").await.unwrap_err(),
            store.set("db:", "v").await.unwrap_err(),
            store.delete("ai-api-key").await.unwrap_err(),
        ] {
            assert_eq!(err.code(), "INVALID_ARGUMENT");
        }
    }
}
