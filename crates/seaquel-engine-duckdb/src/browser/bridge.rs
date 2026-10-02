//! The page's DuckDB-WASM, as the browser driver sees it (Decision 12).
//!
//! TypeScript imports DuckDB-WASM, starts its worker and passes Rust one
//! object with these methods. Each takes a DuckDB-WASM connection id and
//! returns a promise:
//!
//! | method | resolves to | DuckDB-WASM |
//! |---|---|---|
//! | `connect()` | a connection id (a number) | `AsyncDuckDB.connect()` |
//! | `runQuery(c, sql)` | the whole result, IPC **file** bytes (`Uint8Array`) | `runQuery` |
//! | `startPending(c, sql)` | the result's schema as IPC **stream** bytes, or `null` while the query still runs | `startPendingQuery(c, sql, true)` |
//! | `pollPending(c)` | the same, once it's ready, or `null` | `pollPendingQuery` |
//! | `fetchChunk(c)` | the next stream bytes; empty at the end; `null` for "not yet" | `fetchQueryResults` |
//! | `cancel(c)` | whether a running query was cancelled | `cancelPendingQuery` |
//! | `close(c)` | nothing | `disconnect` |
//!
//! **Every method must hand its request to DuckDB-WASM's worker before it
//! returns its promise** (no `await` before the call). The driver relies on
//! that order: a `cancel`, a `ROLLBACK` or a `close` it sends from a `Drop`
//! reaches the worker before the next call's statement. A query error
//! rejects with DuckDB's `Error`, whose `message` becomes the `DbError`'s.
//! The polling loop is Rust's, so dropping a call stops it.
//!
//! The methods are bound as plain functions returning a `Promise`, not as
//! `async` externs: an `async` extern would make the JavaScript call only
//! when first polled, which breaks the order above for the calls a `Drop`
//! makes.

use js_sys::{Promise, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};

#[wasm_bindgen]
extern "C" {
    /// The bridge object the page passes to [`crate::browser_engine`].
    pub type DuckDbBridge;

    #[wasm_bindgen(method, catch)]
    fn connect(this: &DuckDbBridge) -> Result<Promise, JsValue>;

    #[wasm_bindgen(method, catch, js_name = runQuery)]
    fn run_query(this: &DuckDbBridge, conn: u32, sql: &str) -> Result<Promise, JsValue>;

    #[wasm_bindgen(method, catch, js_name = startPending)]
    fn start_pending(this: &DuckDbBridge, conn: u32, sql: &str) -> Result<Promise, JsValue>;

    #[wasm_bindgen(method, catch, js_name = pollPending)]
    fn poll_pending(this: &DuckDbBridge, conn: u32) -> Result<Promise, JsValue>;

    #[wasm_bindgen(method, catch, js_name = fetchChunk)]
    fn fetch_chunk(this: &DuckDbBridge, conn: u32) -> Result<Promise, JsValue>;

    #[wasm_bindgen(method, catch)]
    fn cancel(this: &DuckDbBridge, conn: u32) -> Result<Promise, JsValue>;

    #[wasm_bindgen(method, catch)]
    fn close(this: &DuckDbBridge, conn: u32) -> Result<Promise, JsValue>;
}

/// What a failed bridge call says: DuckDB's message, or a description of
/// what the bridge returned instead of what it should have. Never the SQL
/// the driver sent (DuckDB's own message may quote it, as natively).
pub(crate) type BridgeError = String;

/// The bridge, with its answers checked. No method panics, whatever the
/// object passed in does.
pub(crate) struct Bridge(DuckDbBridge);

impl Bridge {
    pub(crate) fn new(js: DuckDbBridge) -> Self {
        Bridge(js)
    }

    /// `connect`'s promise, so a caller that stops waiting can still close
    /// the connection once it arrives (`driver::connect`).
    pub(crate) fn connect_promise(&self) -> Result<Promise, BridgeError> {
        self.0.connect().map_err(|e| message(&e))
    }

    pub(crate) async fn start_pending(
        &self,
        conn: u32,
        sql: &str,
    ) -> Result<Option<Vec<u8>>, BridgeError> {
        bytes(settle(self.0.start_pending(conn, sql)).await?)
    }

    pub(crate) async fn poll_pending(&self, conn: u32) -> Result<Option<Vec<u8>>, BridgeError> {
        bytes(settle(self.0.poll_pending(conn)).await?)
    }

    pub(crate) async fn fetch_chunk(&self, conn: u32) -> Result<Option<Vec<u8>>, BridgeError> {
        bytes(settle(self.0.fetch_chunk(conn)).await?)
    }

    /// The whole result of `sql`, as an IPC file (it keeps ENUM
    /// dictionaries, which pending results lose). Not cancellable.
    pub(crate) async fn run_query(&self, conn: u32, sql: &str) -> Result<Vec<u8>, BridgeError> {
        bytes(settle(self.0.run_query(conn, sql)).await?)?
            .ok_or_else(|| "runQuery returned no result".to_string())
    }

    /// Sends the close now; the future only waits for its answer.
    pub(crate) fn close(
        &self,
        conn: u32,
    ) -> impl std::future::Future<Output = Result<(), BridgeError>> {
        let call = self.0.close(conn);
        async move { settle(call).await.map(drop) }
    }

    // ── Sent from `Drop`: the request goes out now, its answer is ignored ──

    /// Cancels the query running on `conn`, if any.
    pub(crate) fn post_cancel(&self, conn: u32) {
        forget(self.0.cancel(conn));
    }

    /// Runs `sql` on `conn` (a `ROLLBACK`), ignoring how it ends.
    pub(crate) fn post_query(&self, conn: u32, sql: &str) {
        forget(self.0.run_query(conn, sql));
    }

    pub(crate) fn post_close(&self, conn: u32) {
        forget(self.0.close(conn));
    }
}

/// Waits for a promise the bridge returned (or the error it threw).
pub(crate) async fn settle(call: Result<Promise, JsValue>) -> Result<JsValue, BridgeError> {
    let promise = call.map_err(|e| message(&e))?;
    JsFuture::from(promise).await.map_err(|e| message(&e))
}

/// Lets a promise run to its end with nobody waiting, so a rejection is
/// handled and the request isn't dropped half-way on the Rust side.
fn forget(call: Result<Promise, JsValue>) {
    if let Ok(promise) = call {
        spawn_local(async move {
            let _ = JsFuture::from(promise).await;
        });
    }
}

/// A connection id: a whole number that fits `u32`.
pub(crate) fn connection_id(v: JsValue) -> Result<u32, BridgeError> {
    match v.as_f64() {
        Some(n) if n.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&n) => Ok(n as u32),
        _ => Err("connect returned no connection id".to_string()),
    }
}

/// `Uint8Array` bytes; `None` for `null` or `undefined`.
fn bytes(v: JsValue) -> Result<Option<Vec<u8>>, BridgeError> {
    if v.is_null() || v.is_undefined() {
        return Ok(None);
    }
    v.dyn_into::<Uint8Array>()
        .map(|a| Some(a.to_vec()))
        .map_err(|_| "DuckDB's bridge returned something that isn't bytes".to_string())
}

/// An error's message: an `Error`'s `message`, a thrown string, or a fixed
/// text for anything else.
fn message(e: &JsValue) -> BridgeError {
    if let Some(e) = e.dyn_ref::<js_sys::Error>() {
        return String::from(e.message());
    }
    e.as_string()
        .unwrap_or_else(|| "DuckDB failed without a message".to_string())
}
