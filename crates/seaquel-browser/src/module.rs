//! The module's exports (wasm32 only). See the crate docs for the contract.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use futures::future::{AbortHandle, Abortable};
use futures::StreamExt;
use seaquel_core::{ConnectPolicy, Core, CoreError, Workspace, WorkspaceSpec};
use seaquel_engine_duckdb::{browser_engine, DuckDbBridge};
use seaquel_rpc::{
    dispatch_stream, dispatch_workspace, parse_request, workspace_events, RpcError, WriteOrigin,
};
use seaquel_runtime::WasmExecutor;
use wasm_bindgen::prelude::*;

/// The demo's window id (`src/lib/core/window-id.ts`): every write this
/// module makes carries it, so the page skips its own `storageChanged`
/// events (Decision 17).
const ORIGIN: &str = "demo";

/// Where the metadata file lives, as far as error messages go: it is in
/// memory (`WorkspaceSpec::with_image`).
const DATA_DIR: &str = "/demo";

/// One Core and its one workspace.
pub(crate) struct Open {
    pub(crate) core: Core,
    pub(crate) ws: Arc<Workspace>,
}

thread_local! {
    static OPEN: RefCell<Option<Rc<Open>>> = const { RefCell::new(None) };
    static SUBSCRIPTIONS: RefCell<HashMap<u32, AbortHandle>> = RefCell::new(HashMap::new());
    static NEXT_SUBSCRIPTION: Cell<u32> = const { Cell::new(0) };
    static ON_TRAP: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
}

pub(crate) fn origin() -> WriteOrigin {
    WriteOrigin::new(Some(ORIGIN))
}

/// An `RpcError` as the rejection value: its JSON text. A rejection that
/// isn't a string is a trap or a glue error, never an answer.
pub(crate) fn reject(e: RpcError) -> JsValue {
    let text = serde_json::to_string(&e).unwrap_or_else(|_| {
        r#"{"code":"INTERNAL_ERROR","message":"couldn't encode an error"}"#.to_string()
    });
    JsValue::from_str(&text)
}

fn not_open() -> JsValue {
    reject(RpcError::new(
        "NOT_OPEN",
        "Core isn't open in this page yet",
    ))
}

pub(crate) fn current_open() -> Result<Rc<Open>, JsValue> {
    OPEN.with(|o| o.borrow().clone()).ok_or_else(not_open)
}

fn encode<T: serde::Serialize>(value: &T) -> Result<String, JsValue> {
    serde_json::to_string(value).map_err(|e| {
        reject(RpcError::new(
            "INTERNAL_ERROR",
            format!("couldn't encode the response: {e}"),
        ))
    })
}

/// Builds Core over `bridge` and opens its workspace on `image` (none: an
/// empty file). Shared by `open` and the test hooks' second instance.
pub(crate) async fn build(bridge: DuckDbBridge, image: Option<Vec<u8>>) -> Result<Open, RpcError> {
    let core = Core::builder()
        .connect_policy(ConnectPolicy::Unrestricted)
        .executor(Arc::new(WasmExecutor))
        .engine(browser_engine(bridge))
        .build();
    let ws = core
        .open_workspace(WorkspaceSpec::new(DATA_DIR).with_image(image))
        .await
        .map_err(RpcError::from)?;
    Ok(Open { core, ws })
}

/// Tells the page this instance is gone: a panic aborts right after the
/// hook, so every promise still pending would never settle. The page
/// answers them with `CORE_RESTARTED` and instantiates a fresh module.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        // The location only: a panic's message could quote a value.
        match info.location() {
            Some(at) => log::error!(
                activity = "browser.trap", file = at.file(), line = at.line();
                "Core stopped"
            ),
            None => log::error!(activity = "browser.trap"; "Core stopped"),
        }
        let hook = ON_TRAP.with(|t| t.borrow().clone());
        if let Some(hook) = hook {
            let _ = hook.call0(&JsValue::NULL);
        }
    }));
}

/// Opens Core in the page: the DuckDB engine over `bridge` and the metadata
/// file from `image` (the stored snapshot) or a new one. `on_trap` runs if
/// the module panics; it must not call back into the module synchronously.
/// Resolves to the commit counter after the open (the snapshot baseline);
/// rejects with an `RpcError`'s JSON (`STORAGE_CORRUPT` for an image that
/// isn't a database). A second `open` closes the first Core's connections
/// and streams and replaces it.
#[wasm_bindgen]
pub async fn open(
    bridge: DuckDbBridge,
    image: Option<Vec<u8>>,
    on_trap: Option<js_sys::Function>,
) -> Result<f64, JsValue> {
    crate::log::install();
    install_panic_hook();
    ON_TRAP.with(|t| *t.borrow_mut() = on_trap);
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::maybe_panic_on_open(image.as_deref());
    let previous = OPEN.with(|o| o.borrow_mut().take());
    SUBSCRIPTIONS.with(|s| {
        for (_, handle) in s.borrow_mut().drain() {
            handle.abort();
        }
    });
    if let Some(previous) = previous {
        previous.ws.close_all(&previous.core).await;
    }
    let open = build(bridge, image).await.map_err(reject)?;
    let commits = open.ws.storage().commits();
    OPEN.with(|o| *o.borrow_mut() = Some(Rc::new(open)));
    Ok(commits as f64)
}

