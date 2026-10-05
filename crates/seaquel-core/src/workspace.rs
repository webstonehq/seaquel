//! A workspace: one user's metadata storage and secret store, and the
//! database connections and streams that user opened.
//!
//! The desktop app opens one at startup; the web server opens one per user.
//! Interfaces reach storage and secrets only through the [`Workspace`] Core
//! hands them, and `seaquel-rpc`'s `dispatch_workspace` serves them to the
//! GUIs.
//!
//! **Ownership.** Every connection a workspace opens ([`Workspace::connect`])
//! and every stream it starts ([`Workspace::query_stream`]) is tagged with
//! its [`WorkspaceId`]. Its database calls reach only those: an id it
//! doesn't own gets `CONNECTION_NOT_FOUND`, the same answer as an id that
//! doesn't exist. Stream ids are scoped per workspace. [`Workspace::close_all`]
//! closes everything a workspace owns (the web server's eviction).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
#[cfg(all(feature = "storage", not(target_arch = "wasm32")))]
use std::time::Duration;

use futures::channel::mpsc;

use seaquel_engine::{
    BatchStatement, BoxStream, CancellationToken, DbError, ExecuteResult, QueryResult,
    TransactionError,
};
#[cfg(feature = "secrets")]
use seaquel_secrets::SecretStore;
#[cfg(feature = "storage")]
use seaquel_storage::{Storage, StorageOptions};

use crate::changes::{event_target, ChangeCounter, SeqTicket, StorageChange, WriteOrigin};
use crate::{ChangeSeq, ConnectionHandle, Core, QueryOptions, StoredKind, StreamEvent, Value};

/// The metadata file's name in a desktop data dir.
pub const DESKTOP_STORAGE_FILE: &str = "seaquel.db";

/// What [`crate::Core::open_workspace`] opens. Build it with
/// [`WorkspaceSpec::new`] and the `with_*` methods, since which fields exist
/// depends on Core's features.
#[non_exhaustive]
pub struct WorkspaceSpec {
    /// The desktop app's data dir, or a web user's `DATA_DIR/users/<id>`.
    pub data_dir: PathBuf,
    /// The metadata file's name inside `data_dir`: [`DESKTOP_STORAGE_FILE`]
    /// by default; the web server uses `meta.db`.
    #[cfg(feature = "storage")]
    pub storage_file: String,
    #[cfg(feature = "storage")]
    pub storage_options: StorageOptions,
    /// The desktop app's keychain. The web server has none, and secret calls
    /// on its workspaces fail with `NOT_SUPPORTED`.
    #[cfg(feature = "secrets")]
    pub secrets: Option<Arc<dyn SecretStore>>,
    /// A second process beside the app ([`WorkspaceSpec::second_process`]):
    /// no maintenance writes when it opens.
    #[cfg(feature = "storage")]
    pub second_process: bool,
    /// How often to poll for other connections' commits
    /// ([`WorkspaceSpec::with_external_changes`]); `None`, the default,
    /// doesn't poll.
    #[cfg(all(feature = "storage", not(target_arch = "wasm32")))]
    pub external_changes: Option<Duration>,
}

impl WorkspaceSpec {
    /// Opens the storage as a second process beside the app (phase 7a
    /// Decision 3; the TUI): writable, but only a current file
    /// ([`seaquel_storage::SchemaPolicy::RequireCurrent`]: a missing file is
    /// `STORAGE_NOT_FOUND`, one with schema work left
    /// `STORAGE_NEEDS_UPGRADE`, and nothing is created or migrated), and
    /// with none of the open's maintenance writes: no string-secrets
    /// upgrade (whose keychain items the app's binary wouldn't own), no
    /// name-key or list-metadata refill. Those stay the app's. A row whose
    /// string still holds a pre-5a secret connects with it, as the MCP
    /// server's do. The policy is applied when the workspace opens, so it
    /// holds whatever [`WorkspaceSpec::with_storage_options`] set, before
    /// or after.
    #[cfg(feature = "storage")]
    #[must_use]
    pub fn second_process(mut self) -> Self {
        self.second_process = true;
        self
    }

    /// Polls the storage every `interval` for commits made by any other
    /// connection to the file (another process: the TUI beside the app, or
    /// the app beside the TUI), and announces each change it sees as one
    /// [`WorkspaceEvent::StorageChanged`] of kind [`StoredKind::External`]
    /// (phase 7a Decision 6). Off by default; the desktop app and the TUI
    /// turn it on (1 s), the web server and the demo don't. Native only:
    /// in the browser no one else writes the file.
    ///
    /// The poll runs on Core's executor ([`crate::CoreBuilder::executor`];
    /// without one it doesn't run), starts once the open's own work is
    /// done, and ends with [`Workspace::close`], [`Workspace::close_all`]
    /// or the last reference to the workspace.
    #[cfg(all(feature = "storage", not(target_arch = "wasm32")))]
    #[must_use]
    pub fn with_external_changes(mut self, interval: Duration) -> Self {
        self.external_changes = Some(interval);
        self
    }

    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            #[cfg(feature = "storage")]
            storage_file: DESKTOP_STORAGE_FILE.to_string(),
            #[cfg(feature = "storage")]
            storage_options: StorageOptions::default(),
            #[cfg(feature = "secrets")]
            secrets: None,
            #[cfg(feature = "storage")]
            second_process: false,
            #[cfg(all(feature = "storage", not(target_arch = "wasm32")))]
            external_changes: None,
        }
    }

    #[cfg(feature = "storage")]
    #[must_use]
    pub fn with_storage_file(mut self, name: impl Into<String>) -> Self {
        self.storage_file = name.into();
        self
    }

    /// How the storage opens. `StorageOptions { read_only: true, .. }`
    /// opens it without writing (the CLI).
    #[cfg(feature = "storage")]
    #[must_use]
    pub fn with_storage_options(mut self, options: StorageOptions) -> Self {
        self.storage_options = options;
        self
    }

    /// The browser's open (phase 8 Decision 4): the metadata file in memory,
    /// starting from `image` (the snapshot the page kept), or empty. The
    /// data dir and file name then only name the file in errors.
    #[cfg(all(feature = "storage", target_arch = "wasm32"))]
    #[must_use]
    pub fn with_image(mut self, image: Option<Vec<u8>>) -> Self {
        self.storage_options = StorageOptions::in_memory(image);
        self
    }

    #[cfg(feature = "secrets")]
    #[must_use]
    pub fn with_secrets(mut self, store: Arc<dyn SecretStore>) -> Self {
        self.secrets = Some(store);
        self
    }
}

impl fmt::Debug for WorkspaceSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("WorkspaceSpec");
        s.field("data_dir", &self.data_dir);
        #[cfg(feature = "storage")]
        s.field("storage_file", &self.storage_file)
            .field("storage_options", &self.storage_options);
        #[cfg(feature = "secrets")]
        s.field("secrets", &self.secrets.as_ref().map(|_| "<store>"));
        #[cfg(feature = "storage")]
        s.field("second_process", &self.second_process);
        #[cfg(all(feature = "storage", not(target_arch = "wasm32")))]
        s.field("external_changes", &self.external_changes);
        s.finish()
    }
}

/// A workspace's id: random, made when it opens, and never reused. Core
/// tags the workspace's connections and streams with it.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceId(uuid::Uuid);

