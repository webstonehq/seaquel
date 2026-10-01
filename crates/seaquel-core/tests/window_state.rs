//! The `ui` group through Core (phase 5d-2, Decision 22): each window's
//! view state per project, the fallback a new window starts from, `rev`
//! ordering, the legacy mirror, the bounded prunes and `windowGet`.
#![cfg(all(feature = "storage", feature = "secrets"))]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod common;

use common::{dump, fx_with, insert_rows, web_state_limits, Fx};
use futures::StreamExt;
use seaquel_core::domain::state::{CopiedFrom, WindowFrom};
use seaquel_core::{StateLimits, StoredKind, WorkspaceEvent, WriteOrigin};
use serde_json::value::RawValue;
use serde_json::{json, Value};

async fn fx() -> Fx {
    fx_with(StateLimits::default(), true).await
}

fn o(w: &str) -> WriteOrigin {
    WriteOrigin::new(Some(w))
}

fn state(project: &str, tab: &str, query: &str) -> Value {
    json!({
        "projectId": project, "queryTabs": [{"id": tab, "name": "Q", "query": query}],
        "schemaTabs": [], "explainTabs": [], "erdTabs": [], "tabOrder": [tab],
        "activeQueryTabId": tab, "activeSchemaTabId": null, "activeExplainTabId": null,
        "activeErdTabId": null, "activeView": "query", "activeConnectionId": "c1",
        "extensionsDuckdbTabs": [{"id": "x1", "name": "Ext", "connectionId": "c1"}]
    })
}

fn rv(v: &Value) -> Box<RawValue> {
    RawValue::from_string(v.to_string()).unwrap()
}

async fn save(
    f: &Fx,
    w: &str,
    p: &str,
    rev: u64,
    s: &Value,
) -> seaquel_core::domain::state::WindowStateSaved {
    f.ws.save_window_state(&f.core, &o(w), w, p, rev, rv(s))
        .await
        .unwrap()
        .value
}

async fn load(f: &Fx, w: &str, p: &str) -> seaquel_core::domain::state::WindowStateLoaded {
    f.ws.load_window_state(&f.core, &o(w), w, p)
        .await
        .unwrap()
        .value
}

fn legacy_rows() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        (
            "project_state",
            vec![
                json!({"project_id": "p1", "active_view": "query", "active_connection_id": "c1",
                        "active_query_tab_id": "tab-q1", "tab_order": "[\"tab-q1\"]",
                        "connection_order": "[\"c1\"]", "starred_shared_query_ids": "[]",
                        "starred_shared_dashboard_ids": "[]"}),
            ],
        ),
        (
            "tabs",
            vec![
                json!({"id": "tab-q1", "project_id": "p1", "tab_type": "query", "name": "Legacy",
                        "query": "SELECT 'legacy'"}),
            ],
        ),
    ]
}

#[tokio::test]
async fn a_new_window_copies_the_most_recent_then_legacy_then_empty() {
    let f = fx().await;
    // Nothing: empty, and nothing written.
    let l = load(&f, "main", "p1").await;
    assert_eq!(
        (l.state.is_none(), l.rev, l.copied_from),
        (true, 0, Some(CopiedFrom::Empty))
    );
    assert!(dump(f.ws.storage(), "windows", "window_id")
        .await
        .is_empty());
    // Today's rows: the first window after the upgrade.
    for (t, rows) in legacy_rows() {
        insert_rows(f.ws.storage(), t, &rows).await;
    }
    let l = load(&f, "main", "p1").await;
    assert_eq!(l.copied_from, Some(CopiedFrom::Legacy));
    let s: Value = serde_json::from_str(l.state.unwrap().get()).unwrap();
    assert_eq!(s["queryTabs"][0]["query"], "SELECT 'legacy'");
    assert!(s.get("connectionOrder").is_none() && s.get("savedWorkflows").is_none());
    // Two windows save; a third copies the most recently saved one.
    save(&f, "main", "p1", 1, &state("p1", "a", "main's")).await;
    save(&f, "win-2", "p1", 1, &state("p1", "b", "win-2's")).await;
    let l = load(&f, "win-3", "p1").await;
    assert_eq!((l.rev, l.copied_from), (0, Some(CopiedFrom::Window)));
    assert!(l.state.unwrap().get().contains("win-2's"));
    save(&f, "main", "p1", 2, &state("p1", "a", "main again")).await;
    let l = load(&f, "win-4", "p1").await;
    assert!(l.state.unwrap().get().contains("main again"));
}

