//! State, settings, dashboards and chats (phase 5d-2), as Core writes them:
//! dashboards and their versions, saved workflows, AI chats and their
//! messages, the app-state settings, the AI settings record, themes,
//! onboarding, tutorial progress, import state, and each window's view
//! state with its legacy mirror.
//!
//! Like [`crate::library`], everything here is pure. It checks a call's
//! input against its rules and the interface's [`StateLimits`] (and
//! [`LibraryLimits`] for names and fields), turns drafts into stored rows,
//! applies patches, and rewrites the JSON records Core owns (`aiSettings`,
//! onboarding, a workflow's or theme's id and times) at their top level,
//! keeping every other field byte for byte. Core reads, checks and writes
//! inside one storage transaction and assigns ids and times.
//!
//! The rules are pinned by the fixtures in `tests/fixtures/state`, recorded
//! from the TypeScript this replaces; `changes.json` there lists where Core
//! is meant to differ.
//!
//! Nothing here does I/O or panics on its input or on stored data. No
//! `Debug` or error message shows a name, text, JSON, a setting's value or
//! a secret: ids, kinds, flags and counts only.

use std::collections::HashMap;
use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use serde_json::Value;

use seaquel_types::storage::{
    PersistedAIChat, PersistedAIMessage, PersistedDashboard, PersistedDashboardVersionMeta,
    PersistedProjectState, ThemePreferences,
};

use crate::library::{
    check_name, clearable, field, list_len, opt_field, present, short, within, Checked, Clearable,
    LibraryError, LibraryLimits,
};

pub use crate::library::INVALID_ARGUMENT;

// ── Codes ──

pub const DASHBOARD_NOT_FOUND: &str = "DASHBOARD_NOT_FOUND";
/// `dashboardVersionGet` naming a version its dashboard doesn't have.
pub const DASHBOARD_VERSION_NOT_FOUND: &str = "DASHBOARD_VERSION_NOT_FOUND";
pub const WORKFLOW_NOT_FOUND: &str = "WORKFLOW_NOT_FOUND";
pub const CHAT_NOT_FOUND: &str = "CHAT_NOT_FOUND";
pub const THEME_NOT_FOUND: &str = "THEME_NOT_FOUND";
pub const AI_PROVIDER_NOT_FOUND: &str = "AI_PROVIDER_NOT_FOUND";

// ── Fixed values ──

/// The id prefixes Core keeps, each followed by a v4 uuid.
/// Chats, messages and AI providers are plain uuids.
pub const DASHBOARD_ID_PREFIX: &str = "dashboard-";
pub const DASHBOARD_VERSION_ID_PREFIX: &str = "dver-";
pub const WORKFLOW_ID_PREFIX: &str = "workflow-";
pub const THEME_ID_PREFIX: &str = "theme-";

/// The `app_state` keys Core reads or writes for these calls.
pub const DASHBOARD_VERSION_LIMIT_KEY: &str = "dashboard_version_limit";
pub const LAST_ACTIVE_PROJECT_KEY: &str = "lastActiveProjectId";
pub const AI_SETTINGS_KEY: &str = "aiSettings";

/// An AI provider's keychain key is this plus its id (desktop).
pub const AI_API_KEY_PREFIX: &str = "ai-api-key:";
/// The web vault's scope for an AI provider's key (its `user_credentials`
/// rows, keyed by the provider id).
pub const AI_API_KEY_VAULT_SCOPE: &str = "ai-api-key-provider";

/// What a missing `theme_preferences` row reads as, and what removing the
/// theme in use resets a preference to.
pub const DEFAULT_LIGHT_THEME: &str = "default-light";
pub const DEFAULT_DARK_THEME: &str = "default-dark";

/// The desktop's main window: the prunes never delete it there.
pub const MAIN_WINDOW: &str = "main";
/// Windows unused this long are pruned.
pub const WINDOW_UNUSED_DAYS: u64 = 30;

/// The import sources `importStateSave` takes.
pub const IMPORT_SOURCES: [&str; 2] = ["tableplus", "dbeaver"];

/// An AI provider's types.
pub const AI_PROVIDER_TYPES: [&str; 2] = ["anthropic", "openai-compatible"];

// ── Limits ──

/// What one state call may carry and what a workspace may hold, set per
/// interface with Core's `CoreBuilder::state_limits`. The
/// default is [`StateLimits::DESKTOP`]: no size or count limit, and only
/// the window counts (20 windows, 20 view states per project, `main`
/// spared). The web server sets every one. A size past its limit is
/// refused with `INVALID_ARGUMENT` naming it before anything is read; the
/// counts are checked inside the write transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateLimits {
    /// One window's view state (tabs with their text, layout), in bytes.
    pub max_view_state_bytes: Option<usize>,
    /// One tab's text (a query tab's query, an explain tab's source, a
    /// table editor's definition), in bytes.
    pub max_tab_text_bytes: Option<usize>,
    /// Tabs in one view state.
    pub max_tabs: Option<usize>,
    /// The most windows kept; past them the least recently used go.
    pub max_windows: u32,
    /// The most view states of one project kept (one per window).
    pub max_window_states_per_project: u32,
    /// Never prune the window named [`MAIN_WINDOW`] (the desktop's).
    pub spare_main_window: bool,
    /// One saved workflow's stored JSON (after chart copies are dropped).
    pub max_workflow_bytes: Option<usize>,
    /// Saved workflows per user.
    pub max_workflows: Option<usize>,
    /// A dashboard's widgets, viewport and date filter together.
    pub max_dashboard_bytes: Option<usize>,
    /// Dashboards per user.
    pub max_dashboards: Option<usize>,
    /// One dashboard's versions together (as `max_version_bytes`).
    pub max_dashboard_version_bytes: Option<u64>,
    /// One AI message's content.
    pub max_message_bytes: Option<usize>,
    /// Messages per chat (and per put).
    pub max_messages_per_chat: Option<usize>,
    /// One chat's stored message content together.
    pub max_chat_bytes: Option<u64>,
    /// Chats per user.
    pub max_chats: Option<usize>,
    /// A setting's value, the AI settings record, one user theme, the
    /// onboarding record, one tutorial state.
    pub max_setting_bytes: Option<usize>,
    /// User themes per user.
    pub max_user_themes: Option<usize>,
    /// AI providers in the AI settings record.
    pub max_ai_providers: Option<usize>,
}

impl StateLimits {
    /// The desktop's (and the CLI's and MCP's): no limit but the window
    /// counts.
    pub const DESKTOP: StateLimits = StateLimits {
        max_view_state_bytes: None,
        max_tab_text_bytes: None,
        max_tabs: None,
        max_windows: 20,
        max_window_states_per_project: 20,
        spare_main_window: true,
        max_workflow_bytes: None,
        max_workflows: None,
        max_dashboard_bytes: None,
        max_dashboards: None,
        max_dashboard_version_bytes: None,
        max_message_bytes: None,
        max_messages_per_chat: None,
        max_chat_bytes: None,
        max_chats: None,
        max_setting_bytes: None,
        max_user_themes: None,
        max_ai_providers: None,
    };
}

impl Default for StateLimits {
    fn default() -> Self {
        Self::DESKTOP
    }
}

fn setting_bytes(s: &str, what: &str, limits: &StateLimits) -> Checked {
    within(s, what, limits.max_setting_bytes, "max_setting_bytes")
}

/// `INVALID_ARGUMENT` when a byte count passes `limit` (named `name`).
fn bytes_within(len: usize, what: &str, limit: Option<usize>, name: &str) -> Checked {
    match limit {
        Some(max) if len > max => Err(LibraryError::invalid(format!(
            "The {what} is larger than allowed here ({name}: {max} bytes)."
        ))),
        _ => Ok(()),
    }
}

/// `INVALID_ARGUMENT` when `count` items pass `limit` (named `name`).
fn count_within(count: usize, what: &str, limit: Option<usize>, name: &str) -> Checked {
    match limit {
        Some(max) if count > max => Err(LibraryError::invalid(format!(
            "There are more {what} than allowed here ({name}: {max})."
        ))),
        _ => Ok(()),
    }
}

/// A JSON value's own text has no NUL character (inside a string it is
/// written `\u0000`, which is allowed: the stored text never holds one).
fn raw_text(raw: &RawValue) -> &str {
    raw.get()
}

// ── JSON objects kept in order, values byte for byte ──

/// A JSON object's fields in their stored order, each value as its stored
/// text. A repeated key keeps its first place and its last value, as
/// `JSON.parse` does.
pub(crate) type Obj = Vec<(String, Box<RawValue>)>;

