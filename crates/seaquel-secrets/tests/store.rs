//! One suite, run against every `SecretStore`.
//!
//! `MemoryStore` always runs. `KeychainStore` uses this machine's real
//! keychain, which CI runners don't have, so its tests are ignored and also
//! need `SEAQUEL_TEST_KEYCHAIN=1`:
//!
//! ```sh
//! SEAQUEL_TEST_KEYCHAIN=1 cargo test -p seaquel-secrets --test store -- --ignored
//! ```
//!
//! They use a random `app.seaquel.test.<uuid>` service and random ids, and a
//! drop guard deletes every entry they may have written, even when an
//! assertion fails.

use std::sync::Mutex;

use seaquel_secrets::{KeychainStore, MemoryStore, SecretStore};

/// Hands out random keys, so a keychain run never collides with a real
/// entry, and remembers them. For a keychain run it is also the drop guard:
/// dropping it deletes every key it handed out from `service`.
struct Keys {
    service: Option<String>,
    issued: Mutex<Vec<String>>,
}

impl Keys {
    fn memory() -> Self {
        Keys {
            service: None,
            issued: Mutex::new(Vec::new()),
        }
    }

    fn keychain(service: &str) -> Self {
        Keys {
            service: Some(service.to_string()),
            issued: Mutex::new(Vec::new()),
        }
    }

    /// A fresh `<prefix><random id>` key, or `prefix` itself when it has no
    /// `:` (`license-key`).
    fn key(&self, prefix: &str) -> String {
        let key = if prefix.ends_with(':') {
            format!("{prefix}conn-{}", uuid::Uuid::new_v4())
        } else {
            prefix.to_string()
        };
        self.issued.lock().unwrap().push(key.clone());
        key
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        let Some(service) = &self.service else { return };
        let issued = self.issued.get_mut().unwrap_or_else(|e| e.into_inner());
        for key in issued.iter() {
            if let Ok(entry) = keyring::Entry::new(service, key) {
                let _ = entry.delete_credential();
            }
        }
    }
}

async fn get_missing(store: &dyn SecretStore, keys: &Keys) {
    for prefix in ["db:", "ssh:", "ssh-key:", "ai-api-key:"] {
        let key = keys.key(prefix);
        assert_eq!(store.get(&key).await.unwrap(), None, "{key}");
    }
}

async fn set_get_overwrite_delete(store: &dyn SecretStore, keys: &Keys) {
    let key = keys.key("db:");
    store.set(&key, "first").await.unwrap();
    assert_eq!(store.get(&key).await.unwrap().as_deref(), Some("first"));

    store.set(&key, "second").await.unwrap();
    assert_eq!(store.get(&key).await.unwrap().as_deref(), Some("second"));

    store.delete(&key).await.unwrap();
    assert_eq!(store.get(&key).await.unwrap(), None);
}

async fn keys_are_independent(store: &dyn SecretStore, keys: &Keys) {
    let db = keys.key("db:");
    let id = db.strip_prefix("db:").unwrap();
    let (ssh, ssh_key) = (
        keys.key(&format!("ssh:{id}")),
        keys.key(&format!("ssh-key:{id}")),
    );
    store.set(&db, "db pw").await.unwrap();
    store.set(&ssh, "ssh pw").await.unwrap();
    store.set(&ssh_key, "passphrase").await.unwrap();

    store.delete(&ssh).await.unwrap();
    assert_eq!(store.get(&db).await.unwrap().as_deref(), Some("db pw"));
    assert_eq!(store.get(&ssh).await.unwrap(), None);
    assert_eq!(
        store.get(&ssh_key).await.unwrap().as_deref(),
        Some("passphrase")
    );

    store.delete(&db).await.unwrap();
    store.delete(&ssh_key).await.unwrap();
}

async fn delete_missing_is_ok(store: &dyn SecretStore, keys: &Keys) {
    let key = keys.key("ssh-key:");
    store.delete(&key).await.unwrap();
    store.delete(&key).await.unwrap();
}

async fn utf8_values(store: &dyn SecretStore, keys: &Keys) {
    let key = keys.key("ai-api-key:");
    let value = "pässwörd ✓ 密码 🔑 \"quoted\" 'x' \\ ; = \t";
    store.set(&key, value).await.unwrap();
    assert_eq!(store.get(&key).await.unwrap().as_deref(), Some(value));
    store.delete(&key).await.unwrap();
}

