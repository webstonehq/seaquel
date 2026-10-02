//! The write turn's wait runs on Core's executor (phase 8 Decision 5).
//!
//! In the browser there is no tokio timer, so storage's `WRITE_WAIT` has to
//! race the executor's `sleep`. Core hands storage its executor when it
//! opens a workspace; this checks that natively with an executor whose
//! sleep ends at once, so a write that can't get its turn fails straight
//! away instead of after 30 s on tokio's clock.
#![cfg(feature = "storage")]
// Native-only test: tasks on the test runtime.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use seaquel_core::domain::library::ProjectDraft;
use seaquel_core::{ConnectPolicy, WorkspaceSpec, WriteOrigin};
use seaquel_runtime::{Executor, TokioExecutor};

/// Tokio for everything but `sleep`, which ends at once and is counted.
#[derive(Default)]
struct InstantSleep {
    sleeps: AtomicUsize,
}

impl Executor for InstantSleep {
    fn spawn(&self, future: BoxFuture<'static, ()>) {
        TokioExecutor.spawn(future)
    }

    fn sleep(&self, _duration: Duration) -> BoxFuture<'static, ()> {
        self.sleeps.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }

    fn unix_time(&self) -> Duration {
        TokioExecutor.unix_time()
    }

    fn monotonic(&self) -> Duration {
        TokioExecutor.monotonic()
    }
}

fn draft(name: &str) -> ProjectDraft {
    ProjectDraft {
        name: name.to_string(),
        description: None,
        rename_if_taken: false,
    }
}

#[tokio::test]
async fn the_write_turn_times_out_through_the_executor() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(InstantSleep::default());
    let core = seaquel_core::with_default_plugins()
        .connect_policy(ConnectPolicy::Unrestricted)
        .executor(clock.clone())
        .build();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let origin = WriteOrigin::none();

    let held = ws.storage().write().await.unwrap();
    let before = clock.sleeps.load(Ordering::SeqCst);
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        ws.create_project(&core, &origin, draft("Waits")),
    )
    .await
    .expect("the write turn waited on tokio's clock instead of the executor's")
    .unwrap_err();
    assert_eq!(refused.code, "STORAGE_ERROR", "{refused:?}");
    assert!(
        clock.sleeps.load(Ordering::SeqCst) > before,
        "the wait never asked the executor to sleep"
    );

    // The turn comes back once the holder is done. (Committed rather than
    // dropped: a dropped one rolls back on a task, which with this clock's
    // instant waits would race the next write.)
    held.commit().await.unwrap();
    let made = ws
        .create_project(&core, &origin, draft("Gets its turn"))
        .await
        .unwrap();
    assert_eq!(made.value.name, "Gets its turn");
}