impl WorkspaceId {
    fn random() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl fmt::Debug for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WorkspaceId({})", self.0)
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// [`WorkspaceEvent::ConnectionClosed`]: the connection was lost without
/// being asked to close. Core watches each connection whose driver can tell
/// (`Driver::closed`, the remote DuckDB driver: its helper died) and, when
/// it fires, takes the connection out and announces it with the driver's
/// message (the desktop DuckDB helper plan, Decision 7). Other drivers
/// don't notice a lost connection by themselves: their calls fail instead.
pub const CONNECTION_CLOSED: &str = "CONNECTION_CLOSED";

/// [`WorkspaceEvent::ConnectionClosed`]: the connection's SSH tunnel dropped.
/// Not produced yet (`seaquel-ssh` doesn't report a dropped tunnel); reserved
/// like [`CONNECTION_CLOSED`].
pub const TUNNEL_CLOSED: &str = "TUNNEL_CLOSED";

/// [`WorkspaceEvent::ConnectionClosed`]: [`Workspace::close_all`] closed it
/// (the web server evicted the workspace).
pub const WORKSPACE_EVICTED: &str = "WORKSPACE_EVICTED";

/// [`WorkspaceEvent::ConnectionClosed`]: [`Workspace::close_owned_by`]
/// closed it, because the window that opened it is gone (the web server
/// reaps a window whose last `/rpc/stream` socket stayed closed past its
/// grace period; phase 6 probe F4).
pub const WINDOW_CLOSED: &str = "WINDOW_CLOSED";

/// [`WorkspaceEvent::ConnectionClosed`]: the window that opened it
/// connected the same saved connection again, and [`Workspace::connect`]
/// (or [`Workspace::bind_saved_connection`]) closed this older one once
/// the new one had opened (phase 6 probe F4).
pub const CONNECTION_REPLACED: &str = "CONNECTION_REPLACED";

/// [`CONNECTION_REPLACED`]'s message.
const REPLACED_MESSAGE: &str = "This tab connected again, so its older connection was closed.";

/// A workspace's receivers, held weakly by a lost connection's watcher
/// (`lost.rs`).
pub(crate) type EventSink = std::sync::Weak<Mutex<Vec<mpsc::UnboundedSender<WorkspaceEvent>>>>;

/// Send `event` to every live receiver in `subscribers`, dropping the
/// closed ones.
pub(crate) fn emit_to(
    subscribers: &Mutex<Vec<mpsc::UnboundedSender<WorkspaceEvent>>>,
    event: WorkspaceEvent,
) {
    subscribers
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|tx| tx.unbounded_send(event.clone()).is_ok());
}

/// Something that happened to a workspace without the calling GUI asking,
/// delivered through [`Workspace::events`]. `seaquel-rpc` sends it to the
/// GUIs as `CoreEvent::ConnectionClosed` and `CoreEvent::StorageChanged`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkspaceEvent {
    /// One of the workspace's connections is gone. `code` is
    /// [`WORKSPACE_EVICTED`], [`WINDOW_CLOSED`], [`CONNECTION_REPLACED`] or
    /// [`CONNECTION_CLOSED`] (lost); [`TUNNEL_CLOSED`] is reserved.
    ConnectionClosed {
        connection_id: String,
        code: String,
        message: String,
    },
    /// A stored write committed (phase 5d, Decision 16): exactly one per
    /// write, after its commit, and none for a refused or failed call.
    StorageChanged(StorageChange),
}

/// One user's open storage and secret store, and the owner of the
/// connections and streams they open.
pub struct Workspace {
    id: WorkspaceId,
    /// Set by [`Workspace::close_all`]: no new connection after it.
    closed: AtomicBool,
    /// Cancelled by [`Workspace::close_all`], for the calls it must stop
    /// that aren't streams (an apply's transaction, probe M1). Streams are
    /// cancelled through their own tokens.
    closing: CancellationToken,
    /// The receivers [`Workspace::events`] handed out; a dropped one is
    /// pruned on the next event. `Arc`, so a lost connection's watcher can
    /// announce it through a weak [`EventSink`].
    subscribers: Arc<Mutex<Vec<mpsc::UnboundedSender<WorkspaceEvent>>>>,
    /// Connects and tests in flight ([`ConnectSlot`]), which count toward
    /// [`crate::ConnectionLimits::per_workspace`] with the open connections.
    #[cfg_attr(not(feature = "workspace"), allow(dead_code))]
    connecting: Mutex<Connecting>,
    /// The change sequence (phase 5d, Decision 17).
    changes: ChangeCounter,
    /// Cancelled by [`Workspace::close`] and [`Workspace::close_all`]: ends
    /// the external-changes poll (phase 7a Decision 6).
    poll_stop: CancellationToken,
    /// Whether the external-changes poll is running.
    polling: AtomicBool,
    data_dir: PathBuf,
    #[cfg(feature = "storage")]
    storage: Storage,
    #[cfg(feature = "secrets")]
    secrets: Option<Arc<dyn SecretStore>>,
    /// The files this session's syncs have named (phase 5e, `*` (6)): each
    /// is named at most once.
    #[cfg(all(feature = "git", feature = "storage"))]
    pub(crate) notices: Mutex<seaquel_workspace::shared::NoticeMemory>,
    /// The assistant's turns in flight and their waiters (phase 6).
    #[cfg(feature = "ai")]
    pub(crate) ai: crate::ai::WorkspaceAi,
}

impl Workspace {
    /// Opens the spec's storage. With an `executor` (Core's), storage's
    /// write turn waits on its clock (phase 8 Decision 5): the page's timer
    /// in the browser, tokio's natively.
    pub(crate) async fn open(
        spec: WorkspaceSpec,
        executor: Option<&Arc<dyn seaquel_runtime::Executor>>,
    ) -> Result<Self, CoreError> {
        #[cfg(feature = "storage")]
        let storage = {
            let mut options = spec.storage_options;
            if spec.second_process {
                options.schema = seaquel_storage::SchemaPolicy::RequireCurrent;
            }
            let storage = Storage::open(spec.data_dir.join(&spec.storage_file), options)
                .await
                .map_err(CoreError::from)?;
            match executor {
                Some(executor) => storage.with_executor(Arc::clone(executor)),
                None => storage,
            }
        };
        #[cfg(not(feature = "storage"))]
        let _ = executor;
        let id = WorkspaceId::random();
        Ok(Self {
            id,
            changes: ChangeCounter::new(id.to_string()),
            closed: AtomicBool::new(false),
            closing: CancellationToken::new(),
            poll_stop: CancellationToken::new(),
            polling: AtomicBool::new(false),
            subscribers: Arc::default(),
            connecting: Mutex::default(),
            data_dir: spec.data_dir,
            #[cfg(feature = "storage")]
            storage,
            #[cfg(feature = "secrets")]
            secrets: spec.secrets,
            #[cfg(all(feature = "git", feature = "storage"))]
            notices: Mutex::default(),
            #[cfg(feature = "ai")]
            ai: crate::ai::WorkspaceAi::default(),
        })
    }

    /// Whether the external-changes poll
    /// ([`WorkspaceSpec::with_external_changes`]) is running (tests).
    #[doc(hidden)]
    pub fn polls_external_changes(&self) -> bool {
        self.polling.load(Ordering::SeqCst)
    }

    /// This workspace's id.
    pub fn id(&self) -> WorkspaceId {
        self.id
    }

    /// The dir this workspace was opened on.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The metadata storage. Pass it to the query modules in
    /// [`crate::storage`] (`storage::connections::load_all(ws.storage())`).
    #[cfg(feature = "storage")]
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// The secret store, or `None` on a workspace without one (the web
    /// server's).
    #[cfg(feature = "secrets")]
    pub fn secrets(&self) -> Option<&dyn SecretStore> {
        self.secrets.as_deref()
    }

    /// Close the storage's connections. Calls made after this fail. The web
    /// server calls it when it evicts a workspace, after
    /// [`Workspace::close_all`].
    pub async fn close(&self) {
        self.poll_stop.cancel();
        #[cfg(feature = "storage")]
        self.storage.close().await;
    }
}

// ── Database calls, on the workspace's own connections only ──