struct OrderedObj(Obj);

impl<'de> Deserialize<'de> for OrderedObj {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = OrderedObj;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<OrderedObj, A::Error> {
                let mut out: Obj = Vec::new();
                let mut at: HashMap<String, usize> = HashMap::new();
                while let Some((k, v)) = map.next_entry::<String, Box<RawValue>>()? {
                    match at.get(&k) {
                        Some(&i) => out[i].1 = v,
                        None => {
                            at.insert(k.clone(), out.len());
                            out.push((k, v));
                        }
                    }
                }
                Ok(OrderedObj(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// `text` as an object's fields, or `None` when it isn't a JSON object.
pub(crate) fn parse_obj(text: &str) -> Option<Obj> {
    serde_json::from_str::<OrderedObj>(text).ok().map(|o| o.0)
}

fn obj_get<'a>(obj: &'a Obj, key: &str) -> Option<&'a RawValue> {
    obj.iter().find(|(k, _)| k == key).map(|(_, v)| &**v)
}

/// Sets `key` in place, or adds it at the end.
fn obj_set(obj: &mut Obj, key: &str, value: Box<RawValue>) {
    match obj.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value,
        None => obj.push((key.to_string(), value)),
    }
}

fn obj_remove(obj: &mut Obj, key: &str) -> Option<Box<RawValue>> {
    let i = obj.iter().position(|(k, _)| k == key)?;
    Some(obj.remove(i).1)
}

/// The object's JSON text, its fields in order.
fn render_obj(obj: &Obj) -> String {
    let mut out = String::with_capacity(
        obj.iter()
            .map(|(k, v)| k.len() + v.get().len() + 4)
            .sum::<usize>()
            + 2,
    );
    out.push('{');
    for (i, (k, v)) in obj.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(k).unwrap_or_else(|_| "\"\"".to_string()));
        out.push(':');
        out.push_str(v.get());
    }
    out.push('}');
    out
}

/// `{...defaults, ...parsed}` in linear time: `defaults` holds a few known
/// keys; a parsed field with one of them replaces it in place, any other is
/// appended (`parse_obj` gives each key once).
fn overlay(defaults: &mut Obj, parsed: Obj) {
    let known = defaults.len();
    for (k, v) in parsed {
        match defaults[..known].iter_mut().find(|(d, _)| *d == k) {
            Some(slot) => slot.1 = v,
            None => defaults.push((k, v)),
        }
    }
}

/// A literal JSON value, made from text Core wrote itself (`null` if it
/// somehow isn't JSON).
fn lit(json: &str) -> Box<RawValue> {
    RawValue::from_string(json.to_string()).unwrap_or_default()
}

fn null() -> Box<RawValue> {
    Box::<RawValue>::default()
}

/// A string's JSON value.
fn json_str(s: &str) -> Box<RawValue> {
    lit(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()))
}

fn json_bool(b: bool) -> Box<RawValue> {
    lit(if b { "true" } else { "false" })
}

/// The JSON text as a `RawValue`, when it is JSON.
pub fn to_raw(text: String) -> Option<Box<RawValue>> {
    RawValue::from_string(text).ok()
}

fn is_json_null(v: &RawValue) -> bool {
    v.get().trim() == "null"
}

/// The first byte of a JSON value (`{`, `[`, `"`, …).
fn json_kind(v: &RawValue) -> u8 {
    v.get()
        .trim_start()
        .as_bytes()
        .first()
        .copied()
        .unwrap_or(b' ')
}

fn string_of(v: &RawValue) -> Option<String> {
    serde_json::from_str::<String>(v.get()).ok()
}

/// A body's `name` (a workflow's, a user theme's), which must be text:
/// missing or `null` is "needs a name", another JSON type "is text", and a
/// JSON string that doesn't decode (a lone surrogate, `"\ud800"`, which
/// JavaScript strings can hold and Rust's can't) says so (5d-2 Task 7
/// That was reported as a missing name).
fn name_of(obj: &Obj, what: &str) -> Result<String, LibraryError> {
    let value = obj_get(obj, "name").filter(|v| v.get() != "null");
    let Some(value) = value else {
        return Err(LibraryError::invalid(format!("A {what} needs a name.")));
    };
    if let Some(name) = string_of(value) {
        return Ok(name);
    }
    if value.get().starts_with('"') {
        return Err(LibraryError::invalid(format!(
            "A {what}'s name holds a lone surrogate (half of a character), which can't be stored."
        )));
    }
    Err(LibraryError::invalid(format!("A {what}'s name is text.")))
}

// ── Ids a GUI makes ──

/// A window id or a message id: 1–64 of
/// `[A-Za-z0-9_-]`, the origin's form.
pub fn is_client_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn check_window_id(id: &str) -> Checked {
    if is_client_id(id) {
        Ok(())
    } else {
        Err(LibraryError::invalid(
            "A window id is 1 to 64 letters, digits, '-' or '_'.",
        ))
    }
}

// ── Settings ──

/// The app-state keys the `settings` group reads and writes, by their
/// stored key text. Any other key is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SettingKey {
    /// `default`, `vim` or `emacs`.
    #[serde(rename = "editorKeybindingMode")]
    EditorKeybindingMode,
    /// `"true"` or `"false"`.
    #[serde(rename = "pending_changes_enabled")]
    PendingChangesEnabled,
    /// A version string.
    #[serde(rename = "skippedUpdateVersion")]
    SkippedUpdateVersion,
    /// `stable` or `beta`: which feed the desktop updater asks. Unset is
    /// the app's own default (a pre-release build follows beta).
    #[serde(rename = "updateChannel")]
    UpdateChannel,
    /// A whole number from 0 to 100,000, as text; 0 keeps every version.
    #[serde(rename = "query_version_limit")]
    QueryVersionLimit,
    /// As `query_version_limit`, for dashboards.
    #[serde(rename = "dashboard_version_limit")]
    DashboardVersionLimit,
    /// A JSON object, kept as it is.
    #[serde(rename = "license_nudge")]
    LicenseNudge,
    /// Read only: `windowActivate` writes it.
    #[serde(rename = "lastActiveProjectId")]
    LastActiveProjectId,
    /// Core's: read, and cleared with `null`.
    #[serde(rename = "connectionStringSecretsNotice")]
    ConnectionStringSecretsNotice,
}

impl SettingKey {
    pub const ALL: [SettingKey; 9] = [
        SettingKey::EditorKeybindingMode,
        SettingKey::PendingChangesEnabled,
        SettingKey::SkippedUpdateVersion,
        SettingKey::UpdateChannel,
        SettingKey::QueryVersionLimit,
        SettingKey::DashboardVersionLimit,
        SettingKey::LicenseNudge,
        SettingKey::LastActiveProjectId,
        SettingKey::ConnectionStringSecretsNotice,
    ];

    /// The stored key.
    pub fn as_str(self) -> &'static str {
        match self {
            SettingKey::EditorKeybindingMode => "editorKeybindingMode",
            SettingKey::PendingChangesEnabled => "pending_changes_enabled",
            SettingKey::SkippedUpdateVersion => "skippedUpdateVersion",
            SettingKey::UpdateChannel => "updateChannel",
            SettingKey::QueryVersionLimit => "query_version_limit",
            SettingKey::DashboardVersionLimit => "dashboard_version_limit",
            SettingKey::LicenseNudge => "license_nudge",
            SettingKey::LastActiveProjectId => LAST_ACTIVE_PROJECT_KEY,
            SettingKey::ConnectionStringSecretsNotice => "connectionStringSecretsNotice",
        }
    }

    /// The setting stored under `key`, or `INVALID_ARGUMENT` naming it
    /// (unknown keys, and the keys Core or the storage group own).
    pub fn parse(key: &str) -> Result<SettingKey, LibraryError> {
        Self::ALL
            .into_iter()
            .find(|k| k.as_str() == key)
            .ok_or_else(|| {
                LibraryError::invalid(format!(
                    "{} isn't a setting that can be read or written here.",
                    key_for_message(key)
                ))
            })
    }
}

/// A key for an error message: the key quoted when it is short, else a
/// note of its length (a key is the caller's text).
fn key_for_message(key: &str) -> String {
    if key.len() <= 64 && !key.chars().any(char::is_control) {
        format!("The key {key:?}")
    } else {
        format!("A key of {} bytes", key.len())
    }
}

