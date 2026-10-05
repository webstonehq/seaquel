//! Connections lost without being asked to close (the desktop DuckDB
//! helper plan, Decision 7).
//!
//! A driver that can tell ([`seaquel_engine::Driver::closed`]: the remote
//! DuckDB driver, whose helper process can die) gets a watcher on Core's
//! executor when it connects. When the driver reports a loss, the watcher
//! takes the connection out of Core (its streams are cancelled as closed,
//! its SSH tunnel closed) and announces it on the owning workspace's events
//! as `ConnectionClosed` with [`CONNECTION_CLOSED`] and the driver's
//! message, once. A connection already taken out by a `disconnect`, a
//! replace or `close_all` isn't announced again: whoever takes it out of
//! the map first owns its ending. A driver that ends as asked (`close`, or
//! dropped) resolves its future with `None`, which ends the watcher with
//! nothing done.
//!
//! The watcher holds only weak references to Core's maps and the
//! workspace's receivers, and never the driver, so it keeps nothing alive:
//! dropping Core drops the drivers, whose futures then end.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock, Weak};

use crate::workspace::{emit_to, EventSink, WorkspaceEvent, CONNECTION_CLOSED};
use crate::{cancel_streams_in, Connection, Core, StreamTokens};

/// What the watcher needs to take a lost connection out of Core.
struct Reaper {
    connections: Weak<RwLock<HashMap<String, Connection>>>,
    streams: Weak<StreamTokens>,
    #[cfg(feature = "ssh")]
    tunnels: crate::ssh::LostTunnels,
}

impl Reaper {
    /// Takes `connection_id` out, cancelling its streams and closing its
    /// tunnel. `false` when it was already gone (or Core is).
    fn take_out(&self, connection_id: &str) -> bool {
        let Some(connections) = self.connections.upgrade() else {
            return false;
        };
        let removed = connections
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(connection_id);
        let Some(connection) = removed else {
            return false;
        };
        if let Some(streams) = self.streams.upgrade() {
            cancel_streams_in(&streams, connection_id);
        }
        #[cfg(feature = "ssh")]
        self.tunnels.drop_tunnel_of(connection_id);
        // The driver goes with the last call that still holds it.
        drop(connection);
        true
    }
}

impl Core {
    /// Watch `connection_id` for a loss its driver reports, announcing it
    /// on `sink` (its workspace's receivers; `None` for a connection no
    /// workspace owns). Nothing to do for a driver that can't tell, or
    /// without an executor to run the watcher on.
    pub(crate) fn watch_lost(&self, connection_id: &str, sink: Option<EventSink>) {
        let closed = self
            .connections
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(connection_id)
            .and_then(|c| c.driver.closed());
        let Some(closed) = closed else {
            return;
        };
        let Some(executor) = &self.executor else {
            log::debug!(activity = "db.lost", connection_id = connection_id; "No executor: a lost connection isn't watched");
            return;
        };
        let reaper = Reaper {
            connections: Arc::downgrade(&self.connections),
            streams: Arc::downgrade(&self.streams),
            #[cfg(feature = "ssh")]
            tunnels: self.tunnels.lost_handles(),
        };
        let id = connection_id.to_string();
        executor.spawn(Box::pin(async move {
            let Some(error) = closed.await else {
                return;
            };
            if !reaper.take_out(&id) {
                return;
            }
            log::warn!(activity = "db.lost", connection_id = id.as_str(), code = error.code.as_str(); "A connection was lost");
            if let Some(subscribers) = sink.as_ref().and_then(Weak::upgrade) {
                emit_to(
                    &subscribers,
                    WorkspaceEvent::ConnectionClosed {
                        connection_id: id,
                        code: CONNECTION_CLOSED.to_string(),
                        message: error.message,
                    },
                );
            }
        }));
    }
}
