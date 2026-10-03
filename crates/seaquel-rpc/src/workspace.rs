//! The workspace RPC: one `Request`/`Response` pair for everything the GUIs
//! ask of Core: metadata storage, the library (`library`, see the `library`
//! module), settings (`settings`), each window's view state (`ui`), secrets
//! and database calls (`db`, see the `db` module) on a user's
//! [`Workspace`], plus SSH, git and desktop licensing, and
//! [`dispatch_workspace`].
//!
//! The Tauri app serves it as the `core_call` command and `seaquel-server` as
//! `POST /rpc`. Wire shape, two levels of adjacent tagging:
//!
//! ```json
//! {"method":"storage","params":{"method":"queryHistorySetFavorite","params":{"id":"h","favorite":true}}}
//! {"method":"storage","result":{"method":"queryHistorySetFavorite","result":null}}
//! ```
//!
//! Phase 5d-1 retired the storage group's connection, project, saved-query
//! and version methods (`connectionsLoadAll`, `connectionsSave`, …,
//! `queryVersionsPrune`), and phase 5d-2 its app-state, project-state,
//! dashboard, chat, theme, onboarding, tutorial, import-state and
//! connection-override methods (35 in all): the `library`, `settings` and
//! `ui` groups replaced them (the overrides are retired, Q13), and naming
//! one is an unknown method (`INVALID_ARGUMENT`). Phase 5e retired
//! `sharedReposLoadAll` and `sharedReposSaveAll` the same way: the `shared`
//! group's repo calls replaced them. What stays in the storage group is the
//! query history, the license record and the web vault. Every write, in
//! any group, emits one `StorageChanged` event after it committed,
//! carrying the caller's [`WriteOrigin`].
//!
//! **`method` must come before `params`** at both levels. JSON columns in
//! the storage rows are `serde_json::RawValue`s, which only deserialize when
//! serde can stream the content after it has read the tag; with `params`
//! first it buffers the content and a `RawValue` inside fails. So
//! [`parse_request`] refuses that order for every request, with a clear
//! message, instead of letting it work for some methods and not others. For
//! the same reason a body must be parsed from its bytes, never from a
//! `serde_json::Value` (which sorts object keys, changing stored JSON).
//!
//! Every variant exists in every build, so the generated TypeScript doesn't
//! depend on features. A group whose Core feature is off (`storage`,
//! `secrets`, `git`, `imports`) answers `NOT_SUPPORTED`.
//! [`dispatch_workspace`], the web server's entry point, refuses SSH, git
//! and licensing whatever the features, and the `shared` and `imports`
//! groups on a Core built without `LocalFiles` (phase 5e, Decision 31);
//! the desktop routes the first three to `dispatch_ssh`, `dispatch_git`
//! and `dispatch_license`.

use std::fmt;

use log::debug;
use seaquel_core::{Core, CoreError, Workspace, WriteOrigin};
use seaquel_types::storage::{PersistedCredential, PersistedQueryHistoryItem, PersistedVaultState};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

// ── Errors ──

/// Every workspace call fails with one of these, the same shape as
/// `DbError`. Codes:
///
/// - storage: `LEGACY_STORAGE`, `STORAGE_CORRUPT`, `NO_DATA_DIR` (desktop),
///   `STORAGE_ERROR`, and from a read-only workspace (the CLI)
///   `STORAGE_NEEDS_UPGRADE` and `STORAGE_NOT_FOUND`;
/// - secrets: `INVALID_ARGUMENT` (a bad key), `SECRET_STORE_ERROR`;
/// - `INVALID_ARGUMENT` for a body that isn't a valid request;
/// - `NOT_SUPPORTED` when the workspace or build lacks the piece asked for.
///
/// A library refusal (phase 5d-1) adds `NAME_TAKEN` with `takenBy`, the id
/// of the row that has the name, so the GUI can name it; never a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct RpcError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub taken_by: Option<String>,
}

pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
pub const NOT_SUPPORTED: &str = "NOT_SUPPORTED";

