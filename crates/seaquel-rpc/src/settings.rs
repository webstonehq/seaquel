//! The `settings` group of the workspace RPC (phase 5d-2): the
//! app-state settings (a closed set of keys), the AI settings record and
//! its providers (with their API keys on the desktop), themes, onboarding,
//! tutorial progress and import state, written through Core
//! (`Workspace::set_setting`, `Workspace::create_ai_provider`, …).
//!
//! Wire shape, like the other groups:
//!
//! ```json
//! {"method":"settings","params":{"method":"settingSet","params":{"key":"editorKeybindingMode","value":"vim"}}}
//! {"method":"settings","result":{"method":"settingSet","result":{"value":"vim","seq":{"epoch":"…","n":7}}}}
//! ```
//!
//! - Every result is a `Seqd` (`{value, seq}`), and a write
//!   answers the whole record or row it wrote (a theme write: the
//!   preferences and every user theme).
//! - A setting's `key` is a string on the wire (`SettingKey` in the
//!   generated TypeScript), so an unknown key is Core's `INVALID_ARGUMENT`
//!   naming it, not a parse error. `settingSet`'s `value` must be present:
//!   `null` deletes the row.
//! - `apiKey` on `aiProviderCreate`/`aiProviderUpdate` is desktop only
//!   (absent: keep, `null`: delete, a string: set); a workspace without a
//!   secret store (the web) answers `NOT_SUPPORTED`, its vault stays in the
//!   browser.
//! - JSON bodies (a user theme, an onboarding patch) cross as the text that
//!   came in (`RawValue`); Core rewrites only their top level.
//!
//! `Debug` shows the method only: never a setting's value, a name, JSON, a
//! tutorial's state or a key. Without the `storage` feature every method
//! answers `NOT_SUPPORTED`.

use std::fmt;

use seaquel_core::domain::library::Clearable;
use seaquel_core::domain::state::{
    AiProviderCreated, AiProviderDraft, AiProviderPatch, AiSettingsPatch, ThemeCreated, Themes,
};
use seaquel_core::{Core, Seqd, Workspace, WriteOrigin};
use seaquel_types::storage::{ImportState, TutorialProgress};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

use crate::workspace::RpcError;

/// A `settings` call.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SettingsRequest {
    /// A setting's stored value, `null` when unset.
    SettingGet {
        #[cfg_attr(feature = "ts", ts(as = "seaquel_core::domain::state::SettingKey"))]
        key: String,
    },
    /// Set a setting; `null` deletes it. The value is checked for its key.
    SettingSet {
        #[cfg_attr(feature = "ts", ts(as = "seaquel_core::domain::state::SettingKey"))]
        key: String,
        /// Required, even when `null`.
        #[serde(deserialize_with = "present")]
        value: Option<String>,
    },
    /// The AI settings record, cleaned as the GUI reads it.
    AiSettingsGet,
    AiSettingsPatch {
        patch: AiSettingsPatch,
    },
    AiProviderCreate {
        provider: AiProviderDraft,
        /// Desktop only. Absent: none.
        #[serde(
            default,
            deserialize_with = "seaquel_core::domain::library::clearable",
            skip_serializing_if = "Option::is_none"
        )]
        #[cfg_attr(feature = "ts", ts(optional))]
        api_key: Clearable<String>,
    },
    AiProviderUpdate {
        id: String,
        patch: AiProviderPatch,
        /// Desktop only. Absent: keep; `null`: delete; a string: set.
        #[serde(
            default,
            deserialize_with = "seaquel_core::domain::library::clearable",
            skip_serializing_if = "Option::is_none"
        )]
        #[cfg_attr(feature = "ts", ts(optional))]
        api_key: Clearable<String>,
    },
    AiProviderRemove {
        id: String,
    },
    /// Whether the keychain holds the provider's API key (desktop; the page can't read the key).
    /// One read, for that provider: the
    /// settings form asks when it opens one. `NOT_SUPPORTED` without a
    /// secret store (web: the page asks its vault).
    AiProviderHasKey {
        id: String,
    },
    /// The theme preferences and every user theme.
    ThemesGet,
    ThemePreferencesSet {
        light_theme_id: String,
        dark_theme_id: String,
    },
    UserThemeCreate {
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        theme: Box<RawValue>,
    },
    UserThemeUpdate {
        id: String,
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        theme: Box<RawValue>,
    },
    UserThemeRemove {
        id: String,
    },
    /// The onboarding record (the defaults when there is none).
    OnboardingGet,
    /// A JSON object whose top-level fields replace the stored ones.
    OnboardingPatch {
        #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown>"))]
        patch: Box<RawValue>,
    },
    TutorialList,
    TutorialSave {
        lesson_id: String,
        challenge_id: String,
        /// Kept as text; Core doesn't parse it.
        state: Option<String>,
    },
    TutorialRemoveLesson {
        lesson_id: String,
    },
    TutorialReset,
    ImportStateGet {
        #[cfg_attr(feature = "ts", ts(type = "\"tableplus\" | \"dbeaver\""))]
        source: String,
    },
    ImportStateSave {
        #[cfg_attr(feature = "ts", ts(type = "\"tableplus\" | \"dbeaver\""))]
        source: String,
        has_offered_import: bool,
        last_check_timestamp: Option<String>,
    },
}