#[tokio::test]
async fn the_copy_is_written_as_the_windows_row() {
    let f = fx().await;
    save(&f, "main", "p1", 1, &state("p1", "a", "first")).await;
    let copy = load(&f, "win-2", "p1").await;
    assert_eq!(copy.copied_from, Some(CopiedFrom::Window));
    // Main changes afterwards; the new window keeps what it copied, byte
    // for byte, and reads it as its own row (no copy, nothing written).
    save(&f, "main", "p1", 2, &state("p1", "a", "changed")).await;
    let own = load(&f, "win-2", "p1").await;
    assert_eq!((own.copied_from, own.rev), (None, 0));
    assert_eq!(own.state.unwrap().get(), copy.state.unwrap().get());
    // The window's first save (rev 1) lands over the copy.
    assert!(
        !save(&f, "win-2", "p1", 1, &state("p1", "b", "mine"))
            .await
            .stale
    );
    assert!(load(&f, "win-2", "p1")
        .await
        .state
        .unwrap()
        .get()
        .contains("mine"));
    // The state is stored as sent, extensions tabs included.
    let rows = dump(f.ws.storage(), "window_state", "window_id").await;
    let row = rows.iter().find(|r| r["window_id"] == "win-2").unwrap();
    assert!(row["state"]
        .as_str()
        .unwrap()
        .contains("extensionsDuckdbTabs"));
}

#[tokio::test]
async fn a_window_save_writes_the_legacy_mirror_and_keeps_the_sidebar() {
    let f = fx().await;
    f.ws.set_project_sidebar(&f.core, &o("x"), "p1", vec!["c1".into(), "c9".into()])
        .await
        .unwrap();
    let mut s = state("p1", "a", "SELECT 2");
    s["connectionOrder"] = json!(["ignored"]);
    save(&f, "main", "p1", 1, &s).await;
    let ps = dump(f.ws.storage(), "project_state", "project_id").await;
    assert_eq!(ps[0]["connection_order"], "[\"c1\",\"c9\"]");
    assert_eq!(ps[0]["active_query_tab_id"], "a");
    assert_eq!(ps[0]["active_connection_id"], "c1");
    let tabs = dump(f.ws.storage(), "tabs", "id").await;
    assert_eq!(tabs.len(), 1, "no row for the extensions tab: {tabs:?}");
    assert_eq!(
        (tabs[0]["id"].as_str(), tabs[0]["query"].as_str()),
        (Some("a"), Some("SELECT 2"))
    );
    assert_eq!(
        f.ws.project_sidebar(&f.core, "p1").await.unwrap().value,
        vec!["c1".to_string(), "c9".to_string()]
    );
    // A repeated tab id is skipped by the mirror; the window's row keeps
    // the state as sent.
    let mut rep = state("p1", "t", "one");
    rep["queryTabs"] =
        json!([{"id": "t", "name": "A", "query": "1"}, {"id": "t", "name": "B", "query": "2"}]);
    assert!(!save(&f, "main", "p1", 2, &rep).await.stale);
    assert_eq!(dump(f.ws.storage(), "tabs", "id").await.len(), 1);
    assert!(load(&f, "main", "p1")
        .await
        .state
        .unwrap()
        .get()
        .contains("\"B\""));
}

#[tokio::test]
async fn an_older_rev_is_stale() {
    let f = fx().await;
    assert_eq!(
        save(&f, "main", "p1", 3, &state("p1", "a", "three"))
            .await
            .rev,
        3
    );
    let before = (
        dump(f.ws.storage(), "project_state", "project_id").await,
        dump(f.ws.storage(), "tabs", "id").await,
        dump(f.ws.storage(), "windows", "window_id").await,
    );
    for rev in [3, 2, 0] {
        let s = save(&f, "main", "p1", rev, &state("p1", "b", "older")).await;
        assert!(s.stale, "{rev}");
        assert_eq!(s.rev, 3, "the stored rev, so the page can move past it");
    }
    // A stale save writes nothing else: no mirror, no touch.
    let after = (
        dump(f.ws.storage(), "project_state", "project_id").await,
        dump(f.ws.storage(), "tabs", "id").await,
        dump(f.ws.storage(), "windows", "window_id").await,
    );
    assert_eq!(before, after);
    assert!(load(&f, "main", "p1")
        .await
        .state
        .unwrap()
        .get()
        .contains("three"));
    let s = save(&f, "main", "p1", 4, &state("p1", "b", "four")).await;
    assert!(!s.stale && s.rev == 4);
}