impl Workspace {
    /// A handle for one of this workspace's connections: its dialect,
    /// introspection and EXPLAIN (`seaquel-rpc`'s engine calls), and its
    /// queries. Every call on it checks ownership again, so it fails with
    /// `CONNECTION_NOT_FOUND` once the connection is closed.
    ///
    /// `CONNECTION_NOT_FOUND` now for an id this workspace doesn't own.
    pub fn engine<'a>(
        &self,
        core: &'a Core,
        connection_id: &str,
    ) -> Result<ConnectionHandle<'a>, DbError> {
        let handle = core.connection_handle_as(connection_id, Some(self.id));
        handle.engine()?;
        Ok(handle)
    }

    pub async fn query(
        &self,
        core: &Core,
        connection_id: &str,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<QueryResult, DbError> {
        core.connection_handle_as(connection_id, Some(self.id))
            .query(sql, params)
            .await
    }

    pub async fn execute(
        &self,
        core: &Core,
        connection_id: &str,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<ExecuteResult, DbError> {
        core.connection_handle_as(connection_id, Some(self.id))
            .execute(sql, params)
            .await
    }

    pub async fn transaction(
        &self,
        core: &Core,
        connection_id: &str,
        statements: Vec<BatchStatement>,
    ) -> Result<Vec<u64>, TransactionError> {
        core.connection_handle_as(connection_id, Some(self.id))
            .transaction(statements)
            .await
    }

    /// [`Core::query_stream`] on one of this workspace's connections, under
    /// `stream_id` in this workspace's scope ([`Workspace::cancel`] takes
    /// it). A connection it doesn't own ends the stream with one
    /// `CONNECTION_NOT_FOUND` error, and nothing is registered.
    pub fn query_stream<'a>(
        &self,
        core: &'a Core,
        stream_id: String,
        connection_id: String,
        sql: String,
        params: Vec<Value>,
        options: QueryOptions,
    ) -> BoxStream<'a, StreamEvent> {
        core.query_stream_as(
            Some(self.id),
            stream_id,
            connection_id,
            sql,
            params,
            options,
        )
    }

    /// Cancel this workspace's stream `stream_id`. Another workspace's
    /// stream with the same id isn't touched. An id with no running stream
    /// is remembered (the last 256 such ids per workspace): a stream this
    /// workspace starts under it later ends at once with no events, so a
    /// cancel that overtakes its start still counts. An id whose stream
    /// already ran and finished here isn't remembered: that cancel came too
    /// late. Stream ids must be unique per stream.
    ///
    /// After [`Workspace::close_all`] nothing is remembered: no stream can
    /// start here any more, and the early cancels were dropped with it.
    pub fn cancel(&self, core: &Core, stream_id: &str) {
        core.cancel_stream_as(Some(self.id), stream_id, Some(&self.closed));
    }

    /// Close one of this workspace's connections (and the SSH tunnel it
    /// owns). `CONNECTION_NOT_FOUND` for an id it doesn't own or that isn't
    /// open, and the connection stays open.
    pub async fn disconnect(&self, core: &Core, connection_id: &str) -> Result<(), CoreError> {
        core.disconnect_as(connection_id, Some(self.id))
            .await
            .map_err(|e| CoreError::new(e.code, e.message))
    }

    /// Record that this workspace's connection `connection_id` was opened
    /// for saved connection `saved_id`, when it was connected before the
    /// row existed (the page's `add` connects a form, then
    /// `connectionCreate` answers the id). Once: see
    /// `Core::bind_saved_as`. An assistant turn then runs on it.
    pub async fn bind_saved_connection(
        &self,
        core: &Core,
        connection_id: &str,
        saved_id: &str,
    ) -> Result<(), CoreError> {
        if saved_id.is_empty() {
            return Err(CoreError::new(
                "INVALID_ARGUMENT",
                "A saved connection id is required.",
            ));
        }
        core.bind_saved_as(connection_id, self.id, saved_id)
            .map_err(|e| CoreError::new(e.code, e.message))?;
        // Now it is the window's connection for that saved connection: an
        // older one the window still holds for it goes (phase 6 probe F4).
        if let Some(window) = core.window_of(connection_id, self.id) {
            self.replace_older(core, connection_id, &window, saved_id)
                .await;
        }
        Ok(())
    }

    /// Close the connections window `window` holds for saved connection
    /// `saved_id` that opened before `keep` (its new one), cancelling their streams
    /// as [`Workspace::disconnect`] does, and announce each as
    /// [`CONNECTION_REPLACED`] (phase 6 probe F4: a reload reconnects
    /// everything, and the page's old connections would otherwise stay open
    /// until eviction).
    async fn replace_older(&self, core: &Core, keep: &str, window: &str, saved_id: &str) {
        // Only older ones: of two overlapping connects, the later one's
        // replace must not close the newer connection, or both would go.
        let older = core.window_connections_older_than(self.id, window, saved_id, keep);
        if older.is_empty() {
            return;
        }
        log::info!(activity = "workspace.connect", replaced = older.len(); "Closing a window's older connections");
        self.close_announced(core, older, CONNECTION_REPLACED, REPLACED_MESSAGE)
            .await;
    }

    /// Close `ids` (this workspace's) and announce each one that was still
    /// open as [`WorkspaceEvent::ConnectionClosed`] with `code`, also when
    /// closing its driver failed (it is out of Core either way). One a
    /// concurrent [`Workspace::disconnect`] closed first isn't announced.
    /// Returns how many it announced.
    async fn close_announced(
        &self,
        core: &Core,
        ids: Vec<String>,
        code: &str,
        message: &str,
    ) -> usize {
        let mut taken_out = Vec::new();
        for id in ids {
            // Out of Core and announced before any close is awaited, so a
            // caller dropped mid-close (a reconnect that gives up) has
            // still announced them; the dropped closes drop the drivers and
            // the tunnels.
            let taken = match core.take_out_as(&id, Some(self.id)) {
                Ok(taken) => taken,
                Err(_) => continue,
            };
            self.emit(WorkspaceEvent::ConnectionClosed {
                connection_id: id.clone(),
                code: code.to_string(),
                message: message.to_string(),
            });
            taken_out.push((id, taken));
        }
        let closed = taken_out.len();
        // Side by side (the desktop DuckDB helper plan, Task 4 review I2):
        // one slow close (a DuckDB helper's checkpoint) doesn't hold the
        // others open.
        futures::future::join_all(taken_out.into_iter().map(|(id, taken)| async move {
            if let Err(e) = core.close_taken(taken).await {
                log::warn!(activity = "workspace.close", connection_id = id.as_str(), code = e.code.as_str(); "Closing a connection failed");
            }
        }))
        .await;
        closed
    }

    /// Close everything this workspace owns: cancel its streams, stop its
    /// applies in flight (an atomic one rolls back instead of committing, an
    /// in-order one stops at the statement it runs), and close its
    /// connections and their SSH tunnels. For the web server's eviction.
    /// Afterwards [`Workspace::connect`] fails with `WORKSPACE_CLOSED`, and a
    /// connect still in flight closes what it opened.
    ///
    /// Each connection it closes is announced on [`Workspace::events`] as
    /// [`WorkspaceEvent::ConnectionClosed`] with [`WORKSPACE_EVICTED`], also
    /// when closing its driver failed (it's out of Core either way). One
    /// that a concurrent [`Workspace::disconnect`] closed first
    /// (`CONNECTION_NOT_FOUND`) isn't: the GUI asked for that.
    pub async fn close_all(&self, core: &Core) {
        self.closed.store(true, Ordering::SeqCst);
        self.poll_stop.cancel();
        // An apply in flight drops its transaction (a rollback) or stops at
        // the statement it runs, before the connections close under it.
        self.closing.cancel();
        core.cancel_streams_owned_by(self.id);
        let ids = core.connections_of(self.id);
        log::info!(activity = "workspace.close_all", connections = ids.len(); "Closing a workspace's connections");
        self.close_announced(
            core,
            ids,
            WORKSPACE_EVICTED,
            "The server closed this connection to free resources; \
             reconnect to use it again.",
        )
        .await;
    }

    /// Close every connection the window `origin` opened (cancelling their
    /// streams, as [`Workspace::disconnect`] does), announce each as
    /// [`WorkspaceEvent::ConnectionClosed`] with [`WINDOW_CLOSED`], and
    /// return how many it closed. Other windows' connections and those
    /// opened with no origin stay.
    pub async fn close_owned_by(&self, core: &Core, origin: &str) -> usize {
        let ids = core.connections_of_window(self.id, origin, None);
        if ids.is_empty() {
            return 0;
        }
        log::info!(activity = "workspace.close_window", connections = ids.len(); "Closing a closed window's connections");
        self.close_announced(
            core,
            ids,
            WINDOW_CLOSED,
            "The tab that opened this connection was closed.",
        )
        .await
    }

    /// A new receiver for this workspace's [`WorkspaceEvent`]s from now on.
    /// Every receiver gets every event (the web server has one per open
    /// browser tab); dropping it unsubscribes. The stream never ends by
    /// itself, not even after [`Workspace::close_all`], so a transport
    /// still delivers the eviction events that call produced.
    pub fn events(&self) -> BoxStream<'static, WorkspaceEvent> {
        let (tx, rx) = mpsc::unbounded();
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Receivers dropped since the last event, so a workspace with no
        // events for a long time doesn't pile them up.
        subscribers.retain(|tx| !tx.is_closed());
        subscribers.push(tx);
        Box::pin(rx)
    }

    /// How many receivers [`Workspace::events`] has handed out that
    /// haven't been dropped yet, as of the last prune (tests).
    #[doc(hidden)]
    pub fn event_subscriber_count(&self) -> usize {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Send `event` to every live receiver, dropping the closed ones.
    fn emit(&self, event: WorkspaceEvent) {
        emit_to(&self.subscribers, event);
    }

    /// Where a watcher that outlives this call announces events: weak, so
    /// it doesn't keep the workspace's receivers past the workspace.
    #[cfg_attr(not(feature = "workspace"), allow(dead_code))]
    pub(crate) fn event_sink(&self) -> EventSink {
        Arc::downgrade(&self.subscribers)
    }

    /// The published change sequence (Decision 17): every write numbered up
    /// to it has committed or failed. A read records it before its SELECTs,
    /// so what it returns is at least that new.
    pub fn change_seq(&self) -> ChangeSeq {
        self.changes.published()
    }

    /// A number for a write, taken while it holds the storage's write lock.
    #[cfg_attr(not(feature = "storage"), allow(dead_code))]
    pub(crate) fn take_seq(&self) -> SeqTicket<'_> {
        self.changes.take()
    }

    /// Publish `ticket` after the write's commit and announce it: the one
    /// event a write emits.
    #[cfg_attr(not(feature = "storage"), allow(dead_code))]
    pub(crate) fn announce(
        &self,
        ticket: SeqTicket<'_>,
        kind: StoredKind,
        scope: Option<String>,
        ids: Option<Vec<String>>,
        origin: &WriteOrigin,
    ) -> ChangeSeq {
        let seq = ticket.publish();
        let (scope, ids) = event_target(scope, ids);
        self.emit(WorkspaceEvent::StorageChanged(StorageChange {
            kind,
            scope,
            ids,
            origin: origin.as_deref().map(str::to_string),
            seq: seq.clone(),
        }));
        seq
    }

    /// Announce a write that committed without a Core write transaction:
    /// the storage group's writes, which `seaquel-rpc` makes (Decision 16).
    /// The number is taken after the commit, which the published sequence
    /// allows for (see `changes.rs`). Call it only after the write
    /// succeeded.
    pub fn record_storage_write(
        &self,
        origin: &WriteOrigin,
        kind: StoredKind,
        scope: Option<String>,
        ids: Option<Vec<String>>,
    ) -> ChangeSeq {
        let ticket = self.changes.take();
        self.announce(ticket, kind, scope, ids, origin)
    }

    /// Starts the external-changes poll (phase 7a Decision 6): every
    /// `interval` on `executor`'s clock, [`Storage::external_version`]; a
    /// value that moved since the last poll is one
    /// [`StoredKind::External`] event, after taking a change-sequence
    /// number. The task holds only a weak reference, so it never keeps the
    /// workspace alive, and ends with [`Workspace::close`],
    /// [`Workspace::close_all`] or the last reference.
    ///
    /// The number matters: the GUI applies a refetched row only when its
    /// `seq.n` is higher than the one it holds, and another process's
    /// write doesn't move this workspace's sequence by itself, so without
    /// it the reload's answers would carry the same `n` and be dropped.
    #[cfg(all(feature = "storage", not(target_arch = "wasm32")))]
    pub(crate) fn start_external_poll(
        self: &Arc<Self>,
        executor: Arc<dyn seaquel_runtime::Executor>,
        interval: Duration,
    ) {
        use futures::future::{select, Either};

        let weak = Arc::downgrade(self);
        let stop = self.poll_stop.clone();
        self.polling.store(true, Ordering::SeqCst);
        let clock = Arc::clone(&executor);
        log::info!(activity = "workspace.poll", interval_ms = interval.as_millis() as u64; "Polling for external changes");
        executor.spawn(Box::pin(async move {
            let mut last: Option<i64> = None;
            let mut failing = false;
            // The first poll runs at once: it records the baseline.
            let mut first = true;
            loop {
                if !first {
                    let wait = select(clock.sleep(interval), Box::pin(stop.cancelled())).await;
                    if let Either::Right(_) = wait {
                        break;
                    }
                }
                first = false;
                let Some(ws) = weak.upgrade() else { break };
                if stop.is_cancelled() {
                    break;
                }
                match ws.storage.external_version().await {
                    Ok(Some(version)) => {
                        failing = false;
                        if last.is_some_and(|prev| prev != version) && !stop.is_cancelled() {
                            ws.record_storage_write(
                                &WriteOrigin::none(),
                                StoredKind::External,
                                None,
                                None,
                            );
                        }
                        last = Some(version);
                    }
                    // A write holds the turn: try again on the next tick.
                    Ok(None) => {}
                    Err(_) if stop.is_cancelled() => break,
                    Err(e) => {
                        // Said once per run of failures, not every tick.
                        if !failing {
                            log::warn!(activity = "workspace.poll", code = e.code(); "Polling for external changes failed");
                        }
                        failing = true;
                    }
                }
            }
            if let Some(ws) = weak.upgrade() {
                ws.polling.store(false, Ordering::SeqCst);
            }
        }));
    }

    /// Cancelled once [`Workspace::close_all`] runs.
    #[cfg(feature = "workspace")]
    pub(crate) fn closing(&self) -> &CancellationToken {
        &self.closing
    }

    /// Which of `ids` are this workspace's open connections, in the order
    /// given (`db.alive`). Another workspace's ids count as closed.
    pub fn alive(&self, core: &Core, ids: &[String]) -> Vec<String> {
        let open: std::collections::HashSet<String> =
            core.connections_of(self.id).into_iter().collect();
        ids.iter()
            .filter(|id| open.contains(*id))
            .cloned()
            .collect()
    }

    /// The ids of this workspace's open connections.
    pub fn connection_ids(&self, core: &Core) -> Vec<String> {
        core.connections_of(self.id)
    }

    /// How many cancels for streams not started yet this workspace
    /// remembers (tests).
    #[doc(hidden)]
    pub fn remembered_cancel_count(&self, core: &Core) -> usize {
        core.early_cancel_count_of(self.id)
    }

    /// How many streams this workspace has running.
    pub fn stream_count(&self, core: &Core) -> usize {
        core.stream_count_of(self.id)
    }
}