/// `settingSet`'s checks, before anything is read: the key is one of the
/// closed set and writable, and the value is one it takes (`None` deletes
/// the row and is always allowed but for `lastActiveProjectId`).
pub fn check_setting_set(
    key: &str,
    value: Option<&str>,
    lib: &LibraryLimits,
    limits: &StateLimits,
) -> Result<SettingKey, LibraryError> {
    let k = SettingKey::parse(key)?;
    let refuse = |why: &str| {
        Err(LibraryError::invalid(format!(
            "{} {why}",
            key_for_message(k.as_str())
        )))
    };
    if k == SettingKey::LastActiveProjectId {
        return refuse("is written when a window activates a project, not set directly.");
    }
    let Some(v) = value else {
        return Ok(k);
    };
    if let Some(max) = limits.max_setting_bytes.filter(|max| v.len() > *max) {
        return refuse(&format!(
            "has a value larger than allowed here (max_setting_bytes: {max} bytes)."
        ));
    }
    let ok = match k {
        SettingKey::EditorKeybindingMode => ["default", "vim", "emacs"].contains(&v),
        SettingKey::PendingChangesEnabled => v == "true" || v == "false",
        SettingKey::UpdateChannel => v == "stable" || v == "beta",
        SettingKey::SkippedUpdateVersion => short(v, "version", lib).is_ok(),
        _ if v.contains('\0') => false,
        SettingKey::QueryVersionLimit | SettingKey::DashboardVersionLimit => is_version_limit(v),
        SettingKey::LicenseNudge => parse_obj(v).is_some(),
        SettingKey::ConnectionStringSecretsNotice => {
            return refuse("can only be cleared (set to null).");
        }
        SettingKey::LastActiveProjectId => false,
    };
    if ok {
        Ok(k)
    } else {
        refuse("can't hold this value.")
    }
}

/// A whole number from 0 to 100,000 written in digits.
fn is_version_limit(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 6
        && v.bytes().all(|b| b.is_ascii_digit())
        && v.parse::<u32>().is_ok_and(|n| n <= 100_000)
}

// ── AI settings ──

/// A new AI provider (`aiProviderCreate`). Core makes its id (a uuid).
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AiProviderDraft {
    pub name: String,
    #[serde(rename = "type")]
    #[cfg_attr(feature = "ts", ts(type = "\"anthropic\" | \"openai-compatible\""))]
    pub ty: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub base_url: Option<String>,
}

impl fmt::Debug for AiProviderDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiProviderDraft")
            .field("known_type", &AI_PROVIDER_TYPES.contains(&self.ty.as_str()))
            .field("base_url", &self.base_url.is_some())
            .finish_non_exhaustive()
    }
}

/// A change to an AI provider (`aiProviderUpdate`).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AiProviderPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub name: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "ts",
        ts(optional, type = "\"anthropic\" | \"openai-compatible\"")
    )]
    pub ty: Option<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub base_url: Clearable<String>,
}

impl fmt::Debug for AiProviderPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiProviderPatch")
            .field("name", &self.name.is_some())
            .field(
                "known_type",
                &self.ty.as_deref().map(|t| AI_PROVIDER_TYPES.contains(&t)),
            )
            .field("base_url", &present(&self.base_url))
            .finish()
    }
}

/// A change to the AI settings' flags (`aiSettingsPatch`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AiSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub share_schema_globally: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub share_data_globally: Option<bool>,
}

/// What `aiProviderCreate` answers: the new provider's id and the whole
/// record.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AiProviderCreated {
    pub id: String,
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub settings: Box<RawValue>,
}

impl fmt::Debug for AiProviderCreated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiProviderCreated")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

fn check_provider_type(ty: &str) -> Checked {
    if AI_PROVIDER_TYPES.contains(&ty) {
        Ok(())
    } else {
        Err(LibraryError::invalid(
            "An AI provider's type must be anthropic or openai-compatible.",
        ))
    }
}

pub fn check_ai_provider_draft(d: &AiProviderDraft, limits: &LibraryLimits) -> Checked {
    check_name(&d.name, "AI provider", limits)?;
    check_provider_type(&d.ty)?;
    opt_field(d.base_url.as_deref(), "base URL", limits)
}

pub fn check_ai_provider_patch(p: &AiProviderPatch, limits: &LibraryLimits) -> Checked {
    if let Some(name) = &p.name {
        check_name(name, "AI provider", limits)?;
    }
    if let Some(ty) = &p.ty {
        check_provider_type(ty)?;
    }
    opt_field(
        p.base_url.as_ref().and_then(Option::as_deref),
        "base URL",
        limits,
    )
}

/// An API key's own check (desktop): no NUL, bounded like a field.
pub fn check_api_key(key: &Clearable<String>, limits: &LibraryLimits) -> Checked {
    match key {
        Some(Some(k)) => field(k, "API key", limits),
        _ => Ok(()),
    }
}

/// The AI settings record as Core reads and rewrites it:
/// the four known fields (`enabled`, `providers`, `shareSchemaGlobally`,
/// `shareDataGlobally`) first, each as stored or its default, then every
/// other stored field in its order, values byte for byte. The providers
/// are cleaned of their legacy fields.
#[derive(Clone)]
pub struct AiSettings {
    /// The fields in order; `providers`' value here is ignored and
    /// rendered from [`AiSettings::providers`].
    fields: Obj,
    providers: Vec<Obj>,
}

impl fmt::Debug for AiSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiSettings")
            .field("fields", &self.fields.len())
            .field("providers", &self.providers.len())
            .finish()
    }
}

const PROVIDERS: &str = "providers";

impl AiSettings {
    /// `DEFAULT_AI_SETTINGS`.
    pub fn defaults() -> Self {
        Self {
            fields: vec![
                ("enabled".into(), lit("true")),
                (PROVIDERS.into(), lit("[]")),
                ("shareSchemaGlobally".into(), lit("true")),
                ("shareDataGlobally".into(), lit("false")),
            ],
            providers: Vec::new(),
        }
    }

    /// The record as JSON text.
    pub fn to_json(&self) -> String {
        let providers = format!(
            "[{}]",
            self.providers
                .iter()
                .map(render_obj)
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut fields = self.fields.clone();
        obj_set(&mut fields, PROVIDERS, lit(&providers));
        render_obj(&fields)
    }

    pub fn to_raw(&self) -> Box<RawValue> {
        lit(&self.to_json())
    }

    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    fn provider_index(&self, id: &str) -> Option<usize> {
        self.providers
            .iter()
            .position(|p| obj_get(p, "id").and_then(string_of).as_deref() == Some(id))
    }

    pub fn has_provider(&self, id: &str) -> bool {
        self.provider_index(id).is_some()
    }

    pub fn apply_patch(&mut self, p: &AiSettingsPatch) {
        for (key, v) in [
            ("enabled", p.enabled),
            ("shareSchemaGlobally", p.share_schema_globally),
            ("shareDataGlobally", p.share_data_globally),
        ] {
            if let Some(v) = v {
                obj_set(&mut self.fields, key, json_bool(v));
            }
        }
    }

    /// Appends a provider `{id, name, type, baseUrl?}`.
    pub fn add_provider(&mut self, id: &str, d: &AiProviderDraft) {
        let mut p: Obj = vec![
            ("id".into(), json_str(id)),
            ("name".into(), json_str(&d.name)),
            ("type".into(), json_str(&d.ty)),
        ];
        if let Some(url) = &d.base_url {
            p.push(("baseUrl".into(), json_str(url)));
        }
        self.providers.push(p);
    }

    /// Changes only the fields the patch names, in place; the provider's
    /// other fields stay byte for byte.
    pub fn update_provider(&mut self, id: &str, patch: &AiProviderPatch) -> Checked {
        let i = self.provider_index(id).ok_or_else(provider_not_found)?;
        let p = &mut self.providers[i];
        if let Some(name) = &patch.name {
            obj_set(p, "name", json_str(name));
        }
        if let Some(ty) = &patch.ty {
            obj_set(p, "type", json_str(ty));
        }
        match &patch.base_url {
            Some(Some(url)) => obj_set(p, "baseUrl", json_str(url)),
            Some(None) => {
                obj_remove(p, "baseUrl");
            }
            None => {}
        }
        Ok(())
    }

    /// `enabled` as the GUI reads it: the stored value's JavaScript
    /// truthiness, `true` when absent (phase 6).
    pub fn enabled(&self) -> bool {
        obj_get(&self.fields, "enabled")
            .and_then(|v| serde_json::from_str::<serde_json::Value>(v.get()).ok())
            .is_none_or(|v| match v {
                serde_json::Value::Null => false,
                serde_json::Value::Bool(b) => b,
                serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
                serde_json::Value::String(s) => !s.is_empty(),
                serde_json::Value::Array(_) | serde_json::Value::Object(_) => true,
            })
    }

    /// A provider's type and base URL, as a turn reads them (phase 6).
    pub fn provider(&self, id: &str) -> Option<ProviderInfo> {
        let p = &self.providers[self.provider_index(id)?];
        Some(ProviderInfo {
            ty: obj_get(p, "type")
                .and_then(string_of)
                .unwrap_or_else(|| "anthropic".to_string()),
            base_url: obj_get(p, "baseUrl").and_then(string_of),
        })
    }

    pub fn remove_provider(&mut self, id: &str) -> Checked {
        let i = self.provider_index(id).ok_or_else(provider_not_found)?;
        self.providers.remove(i);
        Ok(())
    }
}

/// A provider as a model call needs it (phase 6): `anthropic` or
/// `openai-compatible` (anything else is read as Anthropic's wire), and
/// its base URL. Its `Debug` leaves out the URL, which may carry a key.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderInfo {
    pub ty: String,
    pub base_url: Option<String>,
}