#[tokio::test]
async fn pruning_is_bounded_and_spares_the_saving_window_and_main() {
    let limits = StateLimits {
        max_windows: 3,
        max_window_states_per_project: 2,
        ..StateLimits::default()
    };
    let f = fx_with(limits, true).await;
    save(&f, "main", "p1", 1, &state("p1", "a", "main")).await;
    for w in ["w1", "w2", "w3", "w4"] {
        save(&f, w, "p1", 1, &state("p1", "a", w)).await;
    }
    let windows: Vec<String> = dump(f.ws.storage(), "windows", "window_id")
        .await
        .iter()
        .map(|r| r["window_id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        windows.contains(&"main".to_string()),
        "main is spared on desktop: {windows:?}"
    );
    assert!(
        windows.contains(&"w4".to_string()),
        "the saving window: {windows:?}"
    );
    assert!(windows.len() <= 4, "{windows:?}");
    let states = dump(f.ws.storage(), "window_state", "window_id").await;
    assert!(
        states.len() <= 3,
        "at most 2 per project, plus main: {states:?}"
    );
    assert!(states.iter().any(|r| r["window_id"] == "main"));

    // Windows unused for 30 days go on the next save; the web spares no
    // `main`.
    let w = fx_with(
        StateLimits {
            max_windows: 50,
            ..web_state_limits()
        },
        false,
    )
    .await;
    save(&w, "main", "p1", 1, &state("p1", "a", "main")).await;
    save(&w, "old", "p1", 1, &state("p1", "a", "old")).await;
    w.clock.advance(31 * 24 * 60 * 60 * 1000);
    save(&w, "new", "p1", 1, &state("p1", "a", "new")).await;
    let left: Vec<String> = dump(w.ws.storage(), "windows", "window_id")
        .await
        .iter()
        .map(|r| r["window_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(left, vec!["new".to_string()]);
    // The legacy rows are never pruned.
    assert_eq!(
        dump(w.ws.storage(), "project_state", "project_id")
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn a_window_id_other_than_the_origin_is_refused() {
    let f = fx().await;
    save(&f, "main", "p1", 1, &state("p1", "a", "main's")).await;
    let s = rv(&state("p1", "b", "overwrite"));
    for origin in [o("win-2"), WriteOrigin::none()] {
        let e =
            f.ws.save_window_state(&f.core, &origin, "main", "p1", 9, s.clone())
                .await
                .unwrap_err();
        assert_eq!(e.code, "INVALID_ARGUMENT");
        assert_eq!(
            f.ws.load_window_state(&f.core, &origin, "main", "p1")
                .await
                .unwrap_err()
                .code,
            "INVALID_ARGUMENT"
        );
        assert_eq!(
            f.ws.get_window(&origin, "main").await.unwrap_err().code,
            "INVALID_ARGUMENT"
        );
        assert_eq!(
            f.ws.activate_window(&f.core, &origin, "main", "p1")
                .await
                .unwrap_err()
                .code,
            "INVALID_ARGUMENT"
        );
    }
    assert!(load(&f, "main", "p1")
        .await
        .state
        .unwrap()
        .get()
        .contains("main's"));
    // A window id outside the origin's form is refused too.
    let bad = "a b";
    assert_eq!(
        f.ws.get_window(&WriteOrigin::new(Some(bad)), bad)
            .await
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );
    // A state for another project than the call's is refused.
    let e =
        f.ws.save_window_state(
            &f.core,
            &o("main"),
            "main",
            "p2",
            2,
            rv(&state("p1", "a", "x")),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, "INVALID_ARGUMENT");
    let e =
        f.ws.save_window_state(
            &f.core,
            &o("main"),
            "main",
            "gone",
            2,
            rv(&state("gone", "a", "x")),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, "PROJECT_NOT_FOUND");
}

#[tokio::test]
async fn window_activate_writes_last_active_project_id() {
    let f = fx().await;
    f.ws.activate_window(&f.core, &o("main"), "main", "p2")
        .await
        .unwrap();
    let rows = dump(f.ws.storage(), "app_state", "key").await;
    assert!(
        rows.iter()
            .any(|r| r["key"] == "lastActiveProjectId" && r["value"] == "p2"),
        "{rows:?}"
    );
    let w = dump(f.ws.storage(), "windows", "window_id").await;
    assert_eq!(w[0]["active_project_id"], "p2");
    assert_eq!(
        f.ws.activate_window(&f.core, &o("main"), "main", "nope")
            .await
            .unwrap_err()
            .code,
        "PROJECT_NOT_FOUND"
    );
}

#[tokio::test]
async fn window_get_falls_back_from_its_own_to_the_recent_to_last_active() {
    let f = fx().await;
    let get = |w: &'static str| {
        let f = &f;
        async move { f.ws.get_window(&o(w), w).await.unwrap().value }
    };
    let v = get("main").await;
    assert_eq!((v.active_project_id, v.from), (None, None));
    insert_rows(
        f.ws.storage(),
        "app_state",
        &[json!({"key": "lastActiveProjectId", "value": "p1"})],
    )
    .await;
    let v = get("main").await;
    assert_eq!(
        (v.active_project_id.as_deref(), v.from),
        (Some("p1"), Some(WindowFrom::LastActive))
    );
    f.ws.activate_window(&f.core, &o("win-2"), "win-2", "p2")
        .await
        .unwrap();
    // A window that saved but never activated has no project of its own.
    save(&f, "win-3", "p1", 1, &state("p1", "a", "x")).await;
    let v = get("main").await;
    assert_eq!(
        (v.active_project_id.as_deref(), v.from),
        (Some("p2"), Some(WindowFrom::Recent))
    );
    f.ws.activate_window(&f.core, &o("main"), "main", "p1")
        .await
        .unwrap();
    let v = get("main").await;
    assert_eq!(
        (v.active_project_id.as_deref(), v.from),
        (Some("p1"), Some(WindowFrom::Window))
    );
    // windowGet writes nothing.
    let before = dump(f.ws.storage(), "windows", "window_id").await;
    get("win-9").await;
    assert_eq!(dump(f.ws.storage(), "windows", "window_id").await, before);
}

#[tokio::test]
async fn a_view_state_event_names_only_its_window() {
    let f = fx().await;
    let mut events = f.ws.events();
    save(&f, "main", "p1", 1, &state("p1", "a", "x")).await;
    load(&f, "win-2", "p1").await; // a copy: a write
    load(&f, "win-2", "p1").await; // its own row: no write, no event
    let mut got = Vec::new();
    while let Ok(Some(e)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), events.next()).await
    {
        if let WorkspaceEvent::StorageChanged(c) = e {
            got.push(c);
        }
    }
    let seen: Vec<_> = got
        .iter()
        .map(|c| (c.kind, c.scope.clone(), c.ids.clone(), c.origin.clone()))
        .collect();
    assert_eq!(
        seen,
        vec![
            (
                StoredKind::ProjectState,
                Some("p1".into()),
                Some(vec!["main".into()]),
                Some("main".into())
            ),
            (
                StoredKind::ProjectState,
                Some("p1".into()),
                Some(vec!["win-2".into()]),
                Some("win-2".into())
            ),
        ]
    );
}

// ── Phase 5d-2 review ──

/// A web limit that holds only the view state's text sizes.
fn only(limits: StateLimits) -> StateLimits {
    StateLimits {
        max_view_state_bytes: None,
        max_tab_text_bytes: None,
        max_tabs: None,
        ..limits
    }
}

async fn seed_legacy(f: &Fx, text: &str) {
    insert_rows(
        f.ws.storage(),
        "project_state",
        &[
            json!({"project_id": "p1", "active_view": "query", "active_query_tab_id": "t1",
                 "tab_order": "[\"t1\"]", "connection_order": "[]",
                 "starred_shared_query_ids": "[]", "starred_shared_dashboard_ids": "[]"}),
        ],
    )
    .await;
    insert_rows(
        f.ws.storage(),
        "tabs",
        &[json!({"id": "t1", "project_id": "p1", "tab_type": "query", "name": "Big", "query": text})],
    )
    .await;
}

fn with_text(mut s: Value, tab: usize, text: &str) -> Value {
    s["queryTabs"][tab]["query"] = json!(text);
    s
}

/// A state stored before 5d-2's web limits (or copied from one) stays
/// saveable: unchanged and smaller saves land, and only growth past a
/// limit is refused, per tab for a tab's text.
#[tokio::test]
async fn an_over_limit_tab_text_stays_saveable_and_can_only_shrink() {
    let limits = StateLimits {
        max_tab_text_bytes: Some(1000),
        ..only(web_state_limits())
    };
    let f = fx_with(limits, false).await;
    seed_legacy(&f, &"a".repeat(2000)).await;
    let l = load(&f, "w", "p1").await;
    assert_eq!(l.copied_from, Some(CopiedFrom::Legacy));
    let loaded: Value = serde_json::from_str(l.state.unwrap().get()).unwrap();
    // Unchanged.
    assert!(!save(&f, "w", "p1", 1, &loaded).await.stale);
    // Smaller, still past the limit.
    let smaller = with_text(loaded.clone(), 0, &"a".repeat(1500));
    assert!(!save(&f, "w", "p1", 2, &smaller).await.stale);
    // Larger than what's stored now: refused, naming the limit and tab.
    let e =
        f.ws.save_window_state(
            &f.core,
            &o("w"),
            "w",
            "p1",
            3,
            rv(&with_text(loaded.clone(), 0, &"a".repeat(1600))),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, "INVALID_ARGUMENT");
    assert!(
        e.message.contains("max_tab_text_bytes") && e.message.contains("t1"),
        "{e:?}"
    );
    // Another tab gets no allowance from t1's.
    let mut two = smaller.clone();
    two["queryTabs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "t2", "name": "New", "query": "b".repeat(1100)}));
    let e =
        f.ws.save_window_state(&f.core, &o("w"), "w", "p1", 3, rv(&two))
            .await
            .unwrap_err();
    assert!(e.message.contains("t2"), "{e:?}");
    // The stored row is the smaller one.
    assert!(load(&f, "w", "p1")
        .await
        .state
        .unwrap()
        .get()
        .contains(&"a".repeat(1500)));
    // A window with no stored row gets no allowance.
    let e =
        f.ws.save_window_state(
            &f.core,
            &o("fresh"),
            "fresh",
            "p2",
            1,
            rv(&with_text(state("p2", "t1", ""), 0, &"a".repeat(1200))),
        )
        .await
        .unwrap_err();
    assert!(e.message.contains("max_tab_text_bytes"), "{e:?}");
}

#[tokio::test]
async fn an_over_limit_view_state_stays_saveable_and_can_only_shrink() {
    let f0 = fx_with(only(web_state_limits()), false).await;
    seed_legacy(&f0, &"a".repeat(3000)).await;
    let copied = load(&f0, "w", "p1").await.state.unwrap().get().len();
    // The same seed on a Core whose total limit the copy is past.
    let limits = StateLimits {
        max_view_state_bytes: Some(copied - 500),
        ..only(web_state_limits())
    };
    let f = fx_with(limits, false).await;
    seed_legacy(&f, &"a".repeat(3000)).await;
    let loaded: Value =
        serde_json::from_str(load(&f, "w", "p1").await.state.unwrap().get()).unwrap();
    assert!(!save(&f, "w", "p1", 1, &loaded).await.stale, "unchanged");
    assert!(
        !save(
            &f,
            "w",
            "p1",
            2,
            &with_text(loaded.clone(), 0, &"a".repeat(2900))
        )
        .await
        .stale,
        "smaller"
    );
    let e =
        f.ws.save_window_state(
            &f.core,
            &o("w"),
            "w",
            "p1",
            3,
            rv(&with_text(loaded.clone(), 0, &"a".repeat(2950))),
        )
        .await
        .unwrap_err();
    assert!(e.message.contains("max_view_state_bytes"), "{e:?}");
}

/// A stored row that doesn't read, with nothing to fall back to, answers
/// its own rev, so the page's first save (rev + 1) lands.
#[tokio::test]
async fn an_empty_load_over_an_unreadable_row_answers_its_rev() {
    let f = fx().await;
    save(&f, "main", "p1", 7, &state("p1", "a", "x")).await;
    sqlx::query("UPDATE window_state SET state = 'nope'")
        .execute(f.ws.storage().pool())
        .await
        .unwrap();
    // The legacy mirror of that save exists; take it away too.
    sqlx::query("DELETE FROM project_state")
        .execute(f.ws.storage().pool())
        .await
        .unwrap();
    let l = load(&f, "main", "p1").await;
    assert_eq!(
        (l.copied_from, l.rev, l.state.is_none()),
        (Some(CopiedFrom::Empty), 7, true)
    );
    assert!(
        !save(&f, "main", "p1", 8, &state("p1", "a", "y"))
            .await
            .stale
    );
}

/// A window's view state comes back byte for byte: spacing, key order,
/// number spellings and fields Core doesn't know included.
#[tokio::test]
async fn a_view_state_round_trips_byte_for_byte() {
    let f = fx().await;
    let text = r#"{ "projectId":"p1","queryTabs" : [{"id":"t","name":"Q","query":"SELECT 'é'","extra":1.50}],
 "schemaTabs":[],"explainTabs":[],"erdTabs":[],"tabOrder":["t"],"activeView":"canvas","future":{"z":1e2,"a":null}}"#;
    let raw = RawValue::from_string(text.to_string()).unwrap();
    f.ws.save_window_state(&f.core, &o("main"), "main", "p1", 1, raw)
        .await
        .unwrap();
    assert_eq!(load(&f, "main", "p1").await.state.unwrap().get(), text);
    // The legacy mirror writes `canvas` as `workflow`, as every open's
    // baseline would.
    let ps = dump(f.ws.storage(), "project_state", "project_id").await;
    assert_eq!(ps[0]["active_view"], "workflow");
}

/// Phase 5d-2 re-review: a body past `max_view_state_bytes` and larger
/// than the stored state is refused before it's parsed (here it isn't even
/// a view state), naming the limit.
#[tokio::test]
async fn an_oversized_body_is_refused_before_it_is_parsed() {
    let limits = StateLimits {
        max_view_state_bytes: Some(1000),
        ..only(web_state_limits())
    };
    let f = fx_with(limits, false).await;
    let body = json!({"pad": "x".repeat(2000)});
    let e =
        f.ws.save_window_state(&f.core, &o("w"), "w", "p1", 1, rv(&body))
            .await
            .unwrap_err();
    assert!(e.message.contains("max_view_state_bytes"), "{e:?}");
}

// ── Phase 5d-2 Task 7 probe fixes ──

/// Writes in one millisecond: `windowGet`, a new window's copy and the
/// legacy mirror all name the last write committed. Before, `windows`
/// broke the tie by the later row and `window_state` by the earlier one,
/// and an update kept its row, so they could disagree.
#[tokio::test]
async fn on_equal_times_the_copy_window_get_and_the_mirror_agree() {
    let f = fx().await;
    f.clock.freeze();
    for w in ["win-a", "win-b", "win-a"] {
        f.ws.activate_window(&f.core, &o(w), w, "p1").await.unwrap();
    }
    // `win-a` activated last: a new window opens its project.
    let got =
        f.ws.get_window(&o("win-new"), "win-new")
            .await
            .unwrap()
            .value;
    assert_eq!(
        (got.active_project_id.as_deref(), got.from),
        (Some("p1"), Some(WindowFrom::Recent))
    );
    // Its row is the older one; `win-b` saving after it wins the copy.
    f.ws.activate_window(&f.core, &o("win-b"), "win-b", "p2")
        .await
        .unwrap();
    let got =
        f.ws.get_window(&o("win-new"), "win-new")
            .await
            .unwrap()
            .value;
    assert_eq!(got.active_project_id.as_deref(), Some("p2"), "win-b, last");

    save(&f, "win-a", "p1", 1, &state("p1", "a", "a first")).await;
    save(&f, "win-b", "p1", 1, &state("p1", "b", "b last")).await;
    let mirror = |f: &Fx| {
        let st = f.ws.storage().clone();
        async move {
            dump(&st, "tabs", "id")
                .await
                .into_iter()
                .map(|r| r["query"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(mirror(&f).await, ["b last"]);
    let copy = load(&f, "win-c", "p1").await;
    assert_eq!(copy.copied_from, Some(CopiedFrom::Window));
    assert!(copy.state.unwrap().get().contains("b last"));
    // `win-a` saves again, updating its row in place: now it's the last.
    save(&f, "win-a", "p1", 2, &state("p1", "a", "a again")).await;
    assert_eq!(mirror(&f).await, ["a again"]);
    let copy = load(&f, "win-d", "p1").await;
    assert!(copy.state.unwrap().get().contains("a again"));
}

/// The page counts `rev` in JavaScript numbers: a save past 2^53 - 1 is
/// refused, so the page's `rev + 1` is always the next number.
#[tokio::test]
async fn a_rev_past_2_pow_53_is_refused() {
    const MAX: u64 = (1 << 53) - 1;
    let f = fx().await;
    assert!(
        !save(&f, "main", "p1", MAX, &state("p1", "a", "max"))
            .await
            .stale
    );
    for rev in [MAX + 1, i64::MAX as u64, u64::MAX] {
        let e =
            f.ws.save_window_state(
                &f.core,
                &o("main"),
                "main",
                "p1",
                rev,
                rv(&state("p1", "a", "x")),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, "INVALID_ARGUMENT", "{rev}");
    }
    assert_eq!(load(&f, "main", "p1").await.rev, MAX);
}
