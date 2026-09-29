//! The workspace RPC: one `Request`/`Response` pair for everything the GUIs
//! ask of Core: metadata storage, secrets and database calls (`db`, see the
//! `db` module) on a user's [`Workspace`], plus SSH, git and desktop
//! licensing, and [`dispatch_workspace`].
//!
//! The Tauri app serves it as the `core_call` command and `seaquel-server` as
//! `POST /rpc`. Wire shape, two levels of adjacent tagging:
//!
//! ```json
//! {"method":"storage","params":{"method":"connectionsRemove","params":{"connectionId":"c1"}}}
//! {"method":"storage","result":{"method":"connectionsRemove","result":null}}
//! ```
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
//! `secrets`) answers `NOT_SUPPORTED`. [`dispatch_workspace`], the web
//! server's entry point, refuses SSH, git and licensing whatever the
//! features; the desktop routes those groups to `dispatch_ssh`,
//! `dispatch_git` and `dispatch_license`.

use std::fmt;

use log::debug;
use seaquel_core::{Core, CoreError, Workspace};
use seaquel_types::storage::{
    DashboardVersionsPrune, ImportState, PersistedAIChat, PersistedAIMessage, PersistedConnection,
    PersistedConnectionOverride, PersistedCredential, PersistedDashboard,
    PersistedDashboardVersion, PersistedProject, PersistedProjectState, PersistedQueryHistoryItem,
    PersistedQueryVersion, PersistedSavedQuery, PersistedVaultState, QueryVersionsPrune,
    SharedReposState, ThemePreferences, TutorialProgress,
};
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
pub const NOT_SUPPORTED: &str = "NOT_SUPPORTED";

impl RpcError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
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
        Self::new(e.code, e.message)
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
#[serde(tag = "method", content = "params", rename_all = "camelCase")]
// `CoreRequest` in TypeScript, where `Request` is the DOM's.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "CoreRequest"))]
pub enum Request {
    Storage(StorageRequest),
    Secret(SecretRequest),
    License(crate::license::DesktopLicenseRequest),
    Git(crate::git::GitRequest),
    Ssh(crate::ssh::SshRequest),
    Db(crate::db::DbRequest),
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
    Secret(SecretResponse),
    License(crate::license::DesktopLicenseResponse),
    Git(crate::git::GitResponse),
    Ssh(crate::ssh::SshResponse),
    Db(crate::db::DbResponse),
}