impl RpcError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            taken_by: None,
        }
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(INVALID_ARGUMENT, message)
    }

    pub fn not_supported(what: &str) -> Self {
        Self::new(NOT_SUPPORTED, format!("{what} isn't supported here"))
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

impl From<CoreError> for RpcError {
    fn from(e: CoreError) -> Self {
        Self {
            code: e.code,
            message: e.message,
            taken_by: e.taken_by,
        }
    }
}

/// Database errors keep their codes (`CONNECTION_NOT_FOUND`, the driver's).
impl From<seaquel_types::DbError> for RpcError {
    fn from(e: seaquel_types::DbError) -> Self {
        Self::new(e.code, e.message)
    }
}

/// For errors from the storage and secret crates, which convert to
/// [`CoreError`] with their codes.
#[cfg(any(feature = "storage", feature = "secrets"))]
fn rpc_error(e: impl Into<CoreError>) -> RpcError {
    RpcError::from(e.into())
}

// ── Request and Response ──

/// A workspace call: `{"method": <group>, "params": <the group's request>}`.
// Requests and responses are built once and moved into or out of one call,
// so their size doesn't matter; boxing the large rows would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    deny_unknown_fields
)]
// `CoreRequest` in TypeScript, where `Request` is the DOM's.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "CoreRequest"))]
pub enum Request {
    Storage(StorageRequest),
    Library(crate::library::LibraryRequest),
    Settings(crate::settings::SettingsRequest),
    Ui(crate::ui::UiRequest),
    Shared(crate::shared::SharedRequest),
    Imports(crate::imports::ImportsRequest),
    Secret(SecretRequest),
    License(crate::license::DesktopLicenseRequest),
    Git(crate::git::GitRequest),
    Ssh(crate::ssh::SshRequest),
    Db(crate::db::DbRequest),
    Ai(crate::ai::AiRequest),
}

/// A call's result: `{"method": <group>, "result": <the group's response>}`,
/// where the group's response repeats its method.
// Not `Deserialize`: responses only go out to the GUIs (the `db` group's
// edit outcomes can't be read).
// Requests and responses are built once and moved into or out of one call,
// so their size doesn't matter; boxing the large rows would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Serialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
// `CoreResponse` in TypeScript, where `Response` is the DOM's.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "CoreResponse"))]
pub enum Response {
    Storage(StorageResponse),
    Library(crate::library::LibraryResponse),
    Settings(crate::settings::SettingsResponse),
    Ui(crate::ui::UiResponse),
    Shared(crate::shared::SharedResponse),
    Imports(crate::imports::ImportsResponse),
    Secret(SecretResponse),
    License(crate::license::DesktopLicenseResponse),
    Git(crate::git::GitResponse),
    Ssh(crate::ssh::SshResponse),
    Db(crate::db::DbResponse),
    Ai(crate::ai::AiResponse),
}

impl Request {
    /// The group's wire name: `storage`, `library`, `settings`, `ui`,
    /// `shared`, `imports`, `secret`, `license`, `git`, `ssh`, `db` or `ai`.
    pub fn group(&self) -> &'static str {
        match self {
            Request::Storage(_) => "storage",
            Request::Library(_) => "library",
            Request::Settings(_) => "settings",
            Request::Ui(_) => "ui",
            Request::Shared(_) => "shared",
            Request::Imports(_) => "imports",
            Request::Secret(_) => "secret",
            Request::License(_) => "license",
            Request::Git(_) => "git",
            Request::Ssh(_) => "ssh",
            Request::Db(_) => "db",
            Request::Ai(_) => "ai",
        }
    }

    /// The method's wire name within its group (`connectionsLoadAll`,
    /// `get`). With [`Request::group`], all a log line may say about a
    /// request.
    pub fn method(&self) -> &'static str {
        match self {
            Request::Storage(r) => r.method(),
            Request::Library(r) => r.method(),
            Request::Settings(r) => r.method(),
            Request::Ui(r) => r.method(),
            Request::Shared(r) => r.method(),
            Request::Imports(r) => r.method(),
            Request::Secret(r) => r.method(),
            Request::License(r) => r.method(),
            Request::Git(r) => r.method(),
            Request::Ssh(r) => r.method(),
            Request::Db(r) => r.method(),
            Request::Ai(r) => r.method(),
        }
    }

    /// Which stream this request is ([`crate::StreamKind`]), or `None` for
    /// a unary call: the one list every transport serves streams by
    /// (`db.queryStream`, `db.run`, `db.page`, `db.tablePage`, `ai.chat`).
    pub fn stream_kind(&self) -> Option<crate::StreamKind> {
        crate::StreamKind::of(self.group(), self.method())
    }

    /// A stream request's `streamId`: what its events carry and
    /// `db.cancel` takes. `None` for a unary call.
    pub fn stream_id(&self) -> Option<&str> {
        match self {
            Request::Db(r) => r.stream_id(),
            Request::Ai(r) => r.stream_id(),
            _ => None,
        }
    }
}