impl fmt::Debug for ProviderInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderInfo")
            .field("ty", &self.ty)
            .field("base_url", &self.base_url.is_some())
            .finish()
    }
}

pub fn provider_not_found() -> LibraryError {
    LibraryError::new(AI_PROVIDER_NOT_FOUND, "AI provider not found.")
}

/// The stored `aiSettings` text as the GUI's load read it:
/// `{...DEFAULT_AI_SETTINGS, ...parsed, providers}` with each provider's
/// `model` and `provider` dropped and `type = type ?? provider ??
/// "anthropic"`. No value, `""`, text that isn't JSON, a value that isn't
/// an object, a `providers` that is neither absent, `null` nor an array,
/// and a `null` provider all read as the defaults, exactly the cases
/// `seaquel-mcp`'s `global_sharing_from` reads as defaults.
///
/// A provider that is neither an object nor `null` (a hand-edited record)
/// reads as `{"type": "anthropic"}`.
pub fn read_ai_settings(raw: Option<&str>) -> AiSettings {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return AiSettings::defaults();
    };
    let Some(parsed) = parse_obj(raw) else {
        return AiSettings::defaults();
    };
    let providers = match obj_get(&parsed, PROVIDERS) {
        None => Vec::new(),
        Some(v) if is_json_null(v) => Vec::new(),
        Some(v) => match serde_json::from_str::<Vec<Box<RawValue>>>(v.get()) {
            Ok(items) if !items.iter().any(|p| is_json_null(p)) => {
                items.iter().map(|p| clean_provider(p)).collect()
            }
            _ => return AiSettings::defaults(),
        },
    };
    let mut settings = AiSettings::defaults();
    overlay(&mut settings.fields, parsed);
    settings.providers = providers;
    settings
}

/// `{ ...rest, type: rest.type ?? provider ?? "anthropic" }` without
/// `model` and `provider`.
fn clean_provider(p: &RawValue) -> Obj {
    let Some(mut obj) = (json_kind(p) == b'{').then(|| parse_obj(p.get())).flatten() else {
        return vec![("type".into(), json_str("anthropic"))];
    };
    obj_remove(&mut obj, "model");
    let legacy = obj_remove(&mut obj, "provider").filter(|v| !is_json_null(v));
    let has_type = obj_get(&obj, "type").is_some_and(|v| !is_json_null(v));
    if !has_type {
        let ty = legacy.unwrap_or_else(|| json_str("anthropic"));
        obj_set(&mut obj, "type", ty);
    }
    obj
}

/// The rewritten record's size check (`max_setting_bytes`).
pub fn check_ai_settings_size(settings: &AiSettings, limits: &StateLimits) -> Checked {
    bytes_within(
        settings.to_json().len(),
        "AI settings record",
        limits.max_setting_bytes,
        "max_setting_bytes",
    )
}

// ── Onboarding ──

/// The onboarding store's fields and their defaults, in its order.
const ONBOARDING_DEFAULTS: [(&str, &str); 6] = [
    ("isFirstRun", "true"),
    ("userBackground", "\"none\""),
    ("hasCompletedWizard", "false"),
    ("showWizardHints", "true"),
    ("dismissedHints", "[]"),
    ("learnEnabled", "true"),
];

/// The onboarding record as read: the six defaults with the stored
/// object's fields over them (and any other stored field after them). A
/// record that is missing or isn't an object reads as the defaults.
pub fn read_onboarding(stored: Option<&RawValue>) -> Box<RawValue> {
    lit(&render_obj(&onboarding_fields(stored)))
}

fn onboarding_fields(stored: Option<&RawValue>) -> Obj {
    let mut obj: Obj = ONBOARDING_DEFAULTS
        .iter()
        .map(|(k, v)| ((*k).to_string(), lit(v)))
        .collect();
    if let Some(stored) = stored.and_then(|s| parse_obj(s.get())) {
        overlay(&mut obj, stored);
    }
    obj
}

/// The backgrounds the onboarding offers (`UserBackground`).
pub const ONBOARDING_BACKGROUNDS: [&str; 3] = ["none", "datagrip", "dbeaver"];

/// `onboardingPatch`'s checks: a JSON object whose fields are among the
/// store's six, each of its type (`dismissedHints` a list of strings,
/// `userBackground` one of [`ONBOARDING_BACKGROUNDS`]).
pub fn check_onboarding_patch(
    patch: &RawValue,
    lib: &LibraryLimits,
    limits: &StateLimits,
) -> Checked {
    setting_bytes(raw_text(patch), "onboarding change", limits)?;
    let Some(obj) = parse_obj(patch.get()) else {
        return Err(LibraryError::invalid(
            "An onboarding change is a JSON object.",
        ));
    };
    for (k, v) in &obj {
        let ok = match k.as_str() {
            "isFirstRun" | "hasCompletedWizard" | "showWizardHints" | "learnEnabled" => {
                serde_json::from_str::<bool>(v.get()).is_ok()
            }
            // The GUI's `UserBackground` (`stores/onboarding.svelte.ts`).
            "userBackground" => {
                string_of(v).is_some_and(|s| ONBOARDING_BACKGROUNDS.contains(&s.as_str()))
            }
            "dismissedHints" => match serde_json::from_str::<Vec<String>>(v.get()) {
                Ok(hints) => {
                    list_len(hints.len(), "hints", lib)?;
                    hints.iter().all(|h| short(h, "hint", lib).is_ok())
                }
                Err(_) => false,
            },
            _ => {
                return Err(LibraryError::invalid(
                    "An onboarding change names a field onboarding doesn't have.",
                ))
            }
        };
        if !ok {
            return Err(LibraryError::invalid(
                "An onboarding field has a value of the wrong type.",
            ));
        }
    }
    Ok(())
}

/// The stored record after `patch` (checked) replaces its top-level
/// fields, read as [`read_onboarding`] reads it.
pub fn merge_onboarding(stored: Option<&RawValue>, patch: &RawValue) -> Box<RawValue> {
    let mut obj = onboarding_fields(stored);
    for (k, v) in parse_obj(patch.get()).unwrap_or_default() {
        obj_set(&mut obj, &k, v);
    }
    lit(&render_obj(&obj))
}

// ── Themes ──

/// What the theme calls answer: the preferences (the defaults when there
/// is no row) and every user theme that reads, as stored.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Themes {
    pub preferences: ThemePreferences,
    #[cfg_attr(feature = "ts", ts(type = "Array<unknown>"))]
    pub user_themes: Vec<Box<RawValue>>,
}

impl fmt::Debug for Themes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Themes")
            .field("user_themes", &self.user_themes.len())
            .finish_non_exhaustive()
    }
}

/// What `userThemeCreate` answers: the new theme's id and every theme.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ThemeCreated {
    pub id: String,
    pub themes: Themes,
}

pub fn default_preferences() -> ThemePreferences {
    ThemePreferences {
        light_theme_id: DEFAULT_LIGHT_THEME.to_string(),
        dark_theme_id: DEFAULT_DARK_THEME.to_string(),
    }
}

