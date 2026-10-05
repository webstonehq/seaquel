//! `StorageChanged` and the change sequence (phase 5d):
//! one event per stored write, after its commit, none for a refusal; `seq`
//! in commit order; a read's `seq` never newer than its data; the origin
//! carried; no values in an event.
#![cfg(all(feature = "storage", feature = "secrets", feature = "workspace"))]
// Native-only tests: tasks on the test runtime and the wall clock.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod common;

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use common::{core, core_with, TestStore};
use futures::StreamExt;
use seaquel_core::domain::library::{
    ConnectionDraft, ConnectionPatch, LabelDraft, LabelPatch, ProjectDraft, SavedQueryDraft,
    SavedQueryPatch, SecretChanges,
};
use seaquel_core::storage::{connections, saved_queries};
use seaquel_core::{
    ChangeSeq, ConnectRequest, Core, LibraryLimits, StorageChange, StoredKind, Workspace,
    WorkspaceEvent, WorkspaceSpec, WriteOrigin,
};
use seaquel_runtime::BoxStream;
use serde_json::{json, Value};

struct Fx {
    dir: tempfile::TempDir,
    core: Arc<Core>,
    ws: Arc<Workspace>,
    store: Arc<TestStore>,
    project: String,
}

async fn fx() -> Fx {
    fx_with(core()).await
}

async fn fx_with(core: Core) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let store = TestStore::new();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()).with_secrets(store.clone()))
        .await
        .unwrap();
    let project = ws
        .create_project(&core, &WriteOrigin::none(), draft_project("Main"))
        .await
        .unwrap()
        .value
        .id;
    Fx {
        dir,
        core: Arc::new(core),
        ws,
        store,
        project,
    }
}

fn draft_project(name: &str) -> ProjectDraft {
    serde_json::from_value(json!({ "name": name })).unwrap()
}

fn draft_connection(project: &str, name: &str, extra: Value) -> ConnectionDraft {
    let mut d = json!({
        "projectId": project, "name": name, "type": "postgres", "host": "db.example.com",
        "port": 5432, "databaseName": "app", "username": "alice",
    });
    for (k, v) in extra.as_object().unwrap() {
        d[k] = v.clone();
    }
    serde_json::from_value(d).unwrap()
}

fn draft_query(project: &str, name: &str, text: &str) -> SavedQueryDraft {
    serde_json::from_value(json!({ "projectId": project, "name": name, "query": text })).unwrap()
}

fn patch<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}