/// Defines [`StorageRequest`], [`StorageResponse`] and
/// `StorageRequest::method` from one list, so a method's request variant,
/// response variant and wire name can't drift apart. Each line is
/// `Variant = "wireName" [request fields] -> [response type]`.
macro_rules! storage_methods {
    ($(
        $(#[$attr:meta])*
        $variant:ident = $name:literal [ $($body:tt)? ] -> [ $(#[$res_attr:meta])* $res:ty ]
    ),* $(,)?) => {
        /// A storage call: one variant per function in `seaquel-storage`'s
        /// query modules, named `<Repo><Method>`. Fields are camelCase, and
        /// rows are the `Persisted*` types from `seaquel_types::storage`.
        #[allow(clippy::large_enum_variant)] // See `Request`.
        #[derive(Debug, Serialize, Deserialize)]
        #[serde(
            tag = "method",
            content = "params",
            rename_all_fields = "camelCase",
            deny_unknown_fields
        )]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
        pub enum StorageRequest {
            $( $(#[$attr])* #[serde(rename = $name)] $variant $($body)?, )*
        }

        /// A storage call's result, as `{"method": …, "result": …}`. Calls
        /// that return nothing have `"result": null`.
        #[allow(clippy::large_enum_variant)] // See `Request`.
        #[derive(Debug, Serialize, Deserialize)]
        #[serde(tag = "method", content = "result")]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
        pub enum StorageResponse {
            $( #[serde(rename = $name)] $variant($(#[$res_attr])* $res), )*
        }

        impl StorageRequest {
            /// The method's wire name.
            pub fn method(&self) -> &'static str {
                match self {
                    $( Self::$variant { .. } => $name, )*
                }
            }
        }
    };
}

storage_methods! {
    // license
    LicenseLoad = "licenseLoad" []
        -> [#[cfg_attr(feature = "ts", ts(type = "unknown"))] Option<Box<RawValue>>],
    LicenseSave = "licenseSave" [{
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        data: Box<RawValue>
    }] -> [()],

    // query_history
    QueryHistoryLoadByConnection = "queryHistoryLoadByConnection" [{ connection_id: String }]
        -> [Vec<PersistedQueryHistoryItem>],
    /// Adds one row and applies the cap (`query_history::append`).
    QueryHistoryAppend = "queryHistoryAppend" [{ item: PersistedQueryHistoryItem }] -> [()],
    /// Sets, not toggles, so writes queued in either order agree.
    QueryHistorySetFavorite = "queryHistorySetFavorite" [{ id: String, favorite: bool }]
        -> [()],
    QueryHistoryRemoveByConnection = "queryHistoryRemoveByConnection"
        [{ connection_id: String }] -> [()],

    // user_credentials
    UserCredentialsLoad = "userCredentialsLoad" [{ scope: String, key: String }]
        -> [Option<PersistedCredential>],
    UserCredentialsSave = "userCredentialsSave" [{ credential: PersistedCredential }] -> [()],
    UserCredentialsRemove = "userCredentialsRemove" [{ scope: String, key: String }] -> [()],
    UserCredentialsRemoveAllForKey = "userCredentialsRemoveAllForKey" [{ key: String }] -> [()],

    // vault_state
    VaultStateLoad = "vaultStateLoad" [] -> [Option<PersistedVaultState>],
    VaultStateSave = "vaultStateSave" [{ state: PersistedVaultState }] -> [()],
    VaultStateReset = "vaultStateReset" [] -> [()],
}

/// A secret call. Keys must be `db:<id>`, `ssh:<id>`, `ssh-key:<id>` or
/// `license-key`; anything else is `INVALID_ARGUMENT`. So is
/// `ai-api-key:<id>` (phase 6, Decision 7): Core reads AI keys itself, and
/// the settings group writes them.
/// `Debug` never shows a value.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    deny_unknown_fields
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SecretRequest {
    Get { key: String },
    Set { key: String, value: String },
    Delete { key: String },
}

impl SecretRequest {
    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            SecretRequest::Get { .. } => "get",
            SecretRequest::Set { .. } => "set",
            SecretRequest::Delete { .. } => "delete",
        }
    }
}

impl fmt::Debug for SecretRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretRequest::Get { key } => f.debug_struct("Get").field("key", key).finish(),
            SecretRequest::Set { key, .. } => f
                .debug_struct("Set")
                .field("key", key)
                .field("value", &"<redacted>")
                .finish(),
            SecretRequest::Delete { key } => f.debug_struct("Delete").field("key", key).finish(),
        }
    }
}