/// A user theme's own checks: a JSON object within `max_setting_bytes`,
/// with a `name` string (bounded) and no `isBuiltIn: true` (built-in
/// themes aren't stored).
pub fn check_user_theme(theme: &RawValue, lib: &LibraryLimits, limits: &StateLimits) -> Checked {
    setting_bytes(raw_text(theme), "theme", limits)?;
    let Some(obj) = parse_obj(theme.get()) else {
        return Err(LibraryError::invalid("A theme is a JSON object."));
    };
    short(&name_of(&obj, "theme")?, "theme name", lib)?;
    if obj_get(&obj, "isBuiltIn").is_some_and(|v| v.get().trim() == "true") {
        return Err(LibraryError::invalid("Built-in themes aren't stored."));
    }
    Ok(())
}

/// The stored JSON of a user theme: the body's fields but `id`,
/// `createdAt` and `updatedAt`, then those three as Core sets them (the
/// `id` equal to the row's id, which today's load reads from the JSON),
/// and `isBuiltIn: false` when the body leaves it out.
pub fn user_theme_json(theme: &RawValue, id: &str, created_at: &str, updated_at: &str) -> String {
    let mut obj = parse_obj(theme.get()).unwrap_or_default();
    for k in ["id", "createdAt", "updatedAt"] {
        obj_remove(&mut obj, k);
    }
    obj.push(("id".into(), json_str(id)));
    if obj_get(&obj, "isBuiltIn").is_none() {
        obj.push(("isBuiltIn".into(), json_bool(false)));
    }
    obj.push(("createdAt".into(), json_str(created_at)));
    obj.push(("updatedAt".into(), json_str(updated_at)));
    render_obj(&obj)
}

/// A stored JSON object's `createdAt` string, if it has one.
pub fn created_at_of(stored: Option<&RawValue>) -> Option<String> {
    let obj = parse_obj(stored?.get())?;
    obj_get(&obj, "createdAt").and_then(string_of)
}

/// A stored JSON object's `id` string, if it has one.
pub fn id_of(stored: &RawValue) -> Option<String> {
    let obj = parse_obj(stored.get())?;
    obj_get(&obj, "id").and_then(string_of)
}

/// A theme id (`themePreferencesSet`, a user theme's id): bounded, no NUL.
pub fn check_theme_id(id: &str, lib: &LibraryLimits) -> Checked {
    short(id, "theme id", lib)?;
    if id.is_empty() {
        return Err(LibraryError::invalid("A theme id can't be empty."));
    }
    Ok(())
}

// ── Saved workflows ──

/// A new saved workflow (`workflowCreate`): the project and today's
/// `SavedWorkflow` JSON without `id`, `projectId`, `createdAt` and
/// `updatedAt`, which Core sets.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct WorkflowDraft {
    pub project_id: String,
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub workflow: Box<RawValue>,
}

impl fmt::Debug for WorkflowDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkflowDraft")
            .field("project_id", &self.project_id)
            .field("workflow_bytes", &self.workflow.get().len())
            .finish()
    }
}

/// A workflow body's own checks: a JSON object with a bounded text
/// `name`, and on a create within `max_workflow_bytes`
/// ([`check_workflow_size`] for an update, which compares with the stored
/// workflow).
pub fn check_workflow_body(body: &RawValue, lib: &LibraryLimits, limits: &StateLimits) -> Checked {
    bytes_within(
        body.get().len(),
        "workflow",
        limits.max_workflow_bytes,
        "max_workflow_bytes",
    )?;
    check_workflow_shape(body, lib)
}

/// A workflow body's shape: a JSON object with a bounded text `name`
/// (a `name` of `5`, or none, was stored).
pub fn check_workflow_shape(body: &RawValue, lib: &LibraryLimits) -> Checked {
    let Some(obj) = parse_obj(body.get()) else {
        return Err(LibraryError::invalid("A workflow is a JSON object."));
    };
    short(&name_of(&obj, "workflow")?, "workflow name", lib)
}

/// The stored JSON of a workflow: `id` first, then the body's fields but
/// `id`, `projectId`, `createdAt` and `updatedAt` byte for byte, then
/// those three as Core sets them.
pub fn workflow_json(
    body: &RawValue,
    id: &str,
    project_id: &str,
    created_at: &str,
    updated_at: &str,
) -> String {
    let mut obj: Obj = vec![("id".into(), json_str(id))];
    for (k, v) in parse_obj(body.get()).unwrap_or_default() {
        if !["id", "projectId", "createdAt", "updatedAt"].contains(&k.as_str()) {
            obj.push((k, v));
        }
    }
    obj.push(("projectId".into(), json_str(project_id)));
    obj.push(("createdAt".into(), json_str(created_at)));
    obj.push(("updatedAt".into(), json_str(updated_at)));
    render_obj(&obj)
}

/// A rename's name (`workflowRename`): not empty
/// after trimming, no NUL, within `max_name_bytes`.
pub fn check_workflow_rename(name: &str, lib: &LibraryLimits) -> Checked {
    check_name(name, "workflow", lib)
}

/// The stored workflow JSON renamed: `name` set to `name` and `updatedAt`
/// to `updated_at` where they are (added at the end when missing), every
/// other field byte for byte. `None` when the stored JSON isn't an object.
/// Done on the stored row inside the rename's write, so a save another
/// window made just before it stays.
pub fn rename_workflow_json(stored: &RawValue, name: &str, updated_at: &str) -> Option<String> {
    let mut obj = parse_obj(stored.get())?;
    obj_set(&mut obj, "name", json_str(name));
    obj_set(&mut obj, "updatedAt", json_str(updated_at));
    Some(render_obj(&obj))
}

/// The stored workflow's size check, after Core set its fields. Past
/// `max_workflow_bytes` it is still accepted when no larger than the stored
/// workflow it replaces (`stored`, its bytes), so a workflow stored before
/// the limit stays saveable and can shrink.
pub fn check_workflow_size(json: &str, stored: Option<usize>, limits: &StateLimits) -> Checked {
    if stored.is_some_and(|b| json.len() <= b) {
        return Ok(());
    }
    bytes_within(
        json.len(),
        "workflow",
        limits.max_workflow_bytes,
        "max_workflow_bytes",
    )
}

// ── Dashboards ──

/// A new dashboard (`dashboardCreate`). Widgets, viewport and date filter
/// are JSON values Core stores as they are.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DashboardDraft {
    pub project_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub description: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub widgets: Box<RawValue>,
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub viewport: Box<RawValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "unknown"))]
    pub date_filter: Option<Box<RawValue>>,
    /// Stored shared (a dashboard the git reconcile found), in one call.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub shared: bool,
    /// A taken name becomes the first free `"<name> (n)"` instead of
    /// `NAME_TAKEN` (as the library's imports): "New
    /// Dashboard" and the git reconcile.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub rename_if_taken: bool,
}

impl fmt::Debug for DashboardDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DashboardDraft")
            .field("project_id", &self.project_id)
            .field("description", &self.description.is_some())
            .field("widgets_bytes", &self.widgets.get().len())
            .field("date_filter", &self.date_filter.is_some())
            .field("shared", &self.shared)
            .field("rename_if_taken", &self.rename_if_taken)
            .finish_non_exhaustive()
    }
}

/// A change to a dashboard (`dashboardUpdate`). Widgets, viewport and the
/// date filter are whole values. `captureVersion` asks for a version of
/// the stored dashboard before the change (the GUI decides which edits
/// are versioned).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DashboardPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub description: Clearable<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "unknown"))]
    pub widgets: Option<Box<RawValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "unknown"))]
    pub viewport: Option<Box<RawValue>>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional, type = "unknown"))]
    pub date_filter: Clearable<Box<RawValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub starred: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub shared: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub capture_version: bool,
}

impl fmt::Debug for DashboardPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fields: Vec<&str> = [
            ("name", present(&self.name)),
            ("description", present(&self.description)),
            ("widgets", present(&self.widgets)),
            ("viewport", present(&self.viewport)),
            ("dateFilter", present(&self.date_filter)),
            ("starred", present(&self.starred)),
            ("shared", present(&self.shared)),
            ("captureVersion", self.capture_version),
        ]
        .into_iter()
        .filter_map(|(n, on)| on.then_some(n))
        .collect();
        f.debug_struct("DashboardPatch")
            .field("fields", &fields)
            .finish()
    }
}

/// What `dashboardUpdate` stored: the row, the version it appended (only
/// with `captureVersion`), and the versions the prune removed. The new
/// version comes without its snapshot, like `dashboardVersionsList`'s:
/// the snapshot is the dashboard before the change, which
/// the page just showed, and the history fetches it only when it's
/// compared or restored (`dashboardVersionGet`).
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DashboardUpdated {
    pub dashboard: PersistedDashboard,
    pub version: Option<PersistedDashboardVersionMeta>,
    pub pruned_version_ids: Vec<String>,
}

