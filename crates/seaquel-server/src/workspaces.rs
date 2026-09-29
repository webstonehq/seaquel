//! The web server's per-user workspaces.
//!
//! Each user's metadata lives at `DATA_DIR/users/<id>/meta.db`, the file the
//! Node server used before this service took storage over (phase 3). The
//! user comes from the `X-Seaquel-User` header, which only the Node process
//! sets (see `main.rs` for why that makes the loopback bind a requirement).
//!
//! [`Workspaces`] keeps an LRU of open workspaces:
//!
//! - **Opened once.** Each user's slot holds a `OnceCell`, so two first
//!   requests for one user that arrive together open the file once; the
//!   second waits for the first.
//! - **Small pools.** Each workspace's pool has at most 2 connections and
//!   closes idle ones after 60 s, so idle users don't hold file handles.
//! - **Eviction never breaks a request.** A request holds an
//!   `Arc<OpenWorkspace>` for as long as it runs. Evicting only drops the
//!   LRU's reference; `Workspace::close()` runs when the last reference goes,
//!   so a request that was in flight when its workspace was evicted finishes
//!   on it. A request for that user after the eviction opens a fresh
//!   workspace on the same file, which SQLite's WAL and `busy_timeout` allow.
//! - **Eviction closes the user's databases** (phase 5a, answered question
//!   2). The evicted workspace's `Workspace::close_all` runs in the
//!   background: it cancels its streams, closes its connections and their
//!   SSH tunnels, and emits `WORKSPACE_EVICTED` for each connection, which
//!   reaches every open `/rpc/stream` socket of that user (see
//!   [`Workspaces::listen`]). A request still holding the workspace ends
//!   cleanly: storage keeps working until it lets go, a database call gets
//!   `CONNECTION_NOT_FOUND`, a connect `WORKSPACE_CLOSED`, and a stream ends.
//!   The cap is hard: [`DEFAULT_CAPACITY`], lowered with
//!   `SEAQUEL_WORKSPACE_CAP` ([`CAPACITY_ENV`]) for tests and manual checks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::storage::StorageOptions;
use seaquel_core::{Core, CoreError, Workspace, WorkspaceSpec};
use seaquel_rpc::CoreEvent;
use tokio::sync::{mpsc, oneshot, OnceCell};

/// The env var naming the data root. Node reads the same one (`auth.ts`, for
/// `auth.db`) and `server.js` passes its whole environment to this process,
/// so both agree.
pub const DATA_DIR_ENV: &str = "DATA_DIR";

/// A web user's metadata file name inside `DATA_DIR/users/<id>/`.
pub const USER_STORAGE_FILE: &str = "meta.db";

/// How many users' workspaces stay open at once. A hard cap: the least
/// recently used one is evicted (and its connections closed) to make room.
pub const DEFAULT_CAPACITY: usize = 1024;

/// Lowers the cap, for tests and manual checks (`SEAQUEL_WORKSPACE_CAP=2`).
/// Read only by this server, clamped to `1..=`[`DEFAULT_CAPACITY`]; a value
/// that isn't a number is ignored.
pub const CAPACITY_ENV: &str = "SEAQUEL_WORKSPACE_CAP";

/// The cap from `value` (the [`CAPACITY_ENV`] variable): clamped to
/// `1..=`[`DEFAULT_CAPACITY`], or `None` when it isn't a whole number.
pub fn capacity_from(value: &str) -> Option<usize> {
    let n: u64 = value.trim().parse().ok()?;
    Some(
        usize::try_from(n)
            .unwrap_or(usize::MAX)
            .clamp(1, DEFAULT_CAPACITY),
    )
}

/// Each workspace's pool: at most 2 connections, idle ones closed after 60 s.
pub fn user_storage_options() -> StorageOptions {
    StorageOptions {
        max_connections: 2,
        idle_timeout: Some(Duration::from_secs(60)),
        read_only: false,
    }
}

/// The data root: `$DATA_DIR`, or the current directory when it's unset,
/// as in Node (`process.env.DATA_DIR ?? process.cwd()`). `server.js` spawns
/// this process without changing its directory, so the two cwds agree.
///
/// An empty `DATA_DIR` counts as unset here. Node would use `""` and put the
/// files under `/users`, which is never what anyone meant.
pub fn data_dir_from_env() -> PathBuf {
    match std::env::var_os(DATA_DIR_ENV) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

/// Why a user id was refused. The messages never repeat the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidUserId {
    Empty,
    Unsafe,
}

