//! The state's pure rules (`seaquel_workspace::state`, phase 5d-2): the
//! checks each state fixture's calls meet, the settings' closed key set,
//! the AI settings read and rewrite, onboarding, the legacy mirror, and the
//! limits. Core's replay (`seaquel-core/tests/state.rs`) runs the same
//! fixtures against storage.

use std::collections::HashSet;

use seaquel_workspace::library::{LibraryError, LibraryLimits, INVALID_ARGUMENT};
use seaquel_workspace::state::*;
use serde_json::value::RawValue;
use serde_json::{json, Value};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/state");
const FILES: [&str; 9] = [
    "view-state.json",
    "workflows.json",
    "dashboards.json",
    "chats.json",
    "settings.json",
    "ai-settings.json",
    "themes.json",
    "misc.json",
    "old-data.json",
];

fn load(file: &str) -> Vec<Value> {
    serde_json::from_str(&std::fs::read_to_string(format!("{FIXTURES}/{file}")).unwrap()).unwrap()
}

fn j<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap_or_else(|e| panic!("{e}"))
}

fn raw(v: &Value) -> Box<RawValue> {
    RawValue::from_string(v.to_string()).unwrap()
}

fn raw_str(s: &str) -> Box<RawValue> {
    RawValue::from_string(s.to_string()).unwrap()
}

/// `<id:n>` tokens become plain ids (`id<n>`), as the GUI's would be.
fn untoken(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace("<id:", "id").replace('>', "")),
        Value::Array(a) => Value::Array(a.iter().map(untoken).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), untoken(v))).collect()),
        other => other.clone(),
    }
}

/// The checks a call meets before anything is read.
fn input_check(call: &Value) -> Result<(), LibraryError> {
    let lib = LibraryLimits::default();
    let st = StateLimits::default();
    let p = untoken(call.get("params").unwrap_or(&Value::Null));
    let s = |k: &str| p[k].as_str().unwrap_or_default().to_string();
    match call["method"].as_str().unwrap() {
        "settingGet" => SettingKey::parse(&s("key")).map(|_| ()),
        "settingSet" => check_setting_set(&s("key"), p["value"].as_str(), &lib, &st).map(|_| ()),
        "dashboardCreate" => check_dashboard_draft(&j(p["dashboard"].clone()), &lib, &st),
        "dashboardUpdate" => check_dashboard_patch(&j(p["patch"].clone()), &lib, &st),
        "workflowCreate" => {
            let d: WorkflowDraft = j(p["workflow"].clone());
            check_workflow_body(&d.workflow, &lib, &st)
        }
        "workflowUpdate" => check_workflow_body(&raw(&p["workflow"]), &lib, &st),
        "chatCreate" => check_chat_draft(&j(p["chat"].clone()), &lib),
        "chatUpdate" => check_chat_patch(&j(p["patch"].clone()), &lib),
        "chatMessagesPut" => {
            let m: Vec<ChatMessageDraft> = j(p["messages"].clone());
            check_messages(&m, &lib, &st)
        }
        "aiProviderCreate" => check_ai_provider_draft(&j(p["provider"].clone()), &lib),
        "aiProviderUpdate" => check_ai_provider_patch(&j(p["patch"].clone()), &lib),
        "aiSettingsPatch" => serde_json::from_value::<AiSettingsPatch>(p["patch"].clone())
            .map(|_| ())
            .map_err(|e| LibraryError::invalid(e.to_string())),
        "onboardingPatch" => check_onboarding_patch(&raw(&p["patch"]), &lib, &st),
        "userThemeCreate" | "userThemeUpdate" => check_user_theme(&raw(&p["theme"]), &lib, &st),
        "themePreferencesSet" => {
            check_theme_id(&s("lightThemeId"), &lib)?;
            check_theme_id(&s("darkThemeId"), &lib)
        }
        "tutorialSave" => {
            check_tutorial_ids(&s("lessonId"), Some(&s("challengeId")), &lib)?;
            check_tutorial_state(p["state"].as_str(), &st)
        }
        "importStateSave" => {
            check_import_source(&s("source"))?;
            check_import_time(p["lastCheckTimestamp"].as_str(), &lib)
        }
        "importStateGet" => check_import_source(&s("source")),
        "windowStateSave" => {
            check_window_id(&s("windowId"))?;
            legacy_mirror(&raw(&p["state"]), &s("projectId"), &st).map(|_| ())
        }
        "windowStateLoad" | "windowGet" | "windowActivate" => check_window_id(&s("windowId")),
        "projectSidebarSet" => {
            let order: Vec<String> = j(p["connectionOrder"].clone());
            check_connection_order(&order, &lib)
        }
        _ => Ok(()),
    }
}