impl fmt::Debug for DashboardUpdated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DashboardUpdated")
            .field("dashboard", &self.dashboard.id)
            .field("version", &self.version.as_ref().map(|v| &v.id))
            .field("pruned_version_ids", &self.pruned_version_ids)
            .finish()
    }
}

fn check_widgets(v: &RawValue) -> Checked {
    if json_kind(v) == b'[' {
        Ok(())
    } else {
        Err(LibraryError::invalid("A dashboard's widgets are a list."))
    }
}

fn check_object(v: &RawValue, what: &str) -> Checked {
    if json_kind(v) == b'{' {
        Ok(())
    } else {
        Err(LibraryError::invalid(format!(
            "A dashboard's {what} is a JSON object."
        )))
    }
}

fn dashboard_bytes(parts: &[usize], limits: &StateLimits) -> Checked {
    bytes_within(
        parts.iter().sum(),
        "dashboard's widgets, viewport and filter",
        limits.max_dashboard_bytes,
        "max_dashboard_bytes",
    )
}

pub fn check_dashboard_draft(
    d: &DashboardDraft,
    lib: &LibraryLimits,
    limits: &StateLimits,
) -> Checked {
    short(&d.project_id, "project id", lib)?;
    check_name(&d.name, "dashboard", lib)?;
    opt_field(d.description.as_deref(), "description", lib)?;
    check_widgets(&d.widgets)?;
    check_object(&d.viewport, "viewport")?;
    if let Some(f) = &d.date_filter {
        check_object(f, "date filter")?;
    }
    dashboard_bytes(
        &[
            d.widgets.get().len(),
            d.viewport.get().len(),
            d.date_filter.as_ref().map_or(0, |f| f.get().len()),
        ],
        limits,
    )
}

/// A dashboard patch's own checks. Its size is checked once it is applied
/// ([`check_dashboard_size`]), against the dashboard as stored.
pub fn check_dashboard_patch(
    p: &DashboardPatch,
    lib: &LibraryLimits,
    _limits: &StateLimits,
) -> Checked {
    if let Some(name) = &p.name {
        check_name(name, "dashboard", lib)?;
    }
    opt_field(
        p.description.as_ref().and_then(Option::as_deref),
        "description",
        lib,
    )?;
    if let Some(w) = &p.widgets {
        check_widgets(w)?;
    }
    if let Some(v) = &p.viewport {
        check_object(v, "viewport")?;
    }
    if let Some(Some(f)) = &p.date_filter {
        check_object(f, "date filter")?;
    }
    Ok(())
}

/// A dashboard's widgets, viewport and filter together, in bytes.
pub fn dashboard_size(row: &PersistedDashboard) -> usize {
    row.widgets.len() + row.viewport.len() + row.date_filter.as_ref().map_or(0, String::len)
}

/// A patched dashboard's size check (`max_dashboard_bytes`): past the
/// limit it is still accepted when no larger than before the patch
/// (`before`, [`dashboard_size`]), so a dashboard stored before the limit
/// stays editable and can shrink.
pub fn check_dashboard_size(
    row: &PersistedDashboard,
    before: Option<usize>,
    limits: &StateLimits,
) -> Checked {
    let now = dashboard_size(row);
    if before.is_some_and(|b| now <= b) {
        return Ok(());
    }
    dashboard_bytes(&[now], limits)
}

pub fn dashboard_from_draft(id: String, d: &DashboardDraft, now: &str) -> PersistedDashboard {
    PersistedDashboard {
        id,
        project_id: d.project_id.clone(),
        name: d.name.clone(),
        viewport: d.viewport.get().to_string(),
        widgets: d.widgets.get().to_string(),
        date_filter: d.date_filter.as_ref().map(|f| f.get().to_string()),
        starred: false,
        shared: d.shared,
        description: d.description.clone(),
        created_at: now.to_string(),
        updated_at: now.to_string(),
        // A link is set by the sync, never by a draft.
        shared_path: None,
    }
}

/// Applies `patch` and answers whether the name's key
/// changed (the name is checked again). `updated_at` becomes `now` unless
/// the patch holds only `starred`, as today.
pub fn apply_dashboard_patch(
    row: &mut PersistedDashboard,
    patch: &DashboardPatch,
    now: &str,
) -> bool {
    let mut renamed = false;
    if let Some(name) = &patch.name {
        renamed = crate::library::name_key(name) != crate::library::name_key(&row.name);
        row.name = name.clone();
    }
    if let Some(d) = &patch.description {
        row.description = d.clone();
    }
    if let Some(w) = &patch.widgets {
        row.widgets = w.get().to_string();
    }
    if let Some(v) = &patch.viewport {
        row.viewport = v.get().to_string();
    }
    if let Some(f) = &patch.date_filter {
        row.date_filter = f.as_ref().map(|f| f.get().to_string());
    }
    if let Some(s) = patch.starred {
        row.starred = s;
    }
    if let Some(s) = patch.shared {
        row.shared = s;
    }
    let only_starred = patch.starred.is_some()
        && patch.name.is_none()
        && patch.description.is_none()
        && patch.widgets.is_none()
        && patch.viewport.is_none()
        && patch.date_filter.is_none()
        && patch.shared.is_none();
    if !only_starred {
        row.updated_at = now.to_string();
    }
    renamed
}

/// Stored JSON text as a value to embed, or as a JSON string when it
/// doesn't parse (a hand-edited file).
fn embed(text: &str) -> Box<RawValue> {
    serde_json::from_str::<Box<RawValue>>(text).unwrap_or_else(|_| json_str(text))
}

/// A version's snapshot of `row` as today's `createDashboardSnapshot` makes
/// it: `{name, description?, widgets, viewport, dateFilter}`.
pub fn dashboard_snapshot(row: &PersistedDashboard) -> String {
    let mut obj: Obj = vec![("name".into(), json_str(&row.name))];
    if let Some(d) = &row.description {
        obj.push(("description".into(), json_str(d)));
    }
    obj.push(("widgets".into(), embed(&row.widgets)));
    obj.push(("viewport".into(), embed(&row.viewport)));
    obj.push((
        "dateFilter".into(),
        row.date_filter.as_deref().map_or_else(null, embed),
    ));
    render_obj(&obj)
}

// ── AI chats ──

/// A new chat (`chatCreate`). Core makes its id (a uuid).
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChatDraft {
    pub connection_id: String,
    pub title: String,
}

impl fmt::Debug for ChatDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatDraft")
            .field("connection_id", &self.connection_id)
            .finish_non_exhaustive()
    }
}

/// A change to a chat (`chatUpdate`): its title, and `touched` to set
/// `updatedAt` to now.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChatPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub touched: bool,
}

impl fmt::Debug for ChatPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatPatch")
            .field("title", &self.title.is_some())
            .field("touched", &self.touched)
            .finish()
    }
}

/// One message of `chatMessagesPut`. Its id is the GUI's.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChatMessageDraft {
    pub id: String,
    #[cfg_attr(feature = "ts", ts(type = "\"user\" | \"assistant\""))]
    pub role: String,
    pub content: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub dashboard_id: Option<String>,
    /// A reply's tool calls (phase 6): a JSON list, stored as
    /// given when present. Absent keeps what the message had (none for a
    /// new one), as an older release's put does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "Array<unknown>"))]
    pub parts: Option<serde_json::Value>,
}

impl fmt::Debug for ChatMessageDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatMessageDraft")
            .field("id", &self.id)
            .field(
                "known_role",
                &(self.role == "user" || self.role == "assistant"),
            )
            .field("content_bytes", &self.content.len())
            .field("query", &self.query.is_some())
            .field("dashboard_id", &self.dashboard_id.is_some())
            .field("parts", &self.parts.as_ref().map(parts_len))
            .finish()
    }
}

/// How many items a `parts` value holds, for `Debug`.
fn parts_len(parts: &serde_json::Value) -> usize {
    parts.as_array().map_or(0, Vec::len)
}

/// A chat's messages (`chatMessagesList`, in `timestamp, rowid` order) or
/// the messages a put stored (`chatMessagesPut`: only those, not the
/// chat's list), with the chat's stored content bytes (the web budget).
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChatMessages {
    pub messages: Vec<PersistedAIMessage>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub stored_bytes: u64,
    /// The chat can't take another message here ([`chat_is_full`]): the
    /// GUI opens it with sending off. Never on the desktop (no limits).
    #[serde(default)]
    pub full: bool,
}