impl InvalidUserId {
    pub fn message(self) -> &'static str {
        match self {
            InvalidUserId::Empty => "the X-Seaquel-User header is empty",
            InvalidUserId::Unsafe => "the X-Seaquel-User header isn't a safe user id",
        }
    }
}

/// The rule the Node server's storage module used (now deleted): non-empty,
/// with no `/`, `\` or `..`. Better Auth's ids are URL-safe, so this only refuses a caller
/// trying to leave its own directory. Leading or trailing whitespace is
/// refused (`"u1 "` would be a second directory for the same user), and so
/// is a lone `.`: it passes that rule but would put the file at
/// `DATA_DIR/users/meta.db`.
pub fn validate_user_id(id: &str) -> Result<(), InvalidUserId> {
    if id.is_empty() {
        return Err(InvalidUserId::Empty);
    }
    if id == "." || id.trim() != id || id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(InvalidUserId::Unsafe);
    }
    Ok(())
}

/// A workspace the LRU opened. Requests hold it through an `Arc`; the last
/// one dropped closes the workspace.
pub struct OpenWorkspace {
    ws: Arc<Workspace>,
    stats: Arc<Stats>,
}

impl OpenWorkspace {
    pub fn workspace(&self) -> &Workspace {
        &self.ws
    }
}

impl Drop for OpenWorkspace {
    fn drop(&mut self) {
        let ws = Arc::clone(&self.ws);
        let stats = Arc::clone(&self.stats);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    ws.close().await;
                    stats.closed.fetch_add(1, Ordering::SeqCst);
                });
            }
            // No runtime (process shutdown): dropping the pool closes its
            // connections without the graceful close.
            Err(_) => {
                stats.closed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
}

#[derive(Default)]
struct Stats {
    opened: AtomicUsize,
    closed: AtomicUsize,
    evicted: AtomicUsize,
}

/// How many events one listener (socket) may have waiting before it counts
/// as lagging: past it the hub drops the listener, and its socket closes
/// with 1013 and [`EVENTS_LAGGED`] so the client reconnects and reloads
/// (phase 5d review, I1). A socket that keeps up never gets near it.
pub const LISTENER_EVENT_BOUND: usize = 1024;

/// How many bytes of events (as [`event_bytes`] counts them) one listener
/// may have waiting before it counts as lagging, like
/// [`LISTENER_EVENT_BOUND`] (phase 5d-1 probe fix, c6). Core keeps each
/// event small (at most 16 KiB of ids, a scope of at most 1 KiB), so this
/// is the backstop: a user's sockets together hold at most
/// [`MAX_LISTENERS_PER_USER`] × 8 MiB.
pub const LISTENER_EVENT_BYTE_BOUND: usize = 8 * 1024 * 1024;

/// The close reason (after code 1013) of a socket whose listener lagged.
pub const EVENTS_LAGGED: &str = "EVENTS_LAGGED";

/// What one waiting event counts for against
/// [`LISTENER_EVENT_BYTE_BOUND`]: its JSON length (what the socket will
/// send) plus a fixed overhead for the allocations behind it.
pub fn event_bytes(event: &CoreEvent) -> usize {
    const OVERHEAD: usize = 128;
    let json = match event {
        // The only events the hub carries; counted without serialising.
        CoreEvent::StorageChanged {
            scope,
            ids,
            origin,
            seq,
            ..
        } => {
            64 + scope.as_ref().map_or(0, String::len)
                + origin.as_ref().map_or(0, String::len)
                + seq.epoch.len()
                + ids
                    .as_ref()
                    .map_or(0, |ids| ids.iter().map(|id| id.len() + 3).sum())
        }
        CoreEvent::ConnectionClosed {
            connection_id,
            code,
            message,
        } => 64 + connection_id.len() + code.len() + message.len(),
        other => serde_json::to_vec(other).map_or(0, |v| v.len()),
    };
    json + OVERHEAD
}

/// One listener as the hub holds it: its bounded channel, the bytes it has
/// waiting (shared with its [`Listener`], which subtracts what it reads),
/// and the lag signal, which fires (the receiver sees the sender dropped)
/// when the hub drops the listener.
struct HubEntry {
    tx: mpsc::Sender<CoreEvent>,
    queued: Arc<AtomicUsize>,
    _lagged: oneshot::Sender<()>,
}

