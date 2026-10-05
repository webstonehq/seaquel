//! The library's hooks into the shared projection (phase 5e):
//! what `library.rs` and `state.rs` call after a write that may
//! change a shared row's file, and around a removal. With Core's `git`
//! feature they run `shared.rs`; without it (the web server, the wasm
//! build) they do nothing, and so does a Core built without
//! [`crate::LocalFiles`].

#[cfg(not(all(feature = "git", feature = "storage")))]
use seaquel_workspace::shared::{Kind, PublishOutcome};

#[cfg(not(all(feature = "git", feature = "storage")))]
use crate::changes::WriteOrigin;
#[cfg(not(all(feature = "git", feature = "storage")))]
use crate::{Core, Workspace};

/// The row a library call changed, by id. The publish reads it again under
/// the repo lock, so a racing write is never overwritten by
/// an older copy.
#[derive(Debug, Clone, Copy)]
// Without `git` the hooks do nothing, so nothing reads the fields.
#[cfg_attr(not(all(feature = "git", feature = "storage")), allow(dead_code))]
pub(crate) enum Publish<'a> {
    /// `renamed`: this call changed the name (by `name_key`) or the folder,
    /// so the file moves.
    Query {
        id: &'a str,
        renamed: bool,
    },
    Dashboard {
        id: &'a str,
        renamed: bool,
    },
    /// `shared_now`: the call turned `isLocalOnly` off, which shares it.
    Connection {
        id: &'a str,
        renamed: bool,
        shared_now: bool,
    },
    /// A linked project's `project.yaml`.
    Project,
}

/// Never made without the `git` feature.
#[cfg(not(all(feature = "git", feature = "storage")))]
pub(crate) struct Unpublish;

#[cfg(not(all(feature = "git", feature = "storage")))]
impl Workspace {
    pub(crate) async fn publish_row(
        &self,
        _core: &Core,
        _origin: &WriteOrigin,
        _project_id: &str,
        _what: Publish<'_>,
    ) -> Option<PublishOutcome> {
        None
    }

    pub(crate) async fn unpublish_begin(
        &self,
        _core: &Core,
        _kind: Kind,
        _id: &str,
        _keeps_row: bool,
    ) -> Result<Option<Unpublish>, crate::CoreError> {
        Ok(None)
    }

    pub(crate) async fn unpublish_end(
        &self,
        _core: &Core,
        _origin: &WriteOrigin,
        _pending: Option<Unpublish>,
        _written: bool,
    ) -> Option<PublishOutcome> {
        None
    }
}