impl fmt::Debug for ChatMessages {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatMessages")
            .field("messages", &self.messages.len())
            .field("stored_bytes", &self.stored_bytes)
            .field("full", &self.full)
            .finish()
    }
}

/// Whether a chat holding `stored_bytes` of content in `messages` messages
/// is full under `limits`: one more message of the largest size allowed
/// could pass `max_chat_bytes`, or it holds `max_messages_per_chat`
/// already. Without those limits (the desktop) it never is.
pub fn chat_is_full(stored_bytes: u64, messages: u64, limits: &StateLimits) -> bool {
    let bytes_full = limits.max_chat_bytes.is_some_and(|max| {
        let next = limits.max_message_bytes.unwrap_or(0) as u64;
        stored_bytes.saturating_add(next) > max
    });
    let count_full = limits
        .max_messages_per_chat
        .is_some_and(|max| messages >= max as u64);
    bytes_full || count_full
}

/// A chat title: bounded and no NUL (it may be empty, as today's first
/// save of an untitled chat is).
fn check_title(title: &str, lib: &LibraryLimits) -> Checked {
    short(title, "chat title", lib)
}

pub fn check_chat_draft(d: &ChatDraft, lib: &LibraryLimits) -> Checked {
    short(&d.connection_id, "connection id", lib)?;
    check_title(&d.title, lib)
}

pub fn check_chat_patch(p: &ChatPatch, lib: &LibraryLimits) -> Checked {
    p.title.as_deref().map_or(Ok(()), |t| check_title(t, lib))
}

/// `chatMessagesPut`'s own checks: at most `max_messages_per_chat`
/// messages, each id of the client form and listed once, a known role,
/// the content within `max_message_bytes`, and the other fields bounded.
pub fn check_messages(
    messages: &[ChatMessageDraft],
    lib: &LibraryLimits,
    limits: &StateLimits,
) -> Checked {
    count_within(
        messages.len(),
        "messages",
        limits.max_messages_per_chat,
        "max_messages_per_chat",
    )?;
    let mut seen = std::collections::HashSet::new();
    for m in messages {
        if !is_client_id(&m.id) {
            return Err(LibraryError::invalid(
                "A message id is 1 to 64 letters, digits, '-' or '_'.",
            ));
        }
        if !seen.insert(m.id.as_str()) {
            return Err(LibraryError::invalid(
                "Two messages of the put have the same id.",
            ));
        }
        if m.role != "user" && m.role != "assistant" {
            return Err(LibraryError::invalid(
                "A message's role must be user or assistant.",
            ));
        }
        within(
            &m.content,
            "message",
            limits.max_message_bytes,
            "max_message_bytes",
        )?;
        short(&m.timestamp, "message time", lib)?;
        within(
            m.query.as_deref().unwrap_or_default(),
            "message's query",
            lib.max_query_bytes,
            "max_query_bytes",
        )?;
        opt_field(m.dashboard_id.as_deref(), "dashboard id", lib)?;
        check_parts(m.parts.as_ref(), limits)?;
    }
    Ok(())
}

/// A message's `parts` (phase 6): a JSON list, within `max_message_bytes`
/// as JSON text, like its content.
pub fn check_parts(parts: Option<&serde_json::Value>, limits: &StateLimits) -> Checked {
    let Some(parts) = parts else {
        return Ok(());
    };
    if !parts.is_array() {
        return Err(LibraryError::invalid("A message's parts must be a list."));
    }
    within(
        &parts.to_string(),
        "message's parts",
        limits.max_message_bytes,
        "max_message_bytes",
    )
}

/// A message id list (`chatMessagesRemove`): bounded like a put.
pub fn check_message_ids(ids: &[String], limits: &StateLimits) -> Checked {
    count_within(
        ids.len(),
        "messages",
        limits.max_messages_per_chat,
        "max_messages_per_chat",
    )?;
    if ids.iter().all(|id| is_client_id(id)) {
        Ok(())
    } else {
        Err(LibraryError::invalid(
            "A message id is 1 to 64 letters, digits, '-' or '_'.",
        ))
    }
}

pub fn message_row(chat_id: &str, m: &ChatMessageDraft) -> PersistedAIMessage {
    PersistedAIMessage {
        id: m.id.clone(),
        chat_id: chat_id.to_string(),
        role: m.role.clone(),
        content: m.content.clone(),
        timestamp: m.timestamp.clone(),
        query: m.query.clone(),
        dashboard_id: m.dashboard_id.clone(),
        parts: m.parts.clone(),
    }
}

pub fn chat_from_draft(id: String, d: &ChatDraft, now: &str) -> PersistedAIChat {
    PersistedAIChat {
        id,
        connection_id: d.connection_id.clone(),
        title: d.title.clone(),
        created_at: now.to_string(),
        updated_at: now.to_string(),
    }
}

/// Applies `patch`: a title replaces the stored one; `touched` sets
/// `updatedAt` to `now` (a title alone doesn't, as today).
pub fn apply_chat_patch(row: &mut PersistedAIChat, patch: &ChatPatch, now: &str) {
    if let Some(t) = &patch.title {
        row.title = t.clone();
    }
    if patch.touched {
        row.updated_at = now.to_string();
    }
}

/// The web chat budget: the chat's stored content bytes, minus those
/// of the messages the put replaces, plus the put's, must stay within
/// `max_chat_bytes`.
pub fn check_chat_budget(stored: u64, replaced: u64, added: u64, limits: &StateLimits) -> Checked {
    let total = stored.saturating_sub(replaced).saturating_add(added);
    match limits.max_chat_bytes {
        Some(max) if total > max => Err(LibraryError::invalid(format!(
            "This chat is full (max_chat_bytes: {max} bytes). Start a new chat to continue."
        ))),
        _ => Ok(()),
    }
}

// ── Tutorial and import state ──

pub fn check_tutorial_ids(
    lesson_id: &str,
    challenge_id: Option<&str>,
    lib: &LibraryLimits,
) -> Checked {
    short(lesson_id, "lesson id", lib)?;
    challenge_id.map_or(Ok(()), |c| short(c, "challenge id", lib))
}

pub fn check_tutorial_state(state: Option<&str>, limits: &StateLimits) -> Checked {
    state.map_or(Ok(()), |s| setting_bytes(s, "tutorial state", limits))
}

pub fn check_import_source(source: &str) -> Checked {
    if IMPORT_SOURCES.contains(&source) {
        Ok(())
    } else {
        Err(LibraryError::invalid(
            "The import source must be tableplus or dbeaver.",
        ))
    }
}

pub fn check_import_time(time: Option<&str>, lib: &LibraryLimits) -> Checked {
    time.map_or(Ok(()), |t| short(t, "import time", lib))
}

// ── Window view state ──

/// Where a window's first load of a project took its state from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum CopiedFrom {
    /// The project's most recently used window's row.
    Window,
    /// Today's `project_state` and `tabs` rows.
    Legacy,
    /// Nothing: the GUI adds the starter tabs.
    Empty,
}

/// What `windowStateLoad` answers: the state (`None` for `empty`), its
/// `rev` (the page counts its saves up from it), and where it was copied
/// from (`None`: the window's own row).
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct WindowStateLoaded {
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub state: Option<Box<RawValue>>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub rev: u64,
    pub copied_from: Option<CopiedFrom>,
}

impl fmt::Debug for WindowStateLoaded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowStateLoaded")
            .field("state_bytes", &self.state.as_ref().map(|s| s.get().len()))
            .field("rev", &self.rev)
            .field("copied_from", &self.copied_from)
            .finish()
    }
}

/// What `windowStateSave` answers: `stale` when a save with this `rev` or
/// a higher one was already stored (nothing was written), and the stored
/// `rev` either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct WindowStateSaved {
    pub stale: bool,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub rev: u64,
}

/// Where `windowGet`'s project came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum WindowFrom {
    /// The window's own active project.
    Window,
    /// The most recently used window's.
    Recent,
    /// `lastActiveProjectId`.
    LastActive,
}

/// What `windowGet` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct WindowActive {
    pub active_project_id: Option<String>,
    pub from: Option<WindowFrom>,
}

/// The highest `rev` a view-state save may carry: 2^53 - 1, the last
/// whole number the page's JavaScript counter holds exactly, so its
/// `rev + 1` is always the next number.
pub const MAX_REV: u64 = (1 << 53) - 1;