/// Every call of every fixture step meets its input checks, except the
/// steps `changes.json` lists as `INVALID_ARGUMENT`, which fail them, and
/// exactly those.
#[test]
fn replays_every_fixture_check() {
    let changes: serde_json::Map<String, Value> =
        serde_json::from_str(&std::fs::read_to_string(format!("{FIXTURES}/changes.json")).unwrap())
            .unwrap();
    let mut listed = HashSet::new();
    for (name, entry) in &changes {
        for (i, step) in entry["expected"]["steps"].as_object().into_iter().flatten() {
            if step["outcome"]["code"] == INVALID_ARGUMENT {
                listed.insert(format!("{name}#{i}"));
            }
        }
    }
    let mut refused = HashSet::new();
    let mut calls = 0;
    for file in FILES {
        for case in load(file) {
            let name = case["name"].as_str().unwrap();
            for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
                for call in step["core"].as_array().into_iter().flatten() {
                    calls += 1;
                    if let Err(e) = input_check(call) {
                        assert_eq!(e.code, INVALID_ARGUMENT, "{name}#{i}: {e}");
                        refused.insert(format!("{name}#{i}"));
                    }
                }
            }
        }
    }
    assert_eq!(
        refused, listed,
        "the input checks refuse exactly the listed steps"
    );
    assert!(calls > 1000, "{calls}");
}

// ── Settings ──

#[test]
fn unknown_setting_keys_are_refused() {
    for key in [
        "somethingElse",
        "",
        "EDITORKEYBINDINGMODE",
        "aiSettings",
        "editorKeybindingMode ",
    ] {
        let e = SettingKey::parse(key).unwrap_err();
        assert_eq!(e.code, INVALID_ARGUMENT);
        assert!(e.message.contains(&format!("{key:?}")), "{e}");
    }
    // A long key isn't echoed.
    let long = "k".repeat(10_000);
    let e = SettingKey::parse(&long).unwrap_err();
    assert!(!e.message.contains(&long[..100]), "{}", e.message.len());
    for k in SettingKey::ALL {
        assert_eq!(SettingKey::parse(k.as_str()).unwrap(), k);
        // The wire form is the stored key.
        assert_eq!(serde_json::to_value(k).unwrap(), json!(k.as_str()));
    }
}

#[test]
fn core_owned_keys_are_refused() {
    let (lib, st) = (LibraryLimits::default(), StateLimits::default());
    for key in [
        "activeRepoId",
        "connectionStringSecretsUpgraded",
        "connectionStringSecretsVacuum",
        "connectionStringSecretsCheckpoint",
        "aiSettings",
    ] {
        assert!(SettingKey::parse(key).is_err(), "{key}");
        assert!(check_setting_set(key, None, &lib, &st).is_err(), "{key}");
    }
    // Written by windowActivate only, even cleared.
    for v in [None, Some("p1")] {
        let e = check_setting_set("lastActiveProjectId", v, &lib, &st).unwrap_err();
        assert!(e.message.contains("lastActiveProjectId"), "{e}");
    }
    // Read, and only cleared.
    assert!(SettingKey::parse("lastActiveProjectId").is_ok());
    assert!(check_setting_set("connectionStringSecretsNotice", None, &lib, &st).is_ok());
    assert!(check_setting_set("connectionStringSecretsNotice", Some("[]"), &lib, &st).is_err());
}

