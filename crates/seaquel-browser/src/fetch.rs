//! The demo's model calls (phase 6): Core's
//! `HttpClient` over a fetch bridge the page passes to `open`.
//!
//! The page owns `fetch` (and CORS); Rust owns the turn. The bridge object
//! has three methods, each taking the request's id, which Rust picks:
//!
//! | method | returns |
//! |---|---|
//! | `start(id, method, url, headers, body)` | a promise of the status, once the head arrives; rejects when the request fails |
//! | `read(id)` | a promise of the next body chunk (`Uint8Array`), or `null` at the end; rejects when the body fails |
//! | `abort(id)` | nothing; stops the request (an unknown or finished id does nothing) |
//!
//! `headers` is an array of `[name, value]` pairs (lowercase names); `body`
//! is a `Uint8Array`, empty for a `GET`. **Dropping a call aborts it**:
//! `send` dropped before the head (a cancel, a timeout) and a body stream
//! dropped before its end both call `abort`, so the provider stops
//! generating. A bridge that throws or answers the wrong shape fails
//! the call; nothing here panics on what the page does.
//!
//! Nothing is logged from here, and an error never carries the URL, a
//! header or the page's message (it could name the URL): only a fixed
//! description and its kind.

use std::cell::Cell;
use std::rc::Rc;

use futures::stream;
use js_sys::{Array, Promise, Uint8Array};
use seaquel_core::ai::http::{HttpClient, HttpError, HttpErrorKind, HttpRequest, HttpResponse};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
    /// The fetch bridge object the page passes to `open`.
    pub type FetchBridge;

    #[wasm_bindgen(method, catch)]
    fn start(
        this: &FetchBridge,
        id: u32,
        method: &str,
        url: &str,
        headers: Array,
        body: Uint8Array,
    ) -> Result<Promise, JsValue>;

    #[wasm_bindgen(method, catch)]
    fn read(this: &FetchBridge, id: u32) -> Result<Promise, JsValue>;

    #[wasm_bindgen(method, catch)]
    fn abort(this: &FetchBridge, id: u32) -> Result<JsValue, JsValue>;
}

/// Core's `HttpClient` over the page's [`FetchBridge`].
pub(crate) struct BridgeHttp {
    bridge: Rc<FetchBridge>,
    next_id: Cell<u32>,
}

impl BridgeHttp {
    pub(crate) fn new(bridge: FetchBridge) -> Self {
        Self {
            bridge: Rc::new(bridge),
            next_id: Cell::new(0),
        }
    }

    fn id(&self) -> u32 {
        let id = self.next_id.get().wrapping_add(1);
        self.next_id.set(id);
        id
    }
}

/// Aborts request `id` when dropped, unless it ended on its own.
struct Abort {
    bridge: Rc<FetchBridge>,
    id: u32,
    armed: bool,
}

impl Drop for Abort {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.bridge.abort(self.id);
        }
    }
}

fn failed(kind: HttpErrorKind, detail: &'static str) -> HttpError {
    HttpError::new(kind, detail)
}

#[seaquel_runtime::async_trait]
impl HttpClient for BridgeHttp {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let id = self.id();
        let headers = Array::new();
        for (name, value) in &req.headers {
            headers.push(&Array::of2(
                &JsValue::from_str(name),
                &JsValue::from_str(value.expose()),
            ));
        }
        let body = Uint8Array::from(req.body.as_slice());
        // Armed before the call: a send dropped while waiting for the head
        // aborts the request.
        let mut guard = Abort {
            bridge: self.bridge.clone(),
            id,
            armed: true,
        };
        let promise = self
            .bridge
            .start(id, req.method.as_str(), &req.url, headers, body)
            .map_err(|_| failed(HttpErrorKind::Connect, "the page's fetch bridge threw"))?;
        let head = JsFuture::from(promise)
            .await
            .map_err(|_| failed(HttpErrorKind::Connect, "the page's fetch failed"))?;
        let status = head
            .as_f64()
            .filter(|s| s.fract() == 0.0 && (100.0..=599.0).contains(s))
            .ok_or_else(|| {
                failed(
                    HttpErrorKind::Other,
                    "the page's fetch bridge answered no status",
                )
            })? as u16;
        let body = stream::unfold(Some(guard_take(&mut guard)), |state| async move {
            let mut guard = state?;
            let promise = match guard.bridge.read(guard.id) {
                Ok(promise) => promise,
                Err(_) => {
                    return Some((
                        Err(failed(HttpErrorKind::Body, "the page's fetch bridge threw")),
                        None,
                    ))
                }
            };
            match JsFuture::from(promise).await {
                Ok(chunk) if chunk.is_null() || chunk.is_undefined() => {
                    // The end: nothing to abort.
                    guard.armed = false;
                    None
                }
                Ok(chunk) => match chunk.dyn_into::<Uint8Array>() {
                    Ok(bytes) => Some((Ok(bytes.to_vec()), Some(guard))),
                    Err(_) => Some((
                        Err(failed(
                            HttpErrorKind::Body,
                            "the page's fetch bridge answered a chunk that isn't bytes",
                        )),
                        None,
                    )),
                },
                Err(_) => Some((
                    Err(failed(HttpErrorKind::Body, "the response body failed")),
                    None,
                )),
            }
        });
        Ok(HttpResponse {
            status,
            body: Box::pin(body),
        })
    }
}

/// Moves the guard's duty into a new one (the body stream's), leaving the
/// original disarmed.
fn guard_take(guard: &mut Abort) -> Abort {
    guard.armed = false;
    Abort {
        bridge: guard.bridge.clone(),
        id: guard.id,
        armed: true,
    }
}
