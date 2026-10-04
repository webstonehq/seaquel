//! The runtime's spawned work. Core calls run as tasks here; the loop
//! watches them so a panic in one ends the TUI instead of leaving it
//! drawing into a terminal the panic hook already restored.
//!
//! [`Runner`] carries out `update`'s effects: each Core call is a task
//! whose answer comes back to the loop as a [`Msg`] through the inbox. It
//! also forwards the workspace's events (`StorageChanged` from other
//! writers, `ConnectionClosed`) and `SecretWait`'s pending flag.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use seaquel_core::{StoredKind, WorkspaceEvent};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::{AbortHandle, JoinSet};

use super::clock;
use super::core::Session;
use super::state_file;
use crate::state::app::{Changed, Effect, Msg, Stamp};
use crate::state::dialogs::CallError;
use crate::state::log::LogLine;
use crate::state::query::SqlText;

/// The tasks the loop watches.
#[derive(Debug, Default)]
pub struct Tasks {
    set: JoinSet<()>,
}

impl Tasks {
    /// Runs `task` on the runtime.
    pub fn spawn(&mut self, task: impl Future<Output = ()> + Send + 'static) -> AbortHandle {
        self.set.spawn(task)
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Aborts every task (the exit): their Core calls are dropped, which
    /// stops a statement still running on the server where the engine can.
    pub fn abort_all(&mut self) {
        self.set.abort_all();
    }

    /// Waits for the next task to end; `true` when it panicked. Pending
    /// forever while there are none.
    pub async fn next_panicked(&mut self) -> bool {
        match self.set.join_next().await {
            Some(Err(e)) => e.is_panic(),
            Some(Ok(())) => false,
            None => std::future::pending().await,
        }
    }
}

/// Where Core's answers arrive.
pub type Inbox = UnboundedReceiver<Msg>;

/// Carries out effects.
#[derive(Debug)]
pub struct Runner {
    session: Option<Arc<Session>>,
    data_dir: Option<PathBuf>,
    tx: UnboundedSender<Msg>,
    pub tasks: Tasks,
    /// Connects in flight, by attempt, so a given-up one can be dropped.
    connects: HashMap<u64, AbortHandle>,
    /// The table page in flight: a new one drops it (one per tab).
    page: Option<AbortHandle>,
    /// Each query tab's run or page in flight, and its stream id.
    runs: HashMap<u64, (AbortHandle, String)>,
    /// Each query tab's explain in flight.
    explains: HashMap<u64, AbortHandle>,
    /// Ask AI's request in flight: a new one, or Esc, drops it.
    generate: Option<AbortHandle>,
    /// The DuckDB helper's lookup or download in flight, by op: Esc drops
    /// it (and with it the partial file).
    install: Option<(u64, AbortHandle)>,
}

impl Runner {
    /// A runner over `session`, writing the state file under `data_dir`.
    pub fn new(session: Option<Arc<Session>>, data_dir: Option<PathBuf>) -> (Runner, Inbox) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let runner = Runner {
            session,
            data_dir,
            tx,
            tasks: Tasks::default(),
            connects: HashMap::new(),
            page: None,
            runs: HashMap::new(),
            explains: HashMap::new(),
            generate: None,
            install: None,
        };
        (runner, rx)
    }

    /// A runner with no Core (the loop's own tests).
    pub fn detached() -> (Runner, Inbox) {
        Runner::new(None, None)
    }

    /// Where the runner's tasks send their answers (tests send their own).
    pub fn sender(&self) -> UnboundedSender<Msg> {
        self.tx.clone()
    }