/// A field that must be present, `null` included (`Option` fields are
/// otherwise optional in serde).
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

/// The method only: params can hold setting values, names, JSON, tutorial
/// state and API keys.
impl fmt::Debug for SettingsRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SettingsRequest")
            .field("method", &self.method())
            .finish_non_exhaustive()
    }
}

impl SettingsRequest {
    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            SettingsRequest::SettingGet { .. } => "settingGet",
            SettingsRequest::SettingSet { .. } => "settingSet",
            SettingsRequest::AiSettingsGet => "aiSettingsGet",
            SettingsRequest::AiSettingsPatch { .. } => "aiSettingsPatch",
            SettingsRequest::AiProviderCreate { .. } => "aiProviderCreate",
            SettingsRequest::AiProviderUpdate { .. } => "aiProviderUpdate",
            SettingsRequest::AiProviderRemove { .. } => "aiProviderRemove",
            SettingsRequest::AiProviderHasKey { .. } => "aiProviderHasKey",
            SettingsRequest::ThemesGet => "themesGet",
            SettingsRequest::ThemePreferencesSet { .. } => "themePreferencesSet",
            SettingsRequest::UserThemeCreate { .. } => "userThemeCreate",
            SettingsRequest::UserThemeUpdate { .. } => "userThemeUpdate",
            SettingsRequest::UserThemeRemove { .. } => "userThemeRemove",
            SettingsRequest::OnboardingGet => "onboardingGet",
            SettingsRequest::OnboardingPatch { .. } => "onboardingPatch",
            SettingsRequest::TutorialList => "tutorialList",
            SettingsRequest::TutorialSave { .. } => "tutorialSave",
            SettingsRequest::TutorialRemoveLesson { .. } => "tutorialRemoveLesson",
            SettingsRequest::TutorialReset => "tutorialReset",
            SettingsRequest::ImportStateGet { .. } => "importStateGet",
            SettingsRequest::ImportStateSave { .. } => "importStateSave",
        }
    }
}

