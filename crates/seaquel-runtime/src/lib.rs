//! Platform seams for Seaquel Core.
//!
//! Core has to build for native targets and for `wasm32-unknown-unknown` (the
//! browser demo). The differences live here, so the rest of Core never writes
//! `cfg(target_arch = "wasm32")`:
//!
//! - [`MaybeSend`] / [`MaybeSync`]: `Send`/`Sync` on native, no bound on wasm32.
//! - [`BoxStream`] / [`BoxFuture`]: `Send` boxes on native, local boxes on wasm32.
//! - [`async_trait`]: `async_trait` on native, `async_trait(?Send)` on wasm32.
//! - [`Executor`]: spawning, sleeping and wall-clock time. Core must not call
//!   `tokio::spawn`, `std::time::Instant` or `SystemTime` directly; clippy
//!   enforces that through `crates/clippy.toml`.

// Lets `#[seaquel_runtime::async_trait]` expand to `::seaquel_runtime::…` paths
// inside this crate too.
extern crate self as seaquel_runtime;

use std::time::Duration;

pub use seaquel_macros::async_trait;

#[doc(hidden)]
pub mod __private {
    pub use async_trait;
}

#[cfg(not(target_arch = "wasm32"))]
mod bounds {
    pub trait MaybeSend: Send {}
    impl<T: Send + ?Sized> MaybeSend for T {}

    pub trait MaybeSync: Sync {}
    impl<T: Sync + ?Sized> MaybeSync for T {}

    pub type BoxStream<'a, T> = futures::stream::BoxStream<'a, T>;
    pub type BoxFuture<'a, T> = futures::future::BoxFuture<'a, T>;
}

#[cfg(target_arch = "wasm32")]
mod bounds {
    pub trait MaybeSend {}
    impl<T: ?Sized> MaybeSend for T {}

    pub trait MaybeSync {}
    impl<T: ?Sized> MaybeSync for T {}

    pub type BoxStream<'a, T> = futures::stream::LocalBoxStream<'a, T>;
    pub type BoxFuture<'a, T> = futures::future::LocalBoxFuture<'a, T>;
}

pub use bounds::{BoxFuture, BoxStream, MaybeSend, MaybeSync};

/// Everything Core needs from the async runtime. Interfaces pass one in:
/// [`TokioExecutor`] on native, [`WasmExecutor`] in the browser.
pub trait Executor: MaybeSend + MaybeSync {
    /// Run `future` in the background. It must not outlive the process.
    fn spawn(&self, future: BoxFuture<'static, ()>);

    /// Resolve after `duration`.
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;

    /// Wall-clock time since the Unix epoch.
    fn unix_time(&self) -> Duration;
}

#[cfg(feature = "tokio")]
mod tokio_executor {
    use super::{BoxFuture, Duration, Executor};

    /// Native executor. Must be used from inside a tokio runtime.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct TokioExecutor;

    impl Executor for TokioExecutor {
        // The one sanctioned `tokio::spawn` in Core's crates.
        #[allow(clippy::disallowed_methods)]
        fn spawn(&self, future: BoxFuture<'static, ()>) {
            tokio::spawn(future);
        }

        fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
            Box::pin(tokio::time::sleep(duration))
        }

        // The one sanctioned `SystemTime` in Core's crates.
        #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
        fn unix_time(&self) -> Duration {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
        }
    }
}

#[cfg(feature = "tokio")]
pub use tokio_executor::TokioExecutor;

#[cfg(target_arch = "wasm32")]
mod wasm_executor {
    use super::{BoxFuture, Duration, Executor};

    /// Browser executor: the page's microtask queue and `setTimeout`.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct WasmExecutor;

    impl Executor for WasmExecutor {
        fn spawn(&self, future: BoxFuture<'static, ()>) {
            wasm_bindgen_futures::spawn_local(future);
        }

        fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
            Box::pin(gloo_timers::future::sleep(duration))
        }

        fn unix_time(&self) -> Duration {
            Duration::from_secs_f64(js_sys::Date::now() / 1000.0)
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_executor::WasmExecutor;

#[cfg(all(test, feature = "tokio"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tokio_executor_spawns_and_sleeps() {
        let exec = TokioExecutor;
        let (tx, rx) = tokio::sync::oneshot::channel();
        exec.spawn(Box::pin(async move {
            let _ = tx.send(42);
        }));
        exec.sleep(Duration::from_millis(1)).await;
        assert_eq!(rx.await.unwrap(), 42);
    }

    #[test]
    fn tokio_executor_reports_time_after_2020() {
        // 2020-01-01T00:00:00Z
        assert!(TokioExecutor.unix_time() > Duration::from_secs(1_577_836_800));
    }

    #[seaquel_runtime::async_trait]
    trait Greeter: MaybeSend + MaybeSync {
        async fn greet(&self) -> &'static str;
    }

    struct Hello;

    #[seaquel_runtime::async_trait]
    impl Greeter for Hello {
        async fn greet(&self) -> &'static str {
            "hello"
        }
    }

    #[tokio::test]
    async fn async_trait_wrapper_produces_send_trait_objects_on_native() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn Greeter>();
        let g: Box<dyn Greeter> = Box::new(Hello);
        assert_eq!(g.greet().await, "hello");
    }
}