/// Each user's event listeners (their open `/rpc/stream` sockets).
struct Hub {
    listeners: Mutex<HashMap<String, Vec<HubEntry>>>,
    bound: usize,
    byte_bound: usize,
}

impl Default for Hub {
    fn default() -> Self {
        Self::with_bounds(LISTENER_EVENT_BOUND, LISTENER_EVENT_BYTE_BOUND)
    }
}

impl Hub {
    fn with_bounds(bound: usize, byte_bound: usize) -> Self {
        Self {
            listeners: Mutex::default(),
            bound: bound.max(1),
            byte_bound: byte_bound.max(1),
        }
    }

    /// Hand `event` to each of `user_id`'s listeners without waiting. One
    /// that's gone, or whose channel is full or would go past its byte
    /// bound (a slow client), is dropped: that fires its lag signal.
    fn send(&self, user_id: &str, event: &CoreEvent) {
        let size = event_bytes(event);
        let mut listeners = self
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(entries) = listeners.get_mut(user_id) {
            let byte_bound = self.byte_bound;
            entries.retain(|entry| {
                // Counted before the send, so the reader can never
                // subtract an event's bytes before they were added.
                let before = entry.queued.fetch_add(size, Ordering::SeqCst);
                if before + size > byte_bound {
                    entry.queued.fetch_sub(size, Ordering::SeqCst);
                    if entry.tx.is_closed() {
                        return false;
                    }
                    log::warn!(activity = "rpc.stream", code = EVENTS_LAGGED; "A socket fell behind on events; closing it");
                    return false;
                }
                match entry.tx.try_send(event.clone()) {
                    Ok(()) => true,
                    Err(e) => {
                        entry.queued.fetch_sub(size, Ordering::SeqCst);
                        if let mpsc::error::TrySendError::Full(_) = e {
                            log::warn!(activity = "rpc.stream", code = EVENTS_LAGGED; "A socket fell behind on events; closing it");
                        }
                        false
                    }
                }
            });
            if entries.is_empty() {
                listeners.remove(user_id);
            }
        }
    }

    fn prune(&self, user_id: &str) {
        let mut listeners = self
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(entries) = listeners.get_mut(user_id) {
            entries.retain(|entry| !entry.tx.is_closed());
            if entries.is_empty() {
                listeners.remove(user_id);
            }
        }
    }
}

/// One user's workspace events (`connectionClosed`, `storageChanged`), from
/// whichever of their workspaces is open (see [`Workspaces::listen`]).
/// Holds at most [`LISTENER_EVENT_BOUND`] of them, and at most
/// [`LISTENER_EVENT_BYTE_BOUND`] bytes; dropping it unsubscribes.
pub struct Listener {
    rx: mpsc::Receiver<CoreEvent>,
    queued: Arc<AtomicUsize>,
    lagged: Option<oneshot::Receiver<()>>,
    user_id: String,
    hub: Arc<Hub>,
}

impl Listener {
    /// The next event; `None` once the hub dropped this listener and its
    /// waiting events are all taken.
    pub async fn recv(&mut self) -> Option<CoreEvent> {
        let event = self.rx.recv().await?;
        self.queued.fetch_sub(event_bytes(&event), Ordering::SeqCst);
        Some(event)
    }

    /// The signal that the hub dropped this listener because it fell
    /// [`LISTENER_EVENT_BOUND`] events (or [`LISTENER_EVENT_BYTE_BOUND`]
    /// bytes) behind, held apart from the listener so a loop can wait on
    /// both. A second call gets one that never fires.
    pub fn lag_signal(&mut self) -> LagSignal {
        LagSignal(self.lagged.take())
    }
}

/// See [`Listener::lag_signal`].
pub struct LagSignal(Option<oneshot::Receiver<()>>);