/// A view-state save's `rev` is at most [`MAX_REV`].
pub fn check_rev(rev: u64) -> Checked {
    if rev > MAX_REV {
        return Err(LibraryError::invalid(
            "The view state's rev is past 2^53 - 1, the largest the page counts exactly.",
        ));
    }
    Ok(())
}

/// A view state's size check before it's parsed (`max_view_state_bytes`).
pub fn check_view_state_size(state: &RawValue, limits: &StateLimits) -> Checked {
    bytes_within(
        state.get().len(),
        "view state",
        limits.max_view_state_bytes,
        "max_view_state_bytes",
    )
}

/// A view state's sizes, as the limits count them: its bytes, its tabs,
/// and each tab's text (a query tab's query, an explain tab's source, a
/// table editor's definition) by tab id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewStateSize {
    pub bytes: usize,
    pub tabs: usize,
    /// The longest text of each tab id.
    pub texts: HashMap<String, usize>,
}

/// A window's view state read as today's `PersistedProjectState`,
/// for the legacy mirror: its `projectId` the project's (a state
/// naming another project is refused), and an `activeView` of `canvas`
/// (from before the workflow rename) written as `workflow`, which the
/// frozen baseline turns it into on every open anyway. The window's own
/// row keeps the state as sent. Storage's `write_legacy_mirror` ignores its
/// connection order, starred lists and workflows, and skips repeated tab
/// ids. No limit is checked here ([`check_view_state_limits`]).
pub fn parse_view_state(
    state: &RawValue,
    project_id: &str,
) -> Result<PersistedProjectState, LibraryError> {
    let not_a_state = || LibraryError::invalid("The view state isn't a project's tabs and layout.");
    let mut obj = parse_obj(state.get()).ok_or_else(not_a_state)?;
    match obj_get(&obj, "projectId").map(string_of) {
        None => obj.insert(0, ("projectId".into(), json_str(project_id))),
        Some(Some(p)) if p == project_id => {}
        Some(_) => {
            return Err(LibraryError::invalid(
                "The view state names another project.",
            ))
        }
    }
    let mut s: PersistedProjectState =
        serde_json::from_str(&render_obj(&obj)).map_err(|_| not_a_state())?;
    if s.active_view == "canvas" {
        s.active_view = "workflow".to_string();
    }
    Ok(s)
}

/// The sizes of `state` (its text) read as `s` ([`parse_view_state`]).
pub fn view_state_size(state: &RawValue, s: &PersistedProjectState) -> ViewStateSize {
    let tabs = s.query_tabs.len()
        + s.schema_tabs.len()
        + s.explain_tabs.len()
        + s.erd_tabs.len()
        + s.statistics_tabs.len()
        + s.workflow_tabs.len()
        + s.starter_tabs.len()
        + s.dashboard_tabs.len()
        + s.create_table_tabs.len()
        + s.data_tabs.len()
        + s.connection_tabs.as_ref().map_or(0, Vec::len)
        + s.extensions_duckdb_tabs.as_ref().map_or(0, Vec::len);
    let mut texts: HashMap<String, usize> = HashMap::new();
    let all = s
        .query_tabs
        .iter()
        .map(|t| (&t.id, t.query.len()))
        .chain(s.explain_tabs.iter().map(|t| (&t.id, t.source_query.len())))
        .chain(
            s.create_table_tabs
                .iter()
                .map(|t| (&t.id, t.table_definition.len())),
        );
    for (id, len) in all {
        let e = texts.entry(id.clone()).or_default();
        *e = (*e).max(len);
    }
    ViewStateSize {
        bytes: state.get().len(),
        tabs,
        texts,
    }
}

/// A view state's limits: its bytes, its tabs and each tab's
/// text. A size past its limit is still accepted when it is no larger than
/// what the window's stored row holds (`stored`), item by item (the whole
/// state against the stored state, a tab's text against the same tab id's
/// stored text), so a state stored before the limits, or copied from one,
/// stays saveable and can shrink; only growth past a limit is refused. A
/// refusal names the limit, and the tab for a tab's text.
pub fn check_view_state_limits(
    new: &ViewStateSize,
    stored: Option<&ViewStateSize>,
    limits: &StateLimits,
) -> Checked {
    let grandfathered = |now: usize, before: Option<usize>| before.is_some_and(|b| now <= b);
    if limits
        .max_view_state_bytes
        .is_some_and(|max| new.bytes > max)
        && !grandfathered(new.bytes, stored.map(|s| s.bytes))
    {
        bytes_within(
            new.bytes,
            "view state",
            limits.max_view_state_bytes,
            "max_view_state_bytes",
        )?;
    }
    if limits.max_tabs.is_some_and(|max| new.tabs > max)
        && !grandfathered(new.tabs, stored.map(|s| s.tabs))
    {
        count_within(new.tabs, "tabs", limits.max_tabs, "max_tabs")?;
    }
    if let Some(max) = limits.max_tab_text_bytes {
        for (id, &len) in &new.texts {
            if len > max && !grandfathered(len, stored.and_then(|s| s.texts.get(id).copied())) {
                let tab = if is_client_id(id) {
                    format!("text of tab {id}")
                } else {
                    "tab's text".to_string()
                };
                bytes_within(len, &tab, Some(max), "max_tab_text_bytes")?;
            }
        }
    }
    Ok(())
}

/// [`parse_view_state`] with the limits checked as for a window with no
/// stored row.
pub fn legacy_mirror(
    state: &RawValue,
    project_id: &str,
    limits: &StateLimits,
) -> Result<PersistedProjectState, LibraryError> {
    let s = parse_view_state(state, project_id)?;
    check_view_state_limits(&view_state_size(state, &s), None, limits)?;
    Ok(s)
}

/// The keys a window's view state leaves out of today's project state:
/// shared or stored elsewhere.
const NOT_VIEW_STATE: [&str; 4] = [
    "savedWorkflows",
    "connectionOrder",
    "starredSharedQueryIds",
    "starredSharedDashboardIds",
];

/// Today's loaded project state as a window's view state: the state
/// `project_state::load` gives, without the saved workflows, the
/// connection order and the starred-shared lists.
pub fn legacy_view(s: &PersistedProjectState) -> Box<RawValue> {
    let text = serde_json::to_string(s).unwrap_or_else(|_| "{}".to_string());
    let mut obj = parse_obj(&text).unwrap_or_default();
    obj.retain(|(k, _)| !NOT_VIEW_STATE.contains(&k.as_str()));
    lit(&render_obj(&obj))
}

/// A connection order (`projectSidebarSet`): bounded like a list of ids.
pub fn check_connection_order(ids: &[String], lib: &LibraryLimits) -> Checked {
    list_len(ids.len(), "connections in the order", lib)?;
    ids.iter()
        .try_for_each(|id| short(id, "connection id", lib))
}

/// A stored connection order read tolerantly: its strings, in order; a
/// value that isn't a list reads as none.
pub fn read_connection_order(stored: Option<&RawValue>) -> Vec<String> {
    let Some(stored) = stored else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<Value>>(stored.get())
        .map(|items| {
            items
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(s: &str) -> Box<RawValue> {
        RawValue::from_string(s.to_string()).unwrap()
    }

    #[test]
    fn objects_keep_their_order_and_bytes() {
        let o = parse_obj(r#"{"b": 1.50, "a":{"x" : [1, 2]},"b":2}"#).unwrap();
        assert_eq!(render_obj(&o), r#"{"b":2,"a":{"x" : [1, 2]}}"#);
        assert!(parse_obj("[1]").is_none());
        assert!(parse_obj("nope").is_none());
    }

    #[test]
    fn a_provider_is_cleaned_like_the_store() {
        let p = clean_provider(&raw(
            r#"{"id":"a","provider":"openai-compatible","model":"m","baseUrl":"u"}"#,
        ));
        assert_eq!(
            render_obj(&p),
            r#"{"id":"a","baseUrl":"u","type":"openai-compatible"}"#
        );
        let p = clean_provider(&raw(r#"{"id":"a","type":null,"provider":null}"#));
        assert_eq!(render_obj(&p), r#"{"id":"a","type":"anthropic"}"#);
        let p = clean_provider(&raw(r#"{"type":"x","provider":"y"}"#));
        assert_eq!(render_obj(&p), r#"{"type":"x"}"#);
        assert_eq!(
            render_obj(&clean_provider(&raw("3"))),
            r#"{"type":"anthropic"}"#
        );
    }
}
