//! Test-only exports (the `test-hooks` feature; never in the demo's module).
//!
//! - `__test_trap(kind)`: a panic on purpose, `"sync"` in this call or
//!   `"async"` inside an async call after an await, for the transport's
//!   trap recovery.
//! - A second Core, the "side" instance, over its own bridge: the engine
//!   suite gives it bridges that misbehave or answer late, without touching
//!   the page's Core.
//! - Calls and streams dropped after a delay, as Core drops a call: the
//!   browser driver must send DuckDB-WASM a cancel (and, for its own
//!   connections, a ROLLBACK and a close) though nobody awaits the call.

use std::cell::RefCell;
use std::rc::Rc;

use futures::future::{select, Either};
use futures::StreamExt;
use seaquel_engine_duckdb::DuckDbBridge;
use seaquel_rpc::{dispatch_stream, dispatch_workspace, parse_request};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::module::{build, origin, reject, serve, serve_stream, Open};

thread_local! {
    static SIDE: RefCell<Option<Rc<Open>>> = const { RefCell::new(None) };
}

/// Panics: `"sync"` in this call, before it returns (the page sees the
/// trap thrown); `"async"` from inside an async task after an await (the
/// panic then happens in wasm-bindgen-futures' task queue, the returned
/// promise never settles, and only the panic hook can tell the page).
#[wasm_bindgen]
pub fn __test_trap(kind: String) -> js_sys::Promise {
    if kind != "async" {
        panic!("a test trap");
    }
    wasm_bindgen_futures::future_to_promise(async {
        sleep(1).await;
        panic!("a test trap");
    })
}

/// The page's `setTimeout`, so a drop happens while DuckDB works.
async fn sleep(ms: u32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let set_timeout: js_sys::Function =
            js_sys::Reflect::get(&js_sys::global(), &"setTimeout".into())
                .expect("setTimeout")
                .into();
        let _ = set_timeout.call2(&JsValue::NULL, &resolve, &JsValue::from(ms));
    });
    let _ = JsFuture::from(promise).await;
}

fn side() -> Result<Rc<Open>, JsValue> {
    SIDE.with(|s| s.borrow().clone())
        .ok_or_else(|| JsValue::from_str("the side instance isn't open"))
}

/// Opens the side instance (an empty metadata file) over `bridge`,
/// replacing any earlier one.
#[wasm_bindgen]
pub async fn __test_side_open(bridge: DuckDbBridge) -> Result<(), JsValue> {
    let previous = SIDE.with(|s| s.borrow_mut().take());
    if let Some(previous) = previous {
        previous.ws.close_all(&previous.core).await;
    }
    let open = build(bridge, None, None).await.map_err(reject)?;
    SIDE.with(|s| *s.borrow_mut() = Some(Rc::new(open)));
    Ok(())
}

/// `call` on the side instance.
#[wasm_bindgen]
pub async fn __test_side_call(body: Vec<u8>) -> Result<String, JsValue> {
    let open = side()?;
    serve(&open, &body).await
}

/// `stream` on the side instance.
#[wasm_bindgen]
pub async fn __test_side_stream(body: Vec<u8>, on_event: js_sys::Function) -> Result<u32, JsValue> {
    let open = side()?;
    serve_stream(&open, &body, &on_event).await
}

/// `call` on the page's instance under another write origin (a window id
/// matching `^[A-Za-z0-9_-]{1,64}$`; anything else is dropped, as the
/// server drops it). The page's own calls always carry `demo`; the state
/// fixtures replay several windows on one file, each under its own id.
#[wasm_bindgen]
pub async fn __test_call_as(body: Vec<u8>, origin: String) -> Result<String, JsValue> {
    let open = crate::module::current_open()?;
    let request = parse_request(&body).map_err(reject)?;
    let response = dispatch_workspace(
        &open.core,
        &open.ws,
        request,
        seaquel_rpc::WriteOrigin::new(Some(&origin)),
    )
    .await
    .map_err(reject)?;
    serde_json::to_string(&response).map_err(|e| JsValue::from_str(&e.to_string()))
}

fn instance(side_instance: bool) -> Result<Rc<Open>, JsValue> {
    if side_instance {
        side()
    } else {
        crate::module::current_open()
    }
}

