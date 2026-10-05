//! The `ui` group of the workspace RPC (phase 5d-2):
//! each window's view state of a project (its open tabs with their text,
//! pane layout, active ids and active connection) and its active project,
//! through Core (`Workspace::load_window_state`, …).
//!
//! Wire shape, like the other groups:
//!
//! ```json
//! {"method":"ui","params":{"method":"windowStateSave","params":{"windowId":"main","projectId":"p","rev":4,"state":{…}}}}
//! {"method":"ui","result":{"method":"windowStateSave","result":{"value":{"stale":false,"rev":4},"seq":{"epoch":"…","n":9}}}}
//! ```
//!
//! **A call names its own window only.** `windowId` must equal the call's
//! [`WriteOrigin`] (the desktop's webview label, the web's checked
//! `X-Seaquel-Origin`), or Core refuses it with `INVALID_ARGUMENT`, so one
//! window or tab can't read or overwrite another's view. A call without an
//! origin names no window and is refused too.
//!
//! The state crosses as the text that came in (`RawValue`) and is stored
//! byte for byte; `rev` is a whole number (`-1`, `1.5` or `1e300` don't
//! parse). `Debug` shows the method only: never tab text or a window id.
//! Without the `storage` feature every method answers `NOT_SUPPORTED`.

use std::fmt;

use seaquel_core::domain::state::{WindowActive, WindowStateLoaded, WindowStateSaved};
use seaquel_core::{Core, Seqd, Workspace, WriteOrigin};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::workspace::RpcError;

/// A `ui` call. `windowId` must be the caller's origin.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum UiRequest {
    /// The window's active project: its own, else the most recently used
    /// window's, else `lastActiveProjectId`. Writes nothing.
    WindowGet { window_id: String },
    /// Make `projectId` the window's active project (and
    /// `lastActiveProjectId`).
    WindowActivate {
        window_id: String,
        project_id: String,
    },
    /// The window's view state of a project; a first load copies the most
    /// recently used window's, else today's rows, else nothing.
    WindowStateLoad {
        window_id: String,
        project_id: String,
    },
    /// Replace the window's view state when `rev` is higher than the
    /// stored one (else `stale`, nothing written).
    WindowStateSave {
        window_id: String,
        project_id: String,
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        rev: u64,
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        state: Box<RawValue>,
    },
}

/// The method only: a view state holds tab text, and a window id is the
/// caller's origin, which nothing logs.
impl fmt::Debug for UiRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiRequest")
            .field("method", &self.method())
            .finish_non_exhaustive()
    }
}

impl UiRequest {
    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            UiRequest::WindowGet { .. } => "windowGet",
            UiRequest::WindowActivate { .. } => "windowActivate",
            UiRequest::WindowStateLoad { .. } => "windowStateLoad",
            UiRequest::WindowStateSave { .. } => "windowStateSave",
        }
    }
}

/// A `ui` call's result, as `{"method": …, "result": {value, seq}}`.
/// `Debug` shows the method and sequence only.
// Not `Deserialize`: results only go out.
#[derive(Serialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum UiResponse {
    WindowGet(Seqd<WindowActive>),
    WindowActivate(Seqd<()>),
    WindowStateLoad(Seqd<WindowStateLoaded>),
    WindowStateSave(Seqd<WindowStateSaved>),
}

impl fmt::Debug for UiResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (method, seq) = match self {
            UiResponse::WindowGet(r) => ("windowGet", &r.seq),
            UiResponse::WindowActivate(r) => ("windowActivate", &r.seq),
            UiResponse::WindowStateLoad(r) => ("windowStateLoad", &r.seq),
            UiResponse::WindowStateSave(r) => ("windowStateSave", &r.seq),
        };
        f.debug_struct("UiResponse")
            .field("method", &method)
            .field("seq", seq)
            .finish_non_exhaustive()
    }
}

#[cfg(not(feature = "storage"))]
pub(crate) async fn ui(
    _: &Core,
    _: &Workspace,
    _: UiRequest,
    _: &WriteOrigin,
) -> Result<UiResponse, RpcError> {
    Err(RpcError::not_supported("Window state"))
}

/// Serve a `ui` call on `ws` for the window `origin` names.
#[cfg(feature = "storage")]
pub(crate) async fn ui(
    core: &Core,
    ws: &Workspace,
    req: UiRequest,
    origin: &WriteOrigin,
) -> Result<UiResponse, RpcError> {
    use UiRequest as Q;
    use UiResponse as R;

    Ok(match req {
        Q::WindowGet { window_id } => R::WindowGet(ws.get_window(origin, &window_id).await?),
        Q::WindowActivate {
            window_id,
            project_id,
        } => R::WindowActivate(
            ws.activate_window(core, origin, &window_id, &project_id)
                .await?,
        ),
        Q::WindowStateLoad {
            window_id,
            project_id,
        } => R::WindowStateLoad(
            ws.load_window_state(core, origin, &window_id, &project_id)
                .await?,
        ),
        Q::WindowStateSave {
            window_id,
            project_id,
            rev,
            state,
        } => R::WindowStateSave(
            ws.save_window_state(core, origin, &window_id, &project_id, rev, state)
                .await?,
        ),
    })
}