/// One workspace call: the request's JSON bytes in, the `CoreResponse`'s
/// JSON out. Rejects with an `RpcError`'s JSON.
#[wasm_bindgen]
pub async fn call(body: Vec<u8>) -> Result<String, JsValue> {
    let open = current_open()?;
    serve(&open, &body).await
}

pub(crate) async fn serve(open: &Open, body: &[u8]) -> Result<String, JsValue> {
    let request = parse_request(body).map_err(reject)?;
    let response = dispatch_workspace(&open.core, &open.ws, request, origin())
        .await
        .map_err(reject)?;
    encode(&response)
}

/// A stream call (`db.queryStream`, `db.run`, `db.page`, `db.tablePage`):
/// each `CoreEvent` goes to `on_event` as JSON text, in order. Resolves
/// with the number of events sent once the stream ends (`db.cancel` ends it
/// early). `on_event` must not call back into the module synchronously.
#[wasm_bindgen]
pub async fn stream(body: Vec<u8>, on_event: js_sys::Function) -> Result<u32, JsValue> {
    let open = current_open()?;
    serve_stream(&open, &body, &on_event).await
}

pub(crate) async fn serve_stream(
    open: &Open,
    body: &[u8],
    on_event: &js_sys::Function,
) -> Result<u32, JsValue> {
    let request = parse_request(body).map_err(reject)?;
    let mut events = dispatch_stream(&open.core, &open.ws, request, origin()).map_err(reject)?;
    let mut sent = 0u32;
    while let Some(event) = events.next().await {
        let text = encode(&event)?;
        sent += 1;
        if on_event
            .call1(&JsValue::NULL, &JsValue::from_str(&text))
            .is_err()
        {
            log::warn!(activity = "browser.stream"; "An event handler threw");
        }
    }
    Ok(sent)
}

/// Subscribes `on_event` to the workspace's `connectionClosed` and
/// `storageChanged` events (JSON text). Returns the id `unsubscribe` takes.
#[wasm_bindgen]
pub fn events(on_event: js_sys::Function) -> Result<u32, JsValue> {
    let open = current_open()?;
    let mut events = workspace_events(&open.ws);
    let (handle, registration) = AbortHandle::new_pair();
    let id = NEXT_SUBSCRIPTION.with(|n| {
        let id = n.get().wrapping_add(1);
        n.set(id);
        id
    });
    SUBSCRIPTIONS.with(|s| s.borrow_mut().insert(id, handle));
    wasm_bindgen_futures::spawn_local(async move {
        let pump = async move {
            while let Some(event) = events.next().await {
                let Ok(text) = serde_json::to_string(&event) else {
                    continue;
                };
                if on_event
                    .call1(&JsValue::NULL, &JsValue::from_str(&text))
                    .is_err()
                {
                    log::warn!(activity = "browser.events"; "An event handler threw");
                }
            }
        };
        let _ = Abortable::new(pump, registration).await;
    });
    Ok(id)
}

/// Ends a subscription `events` made. An unknown id does nothing.
#[wasm_bindgen]
pub fn unsubscribe(id: u32) {
    if let Some(handle) = SUBSCRIPTIONS.with(|s| s.borrow_mut().remove(&id)) {
        handle.abort();
    }
}

/// The metadata file as bytes, to store. Refused (`STORAGE_ERROR`, busy)
/// while a call holds the file, so it never holds an uncommitted write; try
/// again after the call.
#[wasm_bindgen]
pub fn snapshot() -> Result<Vec<u8>, JsValue> {
    let open = current_open()?;
    open.ws
        .storage()
        .snapshot()
        .map_err(|e| reject(RpcError::from(CoreError::from(e))))
}

/// How many write transactions have committed since the open began. It
/// only grows; a snapshot taken at a count holds every commit up to it.
#[wasm_bindgen]
pub fn commits() -> Result<f64, JsValue> {
    Ok(current_open()?.ws.storage().commits() as f64)
}

/// The demo's own start (Decision 19): stores `demo-connection` the first
/// time, afterwards only marks it connected. Resolves with the row as
/// `Seqd` JSON.
#[wasm_bindgen(js_name = ensureDemoConnection)]
pub async fn ensure_demo_connection() -> Result<String, JsValue> {
    let open = current_open()?;
    let row = open
        .ws
        .ensure_demo_connection(&open.core, &origin())
        .await
        .map_err(|e| reject(e.into()))?;
    encode(&row)
}