// ── Connect and test ──

/// `Workspace::connect` for an id with no saved connection, and the library
/// calls (`update_connection`, `remove_connection`) for one.
#[cfg(any(feature = "workspace", feature = "storage"))]
pub const SAVED_CONNECTION_NOT_FOUND: &str = "CONNECTION_NOT_FOUND";

/// `Workspace::connect` when the secret store refused a read the connection
/// needs (a denied keychain prompt, a locked keychain).
#[cfg(feature = "workspace")]
pub const SECRET_UNREADABLE: &str = "SECRET_UNREADABLE";

/// `Workspace::connect` when every secret read the connection needs found
/// no store at all (no Secret Service on a headless Linux host, no login
/// keychain over SSH on macOS; `SecretError::unavailable`), so an
/// interface can ask for the secret instead (phase 7a probe F4). A refusal
/// among the reads is `SECRET_UNREADABLE`.
#[cfg(feature = "workspace")]
pub const SECRET_STORE_UNAVAILABLE: &str = "SECRET_STORE_UNAVAILABLE";

/// The code a workspace without a secret store (the web server's) gives a
/// secret read. `Workspace::connect` no longer fails with it (phase 6 probe
/// F5): a saved row then connects with only what was supplied, as a form
/// does. Kept for the builder's SSH wording and the other reads.
#[cfg(feature = "workspace")]
pub const NO_SECRET_STORE: &str = "NO_SECRET_STORE";