    /// Forwards the workspace's events and the keychain's pending flag as
    /// messages, until the session closes.
    pub fn listen(&mut self) {
        let Some(session) = self.session.clone() else {
            return;
        };
        // Storage changes and closed connections. The TUI's own writes come
        // back with its origin: the write's answer already updated it.
        let tx = self.tx.clone();
        let own = session.origin.as_deref().map(str::to_string);
        let mut events = session.ws.events();
        self.tasks.spawn(async move {
            while let Some(event) = events.next().await {
                let msg = match event {
                    WorkspaceEvent::StorageChanged(change) => {
                        if change.origin.is_some() && change.origin == own {
                            continue;
                        }
                        let changed = match change.kind {
                            StoredKind::External => Changed::External,
                            // AI settings: Ask AI's sharing line and model.
                            StoredKind::Connection
                            | StoredKind::Project
                            | StoredKind::Label
                            | StoredKind::AiSettings => Changed::Library,
                            StoredKind::SavedQuery => Changed::SavedQueries,
                            StoredKind::History => Changed::History,
                            _ => continue,
                        };
                        Msg::Changed(changed)
                    }
                    WorkspaceEvent::ConnectionClosed {
                        connection_id,
                        code,
                        ..
                    } => Msg::Closed {
                        core_id: connection_id,
                        code,
                    },
                    _ => continue,
                };
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        // The keychain: pending or not, and since when.
        let tx = self.tx.clone();
        let mut changed = session.wait.changed();
        self.tasks.spawn(async move {
            loop {
                let pending = *changed.borrow_and_update();
                let now = Instant::now();
                let at = session
                    .wait
                    .pending_for()
                    .and_then(|d| now.checked_sub(d))
                    .unwrap_or(now);
                if tx.send(Msg::Keychain { pending, at }).is_err() {
                    break;
                }
                if changed.changed().await.is_err() {
                    break;
                }
            }
        });
    }

    /// Runs `task` on the session as a task that sends its own messages.
    fn spawn_with<F, Fut>(&mut self, task: F) -> Option<AbortHandle>
    where
        F: FnOnce(Arc<Session>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let session = self.session.clone()?;
        Some(self.tasks.spawn(task(session)))
    }

    /// Runs `call` on the session as a task and sends its message.
    fn call<F, Fut>(&mut self, call: F) -> Option<AbortHandle>
    where
        F: FnOnce(Arc<Session>) -> Fut,
        Fut: Future<Output = Msg> + Send + 'static,
    {
        let Some(session) = self.session.clone() else {
            log::debug!(activity = "tui.effect"; "No Core: effect dropped");
            return None;
        };
        let tx = self.tx.clone();
        let task = call(session);
        Some(self.tasks.spawn(async move {
            let _ = tx.send(task.await);
        }))
    }

    /// Drops the DuckDB helper's lookup or download in flight.
    fn stop_install(&mut self) {
        if let Some((_, handle)) = self.install.take() {
            handle.abort();
        }
    }

    /// Drops a tab's run (or page) in flight: dropping its stream cancels it
    /// in Core.
    fn stop_run(&mut self, tab: u64) {
        if let Some((handle, stream_id)) = self.runs.remove(&tab) {
            handle.abort();
            if let Some(session) = &self.session {
                session.cancel(&stream_id);
            }
        }
    }

    /// Carries out one effect (the loop handles `Quit`, `Suspend`,
    /// `Redraw` and `ExternalEditor` itself).
    pub fn perform(&mut self, effect: Effect) {
        self.connects.retain(|_, handle| !handle.is_finished());
        self.runs.retain(|_, (handle, _)| !handle.is_finished());
        self.explains.retain(|_, handle| !handle.is_finished());
        match effect {
            Effect::Quit | Effect::Suspend | Effect::Redraw => {}
            // The loop gives the terminal to `$EDITOR` itself.
            Effect::ExternalEditor { .. } => {}
            Effect::Run(call) => {
                self.stop_run(call.tab);
                let (tab, stream_id) = (call.tab, call.stream_id.clone());
                let tx = self.tx.clone();
                if let Some(handle) = self.spawn_with(|s| async move {
                    let (tab, op) = (call.tab, call.op);
                    let send = |event| {
                        let _ = tx.send(Msg::Run { tab, op, event });
                    };
                    s.run(&call, send).await;
                }) {
                    self.runs.insert(tab, (handle, stream_id));
                }
            }
            Effect::PageRun(call) => {
                self.stop_run(call.tab);
                let (tab, stream_id) = (call.tab, call.stream_id.clone());
                let tx = self.tx.clone();
                if let Some(handle) = self.spawn_with(|s| async move {
                    let (tab, op) = (call.tab, call.op);
                    let send = |event| {
                        let _ = tx.send(Msg::Run { tab, op, event });
                    };
                    s.page(&call, send).await;
                }) {
                    self.runs.insert(tab, (handle, stream_id));
                }
            }
            Effect::CancelRun { tab, stream_id } => {
                if let Some((handle, _)) = self.runs.remove(&tab) {
                    handle.abort();
                }
                // Also when the task already ended or never started: a cancel
                // that overtakes its start still counts in Core.
                if let (Some(session), Some(id)) = (&self.session, stream_id) {
                    session.cancel(&id);
                }
            }
            Effect::Explain(call) => {
                if let Some(handle) = self.explains.remove(&call.tab) {
                    handle.abort();
                }
                let tab = call.tab;
                if let Some(handle) = self.call(|s| async move {
                    let result = s.explain(&call).await;
                    Msg::Explained {
                        tab: call.tab,
                        op: call.op,
                        result,
                    }
                }) {
                    self.explains.insert(tab, handle);
                }
            }
            Effect::CancelExplain { tab } => {
                if let Some(handle) = self.explains.remove(&tab) {
                    handle.abort();
                }
            }
            Effect::SaveQuery(call) => {
                self.call(|s| async move {
                    let (result, taken_by) = match s.save_query(&call).await {
                        Ok(item) => (Ok(item), None),
                        Err((e, taken_by)) => (Err(e), taken_by),
                    };
                    Msg::QuerySaved {
                        tab: call.tab,
                        text: call.text.into(),
                        result,
                        taken_by,
                        detached: call.detached,
                    }
                });
            }
            Effect::Generate(call) => {
                // One request at a time: dropping the last drops its HTTP
                // request (phase 6 S1).
                if let Some(handle) = self.generate.take() {
                    handle.abort();
                }
                self.generate = self.call(|s| async move {
                    let started = Instant::now();
                    let result = s.generate(&call).await;
                    let elapsed_ms = started.elapsed().as_millis() as u64;
                    if let Err(e) = &result {
                        log::info!(activity = "tui.ask", code = e.code.as_str(), elapsed_ms = elapsed_ms; "Ask AI failed");
                    }
                    Msg::Generated {
                        op: call.op,
                        result: result.map(SqlText),
                        elapsed_ms,
                    }
                });
            }
            Effect::CancelGenerate => {
                if let Some(handle) = self.generate.take() {
                    handle.abort();
                }
            }
            Effect::LoadMentions { project_id } => {
                self.call(|s| async move {
                    let result = s
                        .dashboard_names(&project_id)
                        .await
                        .map(crate::state::ask::Names);
                    Msg::Mentions { project_id, result }
                });
            }
            Effect::CheckDuckdb { op } => {
                self.stop_install();
                let handle = self.call(|s| async move {
                    let result = s.duckdb_offer().await;
                    Msg::DuckdbOffer { op, result }
                });
                self.install = handle.map(|h| (op, h));
            }
            Effect::InstallDuckdb { op } => {
                self.stop_install();
                let tx = self.tx.clone();
                let handle = self.spawn_with(|s| async move {
                    let progress = tx.clone();
                    let result = s
                        .install_duckdb(move |bytes, total| {
                            let _ = progress.send(Msg::InstallProgress { op, bytes, total });
                        })
                        .await;
                    let _ = tx.send(Msg::Installed { op, result });
                });
                self.install = handle.map(|h| (op, h));
            }
            Effect::CancelInstall { op } => {
                if self.install.as_ref().is_some_and(|(o, _)| *o == op) {
                    self.stop_install();
                }
            }
            Effect::LoadLibrary => {
                self.call(|s| async move { Msg::Library(s.library().await) });
            }
            Effect::LoadSaved { project_id } => {
                self.call(|s| async move {
                    let result = s.saved(&project_id).await;
                    Msg::Saved { project_id, result }
                });
            }
            Effect::LoadHistory { connection_id } => {
                self.call(|s| async move {
                    let result = s.history(&connection_id).await;
                    Msg::History {
                        connection_id,
                        result,
                    }
                });
            }
            Effect::LoadSchema { core_id } => {
                self.call(|s| async move {
                    let started = Instant::now();
                    let result = s.schema(&core_id).await;
                    Msg::Schema {
                        core_id,
                        result,
                        stamp: stamp(started),
                    }
                });
            }
            Effect::Connect(call) => {
                let attempt = call.attempt;
                let handle = self.call(|s| async move {
                    let started = Instant::now();
                    let result = s.connect(&call).await;
                    Msg::Connected {
                        attempt: call.attempt,
                        result,
                        stamp: stamp(started),
                    }
                });
                if let Some(handle) = handle {
                    self.connects.insert(attempt, handle);
                }
            }
            Effect::CancelConnect { attempt } => {
                // Dropping the connect closes what it opened (its tunnel).
                if let Some(handle) = self.connects.remove(&attempt) {
                    handle.abort();
                }
            }
            Effect::Disconnect { core_id } => {
                if let Some(session) = self.session.clone() {
                    self.tasks
                        .spawn(async move { session.disconnect(&core_id).await });
                }
            }
            Effect::SavePassword(call) => {
                self.call(|s| async move {
                    let result = s.save_password(&call).await;
                    Msg::PasswordSaved {
                        connection_id: call.connection_id,
                        kinds: call.kinds,
                        result,
                    }
                });
            }
            Effect::SaveState(remembered) => {
                // Disk I/O (and an fsync) off the loop.
                // The number is taken now, in the order the saves were
                // asked for, so the latest one wins (review I3).
                if let Some(dir) = self.data_dir.clone() {
                    let seq = state_file::next_seq();
                    self.tasks.spawn(async move {
                        let saved = tokio::task::spawn_blocking(move || {
                            state_file::save_latest(&dir, &remembered, seq)
                        })
                        .await;
                        if let Ok(Err(e)) = saved {
                            log::warn!(activity = "tui.state_file", error = format!("{:?}", e.kind()).as_str(); "Can't write the state file");
                        }
                    });
                }
            }
            Effect::LoadPage(call) => {
                // Dropping the last page's task drops its stream, which
                // cancels it in Core.
                if let Some(handle) = self.page.take() {
                    handle.abort();
                }
                self.page = self.call(|s| async move {
                    let started = Instant::now();
                    let result = s.table_page(&call).await;
                    Msg::Page {
                        op: call.op,
                        result,
                        stamp: stamp(started),
                    }
                });
            }
            Effect::LoadColumns { core_id, target } => {
                self.call(|s| async move {
                    let result = s.table_columns(&core_id, &target).await;
                    Msg::Columns {
                        core_id,
                        target,
                        result,
                    }
                });
            }
            Effect::LoadMeta { core_id, target } => {
                self.call(|s| async move {
                    let result = s.table_meta(&core_id, &target).await;
                    Msg::Meta {
                        core_id,
                        target,
                        result,
                    }
                });
            }
            Effect::PlanEdit(call) => {
                self.call(|s| async move {
                    let result = s.plan_edit(&call).await;
                    Msg::Planned {
                        id: call.request.id,
                        seq: call.request.seq,
                        result,
                    }
                });
            }
            Effect::Apply(call) => {
                // Always answered (review M6): with no Core, at once; a task
                // dropped before Core answers, `CANCELLED`.
                let guard = ApplyGuard::new(self.tx.clone(), call.op);
                let Some(session) = self.session.clone() else {
                    guard.send(Msg::Applied {
                        op: call.op,
                        result: Err(CallError::new(
                            "NOT_CONNECTED",
                            "There is no Core to apply the changes on.",
                        )),
                        stamp: stamp(Instant::now()),
                    });
                    return;
                };
                self.tasks.spawn(async move {
                    let started = Instant::now();
                    let result = session.apply(&call).await;
                    guard.send(Msg::Applied {
                        op: call.op,
                        result,
                        stamp: stamp(started),
                    });
                });
            }
            Effect::Log(entry) => {
                let _ = self.tx.send(Msg::Log(LogLine {
                    time: clock::wall_clock(),
                    tag: entry.tag,
                    text: entry.text,
                    elapsed: entry.elapsed,
                }));
            }
        }
    }

    /// Closes the session's connections, tunnels and storage.
    pub async fn close(&mut self) {
        if let Some(session) = &self.session {
            session.close().await;
        }
    }
}

/// A Core answer's stamp: now, and how long since `started`.
fn stamp(started: Instant) -> Stamp {
    Stamp {
        time: clock::wall_clock(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

/// An apply's answer, sent once: dropped before [`ApplyGuard::send`] (the
/// task aborted, or no Core to run it), it answers `CANCELLED`, so the
/// model's `committing` always clears (review M6).
pub struct ApplyGuard {
    tx: UnboundedSender<Msg>,
    op: u64,
    sent: bool,
}

impl ApplyGuard {
    pub fn new(tx: UnboundedSender<Msg>, op: u64) -> ApplyGuard {
        ApplyGuard {
            tx,
            op,
            sent: false,
        }
    }

    pub fn send(mut self, msg: Msg) {
        self.sent = true;
        let _ = self.tx.send(msg);
    }
}

impl Drop for ApplyGuard {
    fn drop(&mut self) {
        if !self.sent {
            let _ = self.tx.send(Msg::Applied {
                op: self.op,
                result: Err(CallError::new("CANCELLED", "The commit was stopped.")),
                stamp: stamp(Instant::now()),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::commit::ApplyCall;

    fn call(op: u64) -> ApplyCall {
        ApplyCall {
            op,
            core_id: "core-1".into(),
            connection_id: "conn-1".into(),
            connection_name: "c".into(),
            labels: Vec::new(),
            changes: Vec::new(),
            confirmed: false,
        }
    }

    // Review M6: an apply always answers.
    #[tokio::test]
    async fn an_apply_with_no_core_answers_with_an_error() {
        let (mut runner, mut inbox) = Runner::detached();
        runner.perform(Effect::Apply(call(7)));
        match tokio::time::timeout(std::time::Duration::from_secs(5), inbox.recv())
            .await
            .expect("answered")
        {
            Some(Msg::Applied {
                op: 7,
                result: Err(e),
                ..
            }) => assert_eq!(e.code, "NOT_CONNECTED"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_dropped_apply_answers_cancelled() {
        let (runner, mut inbox) = Runner::detached();
        drop(ApplyGuard::new(runner.sender(), 3));
        match tokio::time::timeout(std::time::Duration::from_secs(5), inbox.recv())
            .await
            .expect("answered")
        {
            Some(Msg::Applied {
                op: 3,
                result: Err(e),
                ..
            }) => assert_eq!(e.code, "CANCELLED"),
            other => panic!("{other:?}"),
        }
        let guard = ApplyGuard::new(runner.sender(), 4);
        guard.send(Msg::Applied {
            op: 4,
            result: Err(crate::state::dialogs::CallError::new("X", "y")),
            stamp: Stamp::default(),
        });
        assert!(matches!(inbox.try_recv(), Ok(Msg::Applied { op: 4, .. })));
        assert!(inbox.try_recv().is_err(), "sent once");
    }
}