/// A secret call's result. `get` gives `null` when there is no entry.
/// `Debug` never shows a value.
#[derive(Serialize, Deserialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SecretResponse {
    Get(Option<String>),
    Set(()),
    Delete(()),
}

impl fmt::Debug for SecretResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretResponse::Get(value) => f
                .debug_tuple("Get")
                .field(&value.as_ref().map(|_| "<redacted>"))
                .finish(),
            SecretResponse::Set(()) => f.write_str("Set"),
            SecretResponse::Delete(()) => f.write_str("Delete"),
        }
    }
}

// ── Parsing ──

const TAG_ORDER_MESSAGE: &str =
    "invalid request: \"method\" must come before \"params\" in the request object";

/// Parse a request from the bytes of its JSON body. Use this, not
/// `serde_json::from_value`: stored JSON columns keep their exact text only
/// when parsed from the original bytes.
///
/// A body with `params` before `method`, at either level, is refused with
/// `INVALID_ARGUMENT` (see the module docs), as is anything else that isn't a
/// valid request.
pub fn parse_request(body: &[u8]) -> Result<Request, RpcError> {
    let mut de = serde_json::Deserializer::from_slice(body);
    TagOrder { depth: 2 }
        .deserialize(&mut de)
        .map_err(|e| invalid_body(&e))?;
    serde_json::from_slice(body).map_err(|e| invalid_body(&e))
}

fn invalid_body(e: &serde_json::Error) -> RpcError {
    let message = e.to_string();
    if message.starts_with(TAG_ORDER_MESSAGE) {
        RpcError::invalid_argument(message)
    } else {
        RpcError::invalid_argument(format!("invalid request: {message}"))
    }
}

/// Walks a request body and fails when an object has `params` before
/// `method`: the request itself and, `depth` levels down, the object under
/// its `params`. Everything else is skipped.
struct TagOrder {
    depth: u8,
}

impl<'de> DeserializeSeed<'de> for TagOrder {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for TagOrder {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut seen_method = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "method" => {
                    seen_method = true;
                    map.next_value::<IgnoredAny>()?;
                }
                "params" if !seen_method => return Err(de::Error::custom(TAG_ORDER_MESSAGE)),
                "params" if self.depth > 1 => map.next_value_seed(TagOrder {
                    depth: self.depth - 1,
                })?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(())
    }

    fn visit_bool<E>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }
}

// ── Dispatch ──