/// `Workspace::connect` or `test` after [`Workspace::close_all`].
#[cfg(feature = "workspace")]
pub const WORKSPACE_CLOSED: &str = "WORKSPACE_CLOSED";

/// How [`Workspace::connect`] treats an SSH server's host key.
#[cfg(feature = "workspace")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyPolicy {
    /// Accept only a host already in known_hosts. An unknown one fails with
    /// `UNKNOWN_HOST_KEY` (its message holds the fingerprint), and
    /// known_hosts isn't written. The MCP server, and the GUI's first try.
    KnownOnly,
    /// Also accept, and record in known_hosts, an unknown host whose key has
    /// this fingerprint (`SHA256:…`), the one the user approved in the trust
    /// prompt. The GUI's retry after the prompt.
    Trust(String),
}

/// What [`Workspace::connect`] and [`Workspace::test`] connect.
#[cfg(feature = "workspace")]
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectTarget {
    /// A saved connection: its row from storage, secrets from the caller
    /// first, then the workspace's secret store under the row's save flags.
    Saved { id: String },
    /// A form the user filled in (add, the reconnect tab, test): never read
    /// from storage or the secret store.
    Form {
        form: Box<seaquel_types::connect::ConnectionForm>,
    },
}

/// A connect or test. Build it with [`ConnectRequest::saved`] or
/// [`ConnectRequest::form`] and the `with_*` methods. `Debug` redacts the
/// secrets.
#[cfg(feature = "workspace")]
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectRequest {
    pub target: ConnectTarget,
    /// Secrets from the caller. Each one supplied wins over the store.
    pub secrets: seaquel_types::connect::SuppliedSecrets,
    pub host_key: HostKeyPolicy,
    /// SQLite: create the file if it doesn't exist.
    pub create_if_missing: bool,
    /// Sets [`seaquel_types::ConnectConfig::restricted`]: a DuckDB
    /// connection opens its instance with no access to files but its own
    /// database, no extension installs or loads, and its configuration
    /// locked. Other engines ignore it. For the MCP server, whose DuckDB
    /// instances are its own; the GUI leaves it off, since the lock can't be
    /// undone on a running instance and would break the editor's file
    /// functions.
    pub restricted: bool,
    /// The saved connection a form connect opens (phase 6, Decision 6):
    /// Core records it on the connection, and an assistant turn refuses a
    /// connection whose recorded id isn't its chat's (`CONNECTION_MISMATCH`).
    /// A saved target records its own id when this is `None`.
    pub saved_connection_id: Option<String>,
    /// The window (write origin) that asks (phase 6 probe F4). Core records
    /// it on the connection; [`Workspace::close_owned_by`] closes a
    /// window's connections, and a window's new connection for a saved
    /// connection replaces its older ones. None: the connection belongs to
    /// no window (the CLI, MCP, tests).
    pub origin: crate::WriteOrigin,
}

#[cfg(feature = "workspace")]
impl ConnectRequest {
    fn new(target: ConnectTarget) -> Self {
        Self {
            origin: crate::WriteOrigin::none(),
            target,
            secrets: seaquel_types::connect::SuppliedSecrets::none(),
            host_key: HostKeyPolicy::KnownOnly,
            create_if_missing: false,
            restricted: false,
            saved_connection_id: None,
        }
    }

    /// A saved connection, with no supplied secrets, known hosts only, and
    /// `create_if_missing` and `restricted` off.
    pub fn saved(id: impl Into<String>) -> Self {
        Self::new(ConnectTarget::Saved { id: id.into() })
    }

    /// A filled-in form, with the same defaults as [`ConnectRequest::saved`].
    pub fn form(form: seaquel_types::connect::ConnectionForm) -> Self {
        Self::new(ConnectTarget::Form {
            form: Box::new(form),
        })
    }

    #[must_use]
    pub fn with_secrets(mut self, secrets: seaquel_types::connect::SuppliedSecrets) -> Self {
        self.secrets = secrets;
        self
    }

    #[must_use]
    pub fn with_host_key(mut self, host_key: HostKeyPolicy) -> Self {
        self.host_key = host_key;
        self
    }

    #[must_use]
    pub fn with_create_if_missing(mut self, create_if_missing: bool) -> Self {
        self.create_if_missing = create_if_missing;
        self
    }

    #[must_use]
    pub fn with_restricted(mut self, restricted: bool) -> Self {
        self.restricted = restricted;
        self
    }

    /// See [`ConnectRequest::origin`].
    #[must_use]
    pub fn with_origin(mut self, origin: crate::WriteOrigin) -> Self {
        self.origin = origin;
        self
    }

    /// See [`ConnectRequest::saved_connection_id`].
    #[must_use]
    pub fn with_saved_connection_id(mut self, id: Option<String>) -> Self {
        self.saved_connection_id = id;
        self
    }

    /// A saved target may only be recorded as itself (review M6); a form
    /// connect's `savedConnectionId` is trusted (the page connects before
    /// the row exists).
    fn check_saved_id(&self) -> Result<(), CoreError> {
        match (&self.saved_connection_id, &self.target) {
            (Some(named), ConnectTarget::Saved { id }) if named != id => Err(CoreError::new(
                "INVALID_ARGUMENT",
                "A saved connection can only be recorded as itself.",
            )),
            _ => Ok(()),
        }
    }

    /// The saved connection Core records on the connection: the one the
    /// request names, else a saved target's own id.
    fn recorded_saved_id(&self) -> Option<String> {
        match (&self.saved_connection_id, &self.target) {
            (Some(id), _) => Some(id.clone()),
            (None, ConnectTarget::Saved { id }) => Some(id.clone()),
            (None, ConnectTarget::Form { .. }) => None,
        }
    }
}

#[cfg(feature = "workspace")]
use seaquel_workspace::connections::{Plan, Target, UnreadableSecret};