async fn empty_value(store: &dyn SecretStore, keys: &Keys) {
    let key = keys.key("db:");
    store.set(&key, "").await.unwrap();
    assert_eq!(store.get(&key).await.unwrap().as_deref(), Some(""));
    store.delete(&key).await.unwrap();
    assert_eq!(store.get(&key).await.unwrap(), None);
}

async fn invalid_keys_are_refused(store: &dyn SecretStore) {
    for key in ["", "db:", "ai-api-key", "password", "db:a\nb"] {
        assert_eq!(
            store.get(key).await.unwrap_err().code(),
            "INVALID_ARGUMENT",
            "{key:?}"
        );
        assert_eq!(
            store.set(key, "v").await.unwrap_err().code(),
            "INVALID_ARGUMENT"
        );
        assert_eq!(
            store.delete(key).await.unwrap_err().code(),
            "INVALID_ARGUMENT"
        );
    }
}

async fn errors_never_include_the_value(store: &dyn SecretStore) {
    let err = store.set("db:", "hunter2-secret").await.unwrap_err();
    for text in [err.to_string(), format!("{err:?}")] {
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("\"db:\""), "{text}");
    }
}

async fn run_suite(store: &dyn SecretStore, keys: &Keys) {
    get_missing(store, keys).await;
    set_get_overwrite_delete(store, keys).await;
    keys_are_independent(store, keys).await;
    delete_missing_is_ok(store, keys).await;
    utf8_values(store, keys).await;
    empty_value(store, keys).await;
    invalid_keys_are_refused(store).await;
    errors_never_include_the_value(store).await;
}

#[tokio::test]
async fn memory_store() {
    run_suite(&MemoryStore::new(), &Keys::memory()).await;
}

#[tokio::test]
async fn memory_store_debug_hides_values() {
    let store = MemoryStore::new();
    store.set("db:conn-1", "hunter2-db").await.unwrap();
    store.set("license-key", "hunter2-license").await.unwrap();
    let debug = format!("{store:?}");
    assert!(!debug.contains("hunter2"), "{debug}");
    assert_eq!(
        debug,
        r#"MemoryStore { keys: ["db:conn-1", "license-key"] }"#
    );
}

#[tokio::test]
async fn memory_stores_are_separate() {
    let (a, b) = (MemoryStore::new(), MemoryStore::new());
    a.set("license-key", "a").await.unwrap();
    assert_eq!(b.get("license-key").await.unwrap(), None);
}

/// `--ignored` alone isn't enough: the keychain tests also need the env var,
/// so running every ignored test doesn't touch the keychain by accident.
fn require_keychain() {
    assert_eq!(
        std::env::var("SEAQUEL_TEST_KEYCHAIN").as_deref(),
        Ok("1"),
        "set SEAQUEL_TEST_KEYCHAIN=1 to run the keychain tests; they use the OS keychain"
    );
}

fn test_service() -> String {
    format!("app.seaquel.test.{}", uuid::Uuid::new_v4())
}

#[tokio::test]
#[ignore = "uses the OS keychain; run with SEAQUEL_TEST_KEYCHAIN=1 and --ignored"]
async fn keychain_store() {
    require_keychain();
    let service = test_service();
    let keys = Keys::keychain(&service);
    run_suite(&KeychainStore::new(service), &keys).await;
}

/// Entries keep `tauri-plugin-keyring`'s layout, `Entry::new(service, key)`,
/// so passwords saved by earlier releases read back, and the other way round.
#[tokio::test]
#[ignore = "uses the OS keychain; run with SEAQUEL_TEST_KEYCHAIN=1 and --ignored"]
async fn keychain_store_matches_the_plugin_layout() {
    require_keychain();
    let service = test_service();
    let keys = Keys::keychain(&service);
    let store = KeychainStore::new(service.clone());

    // Written as the plugin did, read through the store.
    let old = keys.key("db:");
    let entry = keyring::Entry::new(&service, &old).unwrap();
    entry.set_password("saved by 2026.9").unwrap();
    assert_eq!(
        store.get(&old).await.unwrap().as_deref(),
        Some("saved by 2026.9")
    );
    store.delete(&old).await.unwrap();
    assert!(matches!(entry.get_password(), Err(keyring::Error::NoEntry)));

    // Written through the store, read as the plugin did.
    let new = keys.key("license-key");
    store.set(&new, "lic").await.unwrap();
    let raw = keyring::Entry::new(&service, &new).unwrap().get_password();
    assert_eq!(raw.unwrap(), "lic");
    store.delete(&new).await.unwrap();
}
