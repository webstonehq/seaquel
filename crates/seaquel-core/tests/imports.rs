//! Imports from TablePlus and DBeaver through Core (phase 5e):
//! the injected home, `found: false` and `unreadable`, the bounds on what
//! is read and decoded, and the create's duplicate check and order append
//! inside its one transaction.

// The shared test helpers' clock reads the wall clock (native tests).
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod common;

use std::path::Path;
use std::sync::Arc;

use seaquel_core::domain::imports::{default_path, ImportSource};
use seaquel_core::{
    Core, ImportPaths, LocalFiles, StoredKind, Workspace, WorkspaceEvent, WorkspaceSpec,
    WriteOrigin,
};
use serde_json::{json, Value};

use common::{capture_logs, dump, insert_rows, logged, T0};

struct Fx {
    _dir: tempfile::TempDir,
    home: tempfile::TempDir,
    core: Core,
    ws: Arc<Workspace>,
}

fn import_core(home: &Path) -> Core {
    seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .local_files(LocalFiles::Allowed)
        .import_paths(ImportPaths::new(home))
        .build()
}

async fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let core = import_core(home.path());
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    insert_rows(
        ws.storage(),
        "projects",
        &[
            json!({"id": "p1", "name": "Main", "description": null, "git_repo_path": null,
            "created_at": T0, "updated_at": T0}),
        ],
    )
    .await;
    insert_rows(
        ws.storage(),
        "connections",
        &[
            json!({"id": "c1", "project_id": "p1", "name": "Shop", "type": "postgres",
            "host": "db.example.com", "port": 5432, "database_name": "shop", "username": "app"}),
        ],
    )
    .await;
    Fx {
        _dir: dir,
        home,
        core,
        ws,
    }
}

fn origin() -> WriteOrigin {
    WriteOrigin::new(Some("main"))
}

/// DBeaver's file at its default place under `home`.
fn dbeaver_file(home: &Path, text: &str) -> std::path::PathBuf {
    let rel = default_path(ImportSource::Dbeaver, std::env::consts::OS).unwrap();
    let path = home.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

const TWO: &str = r#"{"connections": {
  "pg-1": {"provider": "postgresql", "name": "Shop",
    "configuration": {"host": "db.example.com", "port": "5432", "database": "shop", "user": "app"}},
  "pg-2": {"provider": "postgresql", "name": "Shop",
    "configuration": {"host": "db.example.com", "port": "5432", "database": "reports", "user": "app"}},
  "my-1": {"provider": "mysql", "name": "Legacy",
    "configuration": {"host": "legacy.example.com", "port": "${port}", "database": "old", "user": "root"}}
}}"#;