/// The storage events waiting on `events` now.
async fn drain(events: &mut BoxStream<'static, WorkspaceEvent>) -> Vec<StorageChange> {
    let mut out = Vec::new();
    while let Ok(Some(e)) = tokio::time::timeout(Duration::from_millis(30), events.next()).await {
        if let WorkspaceEvent::StorageChanged(c) = e {
            out.push(c);
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn every_write_emits_one_event_after_commit() {
    let f = fx().await;
    let (core, ws, o) = (&*f.core, &f.ws, &WriteOrigin::new(Some("win-1")));
    // A subscriber that reads the saved connections as each event arrives:
    // the write it announces is already committed.
    let mut watcher = ws.events();
    let reader = {
        let ws = ws.clone();
        tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Some(e) = watcher.next().await {
                if let WorkspaceEvent::StorageChanged(c) = e {
                    let ids: HashSet<String> = connections::load_all(ws.storage())
                        .await
                        .unwrap()
                        .into_iter()
                        .map(|c| c.id)
                        .collect();
                    let done = c.kind == StoredKind::Project && c.ids.is_none();
                    seen.push((c, ids));
                    if done {
                        break;
                    }
                }
            }
            seen
        })
    };
    let mut events = ws.events();

    let expect = |kind: StoredKind, ids: Vec<String>| (kind, ids);
    let mut writes = Vec::new();
    let c = ws
        .create_connection(
            core,
            o,
            draft_connection(&f.project, "A", json!({})),
            SecretChanges::default(),
        )
        .await
        .unwrap()
        .value
        .id;
    writes.push(expect(StoredKind::Connection, vec![c.clone()]));
    ws.update_connection(
        core,
        o,
        &c,
        patch(json!({"host": "h2"})),
        SecretChanges::default(),
    )
    .await
    .unwrap();
    writes.push(expect(StoredKind::Connection, vec![c.clone()]));
    let l = ws
        .create_label(
            core,
            o,
            &f.project,
            patch::<LabelDraft>(json!({"name": "L", "color": "#112233"})),
        )
        .await
        .unwrap()
        .value
        .id;
    writes.push(expect(StoredKind::Label, vec![l.clone()]));
    ws.update_label(
        core,
        o,
        &f.project,
        &l,
        patch::<LabelPatch>(json!({"color": "#445566"})),
    )
    .await
    .unwrap();
    writes.push(expect(StoredKind::Label, vec![l.clone()]));
    ws.remove_label(core, o, &f.project, &l).await.unwrap();
    writes.push(expect(StoredKind::Label, vec![l.clone()]));
    let q = ws
        .create_saved_query(core, o, draft_query(&f.project, "Q", "SELECT 1"))
        .await
        .unwrap()
        .value
        .id;
    writes.push(expect(StoredKind::SavedQuery, vec![q.clone()]));
    ws.update_saved_query(
        core,
        o,
        &q,
        patch::<SavedQueryPatch>(json!({"query": "SELECT 2"})),
    )
    .await
    .unwrap();
    writes.push(expect(StoredKind::SavedQuery, vec![q.clone()]));
    ws.remove_saved_query(core, o, &q).await.unwrap();
    writes.push(expect(StoredKind::SavedQuery, vec![q.clone()]));
    let p2 = ws
        .create_project(core, o, draft_project("Second"))
        .await
        .unwrap()
        .value
        .id;
    writes.push(expect(StoredKind::Project, vec![p2.clone()]));
    ws.update_project(core, o, &p2, patch(json!({"description": "d"})))
        .await
        .unwrap();
    writes.push(expect(StoredKind::Project, vec![p2.clone()]));
    ws.remove_connection(core, o, &c).await.unwrap();
    writes.push(expect(StoredKind::Connection, vec![c.clone()]));
    ws.remove_project(core, o, &p2).await.unwrap();
    writes.push(expect(StoredKind::Project, vec![p2.clone()]));

    let got = drain(&mut events).await;
    let got: Vec<(StoredKind, Vec<String>)> =
        got.into_iter().map(|c| (c.kind, c.ids.unwrap())).collect();
    assert_eq!(got, writes, "exactly one event per write, in order");

    // The watcher saw the connection on its create's event and not on its
    // removal's.
    ws.record_storage_write(&WriteOrigin::none(), StoredKind::Project, None, None);
    let seen = reader.await.unwrap();
    let on = |kind: StoredKind, n: usize| {
        &seen
            .iter()
            .filter(|(c, _)| c.kind == kind)
            .nth(n)
            .unwrap()
            .1
    };
    assert!(
        on(StoredKind::Connection, 0).contains(&c),
        "visible on its create's event"
    );
    assert!(
        !on(StoredKind::Connection, 2).contains(&c),
        "gone on its removal's event"
    );
}

#[tokio::test]
async fn a_refused_or_failed_write_emits_nothing() {
    let f = fx_with(core_with(LibraryLimits {
        max_projects: Some(1),
        ..Default::default()
    }))
    .await;
    let (core, ws, o) = (&*f.core, &f.ws, &WriteOrigin::none());
    let c = ws
        .create_connection(
            core,
            o,
            draft_connection(&f.project, "A", json!({})),
            SecretChanges::default(),
        )
        .await
        .unwrap()
        .value
        .id;
    let mut events = ws.events();
    let before = ws.change_seq();

    let refusals = vec![
        ws.create_connection(
            core,
            o,
            draft_connection(&f.project, " a ", json!({})),
            SecretChanges::default(),
        )
        .await
        .map(|_| ()),
        ws.create_connection(
            core,
            o,
            draft_connection("nope", "B", json!({})),
            SecretChanges::default(),
        )
        .await
        .map(|_| ()),
        ws.create_connection(
            core,
            o,
            draft_connection(&f.project, "B", json!({"port": 70000})),
            SecretChanges::default(),
        )
        .await
        .map(|_| ()),
        ws.update_connection(
            core,
            o,
            "missing",
            ConnectionPatch::default(),
            SecretChanges::default(),
        )
        .await
        .map(|_| ()),
        ws.remove_project(core, o, &f.project).await.map(|_| ()),
        ws.create_project(core, o, draft_project("Past the limit"))
            .await
            .map(|_| ()),
        ws.remove_label(core, o, &f.project, "local")
            .await
            .map(|_| ()),
        ws.update_saved_query(core, o, "missing", SavedQueryPatch::default())
            .await
            .map(|_| ()),
    ];
    // A keychain failure: nothing stored.
    f.store.fail_set.store(true, Ordering::SeqCst);
    let keychain = ws
        .update_connection(
            core,
            o,
            &c,
            patch(json!({"savePassword": true})),
            patch(json!({"db": "pw"})),
        )
        .await;
    f.store.fail_set.store(false, Ordering::SeqCst);
    assert!(refusals.iter().all(Result::is_err), "{refusals:?}");
    assert_eq!(keychain.unwrap_err().code, "SECRET_STORE_ERROR");
    assert!(drain(&mut events).await.is_empty());
    assert_eq!(ws.change_seq().epoch, before.epoch);
}

#[tokio::test(flavor = "multi_thread")]
async fn seq_follows_commit_order_under_concurrent_writers() {
    let f = fx().await;
    let q =
        f.ws.create_saved_query(
            &f.core,
            &WriteOrigin::none(),
            draft_query(&f.project, "Q", "SELECT 0"),
        )
        .await
        .unwrap()
        .value
        .id;
    let tasks: Vec<_> = (1..=24)
        .map(|i| {
            let (core, ws, q) = (f.core.clone(), f.ws.clone(), q.clone());
            tokio::spawn(async move {
                let done = ws
                    .update_saved_query(
                        &core,
                        &WriteOrigin::none(),
                        &q,
                        patch(json!({ "query": format!("SELECT {i}") })),
                    )
                    .await
                    .unwrap();
                (done.seq.n, done.value.version.unwrap().version, i)
            })
        })
        .collect();
    let mut results = Vec::new();
    for t in tasks {
        results.push(t.await.unwrap());
    }
    results.sort_by_key(|(n, _, _)| *n);
    let versions: Vec<f64> = results.iter().map(|(_, v, _)| *v).collect();
    assert!(
        versions.windows(2).all(|w| w[0] < w[1]),
        "versions (numbered inside the transaction) rise with seq: {versions:?}"
    );
    // The last write by seq is the text that's stored.
    let last = results.last().unwrap().2;
    let stored = saved_queries::get(f.ws.storage(), &q)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.query, format!("SELECT {last}"));
    assert_eq!(f.ws.change_seq().n, results.last().unwrap().0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_seq_is_never_newer_than_its_data() {
    let f = fx().await;
    let writers: Vec<_> = (0..40)
        .map(|i| {
            let (core, ws, p) = (f.core.clone(), f.ws.clone(), f.project.clone());
            tokio::spawn(async move {
                let done = ws
                    .create_saved_query(
                        &core,
                        &WriteOrigin::none(),
                        draft_query(&p, &format!("Q{i}"), "SELECT 1"),
                    )
                    .await
                    .unwrap();
                (done.seq.n, done.value.id)
            })
        })
        .collect();
    let readers: Vec<_> = (0..8)
        .map(|_| {
            let (core, ws, p) = (f.core.clone(), f.ws.clone(), f.project.clone());
            tokio::spawn(async move {
                let mut lists = Vec::new();
                for _ in 0..20 {
                    let l = ws.list_saved_queries(&core, &p).await.unwrap();
                    lists.push((
                        l.seq.n,
                        l.value.into_iter().map(|q| q.id).collect::<HashSet<_>>(),
                    ));
                    tokio::task::yield_now().await;
                }
                lists
            })
        })
        .collect();
    let mut created = Vec::new();
    for w in writers {
        created.push(w.await.unwrap());
    }
    for r in readers {
        for (seq, ids) in r.await.unwrap() {
            for (n, id) in &created {
                if *n <= seq {
                    assert!(
                        ids.contains(id),
                        "a list at seq {seq} misses the write numbered {n}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn the_origin_is_carried() {
    let f = fx().await;
    let mut events = f.ws.events();
    f.ws.create_saved_query(
        &f.core,
        &WriteOrigin::new(Some("tab_7-a")),
        draft_query(&f.project, "A", "x"),
    )
    .await
    .unwrap();
    f.ws.create_saved_query(
        &f.core,
        &WriteOrigin::none(),
        draft_query(&f.project, "B", "x"),
    )
    .await
    .unwrap();
    // A malformed origin is dropped, never carried.
    f.ws.create_saved_query(
        &f.core,
        &WriteOrigin::new(Some("bad\norigin")),
        draft_query(&f.project, "C", "x"),
    )
    .await
    .unwrap();
    let got = drain(&mut events).await;
    let origins: Vec<Option<String>> = got.iter().map(|c| c.origin.clone()).collect();
    assert_eq!(origins, vec![Some("tab_7-a".to_string()), None, None]);
    assert!(
        !format!("{got:?}").contains("tab_7-a"),
        "Debug hides the origin"
    );
}

#[tokio::test]
async fn over_100_ids_is_a_kind_reload() {
    let f = fx().await;
    let mut events = f.ws.events();
    let ids = |n: usize| (0..n).map(|i| format!("id-{i}")).collect::<Vec<_>>();
    f.ws.record_storage_write(
        &WriteOrigin::none(),
        StoredKind::Storage,
        None,
        Some(ids(100)),
    );
    f.ws.record_storage_write(
        &WriteOrigin::none(),
        StoredKind::Storage,
        None,
        Some(ids(101)),
    );
    let got = drain(&mut events).await;
    assert_eq!(got[0].ids.as_ref().map(Vec::len), Some(100));
    assert_eq!(got[1].ids, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn history_appends_and_storage_writes_emit() {
    let f = fx().await;
    let (core, ws) = (&*f.core, &f.ws);
    let file = f.dir.path().join("app.sqlite");
    let saved = ws
        .create_connection(
            core,
            &WriteOrigin::none(),
            serde_json::from_value(json!({
                "projectId": f.project, "name": "Lite", "type": "sqlite", "host": "", "port": 0,
                "databaseName": file.to_str().unwrap(), "username": "",
                "connectionString": format!("sqlite://{}", file.display()),
            }))
            .unwrap(),
            SecretChanges::default(),
        )
        .await
        .unwrap()
        .value
        .id;
    let id = ws
        .connect(
            core,
            ConnectRequest::saved(&saved).with_create_if_missing(true),
        )
        .await
        .unwrap();
    let history = json!({"connectionId": saved, "connectionName": "Lite", "connectionLabels": []});
    let mut events = ws.events();

    let run = serde_json::from_value(json!({
        "connectionId": id, "streamId": "r1", "text": "SELECT 1", "target": {"type": "all"},
        "pageSize": 10, "history": history,
    }))
    .unwrap();
    let _: Vec<_> = ws.run(core, run).collect().await;
    let apply = serde_json::from_value(json!({
        "connectionId": id,
        "changes": [
            {"type": "sql", "id": "a", "sql": "CREATE TABLE t (a INTEGER)"},
            {"type": "sql", "id": "b", "sql": "INSERT INTO t VALUES (1)"},
        ],
        "confirmed": true, "history": history,
    }))
    .unwrap();
    ws.apply_changes(core, apply).await.unwrap();
    ws.record_storage_write(
        &WriteOrigin::none(),
        StoredKind::Storage,
        None,
        Some(vec!["k".into()]),
    );

    let got = drain(&mut events).await;
    let kinds: Vec<(StoredKind, Option<String>, usize)> = got
        .iter()
        .map(|c| (c.kind, c.scope.clone(), c.ids.as_ref().map_or(0, Vec::len)))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (StoredKind::History, Some(saved.clone()), 1),
            (StoredKind::History, Some(saved.clone()), 2),
            (StoredKind::Storage, None, 1),
        ]
    );
    ws.disconnect(core, &id).await.unwrap();
}

#[tokio::test]
async fn events_carry_no_values() {
    let f = fx().await;
    let (core, ws, o) = (&*f.core, &f.ws, &WriteOrigin::none());
    let mut events = ws.events();
    let c = ws
        .create_connection(
            core,
            o,
            draft_connection(
                &f.project,
                "canary-name",
                json!({"host": "canary-host", "username": "canary-user",
                       "connectionString": "postgres://canary-user:canary-pw@canary-host/db",
                       "savePassword": true}),
            ),
            patch(json!({"db": "canary-secret"})),
        )
        .await
        .unwrap()
        .value
        .id;
    ws.update_connection(
        core,
        o,
        &c,
        patch(json!({"name": "canary-renamed"})),
        SecretChanges::default(),
    )
    .await
    .unwrap();
    ws.create_saved_query(
        core,
        o,
        draft_query(&f.project, "canary-query", "SELECT 'canary-text'"),
    )
    .await
    .unwrap();
    let got = drain(&mut events).await;
    assert_eq!(got.len(), 3);
    assert!(!format!("{got:?}").contains("canary"), "{got:?}");
}

#[tokio::test]
async fn the_epoch_is_the_workspace_and_changes_when_it_reopens() {
    let f = fx().await;
    let first: ChangeSeq = f.ws.change_seq();
    assert_eq!(first.epoch, f.ws.id().to_string());
    assert!(first.n >= 1, "the fixture's project write is counted");
    f.ws.close().await;
    let again = f
        .core
        .open_workspace(WorkspaceSpec::new(f.dir.path()))
        .await
        .unwrap();
    assert_ne!(again.change_seq().epoch, first.epoch);
    assert_eq!(again.change_seq().n, 0);
}