/// A `settings` call's result, as `{"method": …, "result": {value, seq}}`.
/// `Debug` shows the method and sequence only.
// Not `Deserialize`: results only go out.
#[derive(Serialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SettingsResponse {
    SettingGet(Seqd<Option<String>>),
    SettingSet(Seqd<Option<String>>),
    AiSettingsGet(#[cfg_attr(feature = "ts", ts(as = "crate::SeqdJson"))] Seqd<Box<RawValue>>),
    AiSettingsPatch(#[cfg_attr(feature = "ts", ts(as = "crate::SeqdJson"))] Seqd<Box<RawValue>>),
    AiProviderCreate(Seqd<AiProviderCreated>),
    AiProviderUpdate(#[cfg_attr(feature = "ts", ts(as = "crate::SeqdJson"))] Seqd<Box<RawValue>>),
    AiProviderRemove(#[cfg_attr(feature = "ts", ts(as = "crate::SeqdJson"))] Seqd<Box<RawValue>>),
    AiProviderHasKey(Seqd<bool>),
    ThemesGet(Seqd<Themes>),
    ThemePreferencesSet(Seqd<Themes>),
    UserThemeCreate(Seqd<ThemeCreated>),
    UserThemeUpdate(Seqd<Themes>),
    UserThemeRemove(Seqd<Themes>),
    OnboardingGet(#[cfg_attr(feature = "ts", ts(as = "crate::SeqdJson"))] Seqd<Box<RawValue>>),
    OnboardingPatch(#[cfg_attr(feature = "ts", ts(as = "crate::SeqdJson"))] Seqd<Box<RawValue>>),
    TutorialList(Seqd<Vec<TutorialProgress>>),
    TutorialSave(Seqd<Vec<TutorialProgress>>),
    TutorialRemoveLesson(Seqd<Vec<TutorialProgress>>),
    TutorialReset(Seqd<Vec<TutorialProgress>>),
    ImportStateGet(Seqd<Option<ImportState>>),
    ImportStateSave(Seqd<ImportState>),
}

impl fmt::Debug for SettingsResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use SettingsResponse as R;
        let (method, seq) = match self {
            R::SettingGet(r) => ("settingGet", &r.seq),
            R::SettingSet(r) => ("settingSet", &r.seq),
            R::AiSettingsGet(r) => ("aiSettingsGet", &r.seq),
            R::AiSettingsPatch(r) => ("aiSettingsPatch", &r.seq),
            R::AiProviderCreate(r) => ("aiProviderCreate", &r.seq),
            R::AiProviderUpdate(r) => ("aiProviderUpdate", &r.seq),
            R::AiProviderRemove(r) => ("aiProviderRemove", &r.seq),
            R::AiProviderHasKey(r) => ("aiProviderHasKey", &r.seq),
            R::ThemesGet(r) => ("themesGet", &r.seq),
            R::ThemePreferencesSet(r) => ("themePreferencesSet", &r.seq),
            R::UserThemeCreate(r) => ("userThemeCreate", &r.seq),
            R::UserThemeUpdate(r) => ("userThemeUpdate", &r.seq),
            R::UserThemeRemove(r) => ("userThemeRemove", &r.seq),
            R::OnboardingGet(r) => ("onboardingGet", &r.seq),
            R::OnboardingPatch(r) => ("onboardingPatch", &r.seq),
            R::TutorialList(r) => ("tutorialList", &r.seq),
            R::TutorialSave(r) => ("tutorialSave", &r.seq),
            R::TutorialRemoveLesson(r) => ("tutorialRemoveLesson", &r.seq),
            R::TutorialReset(r) => ("tutorialReset", &r.seq),
            R::ImportStateGet(r) => ("importStateGet", &r.seq),
            R::ImportStateSave(r) => ("importStateSave", &r.seq),
        };
        f.debug_struct("SettingsResponse")
            .field("method", &method)
            .field("seq", seq)
            .finish_non_exhaustive()
    }
}

#[cfg(not(feature = "storage"))]
pub(crate) async fn settings(
    _: &Core,
    _: &Workspace,
    _: SettingsRequest,
    _: &WriteOrigin,
) -> Result<SettingsResponse, RpcError> {
    Err(RpcError::not_supported("Settings"))
}

/// Serve a `settings` call on `ws`, its writes tagged with `origin`.
#[cfg(feature = "storage")]
pub(crate) async fn settings(
    core: &Core,
    ws: &Workspace,
    req: SettingsRequest,
    origin: &WriteOrigin,
) -> Result<SettingsResponse, RpcError> {
    use seaquel_types::storage::ThemePreferences;
    use SettingsRequest as Q;
    use SettingsResponse as R;

    Ok(match req {
        Q::SettingGet { key } => R::SettingGet(ws.get_setting(&key).await?),
        Q::SettingSet { key, value } => {
            R::SettingSet(ws.set_setting(core, origin, &key, value).await?)
        }
        Q::AiSettingsGet => R::AiSettingsGet(ws.get_ai_settings().await?),
        Q::AiSettingsPatch { patch } => {
            R::AiSettingsPatch(ws.patch_ai_settings(core, origin, patch).await?)
        }
        Q::AiProviderCreate { provider, api_key } => R::AiProviderCreate(
            ws.create_ai_provider(core, origin, provider, api_key)
                .await?,
        ),
        Q::AiProviderUpdate { id, patch, api_key } => R::AiProviderUpdate(
            ws.update_ai_provider(core, origin, &id, patch, api_key)
                .await?,
        ),
        Q::AiProviderHasKey { id } => R::AiProviderHasKey(ws.ai_provider_has_key(core, &id).await?),
        Q::AiProviderRemove { id } => {
            R::AiProviderRemove(ws.remove_ai_provider(core, origin, &id).await?)
        }
        Q::ThemesGet => R::ThemesGet(ws.get_themes().await?),
        Q::ThemePreferencesSet {
            light_theme_id,
            dark_theme_id,
        } => R::ThemePreferencesSet(
            ws.set_theme_preferences(
                core,
                origin,
                ThemePreferences {
                    light_theme_id,
                    dark_theme_id,
                },
            )
            .await?,
        ),
        Q::UserThemeCreate { theme } => {
            R::UserThemeCreate(ws.create_user_theme(core, origin, theme).await?)
        }
        Q::UserThemeUpdate { id, theme } => {
            R::UserThemeUpdate(ws.update_user_theme(core, origin, &id, theme).await?)
        }
        Q::UserThemeRemove { id } => {
            R::UserThemeRemove(ws.remove_user_theme(core, origin, &id).await?)
        }
        Q::OnboardingGet => R::OnboardingGet(ws.get_onboarding().await?),
        Q::OnboardingPatch { patch } => {
            R::OnboardingPatch(ws.patch_onboarding(core, origin, patch).await?)
        }
        Q::TutorialList => R::TutorialList(ws.list_tutorial().await?),
        Q::TutorialSave {
            lesson_id,
            challenge_id,
            state,
        } => R::TutorialSave(
            ws.save_tutorial(
                core,
                origin,
                TutorialProgress {
                    lesson_id,
                    challenge_id,
                    state,
                },
            )
            .await?,
        ),
        Q::TutorialRemoveLesson { lesson_id } => {
            R::TutorialRemoveLesson(ws.remove_tutorial_lesson(core, origin, &lesson_id).await?)
        }
        Q::TutorialReset => R::TutorialReset(ws.reset_tutorial(origin).await?),
        Q::ImportStateGet { source } => R::ImportStateGet(ws.get_import_state(&source).await?),
        Q::ImportStateSave {
            source,
            has_offered_import,
            last_check_timestamp,
        } => R::ImportStateSave(
            ws.save_import_state(
                core,
                origin,
                &source,
                ImportState {
                    has_offered_import,
                    last_check_timestamp,
                },
            )
            .await?,
        ),
    })
}