#[cfg(feature = "workspace")]
impl Workspace {
    /// Connect `req.target` on `core`, owned by this workspace, and return
    /// Core's connection id.
    ///
    /// Both targets go through one builder (`seaquel_workspace::connections`,
    /// pinned by the `connect-config-v2` fixtures): supplied secrets win,
    /// and a saved row reads the rest from this workspace's secret store
    /// under its save flags. The SSH tunnel, if any, is opened here and
    /// belongs to the connection: [`Workspace::disconnect`] closes it, a
    /// failed connect closes it, and so does dropping this future before it
    /// finishes. There is one connect attempt.
    ///
    /// Errors: `CONNECTION_NOT_FOUND` (no such saved connection),
    /// `SECRET_UNREADABLE` (the store refused a read the row needs; checked
    /// before anything is opened), `CREDENTIALS_REQUIRED`,
    /// `INVALID_CONNECTION`, `WORKSPACE_CLOSED`, `NOT_SUPPORTED` (a tunnel in
    /// a build without SSH, a saved row without storage), the storage codes,
    /// the SSH codes (`UNKNOWN_HOST_KEY` carries the fingerprint), and Core's
    /// connect errors. No message contains a secret: any secret the driver
    /// or SSH layer echoes is replaced by `<redacted>`.
    pub async fn connect(&self, core: &Core, req: ConnectRequest) -> Result<String, CoreError> {
        self.check_open()?;
        req.check_saved_id()?;
        let window = req.origin.as_deref().map(str::to_string);
        let saved_id = req.recorded_saved_id();
        // The window's older connections for this saved connection, which
        // this one replaces once it has opened (phase 6 probe F4): they
        // don't count against the cap, so a reload at the cap still works.
        let replaced = match (&window, &saved_id) {
            (Some(window), Some(saved)) => core.connections_of_window(self.id, window, Some(saved)),
            _ => Vec::new(),
        };
        // Held until the connection is in Core's map (or the connect
        // failed), so the count never misses it.
        let _slot = self.reserve_slot(core, replaced)?;
        log::info!(activity = "workspace.connect", target = target_kind(&req.target); "Connecting");
        let plan = self.checked_plan(core, &req).await?;
        let tunnel = open_tunnel(core, &plan, &req).await?;
        let secrets = plan.secret_values();
        let redacted = |e: DbError| redact(CoreError::new(e.code, e.message), &secrets);
        let connected = match config(&plan, &req, tunnel.as_ref()) {
            Ok(config) => match core.prepare_connect(&config) {
                Err(e) => Err(redacted(e)),
                Ok(engine) => {
                    // A file one connection holds exclusively (DuckDB
                    // through its helper): the window's older connections
                    // for it would block this one, so they go now, once
                    // nothing but the open itself can refuse it (Decision
                    // 21). A failed open then has closed them anyway;
                    // other engines keep the replace after the open.
                    if let (Some(window), Some(saved)) = (&window, &saved_id) {
                        if engine.exclusive_file(&config) {
                            let older = core.connections_of_window(self.id, window, Some(saved));
                            if !older.is_empty() {
                                log::info!(activity = "workspace.connect", replaced = older.len(); "Closing a window's older connections to a file before reopening it");
                                self.close_announced(
                                    core,
                                    older,
                                    CONNECTION_REPLACED,
                                    REPLACED_MESSAGE,
                                )
                                .await;
                            }
                        }
                    }
                    core.open_prepared(
                        engine,
                        &config,
                        Some(self.id),
                        Some(plan.sql_engine()),
                        saved_id.clone(),
                        window.clone(),
                    )
                    .await
                    .map_err(redacted)
                }
            },
            Err(e) => Err(e),
        };
        match connected {
            Ok(result) => {
                if let Some(tunnel) = tunnel {
                    // Now `disconnect` closes it.
                    tunnel.keep(core, &result.connection_id);
                }
                if self.closed.load(Ordering::SeqCst) {
                    // `close_all` ran while this connected. After `keep`, so
                    // the tunnel goes too even if `close_all` already took
                    // the connection (an unknown id is fine here).
                    let _ = core.disconnect(&result.connection_id).await;
                    return Err(closed_error());
                }
                // Its tunnel is the connection's now, so a watcher that
                // takes a lost connection out closes that too.
                core.watch_lost(&result.connection_id, Some(self.event_sink()));
                if let (Some(window), Some(saved)) = (&window, &saved_id) {
                    self.replace_older(core, &result.connection_id, window, saved)
                        .await;
                }
                Ok(result.connection_id)
            }
            Err(e) => {
                if let Some(tunnel) = tunnel {
                    tunnel.close().await;
                }
                Err(e)
            }
        }
    }

    /// Open `req.target` and close it again without registering it ("Test
    /// connection"), through its SSH tunnel if it has one, which is closed
    /// afterwards. The same builder, secrets and errors as
    /// [`Workspace::connect`].
    pub async fn test(&self, core: &Core, req: ConnectRequest) -> Result<(), CoreError> {
        self.check_open()?;
        let _slot = self.reserve_slot(core, Vec::new())?;
        log::info!(activity = "workspace.test", target = target_kind(&req.target); "Testing a connection");
        let plan = self.checked_plan(core, &req).await?;
        let tunnel = open_tunnel(core, &plan, &req).await?;
        let tested = match config(&plan, &req, tunnel.as_ref()) {
            Ok(config) => core
                .test(&config)
                .await
                .map_err(|e| redact(CoreError::new(e.code, e.message), &plan.secret_values())),
            Err(e) => Err(e),
        };
        if let Some(tunnel) = tunnel {
            tunnel.close().await;
        }
        tested
    }

    /// [`Workspace::plan`] under Core's `ConnectPolicy`:
    ///
    /// - without a policy, refused before anything is read;
    /// - when the policy doesn't allow SSH, a target that asks for a tunnel
    ///   (an enabled tunnel, or a TablePlus `+ssh` URL) is refused as soon
    ///   as the saved row is read or from the form, before any secret-store
    ///   read or credential check; and the finished plan is checked again
    ///   before its tunnel would open, as a backstop.
    ///
    /// The policy's config check runs later, in `Core::connect_as`/`test`.
    async fn checked_plan(&self, core: &Core, req: &ConnectRequest) -> Result<Plan, CoreError> {
        let db_error = |e: DbError| CoreError::new(e.code, e.message);
        core.connect_policy().map_err(db_error)?;
        let row = match &req.target {
            ConnectTarget::Saved { id } => Some(self.load_row(id).await?),
            ConnectTarget::Form { .. } => None,
        };
        let asks_for_tunnel = match (&req.target, &row) {
            (_, Some(row)) => may_tunnel(
                &row.ty,
                row_tunnel_enabled(row),
                row.connection_string.as_deref(),
            ),
            (ConnectTarget::Form { form }, None) => may_tunnel(
                &form.ty,
                form.ssh_enabled,
                Some(form.connection_string.as_str()),
            ),
            (ConnectTarget::Saved { .. }, None) => false,
        };
        if asks_for_tunnel {
            core.check_ssh_allowed().map_err(db_error)?;
        }
        let plan = self.plan(req, row.as_ref()).await?;
        if plan.tunnel(None).is_some() {
            core.check_ssh_allowed().map_err(db_error)?;
        }
        Ok(plan)
    }

    /// A slot under [`crate::ConnectionLimits::per_workspace`], held while
    /// a connect or test runs, or `TOO_MANY_CONNECTIONS` when this
    /// workspace's open connections and the connects and tests in flight
    /// already reach it. The count and the reservation happen under one
    /// lock, and a connect releases its slot only after its connection is
    /// in Core's map, so concurrent calls can't pass the cap.
    ///
    /// `replaced` are the open connections this connect will replace (phase
    /// 6 probe F4): they don't count, so a tab at the cap can reload. The
    /// discount is netted across the connects in flight (review M2): every
    /// slot's ids are kept under the same lock, and an open connection that
    /// any of them will replace is discounted once, however many claim it.
    fn reserve_slot(
        &self,
        core: &Core,
        replaced: Vec<String>,
    ) -> Result<ConnectSlot<'_>, CoreError> {
        let mut connecting = self
            .connecting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(cap) = core.connection_limits().per_workspace {
            let open_ids = core.connections_of(self.id);
            let open = open_ids
                .iter()
                .filter(|id| !connecting.replacing.contains_key(*id) && !replaced.contains(*id))
                .count();
            if open + connecting.count >= cap {
                log::warn!(activity = "workspace.connect", cap = cap; "Refused a connection past the workspace's cap");
                return Err(CoreError::new(
                    crate::TOO_MANY_CONNECTIONS,
                    format!(
                        "You have {cap} connections open, the most allowed here. \
                         Disconnect one and try again."
                    ),
                ));
            }
        }
        connecting.count += 1;
        for id in &replaced {
            *connecting.replacing.entry(id.clone()).or_insert(0) += 1;
        }
        Ok(ConnectSlot {
            connecting: &self.connecting,
            replaced,
        })
    }

    fn check_open(&self) -> Result<(), CoreError> {
        if self.closed.load(Ordering::SeqCst) {
            Err(closed_error())
        } else {
            Ok(())
        }
    }

    /// The builder's plan for `req`, with the store reads it asks for.
    async fn plan(
        &self,
        req: &ConnectRequest,
        row: Option<&seaquel_types::storage::PersistedConnection>,
    ) -> Result<Plan, CoreError> {
        use seaquel_workspace::connections::plan;

        let loaded;
        let target = match (&req.target, row) {
            (ConnectTarget::Saved { .. }, Some(row)) => Target::Saved(row),
            (ConnectTarget::Saved { id }, None) => {
                loaded = self.load_row(id).await?;
                Target::Saved(&loaded)
            }
            (ConnectTarget::Form { form }, _) => Target::Form(form),
        };
        let result = plan(target, &req.secrets, |key: String| async move {
            self.read_secret(&key).await
        })
        .await;
        let plan = result.map_err(|e| CoreError::new(e.code, e.message))?;
        // The builder goes on without an unreadable secret (and says so when
        // it has to give up); here a denied keychain prompt must not turn
        // into a password-less attempt or a misleading auth error. A
        // workspace with no store at all (the web) is different: nothing
        // was refused, there is just nowhere a secret could be, so a saved
        // row connects with what was supplied, as a form does (phase 6
        // probe F5: a trust-auth database saved with `savePassword` on).
        // The builder still words a missing SSH password itself.
        let failed: Vec<&UnreadableSecret> = plan
            .secrets()
            .unreadable
            .iter()
            .filter(|u| u.code != NO_SECRET_STORE)
            .collect();
        if let Some(first) = failed.first() {
            let name = match &target {
                Target::Saved(row) => row.name.as_str(),
                Target::Form(form) => form.name.as_str(),
            };
            // A store that isn't there (probe F4) isn't a refusal; a
            // refusal among the reads still wins.
            if failed.iter().all(|u| u.code == SECRET_STORE_UNAVAILABLE) {
                return Err(unavailable_error(name, first));
            }
            let refused = failed
                .iter()
                .find(|u| u.code != SECRET_STORE_UNAVAILABLE)
                .unwrap_or(first);
            return Err(unreadable_error(name, refused));
        }
        Ok(plan)
    }

    #[cfg(feature = "storage")]
    async fn load_row(
        &self,
        id: &str,
    ) -> Result<seaquel_types::storage::PersistedConnection, CoreError> {
        seaquel_storage::connections::load_all(&self.storage)
            .await?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| {
                CoreError::new(
                    SAVED_CONNECTION_NOT_FOUND,
                    format!("Saved connection not found: {id}"),
                )
            })
    }

    #[cfg(not(feature = "storage"))]
    async fn load_row(
        &self,
        _id: &str,
    ) -> Result<seaquel_types::storage::PersistedConnection, CoreError> {
        Err(CoreError::new(
            "NOT_SUPPORTED",
            "Saved connections aren't available in this build",
        ))
    }

    /// One store read, failing with the store's error code, or
    /// `NO_SECRET_STORE` without a store.
    async fn read_secret(&self, key: &str) -> Result<Option<String>, String> {
        #[cfg(feature = "secrets")]
        if let Some(store) = &self.secrets {
            return store.get(key).await.map_err(|e| {
                if e.unavailable() {
                    SECRET_STORE_UNAVAILABLE.to_string()
                } else {
                    e.code().to_string()
                }
            });
        }
        let _ = key;
        Err(NO_SECRET_STORE.to_string())
    }
}