impl LagSignal {
    /// Resolves once the listener was dropped as lagging. Cancel-safe.
    pub async fn fired(&mut self) {
        match &mut self.0 {
            Some(rx) => {
                let _ = rx.await;
            }
            None => std::future::pending().await,
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.rx.close();
        self.hub.prune(&self.user_id);
    }
}

type Cell = Arc<OnceCell<Arc<OpenWorkspace>>>;

struct Slot {
    cell: Cell,
    last_used: u64,
}

#[derive(Default)]
struct Lru {
    slots: HashMap<String, Slot>,
    clock: u64,
}

/// What one user's `/rpc` calls in flight hold (see
/// [`Workspaces::begin_call`]).
#[derive(Default)]
struct UserCalls {
    /// Their bodies' bytes, together.
    bytes: usize,
    /// The edit calls among them ([`CallSlot::begin_edit`]).
    edits: usize,
}

/// Each user's `/rpc` calls in flight. Kept apart from the LRU: an
/// eviction doesn't end the calls, so it mustn't forget them either.
#[derive(Default)]
struct InFlight {
    per_user: Mutex<HashMap<String, UserCalls>>,
}

/// One call's share of its user's in-flight budget, released when dropped
/// (the call finished, failed or was dropped with its request).
pub struct CallSlot {
    in_flight: Arc<InFlight>,
    user_id: String,
    bytes: usize,
    edit: bool,
}

impl CallSlot {
    /// Count this call as an edit call too, unless `max` of the user's are
    /// running already (then `false`, and nothing changes).
    pub fn begin_edit(&mut self, max: usize) -> bool {
        if self.edit {
            return true;
        }
        let mut per_user = self
            .in_flight
            .per_user
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(calls) = per_user.get_mut(&self.user_id) else {
            return false;
        };
        if calls.edits >= max {
            return false;
        }
        calls.edits += 1;
        self.edit = true;
        true
    }
}

impl Drop for CallSlot {
    fn drop(&mut self) {
        let mut per_user = self
            .in_flight
            .per_user
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(calls) = per_user.get_mut(&self.user_id) {
            calls.bytes = calls.bytes.saturating_sub(self.bytes);
            if self.edit {
                calls.edits = calls.edits.saturating_sub(1);
            }
            if calls.bytes == 0 && calls.edits == 0 {
                per_user.remove(&self.user_id);
            }
        }
    }
}

/// The open workspaces, keyed by user id, at most `capacity` of them.
pub struct Workspaces {
    root: PathBuf,
    capacity: usize,
    options: StorageOptions,
    lru: Mutex<Lru>,
    stats: Arc<Stats>,
    hub: Arc<Hub>,
    in_flight: Arc<InFlight>,
}

impl Workspaces {
    /// Workspaces under `root` (the data root; files go at
    /// `root/users/<id>/meta.db`), with [`DEFAULT_CAPACITY`] and
    /// [`user_storage_options`].
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_capacity(root, DEFAULT_CAPACITY)
    }

    /// These workspaces with another [`LISTENER_EVENT_BOUND`] (at least 1),
    /// for tests of a lagging socket. Call it before anyone listens.
    #[must_use]
    pub fn with_event_bound(mut self, bound: usize) -> Self {
        self.hub = Arc::new(Hub::with_bounds(bound, self.hub.byte_bound));
        self
    }

    /// These workspaces with another [`LISTENER_EVENT_BYTE_BOUND`] (at least
    /// 1), for tests of a lagging socket. Call it before anyone listens.
    #[must_use]
    pub fn with_event_byte_bound(mut self, bytes: usize) -> Self {
        self.hub = Arc::new(Hub::with_bounds(self.hub.bound, bytes));
        self
    }

    /// Like [`Workspaces::new`] with another cap (at least 1). Tests use 2.
    pub fn with_capacity(root: impl Into<PathBuf>, capacity: usize) -> Self {
        Self {
            root: root.into(),
            capacity: capacity.max(1),
            options: user_storage_options(),
            lru: Mutex::new(Lru::default()),
            stats: Arc::default(),
            hub: Arc::default(),
            in_flight: Arc::default(),
        }
    }

    /// Workspaces on [`data_dir_from_env`], capped at [`CAPACITY_ENV`] when
    /// it's set (see there), else [`DEFAULT_CAPACITY`].
    pub fn from_env() -> Self {
        let capacity = match std::env::var(CAPACITY_ENV) {
            Ok(value) => capacity_from(&value).unwrap_or_else(|| {
                log::warn!("{CAPACITY_ENV} isn't a whole number; using {DEFAULT_CAPACITY}");
                DEFAULT_CAPACITY
            }),
            Err(_) => DEFAULT_CAPACITY,
        };
        Self::with_capacity(data_dir_from_env(), capacity)
    }