#[tokio::test(flavor = "multi_thread")]
async fn candidates_read_the_injected_home_only() {
    let f = fx().await;
    dbeaver_file(f.home.path(), TWO);
    let found =
        f.ws.import_candidates(&f.core, ImportSource::Dbeaver, "p1", None)
            .await
            .unwrap();
    assert!(found.found);
    let c = found.candidates.unwrap();
    let keys: Vec<&str> = c.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(keys, ["pg-1", "pg-2", "my-1"]);
    assert_eq!(c[0].duplicate_of.as_deref(), Some("c1"));
    assert_eq!(c[1].duplicate_of, None);
    assert_eq!(
        serde_json::to_value(c[2].problem).unwrap(),
        json!("invalidPort")
    );
    // Another home holds nothing, whatever the process's own holds.
    let empty = tempfile::tempdir().unwrap();
    let other = import_core(empty.path());
    let found =
        f.ws.import_candidates(&other, ImportSource::Dbeaver, "p1", None)
            .await
            .unwrap();
    assert!(!found.found);
    // A Core with no import paths never falls back to the real home.
    let bare = seaquel_core::with_default_plugins()
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .local_files(LocalFiles::Allowed)
        .build();
    for source in [ImportSource::Dbeaver, ImportSource::Tableplus] {
        let found =
            f.ws.import_candidates(&bare, source, "p1", None)
                .await
                .unwrap();
        assert!(!found.found);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_file_is_found_false() {
    let f = fx().await;
    let found =
        f.ws.import_candidates(&f.core, ImportSource::Dbeaver, "p1", None)
            .await
            .unwrap();
    assert_eq!(
        serde_json::to_value(&found).unwrap(),
        json!({"found": false})
    );
    let missing = f.home.path().join("nope.json");
    let found =
        f.ws.import_candidates(
            &f.core,
            ImportSource::Tableplus,
            "p1",
            Some(&missing.to_string_lossy()),
        )
        .await
        .unwrap();
    assert!(!found.found);
    let err =
        f.ws.import_create(
            &f.core,
            &origin(),
            ImportSource::Dbeaver,
            "p1",
            &["pg-1".to_string()],
            None,
        )
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(err.code, "IMPORT_SOURCE_UNREADABLE");
    let err =
        f.ws.import_candidates(&f.core, ImportSource::Dbeaver, "p9", None)
            .await
            .map(|_| ())
            .unwrap_err();
    assert_eq!(err.code, "PROJECT_NOT_FOUND");
}

#[tokio::test(flavor = "multi_thread")]
async fn unreadable_files_answer_cores_own_message() {
    let f = fx().await;
    let home = f.home.path().to_string_lossy().into_owned();
    // Not JSON, a folder, too large, not a plist.
    let path = dbeaver_file(f.home.path(), "{not json");
    let found =
        f.ws.import_candidates(&f.core, ImportSource::Dbeaver, "p1", None)
            .await
            .unwrap();
    assert_eq!(
        found.unreadable.as_deref(),
        Some("The file isn't valid JSON.")
    );
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let found =
        f.ws.import_candidates(&f.core, ImportSource::Dbeaver, "p1", None)
            .await
            .unwrap();
    assert_eq!(found.unreadable.as_deref(), Some("The file can't be read."));
    let big = f.home.path().join("big.json");
    std::fs::write(&big, vec![b' '; 16 * 1024 * 1024 + 1]).unwrap();
    let found =
        f.ws.import_candidates(
            &f.core,
            ImportSource::Dbeaver,
            "p1",
            Some(&big.to_string_lossy()),
        )
        .await
        .unwrap();
    assert_eq!(
        found.unreadable.as_deref(),
        Some("The file is too large to import.")
    );
    let junk = f.home.path().join("junk.plist");
    std::fs::write(&junk, b"<?xml version=\"1.0\"?><plist><array><dict>").unwrap();
    let found =
        f.ws.import_candidates(
            &f.core,
            ImportSource::Tableplus,
            "p1",
            Some(&junk.to_string_lossy()),
        )
        .await
        .unwrap();
    let message = found.unreadable.unwrap();
    assert_eq!(message, "The file isn't a TablePlus connections file.");
    assert!(!message.contains(&home));
}

/// A plist nested far past any connection list, or with millions of
/// values, is unreadable before anything is built (no stack overflow on a
/// test thread's 2 MiB stack).
#[tokio::test(flavor = "multi_thread")]
async fn a_hostile_plist_is_unreadable() {
    let f = fx().await;
    let deep = f.home.path().join("deep.plist");
    let mut text = String::from("<?xml version=\"1.0\"?><plist version=\"1.0\">");
    text.push_str(&"<array>".repeat(200_000));
    text.push_str(&"</array>".repeat(200_000));
    text.push_str("</plist>");
    std::fs::write(&deep, text).unwrap();
    let found =
        f.ws.import_candidates(
            &f.core,
            ImportSource::Tableplus,
            "p1",
            Some(&deep.to_string_lossy()),
        )
        .await
        .unwrap();
    assert!(found.unreadable.is_some(), "{found:?}");
    let wide = f.home.path().join("wide.plist");
    let mut text = String::from("<?xml version=\"1.0\"?><plist version=\"1.0\"><array>");
    text.push_str(&"<true/>".repeat(2_100_000));
    text.push_str("</array></plist>");
    std::fs::write(&wide, text).unwrap();
    let found =
        f.ws.import_candidates(
            &f.core,
            ImportSource::Tableplus,
            "p1",
            Some(&wide.to_string_lossy()),
        )
        .await
        .unwrap();
    assert!(found.unreadable.is_some(), "{found:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn tableplus_plists_decode_as_src_tauri_did() {
    let f = fx().await;
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../seaquel-workspace/tests/fixtures/imports"
    );
    let cases: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}/tableplus.json")).unwrap())
            .unwrap();
    let mut checked = 0;
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let (Some(file), Some(_)) = (case["plist"].as_str(), case["input"].as_array()) else {
            continue;
        };
        let plist = format!("{dir}/{file}");
        let found =
            f.ws.import_candidates(&f.core, ImportSource::Tableplus, "p1", Some(&plist))
                .await
                .unwrap();
        let mut want = seaquel_core::domain::imports::tableplus_candidates(&case["input"]).unwrap();
        seaquel_core::domain::imports::mark_duplicates(&mut want, &[]);
        let strip = |c: &[seaquel_core::domain::imports::ImportCandidate]| -> Value {
            let mut v = serde_json::to_value(c).unwrap();
            for x in v.as_array_mut().unwrap() {
                x.as_object_mut().unwrap().remove("duplicateOf");
            }
            v
        };
        assert_eq!(
            strip(found.candidates.as_deref().unwrap_or_default()),
            strip(&want),
            "{name}"
        );
        checked += 1;
    }
    assert!(checked > 5, "{checked}");
}