/// The connects and tests in flight ([`Workspace::reserve_slot`]): how
/// many, and the open connections they will replace, each with how many of
/// them claim it.
#[derive(Default)]
#[cfg_attr(not(feature = "workspace"), allow(dead_code))]
struct Connecting {
    count: usize,
    replacing: std::collections::HashMap<String, usize>,
}

/// A connect or test in flight (see [`Workspace::reserve_slot`]); dropping
/// it frees the slot and its claims.
#[cfg(feature = "workspace")]
struct ConnectSlot<'a> {
    connecting: &'a Mutex<Connecting>,
    replaced: Vec<String>,
}

#[cfg(feature = "workspace")]
impl Drop for ConnectSlot<'_> {
    fn drop(&mut self) {
        let mut connecting = self
            .connecting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connecting.count = connecting.count.saturating_sub(1);
        for id in &self.replaced {
            if let Some(n) = connecting.replacing.get_mut(id) {
                *n -= 1;
                if *n == 0 {
                    connecting.replacing.remove(id);
                }
            }
        }
    }
}

/// Whether a connection of type `ty` would go through an SSH tunnel, read
/// from the target alone: file engines never tunnel (row 7d); otherwise an
/// enabled tunnel, or a TablePlus `+ssh` URL on a URL engine (row 7b), does.
/// The same rules as `seaquel_workspace::connections::plan`, which stays
/// the authority (`Plan::tunnel`).
#[cfg(feature = "workspace")]
fn may_tunnel(ty: &str, tunnel_enabled: bool, connection_string: Option<&str>) -> bool {
    use seaquel_workspace::connection_string::is_plus_ssh;
    match ty {
        "sqlite" | "duckdb" => false,
        _ if tunnel_enabled => true,
        "postgres" | "mysql" | "mariadb" => connection_string.is_some_and(is_plus_ssh),
        _ => false,
    }
}

/// A saved row's `sshTunnel.enabled`, truthy the way JavaScript reads it
/// (as the builder does).
#[cfg(feature = "workspace")]
fn row_tunnel_enabled(row: &seaquel_types::storage::PersistedConnection) -> bool {
    use serde_json::Value;
    let Some(raw) = row.ssh_tunnel.as_ref() else {
        return false;
    };
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(raw.get()) else {
        return false;
    };
    match obj.get("enabled") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

#[cfg(feature = "workspace")]
fn target_kind(target: &ConnectTarget) -> &'static str {
    match target {
        ConnectTarget::Saved { .. } => "saved",
        ConnectTarget::Form { .. } => "form",
    }
}

#[cfg(feature = "workspace")]
fn closed_error() -> CoreError {
    CoreError::new(
        WORKSPACE_CLOSED,
        "This workspace was closed; reload to connect again.",
    )
}

/// The config for `plan`, through `tunnel` if one is open.
#[cfg(feature = "workspace")]
fn config(
    plan: &Plan,
    req: &ConnectRequest,
    tunnel: Option<&OpenTunnel<'_>>,
) -> Result<seaquel_types::ConnectConfig, CoreError> {
    let mut config = plan
        .config(tunnel.map(OpenTunnel::local_port), req.create_if_missing)
        .map_err(|e| CoreError::new(e.code, e.message))?;
    if req.restricted {
        config.restricted = Some(true);
    }
    Ok(config)
}

/// A tunnel [`open_tunnel`] opened: closed when dropped, until
/// [`OpenTunnel::keep`] hands it to a connection.
#[cfg(all(feature = "workspace", feature = "ssh"))]
struct OpenTunnel<'a> {
    local_port: u16,
    tunnel_id: String,
    guard: crate::ssh::TunnelGuard<'a>,
}

#[cfg(all(feature = "workspace", feature = "ssh"))]
impl OpenTunnel<'_> {
    fn local_port(&self) -> u16 {
        self.local_port
    }

    /// Tie it to the connection: [`Core::disconnect`] closes it.
    fn keep(self, core: &Core, connection_id: &str) {
        core.own_tunnel(connection_id, &self.tunnel_id);
        self.guard.keep();
    }

    async fn close(self) {
        self.guard.close().await;
    }
}

/// Opens the plan's SSH tunnel, if it has one.
#[cfg(all(feature = "workspace", feature = "ssh"))]
async fn open_tunnel<'a>(
    core: &'a Core,
    plan: &Plan,
    req: &ConnectRequest,
) -> Result<Option<OpenTunnel<'a>>, CoreError> {
    let trust = match &req.host_key {
        HostKeyPolicy::KnownOnly => None,
        HostKeyPolicy::Trust(fingerprint) => Some(fingerprint.clone()),
    };
    let Some(config) = plan.tunnel(trust) else {
        return Ok(None);
    };
    let opened = core
        .within_connect_timeout(core.ssh_open(&config), |limit| {
            CoreError::new(
                seaquel_engine::TIMEOUT,
                format!("The SSH server didn't answer within {limit}."),
            )
        })
        .await;
    let info = opened.map_err(|e| {
        let e = redact(e, &plan.secret_values());
        let saved = matches!(req.target, ConnectTarget::Saved { .. });
        if e.code == "UNKNOWN_HOST_KEY" && req.host_key == HostKeyPolicy::KnownOnly && saved {
            CoreError::new(
                e.code,
                format!(
                    "The SSH server of connection {:?} isn't a known host yet. Connect to it \
                     once in the Seaquel app and trust its host key. ({})",
                    plan.name(),
                    e.message
                ),
            )
        } else {
            e
        }
    })?;
    // Closes the tunnel if the caller's future is dropped from here on.
    let guard = core.tunnel_guard(&info.tunnel_id);
    Ok(Some(OpenTunnel {
        local_port: info.local_port,
        tunnel_id: info.tunnel_id,
        guard,
    }))
}

/// No tunnels in this build: a plan that needs one is `NOT_SUPPORTED`.
#[cfg(all(feature = "workspace", not(feature = "ssh")))]
enum OpenTunnel<'a> {
    #[allow(dead_code)]
    Never(std::convert::Infallible, std::marker::PhantomData<&'a ()>),
}

