//! The `settings` group through Core (phase 5d-2, Decision 20): settings
//! rows, the AI settings record rewritten from the stored copy with its API
//! keys in the keychain (desktop) or the vault (web), themes, onboarding,
//! tutorial progress and import state.
//!
//! Secrets are a `TestStore`; nothing touches the real keychain.
#![cfg(all(feature = "storage", feature = "secrets"))]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod common;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use common::{dump, fx_with, insert_rows, web_state_limits, Fx};
use seaquel_core::domain::state::AiProviderDraft;
use seaquel_core::{StateLimits, WriteOrigin};
use seaquel_types::storage::ThemePreferences;
use serde_json::value::RawValue;
use serde_json::{json, Value};

async fn fx() -> Fx {
    fx_with(StateLimits::default(), true).await
}

fn none() -> WriteOrigin {
    WriteOrigin::none()
}

fn j<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}

fn rv(v: Value) -> Box<RawValue> {
    RawValue::from_string(v.to_string()).unwrap()
}

fn provider(name: &str) -> AiProviderDraft {
    j(json!({"name": name, "type": "anthropic"}))
}

async fn stored_ai(f: &Fx) -> Value {
    let rows = dump(f.ws.storage(), "app_state", "key").await;
    let row = rows.iter().find(|r| r["key"] == "aiSettings").unwrap();
    serde_json::from_str(row["value"].as_str().unwrap()).unwrap()
}