/// Run one workspace call. `origin` is the window or tab that sent it (the
/// desktop's webview label, the web's checked `X-Seaquel-Origin`); the
/// `StorageChanged` event of a write carries it, and nothing logs it.
///
/// Logs the group and method name only, never the params or the result:
/// they can hold passwords, connection strings, keys and user data.
///
/// `db` calls go through `ws`, so they reach only its own connections and
/// streams. `db.queryStream` is refused here: `dispatch_stream` serves it.
///
/// **Who may connect, and to what,** is Core's `ConnectPolicy`, not this
/// dispatcher's: a Core built without one refuses `db.connect` and `db.test`
/// with `NOT_SUPPORTED` in every build. The web server's policy limits the
/// config (`web_config::check_connect_config`, run on the config Core builds
/// after the saved row or form and secrets are resolved) and refuses SSH
/// before a tunnel opens; its engines are limited by `with_plugins(WEB_ENGINES)`.
pub async fn dispatch_workspace(
    core: &Core,
    ws: &Workspace,
    req: Request,
    origin: WriteOrigin,
) -> Result<Response, RpcError> {
    let (group, method) = (req.group(), req.method());
    logged(group, method, async {
        match req {
            Request::Storage(r) => storage(ws, r, &origin).await.map(Response::Storage),
            // Boxed (phase 5e re-review R1): a library call may publish a
            // shared file and sync, which makes its future deep; inlined
            // here, it crowds a 2 MiB worker in a debug build.
            Request::Library(r) => Box::pin(crate::library::library(core, ws, r, &origin))
                .await
                .map(Response::Library),
            Request::Settings(r) => crate::settings::settings(core, ws, r, &origin)
                .await
                .map(Response::Settings),
            Request::Ui(r) => crate::ui::ui(core, ws, r, &origin).await.map(Response::Ui),
            // Desktop only (phase 5e, Decision 31): the user's repos and
            // other tools' files, refused on a Core without `LocalFiles`
            // (the web server's) whatever the features. Boxed like the
            // library: a sync's future is deep.
            Request::Shared(r) => {
                require_local_files(core, "Shared projects")?;
                Box::pin(crate::shared::shared(core, ws, r, &origin))
                    .await
                    .map(Response::Shared)
            }
            Request::Imports(r) => {
                require_local_files(core, "Imports")?;
                Box::pin(crate::imports::imports(core, ws, r, &origin))
                    .await
                    .map(Response::Imports)
            }
            Request::Secret(r) => secret(secrets_of(ws), r).await.map(Response::Secret),
            // The desktop serves this group with `dispatch_license`; a web
            // workspace has no activation client.
            Request::License(_) => Err(RpcError::not_supported("Desktop licensing")),
            // The desktop serves this group with `dispatch_git`; the web
            // server has no shared projects.
            Request::Git(_) => Err(RpcError::not_supported("Git")),
            // The desktop serves this group with `dispatch_ssh`. Never here,
            // whatever features the build unified: a web workspace must not
            // open tunnels from the server.
            Request::Ssh(_) => Err(RpcError::not_supported("SSH tunnels")),
            // Boxed like the library (phase 6 probe-fix review I2): a
            // connect's future (plan, tunnel, open, replace) sat inline in
            // every call's and overflowed a 2 MiB stack.
            Request::Db(r) => Box::pin(crate::db::db(core, ws, r, &origin))
                .await
                .map(Response::Db),
            Request::Ai(r) => Box::pin(crate::ai::ai(core, ws, r, &origin))
                .await
                .map(Response::Ai),
        }
    })
    .await
}

/// Run one secret call against `store` alone, with no storage. The desktop
/// uses it so secrets keep working when its metadata file can't be opened.
/// Logs like [`dispatch_workspace`].
#[cfg(feature = "secrets")]
pub async fn dispatch_secret(
    store: Option<&dyn seaquel_core::secrets::SecretStore>,
    req: SecretRequest,
) -> Result<SecretResponse, RpcError> {
    let method = req.method();
    logged("secret", method, secret(store, req)).await
}

/// `NOT_SUPPORTED` unless `core` may touch the user's files.
fn require_local_files(core: &Core, what: &str) -> Result<(), RpcError> {
    match core.local_files() {
        Some(seaquel_core::LocalFiles::Allowed) => Ok(()),
        None => Err(RpcError::not_supported(what)),
    }
}