#[test]
fn each_setting_value_is_checked() {
    let (lib, st) = (LibraryLimits::default(), StateLimits::default());
    let ok = |k: &str, v: &str| check_setting_set(k, Some(v), &lib, &st).is_ok();
    for v in ["default", "vim", "emacs"] {
        assert!(ok("editorKeybindingMode", v));
    }
    for v in ["hyper", "", "VIM", "vim "] {
        assert!(!ok("editorKeybindingMode", v), "{v}");
    }
    assert!(ok("pending_changes_enabled", "true") && ok("pending_changes_enabled", "false"));
    assert!(!ok("pending_changes_enabled", "maybe") && !ok("pending_changes_enabled", "1"));
    assert!(ok("skippedUpdateVersion", "2030.1.2"));
    assert!(!ok("skippedUpdateVersion", "2030.1.3\0"));
    for key in ["query_version_limit", "dashboard_version_limit"] {
        for v in ["0", "10", "100", "100000", "007"] {
            assert!(ok(key, v), "{key} {v}");
        }
        for v in [
            "abc", "100001", "-1", "1.5", "", " 5", "+5", "1e3", "9999999",
        ] {
            assert!(!ok(key, v), "{key} {v}");
        }
    }
    assert!(ok("license_nudge", r#"{"queryCount":1,"answer":null}"#));
    for v in ["[1]", "nope", "null", "1", "\"x\""] {
        assert!(!ok("license_nudge", v), "{v}");
    }
    // `None` deletes, for every writable key.
    for k in SettingKey::ALL {
        if k != SettingKey::LastActiveProjectId {
            assert!(
                check_setting_set(k.as_str(), None, &lib, &st).is_ok(),
                "{k:?}"
            );
        }
    }
    // Every value refusal names the key.
    let e = check_setting_set("skippedUpdateVersion", Some("a\0"), &lib, &st).unwrap_err();
    assert!(e.message.contains("skippedUpdateVersion"), "{e}");
    assert!(!e.message.contains("a\0"));
}

// ── AI settings ──

fn ai(raw: Option<&str>) -> Value {
    serde_json::from_str(&read_ai_settings(raw).to_json()).unwrap()
}

fn defaults() -> Value {
    json!({"enabled": true, "providers": [], "shareSchemaGlobally": true, "shareDataGlobally": false})
}

/// The legacy and malformed seeds (`old-data/ai-settings-*`) read as the
/// store's `initialize` read them (the recorded views; for a JSON array,
/// the defaults, as `changes.json` says).
#[test]
fn ai_settings_read_matches_todays_load() {
    for case in load("old-data.json")
        .into_iter()
        .chain(load("ai-settings.json"))
    {
        let name = case["name"].as_str().unwrap();
        let Some(seed) = case["seed"]["app_state"]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["key"] == "aiSettings"))
        else {
            continue;
        };
        let step = &case["steps"][0];
        // The step's only call is the load, so its view is what it read.
        let calls = step["core"].as_array().map_or(0, Vec::len);
        if calls != 1 || step["core"][0]["method"] != "aiSettingsGet" {
            continue;
        }
        let expected = if name == "old-data/ai-settings-array" {
            defaults()
        } else {
            step["view"]["settings"].clone()
        };
        assert_eq!(ai(seed["value"].as_str()), expected, "{name}");
    }
    for r in [
        None,
        Some(""),
        Some("{not json"),
        Some("null"),
        Some("42"),
        Some("\"s\""),
        Some("[1]"),
    ] {
        assert_eq!(ai(r), defaults(), "{r:?}");
    }
    for r in [
        r#"{"providers":[null]}"#,
        r#"{"providers":5}"#,
        r#"{"providers":{}}"#,
    ] {
        assert_eq!(ai(Some(r)), defaults(), "{r}");
    }
    assert_eq!(
        ai(Some(r#"{"providers":null,"shareDataGlobally":true}"#)),
        json!({"enabled": true, "providers": [], "shareSchemaGlobally": true, "shareDataGlobally": true})
    );
}

#[test]
fn the_rewrite_keeps_fields_it_doesnt_know_byte_for_byte() {
    let stored = r#"{"futureField":{"on" : true, "n": 1.50},"enabled":true,"providers":[{"id":"p1","name":"A","type":"anthropic","region":"eu","limits":[1,2.0]}],"shareDataGlobally":null}"#;
    let mut s = read_ai_settings(Some(stored));
    s.apply_patch(&j(json!({"enabled": false})));
    s.update_provider("p1", &j(json!({"name": "B"}))).unwrap();
    let out = s.to_json();
    // Known fields first (as `{...DEFAULT_AI_SETTINGS, ...parsed}`), the
    // rest in their order, each value as stored.
    assert_eq!(
        out,
        r#"{"enabled":false,"providers":[{"id":"p1","name":"B","type":"anthropic","region":"eu","limits":[1,2.0]}],"shareSchemaGlobally":true,"shareDataGlobally":null,"futureField":{"on" : true, "n": 1.50}}"#
    );
    // Adding and removing providers keeps the others as they were.
    s.add_provider(
        "p2",
        &j(json!({"name": "Local", "type": "openai-compatible", "baseUrl": "http://x"})),
    );
    assert_eq!(s.provider_count(), 2);
    assert!(s.has_provider("p2"));
    s.update_provider("p2", &j(json!({"baseUrl": null, "type": "anthropic"})))
        .unwrap();
    assert!(!s.to_json().contains("http://x"));
    s.remove_provider("p1").unwrap();
    assert_eq!(
        s.remove_provider("p1").unwrap_err().code,
        AI_PROVIDER_NOT_FOUND
    );
    assert_eq!(
        s.update_provider("none", &AiProviderPatch::default())
            .unwrap_err()
            .code,
        AI_PROVIDER_NOT_FOUND
    );
    assert!(s
        .to_json()
        .contains(r#""futureField":{"on" : true, "n": 1.50}"#));
}

// ── Onboarding ──

#[test]
fn onboarding_reads_over_the_defaults_and_merges_patches() {
    let d = read_onboarding(None);
    assert_eq!(
        d.get(),
        r#"{"isFirstRun":true,"userBackground":"none","hasCompletedWizard":false,"showWizardHints":true,"dismissedHints":[],"learnEnabled":true}"#
    );
    assert_eq!(read_onboarding(Some(&raw_str("[1]"))).get(), d.get());
    let stored = raw_str(r#"{"isFirstRun":false,"dismissedHints":["sidebar"],"future":1}"#);
    let merged = merge_onboarding(Some(&stored), &raw_str(r#"{"hasCompletedWizard":true}"#));
    assert_eq!(
        merged.get(),
        r#"{"isFirstRun":false,"userBackground":"none","hasCompletedWizard":true,"showWizardHints":true,"dismissedHints":["sidebar"],"learnEnabled":true,"future":1}"#
    );
    let (lib, st) = (LibraryLimits::default(), StateLimits::default());
    assert!(check_onboarding_patch(&raw_str(r#"{"learnEnabled":false}"#), &lib, &st).is_ok());
    for bad in [
        r#"{"learnEnabled":"no"}"#,
        r#"{"dismissedHints":[1]}"#,
        r#"{"somethingElse":true}"#,
        "[1]",
        r#"{"userBackground":null}"#,
    ] {
        assert!(
            check_onboarding_patch(&raw_str(bad), &lib, &st).is_err(),
            "{bad}"
        );
    }
}

// ── Themes and workflows ──

#[test]
fn a_theme_gets_its_id_and_times_and_keeps_the_rest() {
    let body =
        raw_str(r#"{"name":"Mine","isDark":false,"colors":{"a" : 1},"id":"gui","createdAt":"x"}"#);
    let out: Value = serde_json::from_str(&user_theme_json(&body, "theme-1", "c", "u")).unwrap();
    assert_eq!(
        out,
        json!({"name": "Mine", "isDark": false, "colors": {"a": 1}, "id": "theme-1",
               "isBuiltIn": false, "createdAt": "c", "updatedAt": "u"})
    );
    let (lib, st) = (LibraryLimits::default(), StateLimits::default());
    assert!(check_user_theme(&raw_str(r#"{"name":"A","isBuiltIn":false}"#), &lib, &st).is_ok());
    for bad in [
        r#"{"name":"A","isBuiltIn":true}"#,
        r#"{"isDark":true}"#,
        "[]",
    ] {
        assert!(check_user_theme(&raw_str(bad), &lib, &st).is_err(), "{bad}");
    }
    assert_eq!(
        created_at_of(Some(&raw_str(r#"{"createdAt":"t"}"#))).as_deref(),
        Some("t")
    );
    assert_eq!(created_at_of(Some(&raw_str("\"nope\""))), None);
}

#[test]
fn a_workflow_gets_its_fields_and_keeps_the_rest_byte_for_byte() {
    let body = raw_str(
        r#"{"id":"gui","projectId":"p9","name":"F","nodes":[{"rows":[[1.50]]}],"updatedAt":"x"}"#,
    );
    let out = workflow_json(&body, "workflow-1", "p1", "c", "u");
    assert_eq!(
        out,
        r#"{"id":"workflow-1","name":"F","nodes":[{"rows":[[1.50]]}],"projectId":"p1","createdAt":"c","updatedAt":"u"}"#
    );
}

/// 5d-2 Task 7 probe: Core stored a workflow whose `name` was `5`. A
/// workflow's name must be text, and a missing one is refused too.
#[test]
fn a_workflow_name_is_text() {
    let lib = LibraryLimits::default();
    assert!(check_workflow_shape(&raw_str(r#"{"name":"F","nodes":[]}"#), &lib).is_ok());
    for (bad, says) in [
        (r#"{"nodes":[]}"#, "needs a name"),
        (r#"{"name":null}"#, "needs a name"),
        (r#"{"name":5}"#, "name is text"),
        (r#"{"name":["F"]}"#, "name is text"),
        (r#"{"name":"\ud800"}"#, "lone surrogate"),
    ] {
        let e = check_workflow_shape(&raw_str(bad), &lib).unwrap_err();
        assert_eq!(e.code, INVALID_ARGUMENT, "{bad}");
        assert!(e.message.contains(says), "{bad}: {}", e.message);
        let e = check_workflow_body(&raw_str(bad), &lib, &StateLimits::default()).unwrap_err();
        assert!(e.message.contains(says), "{bad}: {}", e.message);
    }
}

/// 5d-2 Task 7 review: a rename changes only the stored workflow's `name`
/// and `updatedAt`; everything else stays byte for byte, so a rename can't
/// undo another window's save.
#[test]
fn a_workflow_rename_changes_only_its_name_and_time() {
    let stored = raw_str(
        r#"{"id":"w","name":"Old","nodes":[{"n":1.50,"big":12345678901234567890}],"projectId":"p","createdAt":"c","updatedAt":"u","future":{"a" : 1}}"#,
    );
    assert_eq!(
        rename_workflow_json(&stored, "New \"one\"", "t").as_deref(),
        Some(
            r#"{"id":"w","name":"New \"one\"","nodes":[{"n":1.50,"big":12345678901234567890}],"projectId":"p","createdAt":"c","updatedAt":"t","future":{"a" : 1}}"#
        )
    );
    // A workflow with no name or time gets them, at the end.
    assert_eq!(
        rename_workflow_json(&raw_str(r#"{"id":"w"}"#), "N", "t").as_deref(),
        Some(r#"{"id":"w","name":"N","updatedAt":"t"}"#)
    );
    assert_eq!(rename_workflow_json(&raw_str("[1]"), "N", "t"), None);
    let lib = LibraryLimits::default();
    assert!(check_workflow_rename("N", &lib).is_ok());
    for bad in ["", "  ", "a\0b"] {
        assert!(check_workflow_rename(bad, &lib).is_err(), "{bad:?}");
    }
}

/// 5d-2 Task 7 re-review: a hand-edited row with duplicate keys renames as
/// the GUI reads it. `JSON.parse` keeps the last `name`; the stored object
/// is read with duplicates collapsed (the last value, at the first key's
/// place), so no older `name` is left behind to win on the next read.
#[test]
fn a_rename_of_a_row_with_duplicate_keys_leaves_one_name() {
    let stored = raw_str(r#"{"name":"A","nodes":[1],"name":"B","updatedAt":"u","updatedAt":"v"}"#);
    let out = rename_workflow_json(&stored, "New", "t").unwrap();
    assert_eq!(out, r#"{"name":"New","nodes":[1],"updatedAt":"t"}"#);
    let read: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (read["name"].as_str(), read["updatedAt"].as_str()),
        (Some("New"), Some("t"))
    );
}

/// 5d-2 Task 7 probe: a theme named with a lone surrogate was refused as
/// "A theme needs a name." It has one; say what's wrong with it.
#[test]
fn a_theme_name_with_a_lone_surrogate_is_worded_as_such() {
    let (lib, st) = (LibraryLimits::default(), StateLimits::default());
    let e = check_user_theme(&raw_str(r#"{"name":"A\udc00"}"#), &lib, &st).unwrap_err();
    assert!(e.message.contains("lone surrogate"), "{}", e.message);
    assert!(!e.message.contains("needs a name"), "{}", e.message);
    let e = check_user_theme(&raw_str(r#"{"name":7}"#), &lib, &st).unwrap_err();
    assert!(e.message.contains("name is text"), "{}", e.message);
    let e = check_user_theme(&raw_str(r#"{"isDark":true}"#), &lib, &st).unwrap_err();
    assert!(e.message.contains("needs a name"), "{}", e.message);
}

/// 5d-2 Task 7 probe: `onboardingPatch` took any string for
/// `userBackground`; the GUI has three (`UserBackground` in
/// `stores/onboarding.svelte.ts`).
#[test]
fn an_onboarding_background_is_one_the_gui_has() {
    let (lib, st) = (LibraryLimits::default(), StateLimits::default());
    for ok in ["none", "datagrip", "dbeaver"] {
        let patch = raw_str(&format!(r#"{{"userBackground":"{ok}"}}"#));
        assert!(check_onboarding_patch(&patch, &lib, &st).is_ok(), "{ok}");
    }
    for bad in ["", "DataGrip", "tableplus", "none "] {
        let patch = raw_str(&format!(r#"{{"userBackground":"{bad}"}}"#));
        let e = check_onboarding_patch(&patch, &lib, &st).unwrap_err();
        assert_eq!(e.code, INVALID_ARGUMENT, "{bad}");
    }
}

/// 5d-2 Task 7 probe: Core took a `rev` up to `i64::MAX`, but the page
/// counts in JavaScript numbers, so after a save at such a value its
/// `rev + 1` rounded to a number Core refused and the window stopped
/// saving. A `rev` past 2^53 - 1 is refused.
#[test]
fn a_rev_past_2_pow_53_is_refused() {
    assert_eq!(MAX_REV, (1u64 << 53) - 1);
    assert!(check_rev(0).is_ok());
    assert!(check_rev(MAX_REV).is_ok());
    for bad in [MAX_REV + 1, i64::MAX as u64, u64::MAX] {
        let e = check_rev(bad).unwrap_err();
        assert_eq!(e.code, INVALID_ARGUMENT, "{bad}");
        assert!(e.message.contains("rev"), "{}", e.message);
    }
}

// ── The legacy mirror ──

/// Every view state the fixtures save becomes the mirror's
/// `PersistedProjectState` with the same tabs and active ids (Core's
/// replay checks the rows storage writes from it equal today's).
#[test]
fn the_mirror_of_a_state_equals_todays_rows() {
    let mut saves = 0;
    for file in FILES {
        for case in load(file) {
            for step in case["steps"].as_array().unwrap() {
                for call in step["core"].as_array().into_iter().flatten() {
                    if call["method"] != "windowStateSave" {
                        continue;
                    }
                    saves += 1;
                    let state = untoken(&call["params"]["state"]);
                    let project = call["params"]["projectId"].as_str().unwrap();
                    let m = legacy_mirror(&raw(&state), project, &StateLimits::default()).unwrap();
                    let back = serde_json::to_value(&m).unwrap();
                    for key in [
                        "queryTabs",
                        "schemaTabs",
                        "explainTabs",
                        "erdTabs",
                        "statisticsTabs",
                        "workflowTabs",
                        "starterTabs",
                        "dashboardTabs",
                        "createTableTabs",
                        "dataTabs",
                        "tabOrder",
                        "activeView",
                        "activeConnectionId",
                        "activeQueryTabId",
                        "paneLayout",
                    ] {
                        let mut want = state.get(key).cloned().unwrap_or(Value::Null);
                        // The mirror writes `canvas` as `workflow`.
                        if key == "activeView" && want == "canvas" {
                            want = json!("workflow");
                        }
                        let got = back.get(key).cloned().unwrap_or(Value::Null);
                        let empty =
                            |v: &Value| v.is_null() || v.as_array().is_some_and(Vec::is_empty);
                        assert!(
                            want == got || (empty(&want) && empty(&got)),
                            "{key}: {want} vs {got}"
                        );
                    }
                }
            }
        }
    }
    assert!(saves > 100, "{saves}");
}

#[test]
fn a_repeated_tab_id_is_skipped() {
    // The state is kept as sent (the window's row); the mirror doesn't
    // refuse it, and storage's `write_legacy_mirror` skips the repeat.
    let state = json!({"projectId": "p1", "tabOrder": ["t", "t"], "activeView": "query",
        "queryTabs": [{"id": "t", "name": "A", "query": "1"}, {"id": "t", "name": "B", "query": "2"}],
        "schemaTabs": [], "explainTabs": [], "erdTabs": []});
    let m = legacy_mirror(&raw(&state), "p1", &StateLimits::default()).unwrap();
    assert_eq!(m.query_tabs.len(), 2);
    // A state without its project id gets it; another project's is refused.
    let mut no_id = state.clone();
    no_id.as_object_mut().unwrap().remove("projectId");
    assert_eq!(
        legacy_mirror(&raw(&no_id), "p1", &StateLimits::default())
            .unwrap()
            .project_id,
        "p1"
    );
    assert!(legacy_mirror(&raw(&state), "p2", &StateLimits::default()).is_err());
    assert!(legacy_mirror(&raw_str("[1]"), "p1", &StateLimits::default()).is_err());
    assert!(legacy_mirror(
        &raw_str(r#"{"queryTabs": 5}"#),
        "p1",
        &StateLimits::default()
    )
    .is_err());
}

#[test]
fn the_legacy_view_leaves_out_what_isnt_a_windows() {
    let s: seaquel_types::storage::PersistedProjectState = j(json!({
        "projectId": "p1", "queryTabs": [], "schemaTabs": [], "explainTabs": [], "erdTabs": [],
        "tabOrder": [], "activeView": "query", "connectionOrder": ["c1"],
        "savedWorkflows": [{"id": "w"}], "starredSharedQueryIds": [], "starredSharedDashboardIds": []
    }));
    let v: Value = serde_json::from_str(legacy_view(&s).get()).unwrap();
    for gone in [
        "connectionOrder",
        "savedWorkflows",
        "starredSharedQueryIds",
        "starredSharedDashboardIds",
    ] {
        assert!(v.get(gone).is_none(), "{gone}");
    }
    assert_eq!(v["projectId"], "p1");
}

// ── Limits, panics, Debug ──

fn web() -> (LibraryLimits, StateLimits) {
    (
        LibraryLimits {
            max_name_bytes: Some(1024),
            max_field_bytes: Some(64 * 1024),
            max_query_bytes: Some(2 * 1024 * 1024),
            max_list_items: Some(1_000),
            max_connections: Some(10_000),
            max_projects: Some(1_000),
            max_saved_queries: Some(50_000),
            max_version_bytes: Some(16 * 1024 * 1024),
        },
        StateLimits {
            max_view_state_bytes: Some(8 * 1024 * 1024),
            max_tab_text_bytes: Some(2 * 1024 * 1024),
            max_tabs: Some(500),
            max_windows: 50,
            max_window_states_per_project: 20,
            spare_main_window: false,
            max_workflow_bytes: Some(16 * 1024 * 1024),
            max_workflows: Some(1_000),
            max_dashboard_bytes: Some(4 * 1024 * 1024),
            max_dashboards: Some(1_000),
            max_dashboard_version_bytes: Some(16 * 1024 * 1024),
            max_message_bytes: Some(1024 * 1024),
            max_messages_per_chat: Some(5_000),
            max_chat_bytes: Some(64 * 1024 * 1024),
            max_chats: Some(10_000),
            max_setting_bytes: Some(256 * 1024),
            max_user_themes: Some(200),
            max_ai_providers: Some(50),
        },
    )
}

#[test]
fn limits_are_the_interfaces() {
    let desktop = StateLimits::default();
    assert_eq!(desktop, StateLimits::DESKTOP);
    assert_eq!(
        (desktop.max_windows, desktop.max_window_states_per_project),
        (20, 20)
    );
    assert!(desktop.spare_main_window);
    let (wl, ws) = web();
    let dl = LibraryLimits::default();
    let big = "x".repeat(5 * 1024 * 1024);
    let d: DashboardDraft =
        j(json!({"projectId": "p", "name": "D", "widgets": [big], "viewport": {}}));
    assert!(
        check_dashboard_draft(&d, &dl, &desktop).is_ok(),
        "the desktop has no limits"
    );
    assert!(check_dashboard_draft(&d, &wl, &ws)
        .unwrap_err()
        .message
        .contains("max_dashboard_bytes"));
    let m = vec![j::<ChatMessageDraft>(
        json!({"id": "m", "role": "user", "content": "x".repeat(2 * 1024 * 1024), "timestamp": "t"}),
    )];
    assert!(check_messages(&m, &dl, &desktop).is_ok());
    assert!(check_messages(&m, &wl, &ws)
        .unwrap_err()
        .message
        .contains("max_message_bytes"));
    let theme = raw(&json!({"name": "T", "pad": "x".repeat(300 * 1024)}));
    assert!(check_user_theme(&theme, &dl, &desktop).is_ok());
    assert!(check_user_theme(&theme, &wl, &ws)
        .unwrap_err()
        .message
        .contains("max_setting_bytes"));
    assert!(check_chat_budget(64 * 1024 * 1024, 0, 0, &ws).is_ok());
    assert!(check_chat_budget(64 * 1024 * 1024, 0, 1, &ws).is_err());
    assert!(check_chat_budget(64 * 1024 * 1024, 10, 10, &ws).is_ok());
    assert!(check_chat_budget(u64::MAX, 0, u64::MAX, &desktop).is_ok());
    let wf = format!(
        r#"{{"name":"W","nodes":"{}"}}"#,
        "x".repeat(17 * 1024 * 1024)
    );
    assert!(check_workflow_body(&raw_str(&wf), &dl, &desktop).is_ok());
    assert!(check_workflow_body(&raw_str(&wf), &wl, &ws)
        .unwrap_err()
        .message
        .contains("max_workflow_bytes"));
}

#[test]
fn checks_never_panic() {
    let (wl, ws) = web();
    let zero = StateLimits {
        max_view_state_bytes: Some(0),
        max_tab_text_bytes: Some(0),
        max_tabs: Some(0),
        max_windows: 0,
        max_window_states_per_project: 0,
        spare_main_window: false,
        max_workflow_bytes: Some(0),
        max_workflows: Some(0),
        max_dashboard_bytes: Some(0),
        max_dashboards: Some(0),
        max_dashboard_version_bytes: Some(0),
        max_message_bytes: Some(0),
        max_messages_per_chat: Some(0),
        max_chat_bytes: Some(0),
        max_chats: Some(0),
        max_setting_bytes: Some(0),
        max_user_themes: Some(0),
        max_ai_providers: Some(0),
    };
    let texts = [
        "",
        " ",
        "\u{0}",
        "\u{10ffff}",
        "ß",
        "🦀",
        "{",
        "[",
        "null",
        "{}",
        "[]",
        "\"x\"",
        "1e999",
        r#"{"a":"#,
        r#"{"providers":[{"type":null}]}"#,
        r#"{"providers":[1,"s",[],{}]}"#,
        r#"{"a":1,"a":2}"#,
        r#"{"":null}"#,
        "\u{feff}{}",
    ];
    for (lib, st) in [
        (LibraryLimits::default(), StateLimits::default()),
        (wl, ws),
        (wl, zero),
    ] {
        for t in texts {
            let _ = read_ai_settings(Some(t)).to_json();
            let _ = SettingKey::parse(t);
            for k in SettingKey::ALL {
                let _ = check_setting_set(k.as_str(), Some(t), &lib, &st);
            }
            let _ = check_window_id(t);
            let _ = is_client_id(t);
            let _ = check_import_source(t);
            let _ = check_theme_id(t, &lib);
            if let Ok(r) = RawValue::from_string(t.to_string()) {
                let _ = read_onboarding(Some(&r));
                let _ = check_onboarding_patch(&r, &lib, &st);
                let _ = merge_onboarding(Some(&r), &r);
                let _ = check_user_theme(&r, &lib, &st);
                let _ = user_theme_json(&r, t, t, t);
                let _ = check_workflow_body(&r, &lib, &st);
                let _ = workflow_json(&r, t, t, t, t);
                let _ = legacy_mirror(&r, t, &st);
                let _ = created_at_of(Some(&r));
                let _ = id_of(&r);
                let _ = read_connection_order(Some(&r));
            }
            let d =
                json!({"projectId": t, "name": t, "description": t, "widgets": [], "viewport": {}});
            if let Ok(d) = serde_json::from_value::<DashboardDraft>(d) {
                let _ = check_dashboard_draft(&d, &lib, &st);
                let mut row = dashboard_from_draft(t.into(), &d, t);
                row.widgets = t.into();
                row.viewport = t.into();
                row.date_filter = Some(t.into());
                let _ = dashboard_snapshot(&row);
                let _ = apply_dashboard_patch(&mut row, &j(json!({"name": t})), t);
                let _ = check_dashboard_size(&row, Some(usize::MAX), &st);
                let _ = check_dashboard_size(&row, None, &st);
            }
            let m = json!({"id": t, "role": t, "content": t, "timestamp": t, "query": t, "dashboardId": t});
            if let Ok(m) = serde_json::from_value::<ChatMessageDraft>(m) {
                let _ = check_messages(&[m.clone(), m], &lib, &st);
            }
            let _ = check_message_ids(&[t.to_string()], &st);
            let mut s = read_ai_settings(Some(t));
            let _ = s.update_provider(t, &AiProviderPatch::default());
            let _ = s.remove_provider(t);
        }
    }
}

#[test]
fn debug_shows_no_text_names_or_values() {
    let c = "CANARY-51c0";
    let out = format!(
        "{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
        j::<DashboardDraft>(
            json!({"projectId": "p", "name": c, "description": c, "widgets": [c], "viewport": {"c": c}})
        ),
        j::<DashboardPatch>(json!({"name": c, "widgets": [c], "dateFilter": {"c": c}})),
        j::<WorkflowDraft>(json!({"projectId": "p", "workflow": {"name": c}})),
        j::<ChatDraft>(json!({"connectionId": "c", "title": c})),
        j::<ChatPatch>(json!({"title": c})),
        j::<ChatMessageDraft>(
            json!({"id": "m", "role": "user", "content": c, "timestamp": c, "query": c})
        ),
        j::<AiProviderDraft>(json!({"name": c, "type": "anthropic", "baseUrl": c})),
        j::<AiProviderPatch>(json!({"name": c, "baseUrl": c})),
        read_ai_settings(Some(&format!(r#"{{"x":"{c}"}}"#))),
        WindowStateLoaded {
            state: Some(raw_str(&format!("\"{c}\""))),
            rev: 1,
            copied_from: None
        },
        Themes {
            preferences: seaquel_types::storage::ThemePreferences {
                light_theme_id: "l".into(),
                dark_theme_id: "d".into()
            },
            user_themes: vec![raw_str(&format!("\"{c}\""))],
        },
    );
    assert!(!out.contains("CANARY"), "{out}");
}

#[test]
fn params_refuse_unknown_fields() {
    for (what, v) in [
        (
            "DashboardDraft",
            json!({"projectId": "p", "name": "n", "widgets": [], "viewport": {}, "x": 1}),
        ),
        ("DashboardPatch", json!({"x": 1})),
        (
            "WorkflowDraft",
            json!({"projectId": "p", "workflow": {}, "x": 1}),
        ),
        (
            "ChatDraft",
            json!({"connectionId": "c", "title": "t", "x": 1}),
        ),
        ("ChatPatch", json!({"x": 1})),
        (
            "ChatMessageDraft",
            json!({"id": "m", "role": "user", "content": "", "timestamp": "", "x": 1}),
        ),
        (
            "AiProviderDraft",
            json!({"name": "n", "type": "anthropic", "apiKey": "k"}),
        ),
        ("AiProviderPatch", json!({"id": "x"})),
        ("AiSettingsPatch", json!({"providers": []})),
    ] {
        let refused = match what {
            "DashboardDraft" => serde_json::from_value::<DashboardDraft>(v).is_err(),
            "DashboardPatch" => serde_json::from_value::<DashboardPatch>(v).is_err(),
            "WorkflowDraft" => serde_json::from_value::<WorkflowDraft>(v).is_err(),
            "ChatDraft" => serde_json::from_value::<ChatDraft>(v).is_err(),
            "ChatPatch" => serde_json::from_value::<ChatPatch>(v).is_err(),
            "ChatMessageDraft" => serde_json::from_value::<ChatMessageDraft>(v).is_err(),
            "AiProviderDraft" => serde_json::from_value::<AiProviderDraft>(v).is_err(),
            "AiProviderPatch" => serde_json::from_value::<AiProviderPatch>(v).is_err(),
            _ => serde_json::from_value::<AiSettingsPatch>(v).is_err(),
        };
        assert!(refused, "{what}");
    }
    // `dateFilter: null` clears; absent keeps.
    let p: DashboardPatch = j(json!({"dateFilter": null}));
    assert!(matches!(p.date_filter, Some(None)));
    let p: DashboardPatch = j(json!({}));
    assert!(p.date_filter.is_none());
}

/// Phase 5d-2 review: a size past a web limit is accepted when it's no
/// larger than what's stored, item by item; only growth is refused.
#[test]
fn over_limit_sizes_can_stay_or_shrink_but_not_grow() {
    let limits = StateLimits {
        max_view_state_bytes: Some(100),
        max_tab_text_bytes: Some(10),
        max_tabs: Some(1),
        ..StateLimits::default()
    };
    let size = |bytes: usize, tabs: usize, texts: &[(&str, usize)]| ViewStateSize {
        bytes,
        tabs,
        texts: texts.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
    };
    let stored = size(200, 2, &[("a", 50)]);
    let ok = |new: ViewStateSize| check_view_state_limits(&new, Some(&stored), &limits).is_ok();
    assert!(ok(size(200, 2, &[("a", 50)])), "unchanged");
    assert!(ok(size(150, 2, &[("a", 40)])), "smaller");
    assert!(!ok(size(201, 2, &[("a", 50)])), "a larger state");
    assert!(!ok(size(150, 3, &[("a", 50)])), "more tabs");
    assert!(!ok(size(150, 2, &[("a", 51)])), "a longer text");
    let e =
        check_view_state_limits(&size(90, 1, &[("b", 11)]), Some(&stored), &limits).unwrap_err();
    assert!(
        e.message.contains("tab b") && e.message.contains("max_tab_text_bytes"),
        "{e}"
    );
    assert!(check_view_state_limits(&size(200, 2, &[("a", 50)]), None, &limits).is_err());
    assert!(
        check_view_state_limits(&size(200, 2, &[("a", 50)]), None, &StateLimits::default()).is_ok()
    );
    // Workflows compare their whole stored JSON.
    let wl = StateLimits {
        max_workflow_bytes: Some(10),
        ..StateLimits::default()
    };
    let json = "x".repeat(20);
    assert!(check_workflow_size(&json, Some(20), &wl).is_ok());
    assert!(check_workflow_size(&json, Some(19), &wl).is_err());
    assert!(check_workflow_size(&json, None, &wl).is_err());
}

/// The mirror writes `canvas` as `workflow`; the tabs and ids are kept.
#[test]
fn the_mirror_writes_canvas_as_workflow() {
    let state = json!({"projectId": "p1", "tabOrder": [], "activeView": "canvas",
        "queryTabs": [], "schemaTabs": [], "explainTabs": [], "erdTabs": []});
    assert_eq!(
        parse_view_state(&raw(&state), "p1").unwrap().active_view,
        "workflow"
    );
}

/// Reading a record with many fields is linear: `{...defaults, ...parsed}`
/// replaces only the few default keys in place.
#[test]
// A native test timing itself.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
fn reads_of_large_records_are_linear() {
    let fields: Vec<String> = (0..50_000).map(|i| format!("\"k{i}\":{i}")).collect();
    let big = format!("{{{},\"enabled\":false}}", fields.join(","));
    let started = std::time::Instant::now();
    let v: Value = serde_json::from_str(&read_ai_settings(Some(&big)).to_json()).unwrap();
    let _ = read_onboarding(Some(&raw_str(&big)));
    assert_eq!(v["enabled"], false);
    assert_eq!(v["k49999"], 49999);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}