    /// The most users whose workspaces stay open at once.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Listen for `user_id`'s [`CoreEvent::ConnectionClosed`] events (a
    /// `/rpc/stream` socket holds one): those of the workspace open now and
    /// of any opened later for the user, so an eviction reaches a socket
    /// that was opened before it.
    ///
    /// At most [`MAX_LISTENERS_PER_USER`] at once per user:
    /// [`ListenError::TooMany`] beyond that.
    pub fn listen(&self, user_id: &str) -> Result<Listener, ListenError> {
        validate_user_id(user_id).map_err(ListenError::InvalidUser)?;
        let (tx, rx) = mpsc::channel(self.hub.bound);
        let (lag_tx, lagged) = oneshot::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let mut listeners = self
            .hub
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let entries = listeners.entry(user_id.to_string()).or_default();
        entries.retain(|entry| !entry.tx.is_closed());
        if entries.len() >= MAX_LISTENERS_PER_USER {
            return Err(ListenError::TooMany);
        }
        entries.push(HubEntry {
            tx,
            queued: Arc::clone(&queued),
            _lagged: lag_tx,
        });
        Ok(Listener {
            rx,
            queued,
            lagged: Some(lagged),
            user_id: user_id.to_string(),
            hub: Arc::clone(&self.hub),
        })
    }