fn provider_names(v: &Value) -> Vec<String> {
    v["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

// ── Settings ──

#[tokio::test]
async fn a_null_setting_deletes_its_row() {
    let f = fx().await;
    f.ws.set_setting(&f.core, &none(), "editorKeybindingMode", Some("vim".into()))
        .await
        .unwrap();
    assert_eq!(
        f.ws.get_setting("editorKeybindingMode")
            .await
            .unwrap()
            .value
            .as_deref(),
        Some("vim")
    );
    f.ws.set_setting(&f.core, &none(), "editorKeybindingMode", None)
        .await
        .unwrap();
    let rows = dump(f.ws.storage(), "app_state", "key").await;
    assert!(
        rows.iter().all(|r| r["key"] != "editorKeybindingMode"),
        "{rows:?}"
    );
    assert_eq!(
        f.ws.get_setting("editorKeybindingMode")
            .await
            .unwrap()
            .value,
        None
    );
    // A row holding NULL (older writes) reads as unset too.
    insert_rows(
        f.ws.storage(),
        "app_state",
        &[json!({"key": "license_nudge", "value": null})],
    )
    .await;
    assert_eq!(f.ws.get_setting("license_nudge").await.unwrap().value, None);
}

#[tokio::test]
async fn refused_settings_write_nothing() {
    let f = fx().await;
    for (key, value) in [
        ("somethingElse", Some("1")),
        ("activeRepoId", Some("r")),
        ("connectionStringSecretsUpgraded", None),
        ("lastActiveProjectId", Some("p1")),
        ("connectionStringSecretsNotice", Some("[]")),
        ("query_version_limit", Some("abc")),
    ] {
        let e =
            f.ws.set_setting(&f.core, &none(), key, value.map(str::to_string))
                .await
                .unwrap_err();
        assert_eq!(e.code, "INVALID_ARGUMENT", "{key}");
        assert!(e.message.contains(key), "{e:?}");
    }
    // Only the open's own key (the string-secrets upgrade found nothing).
    let rows = dump(f.ws.storage(), "app_state", "key").await;
    assert!(
        rows.iter()
            .all(|r| r["key"] == "connectionStringSecretsUpgraded"),
        "{rows:?}"
    );
    assert_eq!(
        f.ws.get_setting("activeRepoId").await.unwrap_err().code,
        "INVALID_ARGUMENT"
    );
}

// ── AI settings ──

#[tokio::test(flavor = "multi_thread")]
async fn two_windows_editing_different_providers_both_land() {
    let f = Arc::new(fx().await);
    let mut tasks = Vec::new();
    for i in 0..10 {
        let f = f.clone();
        tasks.push(tokio::spawn(async move {
            f.ws.create_ai_provider(
                &f.core,
                &WriteOrigin::new(Some(&format!("tab-{i}"))),
                provider(&format!("P{i}")),
                None,
            )
            .await
            .unwrap()
            .value
            .id
        }));
    }
    let mut ids = Vec::new();
    for t in tasks {
        ids.push(t.await.unwrap());
    }
    let stored = stored_ai(&f).await;
    assert_eq!(stored["providers"].as_array().unwrap().len(), 10);
    // Renames of different providers from two windows both land.
    let (a, b) = (ids[0].clone(), ids[1].clone());
    let (fa, fb) = (f.clone(), f.clone());
    let ta = tokio::spawn(async move {
        fa.ws
            .update_ai_provider(&fa.core, &none(), &a, j(json!({"name": "A2"})), None)
            .await
            .unwrap()
    });
    let tb = tokio::spawn(async move {
        fb.ws
            .update_ai_provider(&fb.core, &none(), &b, j(json!({"name": "B2"})), None)
            .await
            .unwrap()
    });
    ta.await.unwrap();
    tb.await.unwrap();
    let names = provider_names(&stored_ai(&f).await);
    assert!(
        names.contains(&"A2".to_string()) && names.contains(&"B2".to_string()),
        "{names:?}"
    );
}

#[tokio::test]
async fn the_rewrite_reads_the_stored_copy_and_keeps_unknown_fields() {
    let f = fx().await;
    let stored = r#"{"enabled":true,"providers":[{"id":"prov-1","name":"Old","provider":"openai-compatible","model":"m","region":"eu"}],"futureField":{"on":true}}"#;
    insert_rows(
        f.ws.storage(),
        "app_state",
        &[json!({"key": "aiSettings", "value": stored})],
    )
    .await;
    let got =
        f.ws.patch_ai_settings(&f.core, &none(), j(json!({"shareDataGlobally": true})))
            .await
            .unwrap()
            .value;
    let row = dump(f.ws.storage(), "app_state", "key").await;
    let text = row[0]["value"].as_str().unwrap();
    assert_eq!(text, got.get(), "the answer is the stored record");
    assert_eq!(
        text,
        r#"{"enabled":true,"providers":[{"id":"prov-1","name":"Old","region":"eu","type":"openai-compatible"}],"shareSchemaGlobally":true,"shareDataGlobally":true,"futureField":{"on":true}}"#
    );
}

#[tokio::test]
async fn an_api_key_is_written_before_the_record_and_taken_back_on_failure() {
    let limits = StateLimits {
        max_ai_providers: Some(1),
        ..StateLimits::default()
    };
    let f = Arc::new(fx_with(limits, true).await);
    // Hold the keychain write: the record isn't written yet, and the write
    // lock isn't held (another write goes through meanwhile).
    f.store.block_set.store(true, Ordering::SeqCst);
    let entered = f.store.entered_set.notified();
    let g = f.clone();
    let create = tokio::spawn(async move {
        g.ws.create_ai_provider(
            &g.core,
            &none(),
            provider("With key"),
            Some(Some("sk-1".into())),
        )
        .await
    });
    entered.await;
    let written = dump(f.ws.storage(), "app_state", "key").await;
    assert!(
        written.iter().all(|r| r["key"] != "aiSettings"),
        "record after the key"
    );
    // Meanwhile another window fills the only provider slot (no key).
    tokio::time::timeout(
        Duration::from_secs(5),
        f.ws.create_ai_provider(&f.core, &none(), provider("Other"), None),
    )
    .await
    .expect("no keychain call holds the write lock")
    .unwrap();
    f.store.release();
    let e = create.await.unwrap().unwrap_err();
    assert_eq!(e.code, "INVALID_ARGUMENT");
    assert!(
        f.store.entries().is_empty(),
        "the key was taken back: {:?}",
        f.store.entries()
    );
    assert_eq!(provider_names(&stored_ai(&f).await), ["Other"]);

    // An update's key replaces the old one first; a record write that then
    // fails (the provider was removed meanwhile) takes the key away with
    // the provider.
    let id =
        f.ws.create_ai_provider(&f.core, &none(), provider("Other2"), None)
            .await
            .unwrap_err();
    assert_eq!(id.code, "INVALID_ARGUMENT", "still at the limit");
    let other = stored_ai(&f).await["providers"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    f.store.put(&format!("ai-api-key:{other}"), "sk-old");
    f.store.block_set.store(true, Ordering::SeqCst);
    let entered = f.store.entered_set.notified();
    let (g, o) = (f.clone(), other.clone());
    let update = tokio::spawn(async move {
        g.ws.update_ai_provider(
            &g.core,
            &none(),
            &o,
            j(json!({"name": "N"})),
            Some(Some("sk-new".into())),
        )
        .await
    });
    entered.await;
    f.ws.remove_ai_provider(&f.core, &none(), &other)
        .await
        .unwrap();
    f.store.release();
    assert_eq!(
        update.await.unwrap().unwrap_err().code,
        "AI_PROVIDER_NOT_FOUND"
    );
    assert!(f.store.entries().is_empty(), "{:?}", f.store.entries());
}

#[tokio::test]
async fn a_failed_update_puts_the_old_key_back() {
    let limits = StateLimits {
        max_setting_bytes: Some(400),
        ..StateLimits::default()
    };
    let f = Arc::new(fx_with(limits, true).await);
    let id =
        f.ws.create_ai_provider(&f.core, &none(), provider("A"), Some(Some("sk-old".into())))
            .await
            .unwrap()
            .value
            .id;
    let before = stored_ai(&f).await.to_string().len();
    f.store.block_set.store(true, Ordering::SeqCst);
    let entered = f.store.entered_set.notified();
    let (g, i) = (f.clone(), id.clone());
    // 50 bytes more: fits the record as it is when the update starts.
    let update = tokio::spawn(async move {
        g.ws.update_ai_provider(
            &g.core,
            &none(),
            &i,
            j(json!({"name": "A".repeat(51)})),
            Some(Some("sk-new".into())),
        )
        .await
    });
    entered.await;
    // Meanwhile another window grows the record to 390 bytes (a provider
    // object is 75 bytes and its name), so the update's rewrite no longer
    // fits.
    let name = "x".repeat(390 - before - 75);
    f.ws.create_ai_provider(
        &f.core,
        &none(),
        j(json!({"name": name, "type": "anthropic"})),
        None,
    )
    .await
    .unwrap();
    assert_eq!(stored_ai(&f).await.to_string().len(), 390);
    f.store.release();
    let e = update.await.unwrap().unwrap_err();
    assert!(e.message.contains("max_setting_bytes"), "{e:?}");
    assert_eq!(
        f.store
            .entries()
            .get(&format!("ai-api-key:{id}"))
            .map(String::as_str),
        Some("sk-old")
    );
}

#[tokio::test]
async fn an_update_with_a_null_key_deletes_it_after_the_commit() {
    let f = fx().await;
    let id =
        f.ws.create_ai_provider(&f.core, &none(), provider("A"), Some(Some("sk".into())))
            .await
            .unwrap()
            .value
            .id;
    assert_eq!(f.store.entries().len(), 1);
    // Left out: kept.
    f.ws.update_ai_provider(&f.core, &none(), &id, j(json!({"name": "B"})), None)
        .await
        .unwrap();
    assert_eq!(f.store.entries().len(), 1);
    f.ws.update_ai_provider(
        &f.core,
        &none(),
        &id,
        j(json!({"type": "openai-compatible", "baseUrl": "http://x"})),
        Some(None),
    )
    .await
    .unwrap();
    assert!(f.store.entries().is_empty());
    let p = &stored_ai(&f).await["providers"][0];
    assert_eq!(
        (p["type"].as_str(), p["baseUrl"].as_str()),
        (Some("openai-compatible"), Some("http://x"))
    );
}

#[tokio::test]
async fn an_api_key_on_web_is_not_supported() {
    let f = fx_with(web_state_limits(), false).await;
    let id =
        f.ws.create_ai_provider(&f.core, &none(), provider("A"), None)
            .await
            .unwrap()
            .value
            .id;
    for key in [Some(Some("sk".to_string())), Some(None)] {
        let e =
            f.ws.update_ai_provider(&f.core, &none(), &id, j(json!({"name": "B"})), key.clone())
                .await
                .unwrap_err();
        assert_eq!(e.code, "NOT_SUPPORTED");
        let e =
            f.ws.create_ai_provider(&f.core, &none(), provider("C"), key)
                .await
                .unwrap_err();
        assert_eq!(e.code, "NOT_SUPPORTED");
    }
    assert_eq!(
        provider_names(&stored_ai(&f).await),
        ["A"],
        "nothing written"
    );
}

#[tokio::test]
async fn removing_a_provider_deletes_its_key_or_vault_rows() {
    // Desktop: the keychain entry.
    let f = fx().await;
    let id =
        f.ws.create_ai_provider(&f.core, &none(), provider("A"), Some(Some("sk".into())))
            .await
            .unwrap()
            .value
            .id;
    f.ws.remove_ai_provider(&f.core, &none(), &id)
        .await
        .unwrap();
    assert!(f.store.entries().is_empty());
    assert_eq!(
        f.ws.remove_ai_provider(&f.core, &none(), &id)
            .await
            .unwrap_err()
            .code,
        "AI_PROVIDER_NOT_FOUND"
    );
    // Web: the vault's rows for that provider, in the same transaction.
    let w = fx_with(web_state_limits(), false).await;
    let id =
        w.ws.create_ai_provider(&w.core, &none(), provider("A"), None)
            .await
            .unwrap()
            .value
            .id;
    let cred = |scope: &str, key: &str| json!({"scope": scope, "key": key, "nonce": "n", "ciphertext": "c", "updated_at": "t"});
    insert_rows(
        w.ws.storage(),
        "user_credentials",
        &[
            cred("ai-api-key-provider", &id),
            cred("ai-api-key-provider", "other"),
            cred("db", &id),
        ],
    )
    .await;
    w.ws.remove_ai_provider(&w.core, &none(), &id)
        .await
        .unwrap();
    let left: Vec<(String, String)> = dump(w.ws.storage(), "user_credentials", "scope, key")
        .await
        .iter()
        .map(|r| {
            (
                r["scope"].as_str().unwrap().into(),
                r["key"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        left,
        vec![
            ("ai-api-key-provider".into(), "other".into()),
            ("db".into(), id.clone())
        ]
    );
}

// ── Themes ──

#[tokio::test]
async fn removing_the_theme_in_use_resets_the_preference() {
    let f = fx().await;
    let a =
        f.ws.create_user_theme(&f.core, &none(), rv(json!({"name": "A", "isDark": false})))
            .await
            .unwrap()
            .value
            .id;
    let b =
        f.ws.create_user_theme(&f.core, &none(), rv(json!({"name": "B", "isDark": true})))
            .await
            .unwrap()
            .value
            .id;
    f.ws.set_theme_preferences(
        &f.core,
        &none(),
        ThemePreferences {
            light_theme_id: a.clone(),
            dark_theme_id: b.clone(),
        },
    )
    .await
    .unwrap();
    let t =
        f.ws.remove_user_theme(&f.core, &none(), &a)
            .await
            .unwrap()
            .value;
    assert_eq!(
        (
            t.preferences.light_theme_id.as_str(),
            t.preferences.dark_theme_id.as_str()
        ),
        ("default-light", b.as_str())
    );
    let t =
        f.ws.remove_user_theme(&f.core, &none(), &b)
            .await
            .unwrap()
            .value;
    assert_eq!(t.preferences.dark_theme_id, "default-dark");
    assert!(t.user_themes.is_empty());
    assert_eq!(
        f.ws.remove_user_theme(&f.core, &none(), &b)
            .await
            .unwrap_err()
            .code,
        "THEME_NOT_FOUND"
    );
    // No preferences row reads as the defaults, and a remove of a theme
    // not in use writes none.
    let g = fx().await;
    let c =
        g.ws.create_user_theme(&g.core, &none(), rv(json!({"name": "C"})))
            .await
            .unwrap()
            .value
            .id;
    g.ws.remove_user_theme(&g.core, &none(), &c).await.unwrap();
    assert!(dump(g.ws.storage(), "theme_preferences", "id")
        .await
        .is_empty());
    let got = g.ws.get_themes().await.unwrap().value.preferences;
    assert_eq!(got.light_theme_id, "default-light");
}

#[tokio::test]
async fn a_theme_update_keeps_its_id_and_created_at() {
    let f = fx().await;
    let c =
        f.ws.create_user_theme(&f.core, &none(), rv(json!({"name": "A"})))
            .await
            .unwrap()
            .value;
    let before: Value = serde_json::from_str(c.themes.user_themes[0].get()).unwrap();
    let t =
        f.ws.update_user_theme(
            &f.core,
            &none(),
            &c.id,
            rv(json!({"name": "B", "id": "spoof", "createdAt": "x"})),
        )
        .await
        .unwrap()
        .value;
    let after: Value = serde_json::from_str(t.user_themes[0].get()).unwrap();
    assert_eq!(
        (&after["id"], &after["createdAt"], &after["name"]),
        (&before["id"], &before["createdAt"], &json!("B"))
    );
    assert!(after["updatedAt"].as_str() > before["updatedAt"].as_str());
    assert_eq!(
        f.ws.update_user_theme(&f.core, &none(), "theme-none", rv(json!({"name": "x"})))
            .await
            .unwrap_err()
            .code,
        "THEME_NOT_FOUND"
    );
    assert_eq!(
        f.ws.create_user_theme(
            &f.core,
            &none(),
            rv(json!({"name": "x", "isBuiltIn": true}))
        )
        .await
        .unwrap_err()
        .code,
        "INVALID_ARGUMENT"
    );
}

// ── Onboarding, tutorial, import ──

#[tokio::test]
async fn onboarding_patches_merge_into_the_stored_record() {
    let f = fx().await;
    let d = f.ws.get_onboarding().await.unwrap().value;
    assert_eq!(
        serde_json::from_str::<Value>(d.get()).unwrap()["isFirstRun"],
        true
    );
    f.ws.patch_onboarding(&f.core, &none(), rv(json!({"dismissedHints": ["a"]})))
        .await
        .unwrap();
    // A second window's patch of another field keeps the first's.
    let merged =
        f.ws.patch_onboarding(&f.core, &none(), rv(json!({"learnEnabled": false})))
            .await
            .unwrap()
            .value;
    let v: Value = serde_json::from_str(merged.get()).unwrap();
    assert_eq!(
        (v["dismissedHints"].clone(), v["learnEnabled"].clone()),
        (json!(["a"]), json!(false))
    );
    assert_eq!(
        f.ws.patch_onboarding(&f.core, &none(), rv(json!({"x": 1})))
            .await
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );
}

#[tokio::test]
async fn tutorial_and_import_state_round_trip() {
    let f = fx().await;
    f.ws.save_tutorial(
        &f.core,
        &none(),
        j(json!({"lessonId": "l1", "challengeId": "c1", "state": null})),
    )
    .await
    .unwrap();
    let all =
        f.ws.save_tutorial(
            &f.core,
            &none(),
            j(json!({"lessonId": "l2", "challengeId": "c1", "state": "{not json"})),
        )
        .await
        .unwrap()
        .value;
    assert_eq!(all.len(), 2);
    assert_eq!(
        f.ws.remove_tutorial_lesson(&f.core, &none(), "l1")
            .await
            .unwrap()
            .value
            .len(),
        1
    );
    assert!(f.ws.reset_tutorial(&none()).await.unwrap().value.is_empty());
    assert!(f.ws.list_tutorial().await.unwrap().value.is_empty());
    assert_eq!(
        f.ws.get_import_state("tableplus").await.unwrap().value,
        None
    );
    f.ws.save_import_state(
        &f.core,
        &none(),
        "tableplus",
        j(json!({"hasOfferedImport": true, "lastCheckTimestamp": "t"})),
    )
    .await
    .unwrap();
    assert!(
        f.ws.get_import_state("tableplus")
            .await
            .unwrap()
            .value
            .unwrap()
            .has_offered_import
    );
    assert_eq!(
        f.ws.get_import_state("navicat").await.unwrap_err().code,
        "INVALID_ARGUMENT"
    );
}

/// Phase 5d-2 review: a failed update puts the old key back only while the
/// entry still holds this call's key; one another call set meanwhile stays.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_update_keeps_a_key_another_call_set_meanwhile() {
    let f = Arc::new(fx().await);
    let id =
        f.ws.create_ai_provider(&f.core, &none(), provider("A"), Some(Some("sk-old".into())))
            .await
            .unwrap()
            .value
            .id;
    let name = format!("ai-api-key:{id}");
    // Hold the write lock, so the update sets its key and then waits.
    let mut tx = f.ws.storage().write().await.unwrap();
    let (g, i) = (f.clone(), id.clone());
    let update = tokio::spawn(async move {
        g.ws.update_ai_provider(
            &g.core,
            &none(),
            &i,
            j(json!({"name": "B"})),
            Some(Some("sk-new".into())),
        )
        .await
    });
    while f.store.entries().get(&name).map(String::as_str) != Some("sk-new") {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Meanwhile: another call's key, and the provider removed.
    f.store.put(&name, "sk-other");
    seaquel_core::storage::app_state::set_in(&mut tx, "aiSettings", Some(r#"{"providers":[]}"#))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let e = update.await.unwrap().unwrap_err();
    assert_eq!(e.code, "AI_PROVIDER_NOT_FOUND");
    assert_eq!(
        f.store.entries().get(&name).map(String::as_str),
        Some("sk-other")
    );
}