/// A call whose future is dropped after `ms`, on the page's instance or the
/// side one: `{"dropped": true}`, or `{"result": …}`/`{"error": …}` if it
/// finished first.
#[wasm_bindgen]
pub async fn __test_call_dropped_after(
    body: Vec<u8>,
    ms: u32,
    side_instance: bool,
) -> Result<String, JsValue> {
    let open = instance(side_instance)?;
    let request = parse_request(&body).map_err(reject)?;
    let work = dispatch_workspace(&open.core, &open.ws, request, origin());
    futures::pin_mut!(work);
    let timer = sleep(ms);
    futures::pin_mut!(timer);
    Ok(match select(work, timer).await {
        Either::Left((Ok(response), _)) => format!(
            r#"{{"result":{}}}"#,
            serde_json::to_string(&response).unwrap_or_default()
        ),
        Either::Left((Err(e), _)) => format!(
            r#"{{"error":{}}}"#,
            serde_json::to_string(&e).unwrap_or_default()
        ),
        Either::Right(_) => r#"{"dropped":true}"#.to_string(),
    })
}

/// A stream whose events are read until `ms` passes, then dropped (not
/// cancelled through `db.cancel`): the events it gave, as a JSON array, and
/// whether it was dropped.
#[wasm_bindgen]
pub async fn __test_stream_dropped_after(
    body: Vec<u8>,
    ms: u32,
    side_instance: bool,
) -> Result<String, JsValue> {
    let open = instance(side_instance)?;
    let request = parse_request(&body).map_err(reject)?;
    let mut events = dispatch_stream(&open.core, &open.ws, request, origin()).map_err(reject)?;
    let mut got = Vec::new();
    let timer = sleep(ms);
    futures::pin_mut!(timer);
    let dropped = loop {
        let next = events.next();
        futures::pin_mut!(next);
        match select(next, timer.as_mut()).await {
            Either::Left((Some(event), _)) => {
                got.push(serde_json::to_value(&event).unwrap_or_default())
            }
            Either::Left((None, _)) => break false,
            Either::Right(_) => break true,
        }
    };
    drop(events);
    Ok(serde_json::json!({"events": got, "dropped": dropped}).to_string())
}

/// A connect with `restricted` set (the MCP server's flag, not on the wire)
/// on a DuckDB form: `"ok"` (then disconnected) or the error's JSON.
#[wasm_bindgen]
pub async fn __test_connect_restricted(side_instance: bool) -> Result<String, JsValue> {
    let open = instance(side_instance)?;
    let form = serde_json::from_value(serde_json::json!({
        "name": "Restricted", "type": "duckdb", "host": "", "port": 0,
        "databaseName": ":memory:", "username": "", "connectionString": "",
        "sshEnabled": false, "sshHost": "", "sshPort": 22, "sshUsername": "",
        "sshAuthMethod": "password", "sshKeyPath": "",
        "savePassword": false, "saveSshPassword": false, "saveSshKeyPassphrase": false,
    }))
    .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let request = seaquel_core::ConnectRequest::form(form).with_restricted(true);
    Ok(match open.ws.connect(&open.core, request).await {
        Ok(id) => {
            let _ = open.ws.disconnect(&open.core, &id).await;
            "ok".to_string()
        }
        Err(e) => serde_json::to_string(&seaquel_rpc::RpcError::from(e)).unwrap_or_default(),
    })
}

/// `Core::explain_read_only` (the MCP server's EXPLAIN, not on the wire):
/// the plan's JSON, or the error's.
#[wasm_bindgen]
pub async fn __test_explain_read_only(
    connection_id: String,
    sql: String,
) -> Result<String, JsValue> {
    let open = crate::module::current_open()?;
    Ok(
        match open
            .core
            .explain_read_only(&connection_id, &sql, vec![], None)
            .await
        {
            Ok(plan) => serde_json::to_string(&plan).unwrap_or_default(),
            Err(e) => {
                serde_json::json!({"error": {"code": e.code, "message": e.message}}).to_string()
            }
        },
    )
}

/// An image starting with this panics `open`, on every instance: a stored
/// snapshot that makes Core trap deterministically.
const PANIC_IMAGE_PREFIX: &[u8] = b"SEAQUEL_TEST_PANIC";

/// Called first thing in `open` (test builds only): panics, inside the
/// open's task, when `image` starts with [`PANIC_IMAGE_PREFIX`], or when
/// the page's `globalThis.__seaquelTestPanicOpens` is above 0 (it is
/// decremented first, so a test can make the next N opens trap).
pub(crate) fn maybe_panic_on_open(image: Option<&[u8]>) {
    if image.is_some_and(|i| i.starts_with(PANIC_IMAGE_PREFIX)) {
        panic!("a test trap: the image");
    }
    let global = js_sys::global();
    let key = JsValue::from_str("__seaquelTestPanicOpens");
    let left = js_sys::Reflect::get(&global, &key)
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    if left > 0.0 {
        let _ = js_sys::Reflect::set(&global, &key, &JsValue::from_f64(left - 1.0));
        panic!("a test trap: the open");
    }
}