#[tokio::test(flavor = "multi_thread")]
async fn create_checks_duplicates_inside_the_transaction() {
    let f = fx().await;
    dbeaver_file(f.home.path(), TWO);
    let keys = ["pg-2".to_string()];
    let o = origin();
    let (a, b) = tokio::join!(
        f.ws.import_create(&f.core, &o, ImportSource::Dbeaver, "p1", &keys, None),
        f.ws.import_create(&f.core, &o, ImportSource::Dbeaver, "p1", &keys, None),
    );
    let statuses: Vec<String> = [a.unwrap(), b.unwrap()]
        .iter()
        .map(|r| r.value.results[0].status.clone())
        .collect();
    let mut sorted = statuses.clone();
    sorted.sort();
    assert_eq!(sorted, ["duplicate", "imported"], "{statuses:?}");
    let conns = dump(f.ws.storage(), "connections", "rowid").await;
    assert_eq!(conns.len(), 2);
    // Within one call too: the second of two keys naming one database.
    let same = r#"{"connections": {
      "a": {"provider": "postgresql", "name": "One", "configuration": {"host": "h", "port": "1", "database": "d", "user": "u"}},
      "b": {"provider": "postgresql", "name": "Two", "configuration": {"host": "h", "port": "1", "database": "d", "user": "u"}}
    }}"#;
    dbeaver_file(f.home.path(), same);
    let out =
        f.ws.import_create(
            &f.core,
            &origin(),
            ImportSource::Dbeaver,
            "p1",
            &["a".to_string(), "b".to_string(), "zzz".to_string()],
            None,
        )
        .await
        .unwrap()
        .value;
    let got: Vec<(&str, &str)> = out
        .results
        .iter()
        .map(|r| (r.key.as_str(), r.status.as_str()))
        .collect();
    assert_eq!(
        got,
        [("a", "imported"), ("b", "duplicate"), ("zzz", "notFound")]
    );
    assert_eq!(out.results[1].duplicate_of, out.results[0].id);
}

#[tokio::test(flavor = "multi_thread")]
async fn create_appends_to_the_order_in_the_same_transaction() {
    let f = fx().await;
    dbeaver_file(f.home.path(), TWO);
    let mut events = f.ws.events();
    let out =
        f.ws.import_create(
            &f.core,
            &origin(),
            ImportSource::Dbeaver,
            "p1",
            &["pg-1".to_string(), "pg-2".to_string()],
            None,
        )
        .await
        .unwrap();
    let results = &out.value.results;
    assert_eq!(results[0].status, "duplicate");
    assert_eq!(results[1].status, "imported");
    let id = results[1].id.clone().unwrap();
    // No `project_state` row before: the order starts as the project's
    // connections, then the new one.
    let state = dump(f.ws.storage(), "project_state", "project_id").await;
    assert_eq!(
        serde_json::from_str::<Value>(state[0]["connection_order"].as_str().unwrap()).unwrap(),
        json!(["c1", id])
    );
    let conns = dump(f.ws.storage(), "connections", "rowid").await;
    let new = conns.iter().find(|c| c["id"] == id.as_str()).unwrap();
    assert_eq!(new["is_local_only"], 1);
    assert_eq!(new["name"], "Shop (2)", "renamed when taken");
    assert_eq!(new["database_name"], "reports");
    // One connection event and one project event, after the one commit.
    let mut kinds = Vec::new();
    while let Ok(Some(e)) = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        futures::StreamExt::next(&mut events),
    )
    .await
    {
        if let WorkspaceEvent::StorageChanged(c) = e {
            kinds.push(c.kind);
        }
    }
    assert_eq!(kinds, [StoredKind::Connection, StoredKind::Project]);
    // A key whose candidate has a problem refuses the whole call.
    let err =
        f.ws.import_create(
            &f.core,
            &origin(),
            ImportSource::Dbeaver,
            "p1",
            &["pg-2".to_string(), "my-1".to_string()],
            None,
        )
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert_eq!(dump(f.ws.storage(), "connections", "rowid").await.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn imports_log_no_names_hosts_or_paths() {
    capture_logs();
    let f = fx().await;
    dbeaver_file(
        f.home.path(),
        r#"{"connections": {"canary-key": {"provider": "postgresql", "name": "CanaryName",
          "configuration": {"host": "canary-host.example", "port": "5432", "database": "canarydb", "user": "canaryuser"}}}}"#,
    );
    f.ws.import_candidates(&f.core, ImportSource::Dbeaver, "p1", None)
        .await
        .unwrap();
    f.ws.import_create(
        &f.core,
        &origin(),
        ImportSource::Dbeaver,
        "p1",
        &["canary-key".to_string()],
        None,
    )
    .await
    .unwrap();
    let log = logged();
    for canary in ["canary", "Canary", &f.home.path().to_string_lossy()] {
        assert!(!log.contains(canary), "{canary:?} was logged:\n{log}");
    }
    assert!(log.contains("imports.create"));
}