impl Request {
    /// The group's wire name: `storage`, `secret`, `license`, `git`, `ssh`
    /// or `db`.
    pub fn group(&self) -> &'static str {
        match self {
            Request::Storage(_) => "storage",
            Request::Secret(_) => "secret",
            Request::License(_) => "license",
            Request::Git(_) => "git",
            Request::Ssh(_) => "ssh",
            Request::Db(_) => "db",
        }
    }

    /// The method's wire name within its group (`connectionsLoadAll`,
    /// `get`). With [`Request::group`], all a log line may say about a
    /// request.
    pub fn method(&self) -> &'static str {
        match self {
            Request::Storage(r) => r.method(),
            Request::Secret(r) => r.method(),
            Request::License(r) => r.method(),
            Request::Git(r) => r.method(),
            Request::Ssh(r) => r.method(),
            Request::Db(r) => r.method(),
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
        #[serde(tag = "method", content = "params", rename_all_fields = "camelCase")]
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
    // ai_chats
    AiChatsLoadByConnection = "aiChatsLoadByConnection" [{ connection_id: String }]
        -> [Vec<PersistedAIChat>],
    AiChatsSaveChat = "aiChatsSaveChat" [{ chat: PersistedAIChat }] -> [()],
    AiChatsRemoveChat = "aiChatsRemoveChat" [{ chat_id: String }] -> [()],
    AiChatsRemoveByConnection = "aiChatsRemoveByConnection" [{ connection_id: String }] -> [()],
    AiChatsLoadMessages = "aiChatsLoadMessages" [{ chat_id: String }]
        -> [Vec<PersistedAIMessage>],
    AiChatsReplaceAllMessages = "aiChatsReplaceAllMessages"
        [{ chat_id: String, messages: Vec<PersistedAIMessage> }] -> [()],

    // app_state
    AppStateGet = "appStateGet" [{ key: String }] -> [Option<String>],
    AppStateSet = "appStateSet" [{ key: String, value: Option<String> }] -> [()],

    // connection_overrides
    ConnectionOverridesLoad = "connectionOverridesLoad" [{ shared_connection_id: String }]
        -> [Option<PersistedConnectionOverride>],
    ConnectionOverridesLoadAll = "connectionOverridesLoadAll" []
        -> [Vec<PersistedConnectionOverride>],
    ConnectionOverridesSave = "connectionOverridesSave"
        [{ connection_override: PersistedConnectionOverride }] -> [()],
    ConnectionOverridesRemove = "connectionOverridesRemove" [{ shared_connection_id: String }]
        -> [()],

    // connections
    ConnectionsLoadAll = "connectionsLoadAll" [] -> [Vec<PersistedConnection>],
    ConnectionsSave = "connectionsSave" [{ connection: PersistedConnection }] -> [()],
    ConnectionsRemove = "connectionsRemove" [{ connection_id: String }] -> [()],

    // dashboard_versions
    DashboardVersionsLoadByDashboard = "dashboardVersionsLoadByDashboard"
        [{ dashboard_id: String }] -> [Vec<PersistedDashboardVersion>],
    DashboardVersionsLoadByProject = "dashboardVersionsLoadByProject" [{ project_id: String }]
        -> [Vec<PersistedDashboardVersion>],
    DashboardVersionsInsert = "dashboardVersionsInsert" [{ version: PersistedDashboardVersion }]
        -> [()],
    /// `params` is the prune itself: which versions go, and the snapshot
    /// the TypeScript computed for the oldest one kept.
    DashboardVersionsPrune = "dashboardVersionsPrune" [(DashboardVersionsPrune)] -> [()],

    // dashboards
    DashboardsLoadByProject = "dashboardsLoadByProject" [{ project_id: String }]
        -> [Vec<PersistedDashboard>],
    DashboardsSave = "dashboardsSave" [{ dashboard: PersistedDashboard }] -> [()],
    DashboardsRemove = "dashboardsRemove" [{ id: String }] -> [()],
    DashboardsRemoveByProject = "dashboardsRemoveByProject" [{ project_id: String }] -> [()],

    // import_state
    ImportStateLoad = "importStateLoad" [{ source: String }] -> [Option<ImportState>],
    ImportStateSave = "importStateSave"
        [{ source: String, has_offered_import: bool, last_check_timestamp: Option<String> }]
        -> [()],

    // license
    LicenseLoad = "licenseLoad" []
        -> [#[cfg_attr(feature = "ts", ts(type = "unknown"))] Option<Box<RawValue>>],
    LicenseSave = "licenseSave" [{
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        data: Box<RawValue>
    }] -> [()],

    // onboarding
    OnboardingLoad = "onboardingLoad" []
        -> [#[cfg_attr(feature = "ts", ts(type = "unknown"))] Option<Box<RawValue>>],
    OnboardingSave = "onboardingSave" [{
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        data: Box<RawValue>
    }] -> [()],

    // project_state
    ProjectStateLoad = "projectStateLoad" [{ project_id: String }]
        -> [Option<PersistedProjectState>],
    ProjectStateSave = "projectStateSave" [{ state: PersistedProjectState }] -> [()],
    ProjectStateRemove = "projectStateRemove" [{ project_id: String }] -> [()],

    // projects
    ProjectsLoadAll = "projectsLoadAll" [] -> [Vec<PersistedProject>],
    ProjectsSave = "projectsSave" [{ project: PersistedProject }] -> [()],
    ProjectsSaveAll = "projectsSaveAll" [{ projects: Vec<PersistedProject> }] -> [()],
    ProjectsRemove = "projectsRemove" [{ project_id: String }] -> [()],

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

    // query_versions
    QueryVersionsLoadByQuery = "queryVersionsLoadByQuery" [{ query_id: String }]
        -> [Vec<PersistedQueryVersion>],
    QueryVersionsLoadByProject = "queryVersionsLoadByProject" [{ project_id: String }]
        -> [Vec<PersistedQueryVersion>],
    QueryVersionsInsert = "queryVersionsInsert" [{ version: PersistedQueryVersion }] -> [()],
    /// `params` is the prune itself (see `DashboardVersionsPrune`).
    QueryVersionsPrune = "queryVersionsPrune" [(QueryVersionsPrune)] -> [()],

    // saved_queries
    SavedQueriesLoadByProject = "savedQueriesLoadByProject" [{ project_id: String }]
        -> [Vec<PersistedSavedQuery>],
    SavedQueriesSaveAll = "savedQueriesSaveAll"
        [{ project_id: String, queries: Vec<PersistedSavedQuery> }] -> [()],
    SavedQueriesRemoveByProject = "savedQueriesRemoveByProject" [{ project_id: String }]
        -> [()],

    // shared_repos
    SharedReposLoadAll = "sharedReposLoadAll" [] -> [SharedReposState],
    SharedReposSaveAll = "sharedReposSaveAll" [{
        #[cfg_attr(feature = "ts", ts(as = "Vec<seaquel_types::storage::PersistedSharedQueryRepo>"))]
        repos: Vec<Box<RawValue>>,
        active_repo_id: Option<String>
    }] -> [()],

    // themes
    ThemesLoadPreferences = "themesLoadPreferences" [] -> [Option<ThemePreferences>],
    ThemesSavePreferences = "themesSavePreferences"
        [{ light_theme_id: String, dark_theme_id: String }] -> [()],
    ThemesLoadUserThemes = "themesLoadUserThemes" []
        -> [#[cfg_attr(feature = "ts", ts(type = "Array<unknown>"))] Vec<Box<RawValue>>],
    ThemesSaveUserThemes = "themesSaveUserThemes" [{
        #[cfg_attr(feature = "ts", ts(type = "Array<unknown>"))]
        themes: Vec<Box<RawValue>>
    }] -> [()],

    // tutorial
    TutorialLoadAll = "tutorialLoadAll" [] -> [Vec<TutorialProgress>],
    TutorialSave = "tutorialSave"
        [{ lesson_id: String, challenge_id: String, state: Option<String> }] -> [()],
    TutorialRemoveLesson = "tutorialRemoveLesson" [{ lesson_id: String }] -> [()],
    TutorialRemoveAll = "tutorialRemoveAll" [] -> [()],

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

/// A secret call. Keys must be `db:<id>`, `ssh:<id>`, `ssh-key:<id>`,
/// `license-key` or `ai-api-key:<id>`; anything else is `INVALID_ARGUMENT`.
/// `Debug` never shows a value.
#[derive(Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "camelCase")]
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

/// Run one workspace call.
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
) -> Result<Response, RpcError> {
    let (group, method) = (req.group(), req.method());
    logged(group, method, async {
        match req {
            Request::Storage(r) => storage(ws, r).await.map(Response::Storage),
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
            Request::Db(r) => crate::db::db(core, ws, r).await.map(Response::Db),
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
async fn storage(_: &Workspace, _: StorageRequest) -> Result<StorageResponse, RpcError> {
    Err(RpcError::not_supported("Storage"))
}

#[cfg(feature = "storage")]
async fn storage(ws: &Workspace, req: StorageRequest) -> Result<StorageResponse, RpcError> {
    storage_call(ws, req).await.map_err(rpc_error)
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
        Q::AiChatsLoadByConnection { connection_id } => {
            R::AiChatsLoadByConnection(ai_chats::load_by_connection(st, &connection_id).await?)
        }
        Q::AiChatsSaveChat { chat } => R::AiChatsSaveChat(ai_chats::save_chat(st, &chat).await?),
        Q::AiChatsRemoveChat { chat_id } => {
            R::AiChatsRemoveChat(ai_chats::remove_chat(st, &chat_id).await?)
        }
        Q::AiChatsRemoveByConnection { connection_id } => {
            R::AiChatsRemoveByConnection(ai_chats::remove_by_connection(st, &connection_id).await?)
        }
        Q::AiChatsLoadMessages { chat_id } => {
            R::AiChatsLoadMessages(ai_chats::load_messages(st, &chat_id).await?)
        }
        Q::AiChatsReplaceAllMessages { chat_id, messages } => R::AiChatsReplaceAllMessages(
            ai_chats::replace_all_messages(st, &chat_id, &messages).await?,
        ),

        Q::AppStateGet { key } => R::AppStateGet(app_state::get(st, &key).await?),
        Q::AppStateSet { key, value } => {
            R::AppStateSet(app_state::set(st, &key, value.as_deref()).await?)
        }

        Q::ConnectionOverridesLoad {
            shared_connection_id,
        } => {
            R::ConnectionOverridesLoad(connection_overrides::load(st, &shared_connection_id).await?)
        }
        Q::ConnectionOverridesLoadAll => {
            R::ConnectionOverridesLoadAll(connection_overrides::load_all(st).await?)
        }
        Q::ConnectionOverridesSave {
            connection_override,
        } => {
            R::ConnectionOverridesSave(connection_overrides::save(st, &connection_override).await?)
        }
        Q::ConnectionOverridesRemove {
            shared_connection_id,
        } => R::ConnectionOverridesRemove(
            connection_overrides::remove(st, &shared_connection_id).await?,
        ),

        Q::ConnectionsLoadAll => R::ConnectionsLoadAll(connections::load_all(st).await?),
        Q::ConnectionsSave { connection } => {
            R::ConnectionsSave(connections::save(st, &connection).await?)
        }
        Q::ConnectionsRemove { connection_id } => {
            R::ConnectionsRemove(connections::remove(st, &connection_id).await?)
        }

        Q::DashboardVersionsLoadByDashboard { dashboard_id } => {
            R::DashboardVersionsLoadByDashboard(
                dashboard_versions::load_by_dashboard(st, &dashboard_id).await?,
            )
        }
        Q::DashboardVersionsLoadByProject { project_id } => R::DashboardVersionsLoadByProject(
            dashboard_versions::load_by_project(st, &project_id).await?,
        ),
        Q::DashboardVersionsInsert { version } => {
            R::DashboardVersionsInsert(dashboard_versions::insert(st, &version).await?)
        }
        Q::DashboardVersionsPrune(prune) => {
            R::DashboardVersionsPrune(dashboard_versions::prune(st, &prune).await?)
        }

        Q::DashboardsLoadByProject { project_id } => {
            R::DashboardsLoadByProject(dashboards::load_by_project(st, &project_id).await?)
        }
        Q::DashboardsSave { dashboard } => {
            R::DashboardsSave(dashboards::save(st, &dashboard).await?)
        }
        Q::DashboardsRemove { id } => R::DashboardsRemove(dashboards::remove(st, &id).await?),
        Q::DashboardsRemoveByProject { project_id } => {
            R::DashboardsRemoveByProject(dashboards::remove_by_project(st, &project_id).await?)
        }

        Q::ImportStateLoad { source } => R::ImportStateLoad(import_state::load(st, &source).await?),
        Q::ImportStateSave {
            source,
            has_offered_import,
            last_check_timestamp,
        } => R::ImportStateSave(
            import_state::save(
                st,
                &source,
                has_offered_import,
                last_check_timestamp.as_deref(),
            )
            .await?,
        ),

        Q::LicenseLoad => R::LicenseLoad(license::load(st).await?),
        Q::LicenseSave { data } => R::LicenseSave(license::save(st, &data).await?),

        Q::OnboardingLoad => R::OnboardingLoad(onboarding::load(st).await?),
        Q::OnboardingSave { data } => R::OnboardingSave(onboarding::save(st, &data).await?),

        Q::ProjectStateLoad { project_id } => {
            R::ProjectStateLoad(project_state::load(st, &project_id).await?)
        }
        Q::ProjectStateSave { state } => {
            R::ProjectStateSave(project_state::save(st, &state).await?)
        }
        Q::ProjectStateRemove { project_id } => {
            R::ProjectStateRemove(project_state::remove(st, &project_id).await?)
        }

        Q::ProjectsLoadAll => R::ProjectsLoadAll(projects::load_all(st).await?),
        Q::ProjectsSave { project } => R::ProjectsSave(projects::save(st, &project).await?),
        Q::ProjectsSaveAll { projects: all } => {
            R::ProjectsSaveAll(projects::save_all(st, &all).await?)
        }
        Q::ProjectsRemove { project_id } => {
            R::ProjectsRemove(projects::remove(st, &project_id).await?)
        }

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

        Q::QueryVersionsLoadByQuery { query_id } => {
            R::QueryVersionsLoadByQuery(query_versions::load_by_query(st, &query_id).await?)
        }
        Q::QueryVersionsLoadByProject { project_id } => {
            R::QueryVersionsLoadByProject(query_versions::load_by_project(st, &project_id).await?)
        }
        Q::QueryVersionsInsert { version } => {
            R::QueryVersionsInsert(query_versions::insert(st, &version).await?)
        }
        Q::QueryVersionsPrune(prune) => {
            R::QueryVersionsPrune(query_versions::prune(st, &prune).await?)
        }

        Q::SavedQueriesLoadByProject { project_id } => {
            R::SavedQueriesLoadByProject(saved_queries::load_by_project(st, &project_id).await?)
        }
        Q::SavedQueriesSaveAll {
            project_id,
            queries,
        } => R::SavedQueriesSaveAll(saved_queries::save_all(st, &project_id, &queries).await?),
        Q::SavedQueriesRemoveByProject { project_id } => {
            R::SavedQueriesRemoveByProject(saved_queries::remove_by_project(st, &project_id).await?)
        }

        Q::SharedReposLoadAll => R::SharedReposLoadAll(shared_repos::load_all(st).await?),
        Q::SharedReposSaveAll {
            repos,
            active_repo_id,
        } => R::SharedReposSaveAll(
            shared_repos::save_all(st, &repos, active_repo_id.as_deref()).await?,
        ),

        Q::ThemesLoadPreferences => R::ThemesLoadPreferences(themes::load_preferences(st).await?),
        Q::ThemesSavePreferences {
            light_theme_id,
            dark_theme_id,
        } => R::ThemesSavePreferences(
            themes::save_preferences(st, &light_theme_id, &dark_theme_id).await?,
        ),
        Q::ThemesLoadUserThemes => R::ThemesLoadUserThemes(themes::load_user_themes(st).await?),
        Q::ThemesSaveUserThemes { themes: all } => {
            R::ThemesSaveUserThemes(themes::save_user_themes(st, &all).await?)
        }

        Q::TutorialLoadAll => R::TutorialLoadAll(tutorial::load_all(st).await?),
        Q::TutorialSave {
            lesson_id,
            challenge_id,
            state,
        } => {
            R::TutorialSave(tutorial::save(st, &lesson_id, &challenge_id, state.as_deref()).await?)
        }
        Q::TutorialRemoveLesson { lesson_id } => {
            R::TutorialRemoveLesson(tutorial::remove_lesson(st, &lesson_id).await?)
        }
        Q::TutorialRemoveAll => R::TutorialRemoveAll(tutorial::remove_all(st).await?),

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