#[cfg(all(feature = "workspace", not(feature = "ssh")))]
impl OpenTunnel<'_> {
    fn local_port(&self) -> u16 {
        match *self {
            Self::Never(never, _) => match never {},
        }
    }

    fn keep(self, _core: &Core, _connection_id: &str) {
        match self {
            Self::Never(never, _) => match never {},
        }
    }

    async fn close(self) {
        match self {
            Self::Never(never, _) => match never {},
        }
    }
}

#[cfg(all(feature = "workspace", not(feature = "ssh")))]
async fn open_tunnel<'a>(
    _core: &'a Core,
    plan: &Plan,
    _req: &ConnectRequest,
) -> Result<Option<OpenTunnel<'a>>, CoreError> {
    match plan.tunnel(None) {
        Some(_) => Err(CoreError::new(
            "NOT_SUPPORTED",
            format!(
                "Connection {:?} goes through an SSH tunnel, which this build doesn't support",
                plan.name()
            ),
        )),
        None => Ok(None),
    }
}

/// What a secret key holds, for a message.
#[cfg(feature = "workspace")]
fn secret_what(key: &str) -> &'static str {
    if key.starts_with("ssh-key:") {
        "SSH key passphrase"
    } else if key.starts_with("ssh:") {
        "SSH password"
    } else {
        "password"
    }
}

/// The platform's secret store, for a message: the macOS keychain, the
/// Secret Service on Linux, Windows Credential Manager.
#[cfg(feature = "workspace")]
fn store_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "the keychain"
    } else if cfg!(windows) {
        "Windows Credential Manager"
    } else {
        "the system keyring (Secret Service)"
    }
}

/// What to do when the store refused a read, per platform.
#[cfg(feature = "workspace")]
fn store_allow_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "Allow Seaquel to access the keychain when the system asks"
    } else if cfg!(windows) {
        "Check the entry in Windows Credential Manager"
    } else {
        "Unlock the system keyring when it asks"
    }
}

#[cfg(feature = "workspace")]
fn unreadable_error(name: &str, failed: &UnreadableSecret) -> CoreError {
    let what = secret_what(&failed.key);
    CoreError::new(
        SECRET_UNREADABLE,
        format!(
            "Seaquel couldn't read the saved {what} of connection {name:?} from {} ({}). \
             {}, or open the connection in the Seaquel app and save the {what} again.",
            store_name(),
            failed.code,
            store_allow_hint(),
        ),
    )
}

#[cfg(feature = "workspace")]
fn unavailable_error(name: &str, failed: &UnreadableSecret) -> CoreError {
    let what = secret_what(&failed.key);
    CoreError::new(
        SECRET_STORE_UNAVAILABLE,
        format!(
            "Seaquel couldn't reach {} to read the saved {what} of connection {name:?}: it \
             isn't available in this session.",
            store_name(),
        ),
    )
}

/// Secrets shorter than this aren't redacted (see [`redact`]).
#[cfg(feature = "workspace")]
const MIN_REDACTED_LEN: usize = 4;

/// `e` with every secret in `secrets` ([`Plan::secret_values`]), raw or
/// percent-encoded as it goes into a URL, replaced by `<redacted>`.
///
/// Secrets shorter than [`MIN_REDACTED_LEN`] characters are left alone: a
/// one- to three-character string matches ordinary text ("pw", "sa", "1"),
/// so replacing it would garble the message, and where the `<redacted>`
/// markers landed would itself give the secret away. Drivers don't echo
/// passwords; this is a second line of defence for real ones.
#[cfg(feature = "workspace")]
fn redact(mut e: CoreError, secrets: &[String]) -> CoreError {
    use seaquel_workspace::connection_string::encode_uri_component;
    // Longest first, so a secret that contains another is replaced whole.
    let mut secrets: Vec<&String> = secrets
        .iter()
        .filter(|s| s.chars().count() >= MIN_REDACTED_LEN)
        .collect();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in secrets {
        for form in [secret.clone(), encode_uri_component(secret)] {
            if e.message.contains(&form) {
                e.message = e.message.replace(&form, "<redacted>");
            }
        }
    }
    e
}

impl fmt::Debug for Workspace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Workspace");
        s.field("id", &self.id);
        s.field("data_dir", &self.data_dir);
        #[cfg(feature = "storage")]
        s.field("storage", &self.storage.path());
        #[cfg(feature = "secrets")]
        s.field("secrets", &self.secrets.as_ref().map(|_| "<store>"));
        s.finish()
    }
}

/// A Core failure outside a database connection, with the same shape as
/// `DbError`. Storage keeps its codes: `LEGACY_STORAGE`, `STORAGE_CORRUPT`,
/// `NO_DATA_DIR`, `STORAGE_ERROR`, and from a read-only open
/// `STORAGE_NEEDS_UPGRADE` and `STORAGE_NOT_FOUND`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    pub code: String,
    pub message: String,
    /// For `NAME_TAKEN` (phase 5d, Q3): the id of the row that has the name.
    pub taken_by: Option<String>,
}

impl CoreError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            taken_by: None,
        }
    }
}

impl From<seaquel_workspace::library::LibraryError> for CoreError {
    fn from(e: seaquel_workspace::library::LibraryError) -> Self {
        Self {
            code: e.code,
            message: e.message,
            taken_by: e.taken_by,
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CoreError {}

#[cfg(feature = "storage")]
impl From<seaquel_storage::StorageError> for CoreError {
    fn from(e: seaquel_storage::StorageError) -> Self {
        Self::new(e.code(), e.to_string())
    }
}

#[cfg(feature = "secrets")]
impl From<seaquel_secrets::SecretError> for CoreError {
    fn from(e: seaquel_secrets::SecretError) -> Self {
        Self::new(e.code(), e.to_string())
    }
}

#[cfg(all(test, feature = "workspace"))]
mod redact_tests {
    use super::*;

    fn secrets() -> Vec<String> {
        vec!["db p@ss%1".into(), "ssh/pw+x".into(), "key:phrase&".into()]
    }

    fn redacted(message: &str, secrets: &[String]) -> String {
        redact(CoreError::new("X", message), secrets).message
    }

    #[test]
    fn raw_and_url_encoded_secrets_are_redacted() {
        let s = secrets();
        for (raw, encoded) in [
            ("db p@ss%1", "db%20p%40ss%251"),
            ("ssh/pw+x", "ssh%2Fpw%2Bx"),
            ("key:phrase&", "key%3Aphrase%26"),
        ] {
            assert_eq!(
                redacted(&format!("failed near {raw} here"), &s),
                "failed near <redacted> here"
            );
            assert_eq!(
                redacted(&format!("postgres://u:{encoded}@h/db refused"), &s),
                "postgres://u:<redacted>@h/db refused"
            );
        }
        assert_eq!(redacted("nothing secret", &s), "nothing secret");
    }

    #[test]
    fn short_secrets_are_left_alone() {
        let s = vec!["sa".to_string(), "abc".to_string()];
        assert_eq!(
            redacted("login failed for user sa (abc)", &s),
            "login failed for user sa (abc)"
        );
        // Four characters is enough.
        let s = vec!["abcd".to_string()];
        assert_eq!(redacted("x abcd y", &s), "x <redacted> y");
    }

    #[test]
    fn a_connect_request_redacts_its_secrets() {
        let req =
            ConnectRequest::saved("c1").with_secrets(seaquel_types::connect::SuppliedSecrets {
                db: Some("hunter2-db".into()),
                ssh: Some("hunter2-ssh".into()),
                ssh_key: Some("hunter2-key".into()),
            });
        let debug = format!("{req:?} {req:#?}");
        assert!(!debug.contains("hunter2"), "{debug}");

        let form: seaquel_types::connect::ConnectionForm =
            serde_json::from_value(serde_json::json!({
                "type": "postgres",
                "connectionString": "postgres://alice:hunter2-in-string@db.example.com/app",
            }))
            .unwrap();
        let req = ConnectRequest::form(form);
        let debug = format!("{req:?} {req:#?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(
            debug.contains("postgres://alice@db.example.com/app"),
            "{debug}"
        );
    }
}