    /// How many event listeners (open `/rpc/stream` sockets) `user_id`
    /// holds now, for tests and diagnostics.
    pub fn listener_count(&self, user_id: &str) -> usize {
        let mut listeners = self
            .hub
            .listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        listeners.get_mut(user_id).map_or(0, |entries| {
            entries.retain(|entry| !entry.tx.is_closed());
            entries.len()
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A slot for a call of `user_id`'s whose body is `bytes` long, or
    /// `None` when their calls in flight already hold bodies that, with
    /// this one, pass `budget`. A lone call always gets one, whatever its
    /// size, and so does a body under `SMALL_CALL_BYTES` (it still counts).
    /// Hold it for the whole call.
    pub fn begin_call(&self, user_id: &str, bytes: usize, budget: usize) -> Option<CallSlot> {
        let mut per_user = self
            .in_flight
            .per_user
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let calls = per_user.entry(user_id.to_string()).or_default();
        if bytes >= crate::routes::rpc::SMALL_CALL_BYTES
            && calls.bytes > 0
            && calls.bytes.saturating_add(bytes) > budget
        {
            return None;
        }
        // Counted even when empty, so an empty body still holds its user's
        // entry (and an edit slot) until it ends.
        calls.bytes = calls.bytes.saturating_add(bytes.max(1));
        Some(CallSlot {
            in_flight: Arc::clone(&self.in_flight),
            user_id: user_id.to_string(),
            bytes: bytes.max(1),
            edit: false,
        })
    }

    /// How many users have a call in flight (tests).
    #[doc(hidden)]
    pub fn users_with_calls_in_flight(&self) -> usize {
        self.in_flight
            .per_user
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// `root/users/<id>`, the directory a user's workspace opens on.
    pub fn user_dir(&self, user_id: &str) -> Result<PathBuf, InvalidUserId> {
        validate_user_id(user_id)?;
        Ok(self.root.join("users").join(user_id))
    }

    /// The user's workspace, opening it (and evicting the least recently
    /// used one when full, which closes its connections in the background)
    /// if it isn't open. Hold the result for the whole request.
    pub async fn get(
        &self,
        core: &Arc<Core>,
        user_id: &str,
    ) -> Result<Arc<OpenWorkspace>, GetError> {
        let dir = self.user_dir(user_id).map_err(GetError::InvalidUser)?;

        let (cell, evicted) = {
            let mut lru = self.lru.lock().unwrap_or_else(PoisonError::into_inner);
            lru.clock += 1;
            let now = lru.clock;
            if let Some(slot) = lru.slots.get_mut(user_id) {
                slot.last_used = now;
                (Arc::clone(&slot.cell), None)
            } else {
                let evicted = if lru.slots.len() >= self.capacity {
                    let oldest = lru
                        .slots
                        .iter()
                        .min_by_key(|(_, slot)| slot.last_used)
                        .map(|(id, _)| id.clone());
                    oldest.and_then(|id| lru.slots.remove(&id))
                } else {
                    None
                };
                let cell: Cell = Arc::default();
                lru.slots.insert(
                    user_id.to_string(),
                    Slot {
                        cell: Arc::clone(&cell),
                        last_used: now,
                    },
                );
                (cell, evicted)
            }
        };
        // Outside the lock.
        if let Some(slot) = evicted {
            self.close_evicted(core, slot.cell);
        }

        let opened = cell
            .get_or_try_init(|| async {
                let spec = WorkspaceSpec::new(dir)
                    .with_storage_file(USER_STORAGE_FILE)
                    .with_storage_options(self.options.clone());
                let ws = core.open_workspace(spec).await?;
                self.stats.opened.fetch_add(1, Ordering::SeqCst);
                self.forward_events(&ws, user_id);
                Ok::<_, CoreError>(Arc::new(OpenWorkspace {
                    ws,
                    stats: Arc::clone(&self.stats),
                }))
            })
            .await;

        match opened {
            Ok(open) => {
                let open = Arc::clone(open);
                // Evicted while it opened: the evicting request's close may
                // have run before this cell was set, so close it here too
                // (`close_all` is idempotent). Nothing opened outside the
                // LRU is left with open connections.
                let in_lru = self
                    .lru
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .slots
                    .get(user_id)
                    .is_some_and(|slot| Arc::ptr_eq(&slot.cell, &cell));
                // No deterministic test: the eviction has to land between
                // this cell's open starting and finishing, and the test has
                // no hook to pause an open there (`close_evicted`, which
                // waits for the open, covers the same case from the other
                // side). `close_all` is idempotent, so a double close is fine.
                if !in_lru {
                    open.ws.close_all(core).await;
                }
                Ok(open)
            }
            Err(e) => {
                // Don't keep a slot for a file that failed to open; the next
                // request tries again.
                let mut lru = self.lru.lock().unwrap_or_else(PoisonError::into_inner);
                if lru
                    .slots
                    .get(user_id)
                    .is_some_and(|slot| Arc::ptr_eq(&slot.cell, &cell))
                {
                    lru.slots.remove(user_id);
                }
                Err(GetError::Open(e))
            }
        }
    }

    /// Pass `ws`'s events to `user_id`'s listeners until `ws` is dropped
    /// (its event stream ends then). Subscribed before anyone can use it, so
    /// no event is missed.
    fn forward_events(&self, ws: &Workspace, user_id: &str) {
        let mut events = seaquel_rpc::workspace_events(ws);
        let hub = Arc::clone(&self.hub);
        let user_id = user_id.to_string();
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                hub.send(&user_id, &event);
            }
        });
    }

    /// Close everything an evicted workspace owns (`Workspace::close_all`)
    /// in the background. Its storage closes when the last request holding
    /// it lets go. A workspace still opening when it was evicted is closed
    /// once it has opened; one that failed to open has nothing to close.
    fn close_evicted(&self, core: &Arc<Core>, cell: Cell) {
        let core = Arc::clone(core);
        let stats = Arc::clone(&self.stats);
        tokio::spawn(async move {
            let open = match cell.get() {
                Some(open) => Some(Arc::clone(open)),
                // Waits for an open in progress; otherwise fails at once
                // without opening anything.
                None => cell
                    .get_or_try_init(|| async { Err(()) })
                    .await
                    .ok()
                    .cloned(),
            };
            drop(cell);
            if let Some(open) = open {
                open.ws.close_all(&core).await;
                stats.evicted.fetch_add(1, Ordering::SeqCst);
            }
        });
    }

    /// How many users have a slot in the LRU.
    pub fn len(&self) -> usize {
        self.lru
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .slots
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `user_id` has a slot in the LRU.
    pub fn contains(&self, user_id: &str) -> bool {
        self.lru
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .slots
            .contains_key(user_id)
    }

    /// Workspaces opened so far, for tests and diagnostics.
    pub fn opened(&self) -> usize {
        self.stats.opened.load(Ordering::SeqCst)
    }

    /// Workspaces closed so far (the close finished), for tests and
    /// diagnostics.
    pub fn closed(&self) -> usize {
        self.stats.closed.load(Ordering::SeqCst)
    }

    /// Evicted workspaces whose connections are closed
    /// (`Workspace::close_all` finished), for tests and diagnostics.
    pub fn evicted(&self) -> usize {
        self.stats.evicted.load(Ordering::SeqCst)
    }
}

/// How many event listeners (open `/rpc/stream` sockets) one user may
/// hold at once.
pub const MAX_LISTENERS_PER_USER: usize = 8;

/// Why [`Workspaces::listen`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenError {
    InvalidUser(InvalidUserId),
    /// The user already has [`MAX_LISTENERS_PER_USER`].
    TooMany,
}

/// Why [`Workspaces::get`] failed.
#[derive(Debug)]
pub enum GetError {
    InvalidUser(InvalidUserId),
    Open(CoreError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_ids_follow_the_storage_ts_rule() {
        for ok in [
            "tTWa8mP780qWiJrRHni8dww7Oz4xBY2y",
            "a",
            "a.b",
            "user-1_x",
            "é",
        ] {
            assert_eq!(validate_user_id(ok), Ok(()), "{ok}");
        }
        assert_eq!(validate_user_id(""), Err(InvalidUserId::Empty));
        for bad in [
            ".", "..", "a/b", "/abs", "a\\b", "..a", "a..", "a/../b", " u1", "u1 ", "\tu1", "u1\n",
            " ",
        ] {
            assert_eq!(validate_user_id(bad), Err(InvalidUserId::Unsafe), "{bad}");
        }
    }

    #[test]
    fn user_files_go_under_users() {
        let w = Workspaces::new("/data");
        assert_eq!(
            w.user_dir("u1").unwrap().join(USER_STORAGE_FILE),
            PathBuf::from("/data/users/u1/meta.db")
        );
        assert!(w.user_dir("../x").is_err());
    }

    fn changed(id_len: usize) -> CoreEvent {
        CoreEvent::StorageChanged {
            kind: seaquel_core::StoredKind::Storage,
            scope: None,
            ids: Some(vec!["k".repeat(id_len)]),
            origin: None,
            seq: seaquel_core::ChangeSeq {
                epoch: "e".into(),
                n: 1,
            },
        }
    }

    /// Phase 5d-1 probe fix: a listener's queue is bounded by bytes as well
    /// as by count, and reading an event frees its bytes.
    #[tokio::test]
    async fn a_listener_past_its_byte_bound_is_dropped_as_lagging() {
        let w = Workspaces::new("/d")
            .with_event_bound(1_000)
            .with_event_byte_bound(10_000);
        let mut kept = w.listen("u").unwrap();
        let mut slow = w.listen("u").unwrap();
        let mut slow_lag = slow.lag_signal();
        let event = changed(1_000);
        let size = event_bytes(&event);
        assert!(size > 1_000 && size < 2_000, "{size}");
        // `kept` reads every event, so its bytes never pile up.
        for _ in 0..20 {
            w.hub.send("u", &event);
            assert!(kept.recv().await.is_some());
        }
        // `slow` read none: it went past 10,000 bytes long before 1,000
        // events.
        assert_eq!(w.listener_count("u"), 1);
        tokio::time::timeout(std::time::Duration::from_secs(5), slow_lag.fired())
            .await
            .expect("the lag signal fired");
        let mut queued = 0;
        while slow.recv().await.is_some() {
            queued += 1;
        }
        assert_eq!(queued, 10_000 / size, "it held what fit, no more");
        // `kept` still gets events.
        w.hub.send("u", &event);
        assert!(kept.recv().await.is_some());
    }

    #[test]
    fn capacity_is_at_least_one() {
        assert_eq!(Workspaces::with_capacity("/d", 0).capacity, 1);
    }

    #[test]
    fn the_capacity_override_only_lowers_the_cap() {
        assert_eq!(capacity_from("2"), Some(2));
        assert_eq!(capacity_from(" 16 "), Some(16));
        assert_eq!(capacity_from("0"), Some(1));
        assert_eq!(capacity_from("1024"), Some(DEFAULT_CAPACITY));
        assert_eq!(capacity_from("100000"), Some(DEFAULT_CAPACITY));
        assert_eq!(capacity_from("99999999999999999999999"), None);
        for bad in ["", "two", "-1", "1.5", "2k"] {
            assert_eq!(capacity_from(bad), None, "{bad}");
        }
    }
}