/// Log a call's group and method, and its error code if it fails.
pub(crate) async fn logged<T>(
    group: &'static str,
    method: &'static str,
    call: impl std::future::Future<Output = Result<T, RpcError>>,
) -> Result<T, RpcError> {
    debug!(activity = "rpc.call", group = group, method = method; "Workspace call");
    let result = call.await;
    if let Err(e) = &result {
        debug!(activity = "rpc.call", group = group, method = method, code = e.code.as_str(); "Workspace call failed");
    }
    result
}

#[cfg(feature = "secrets")]
fn secrets_of(ws: &Workspace) -> Option<&dyn seaquel_core::secrets::SecretStore> {
    ws.secrets()
}

#[cfg(not(feature = "secrets"))]
fn secrets_of(_: &Workspace) -> Option<std::convert::Infallible> {
    None
}

#[cfg(not(feature = "storage"))]
async fn storage(
    _: &Workspace,
    _: StorageRequest,
    _: &WriteOrigin,
) -> Result<StorageResponse, RpcError> {
    Err(RpcError::not_supported("Storage"))
}

#[cfg(feature = "storage")]
async fn storage(
    ws: &Workspace,
    req: StorageRequest,
    origin: &WriteOrigin,
) -> Result<StorageResponse, RpcError> {
    let change = storage_change(&req);
    let response = storage_call(ws, req).await.map_err(rpc_error)?;
    // Phase 5d, Decision 16: every storage write emits one event after it
    // committed, with the writer's origin so its own window can skip it.
    if let Some((kind, scope, ids)) = change {
        ws.record_storage_write(origin, kind, scope, ids);
    }
    Ok(response)
}

/// A storage write's event: its kind, scope and ids.
#[cfg(feature = "storage")]
type Change = (
    seaquel_core::StoredKind,
    Option<String>,
    Option<Vec<String>>,
);

/// For a storage write, the event's kind, scope and ids: the id or key the
/// method names, never a value (the Rust twin of the TypeScript's
/// `STORAGE_METHOD_KIND`). `None` for a read. The query history's writes
/// are kind `history`, as Core's own appends are; `queryHistorySetFavorite`
/// knows no connection, so its event has the row's id and no scope (the
/// change feed finds the row by id).
#[cfg(feature = "storage")]
fn storage_change(req: &StorageRequest) -> Option<Change> {
    use seaquel_core::StoredKind::{History, Storage};
    use StorageRequest as Q;
    let one = |id: &str| Some((Storage, None, Some(vec![id.to_string()])));
    let all = || Some((Storage, None, None));
    match req {
        // Reads.
        Q::LicenseLoad
        | Q::QueryHistoryLoadByConnection { .. }
        | Q::UserCredentialsLoad { .. }
        | Q::VaultStateLoad => None,
        // Writes.
        Q::LicenseSave { .. } | Q::VaultStateSave { .. } | Q::VaultStateReset => all(),
        Q::QueryHistoryAppend { item } => Some((
            History,
            Some(item.connection_id.clone()),
            Some(vec![item.id.clone()]),
        )),
        Q::QueryHistorySetFavorite { id, .. } => Some((History, None, Some(vec![id.clone()]))),
        Q::QueryHistoryRemoveByConnection { connection_id } => {
            Some((History, Some(connection_id.clone()), None))
        }
        Q::UserCredentialsSave { credential } => one(&credential.key),
        Q::UserCredentialsRemove { key, .. } | Q::UserCredentialsRemoveAllForKey { key } => {
            one(key)
        }
    }
}

