//! An in-memory store: the test double Core's tests use.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::{validate_key, SecretError, SecretStore};

/// Secrets in a map, gone when the store drops. `Debug` lists the keys and
/// never the values.
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        // A panic while holding the lock can't leave the map half-updated.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut keys: Vec<String> = self.entries().keys().cloned().collect();
        keys.sort();
        f.debug_struct("MemoryStore").field("keys", &keys).finish()
    }
}

#[seaquel_runtime::async_trait]
impl SecretStore for MemoryStore {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        validate_key(key)?;
        Ok(self.entries().get(key).cloned())
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        validate_key(key)?;
        self.entries().insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        validate_key(key)?;
        self.entries().remove(key);
        Ok(())
    }
}
