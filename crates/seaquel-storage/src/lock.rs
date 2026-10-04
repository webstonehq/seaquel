//! The async mutex behind the write turn and the in-memory pool (phase 8
//! Decision 5): tokio's natively, as before, and `futures::lock`'s on
//! wasm32, so the browser module links no tokio. Both are fair enough for
//! one process's writers; the helpers paper over their `try_lock` shapes.

use std::sync::Arc;

#[cfg(all(not(target_arch = "wasm32"), test))]
use tokio::sync::MutexGuard;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use tokio::sync::{Mutex, OwnedMutexGuard};

#[cfg(target_arch = "wasm32")]
pub(crate) use futures::lock::{Mutex, MutexGuard, OwnedMutexGuard};

/// The owned guard if no one holds the mutex, else `None`
/// (`Storage::external_version` natively, the in-memory pool on wasm32).
pub(crate) fn try_lock_owned<T>(m: &Arc<Mutex<T>>) -> Option<OwnedMutexGuard<T>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Arc::clone(m).try_lock_owned().ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        m.try_lock_owned()
    }
}

/// The guard if no one holds the mutex, else `None`.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn try_lock<T>(m: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        m.try_lock().ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        m.try_lock()
    }
}