#[cfg(feature = "storage")]
async fn storage_call(
    ws: &Workspace,
    req: StorageRequest,
) -> Result<StorageResponse, seaquel_core::storage::StorageError> {
    use seaquel_core::storage::*;
    use StorageRequest as Q;
    use StorageResponse as R;

    let st = ws.storage();
    Ok(match req {
        Q::LicenseLoad => R::LicenseLoad(license::load(st).await?),
        Q::LicenseSave { data } => R::LicenseSave(license::save(st, &data).await?),

        Q::QueryHistoryLoadByConnection { connection_id } => R::QueryHistoryLoadByConnection(
            query_history::load_by_connection(st, &connection_id).await?,
        ),
        Q::QueryHistoryAppend { item } => {
            R::QueryHistoryAppend(query_history::append(st, &item).await?)
        }
        Q::QueryHistorySetFavorite { id, favorite } => {
            R::QueryHistorySetFavorite(query_history::set_favorite(st, &id, favorite).await?)
        }
        Q::QueryHistoryRemoveByConnection { connection_id } => R::QueryHistoryRemoveByConnection(
            query_history::remove_by_connection(st, &connection_id).await?,
        ),

        Q::UserCredentialsLoad { scope, key } => {
            R::UserCredentialsLoad(user_credentials::load(st, &scope, &key).await?)
        }
        Q::UserCredentialsSave { credential } => {
            R::UserCredentialsSave(user_credentials::save(st, &credential).await?)
        }
        Q::UserCredentialsRemove { scope, key } => {
            R::UserCredentialsRemove(user_credentials::remove(st, &scope, &key).await?)
        }
        Q::UserCredentialsRemoveAllForKey { key } => {
            R::UserCredentialsRemoveAllForKey(user_credentials::remove_all_for_key(st, &key).await?)
        }

        Q::VaultStateLoad => R::VaultStateLoad(vault_state::load(st).await?),
        Q::VaultStateSave { state } => R::VaultStateSave(vault_state::save(st, &state).await?),
        Q::VaultStateReset => R::VaultStateReset(vault_state::reset(st).await?),
    })
}

#[cfg(not(feature = "secrets"))]
async fn secret(
    _: Option<std::convert::Infallible>,
    _: SecretRequest,
) -> Result<SecretResponse, RpcError> {
    Err(RpcError::not_supported("Secret storage"))
}

/// The keychain prefix of an AI provider's key.
#[cfg(feature = "secrets")]
const AI_KEY_PREFIX: &str = "ai-api-key:";

/// The keys the `secret` group takes: `<prefix><id>`, or [`LICENSE_KEY`].
#[cfg(feature = "secrets")]
const GROUP_KEY_PREFIXES: [&str; 3] = ["db:", "ssh:", "ssh-key:"];
#[cfg(feature = "secrets")]
const LICENSE_KEY: &str = "license-key";

#[cfg(feature = "secrets")]
async fn secret(
    store: Option<&dyn seaquel_core::secrets::SecretStore>,
    req: SecretRequest,
) -> Result<SecretResponse, RpcError> {
    use seaquel_core::secrets::validate_key;

    let key = match &req {
        SecretRequest::Get { key }
        | SecretRequest::Set { key, .. }
        | SecretRequest::Delete { key } => key,
    };
    // A bad key is refused the same way with or without a store.
    //
    // Phase 6, Decision 7: Core reads a provider's key itself and writes it
    // with the settings call that carries it, so no page reads, sets or
    // deletes one here (on the desktop the key never reaches the webview).
    if key.starts_with(AI_KEY_PREFIX) {
        return Err(RpcError::invalid_argument(
            "AI provider keys are managed by Core: the secret group can't read, set or \
             delete them",
        ));
    }
    // The store's own check also takes AI keys, so this group names its
    // own forms first (review M3), then lets the store check the id.
    let ours = GROUP_KEY_PREFIXES.iter().any(|p| key.starts_with(p)) || key == LICENSE_KEY;
    if !ours {
        return Err(RpcError::invalid_argument(
            "invalid secret key: expected db:<id>, ssh:<id>, ssh-key:<id> or license-key",
        ));
    }
    validate_key(key).map_err(rpc_error)?;
    let store = store.ok_or_else(|| RpcError::not_supported("Secret storage on this workspace"))?;
    Ok(match req {
        SecretRequest::Get { key } => {
            SecretResponse::Get(store.get(&key).await.map_err(rpc_error)?)
        }
        SecretRequest::Set { key, value } => {
            SecretResponse::Set(store.set(&key, &value).await.map_err(rpc_error)?)
        }
        SecretRequest::Delete { key } => {
            SecretResponse::Delete(store.delete(&key).await.map_err(rpc_error)?)
        }
    })
}
