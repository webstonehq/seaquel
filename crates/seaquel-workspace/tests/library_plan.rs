//! The library's pure rules (`seaquel_workspace::library`, phase 5d-1):
//! the checks each library fixture's calls meet, names, patches, versions
//! and limits. Core's replay (`seaquel-core/tests/library.rs`) runs the
//! same fixtures against storage.

use std::collections::HashSet;

use seaquel_types::storage::{ConnectionLabel, PersistedConnection};
use seaquel_workspace::library::*;
use serde_json::{json, Value};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/library");
const FILES: [&str; 5] = [
    "connections.json",
    "projects.json",
    "saved-queries.json",
    "imports.json",
    "legacy-strings.json",
];

fn load(file: &str) -> Vec<Value> {
    serde_json::from_str(&std::fs::read_to_string(format!("{FIXTURES}/{file}")).unwrap()).unwrap()
}

fn j<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap_or_else(|e| panic!("{e}"))
}

/// The checks a call meets before anything is read.
fn input_check(call: &Value) -> Result<(), LibraryError> {
    let none = LibraryLimits::default();
    let p = &call["params"];
    match call["method"].as_str().unwrap() {
        "connectionCreate" => {
            let d: ConnectionDraft = j(p["connection"].clone());
            check_connection_draft(&d, &none)?;
            let s: SecretChanges = p.get("secrets").map(|s| j(s.clone())).unwrap_or_default();
            check_secret_values(&s)?;
            check_secret_flags(&s, &connection_from_draft("c".into(), &d, "now"))
        }
        "connectionUpdate" => {
            let patch: ConnectionPatch = j(p["patch"].clone());
            check_connection_patch(&patch, &none)?;
            let s: SecretChanges = p.get("secrets").map(|s| j(s.clone())).unwrap_or_default();
            check_secret_values(&s)
        }
        "projectCreate" => check_project_draft(&j(p["project"].clone()), &none),
        "projectUpdate" => check_project_patch(&j(p["patch"].clone()), &none),
        "labelCreate" => check_label_draft(&j(p["label"].clone()), &none),
        "labelUpdate" => {
            check_custom_label_id(p["labelId"].as_str().unwrap(), &none)?;
            check_label_patch(&j(p["patch"].clone()), &none)
        }
        "labelRemove" => check_custom_label_id(p["labelId"].as_str().unwrap(), &none),
        "savedQueryCreate" => {
            let d: SavedQueryDraft = j(p["query"].clone());
            check_saved_query_draft(&d, &none)?;
            check_saved_query(&saved_query_from_draft("q".into(), &d, "now"), &none)
        }
        "savedQueryUpdate" => check_saved_query_patch(&j(p["patch"].clone()), &none),
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
                let list = match &step["library"] {
                    Value::Null => continue,
                    Value::Array(a) => a.clone(),
                    one => vec![one.clone()],
                };
                for call in &list {
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
    assert!(calls > 150, "{calls}");
}

#[test]
fn a_name_that_differs_only_in_case_or_spaces_is_taken() {
    let rows = [
        ("c1", "Ärger DB"),
        ("c2", "Straße"),
        ("c3", "Café"),
        ("c4", "İstanbul"),
    ];
    let taken = |n: &str| find_taken(n, rows.iter().copied(), None);
    assert_eq!(taken(" ärger db "), Some("c1".into()));
    assert_eq!(taken("\tÄRGER DB\u{3000}"), Some("c1".into()));
    assert_eq!(taken("STRASSE"), Some("c2".into()));
    assert_eq!(taken("strasse"), Some("c2".into()));
    assert_eq!(taken("Cafe\u{301}"), Some("c3".into()), "NFD equals NFC");
    assert_eq!(taken("CAFÉ"), Some("c3".into()));
    assert_eq!(
        taken("i\u{307}stanbul"),
        Some("c4".into()),
        "full folding of İ"
    );
    assert_eq!(taken("Cafe"), None);
    assert_eq!(taken("Ärger DB 2"), None);
    // The row being renamed isn't a clash with itself.
    assert_eq!(
        find_taken("STRASSE", rows.iter().copied(), Some("c2")),
        None
    );
    // Import names count up.
    let keys: HashSet<String> = ["Local", "local (2)", "LOCAL (3)"]
        .iter()
        .map(|n| name_key(n))
        .collect();
    assert_eq!(free_name("Local", &keys), "Local (4)");
    assert_eq!(
        free_name("main", &HashSet::from([name_key("Main")])),
        "main (2)"
    );
}

fn row() -> PersistedConnection {
    j(json!({
        "id": "c1", "projectId": "p1", "name": "A", "type": "postgres", "host": "h",
        "port": 5432, "databaseName": "d", "username": "u", "labelIds": ["local"],
        "aiShareSchema": true, "activeAIModel": "m", "sslMode": "require",
    }))
}

#[test]
fn a_nul_anywhere_is_refused() {
    let none = LibraryLimits::default();
    let base = json!({"projectId": "p1", "name": "A", "type": "postgres", "host": "h",
                      "port": 5432, "databaseName": "d", "username": "u"});
    for field in [
        "name",
        "host",
        "databaseName",
        "username",
        "sslMode",
        "connectionString",
        "sharedConnectionId",
        "activeAIProviderId",
        "activeAIModel",
    ] {
        let mut d = base.clone();
        d[field] = json!("a\0b");
        let e = check_connection_draft(&j(d), &none).unwrap_err();
        assert_eq!(e.code, INVALID_ARGUMENT, "{field}");
        assert!(!e.message.contains("a\0b"));
    }
    let mut d = base.clone();
    d["labelIds"] = json!(["lo\0cal"]);
    assert!(check_connection_draft(&j(d), &none).is_err());
    let mut d = base.clone();
    d["sshTunnel"] = json!({"enabled": true, "host": "b\0", "port": 22, "username": "u", "authMethod": "password"});
    assert!(check_connection_draft(&j(d), &none).is_err());
    assert!(check_connection_patch(&j(json!({"host": "\0"})), &none).is_err());
    assert!(check_connection_patch(&j(json!({"sslMode": "\0"})), &none).is_err());
    assert!(check_secret_values(&j(json!({"db": "p\0w"}))).is_err());
    assert!(check_project_draft(&j(json!({"name": "a", "description": "\0"})), &none).is_err());
    assert!(check_project_patch(&j(json!({"gitRepoPath": "/r\0"})), &none).is_err());
    assert!(check_label_draft(&j(json!({"name": "\0x", "color": "#000000"})), &none).is_err());
    for q in [
        json!({"projectId": "p", "name": "a", "query": "SELECT '\0'"}),
        json!({"projectId": "p", "name": "a", "query": "x", "folder": "\0"}),
        json!({"projectId": "p", "name": "a", "query": "x", "tags": ["\0"]}),
        json!({"projectId": "p", "name": "a", "query": "x", "parameters": [{"name": "\0", "type": "text"}]}),
    ] {
        assert!(
            check_saved_query_draft(&j(q.clone()), &none).is_err(),
            "{q}"
        );
    }
    assert!(check_saved_query_patch(&j(json!({"description": "\0"})), &none).is_err());
}

#[test]
fn port_must_be_whole_and_in_range() {
    for ok in [0.0, 1.0, 5432.0, 65535.0] {
        assert!(check_port(ok, "port").is_ok(), "{ok}");
    }
    for bad in [-1.0, 65536.0, 22.5, 1e9, f64::NAN, f64::INFINITY, -0.5] {
        assert!(check_port(bad, "port").is_err(), "{bad}");
    }
    let none = LibraryLimits::default();
    let d = |port: f64| {
        json!({"projectId": "p1", "name": "A", "type": "postgres", "host": "h",
                                "port": port, "databaseName": "d", "username": "u"})
    };
    assert!(check_connection_draft(&j(d(70000.0)), &none).is_err());
    let mut t = d(5432.0);
    t["sshTunnel"] = json!({"enabled": true, "host": "b", "port": 22.5, "username": "u", "authMethod": "password"});
    assert!(
        check_connection_draft(&j(t), &none).is_err(),
        "the tunnel's port too"
    );
    assert!(check_connection_patch(&j(json!({"port": 1.5})), &none).is_err());
    // Types.
    assert!(check_type("postgresql").is_err());
    for ty in ENGINE_TYPES {
        assert!(check_type(ty).is_ok());
    }
}

#[test]
fn unknown_label_ids_are_refused() {
    let custom = vec![ConnectionLabel {
        id: "label-a".into(),
        name: "A".into(),
        is_predefined: false,
        color: "#000000".into(),
    }];
    assert!(check_labels(
        &[
            "local".into(),
            "staging".into(),
            "prod".into(),
            "label-a".into()
        ],
        &custom
    )
    .is_ok());
    let e = check_labels(&["label-b".into()], &custom).unwrap_err();
    assert_eq!(e.code, LABEL_NOT_FOUND);
    assert!(check_labels(&["dev".into()], &[]).is_err());
    // Predefined ids can't be changed or removed.
    for id in PREDEFINED_LABEL_IDS {
        assert!(check_custom_label_id(id, &LibraryLimits::default()).is_err());
    }
    assert!(check_custom_label_id("label-a", &LibraryLimits::default()).is_ok());
    assert!(check_colour("#a1B2c3").is_ok());
    for bad in ["red", "#abc", "#abcdeg", "a1b2c3d", "#abcdef0", "＃abcdef"] {
        assert!(check_colour(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_patch_keeps_absent_fields_and_clears_nulls() {
    let mut r = row();
    apply_connection_patch(&mut r, &j(json!({"host": "h2"})), "now");
    assert_eq!(r.host, "h2");
    assert_eq!(r.ai_share_schema, Some(true), "absent: kept");
    assert_eq!(r.active_ai_model.as_deref(), Some("m"));
    assert_eq!(r.ssl_mode.as_deref(), Some("require"));
    assert_eq!(r.label_ids, vec!["local"]);
    assert_eq!(r.last_connected, None, "not connected: lastConnected kept");

    apply_connection_patch(
        &mut r,
        &j(
            json!({"aiShareSchema": null, "activeAIModel": null, "sslMode": null, "connected": true}),
        ),
        "now",
    );
    assert_eq!(r.ai_share_schema, None, "null: cleared");
    assert_eq!(r.active_ai_model, None);
    assert_eq!(r.ssl_mode, None);
    assert_eq!(r.last_connected.as_deref(), Some("now"));

    apply_connection_patch(
        &mut r,
        &j(json!({"labelIds": ["prod", "prod", "local"]})),
        "now",
    );
    assert_eq!(r.label_ids, vec!["prod", "local"], "a repeated id once");

    // A Clearable round-trips: absent, null and a value stay distinct.
    let p: ConnectionPatch = j(json!({"sslMode": null, "activeAIModel": "x"}));
    assert_eq!(p.ssl_mode, Some(None));
    assert_eq!(p.active_ai_model, Some(Some("x".into())));
    assert_eq!(p.connection_string, None);
    let back = serde_json::to_value(&p).unwrap();
    assert_eq!(back, json!({"sslMode": null, "activeAIModel": "x"}));
    // `{}` for a Clearable isn't a value.
    assert!(serde_json::from_value::<ConnectionPatch>(json!({"sslMode": {}})).is_err());
    // Unknown fields are refused.
    assert!(serde_json::from_value::<ConnectionPatch>(json!({"hots": "x"})).is_err());

    // Saved queries: text changes are noticed, starring alone keeps
    // updatedAt.
    let mut q: seaquel_types::storage::PersistedSavedQuery = j(json!({
        "id": "q1", "name": "Q", "query": "SELECT 1", "projectId": "p1",
        "createdAt": "t0", "updatedAt": "t0", "folder": null,
    }));
    let c = apply_saved_query_patch(&mut q, &j(json!({"starred": true})), "t1");
    assert_eq!(q.updated_at, "t0");
    assert_eq!(c, SavedQueryChange::default());
    let c = apply_saved_query_patch(&mut q, &j(json!({"query": "SELECT 1"})), "t1");
    assert_eq!(c.previous_text, None, "unchanged text");
    assert_eq!(q.updated_at, "t1");
    let c = apply_saved_query_patch(&mut q, &j(json!({"query": "SELECT 2", "folder": ""})), "t2");
    assert_eq!(c.previous_text.as_deref(), Some("SELECT 1"));
    assert!(!c.renamed, "NULL and \"\" are one folder");
    let c = apply_saved_query_patch(&mut q, &j(json!({"name": "q"})), "t3");
    assert!(!c.renamed, "a case-only rename isn't a clash");
}

fn metas(spec: &str) -> Vec<VersionMeta> {
    // "KddKd": K a keyframe, d a diff, numbered from 1.
    spec.chars()
        .enumerate()
        .map(|(i, c)| VersionMeta {
            id: format!("v{}", i + 1),
            version: (i + 1) as f64,
            keyframe: c == 'K',
            bytes: 10,
        })
        .collect()
}

/// Phase 5d-1 probe fix: with a byte budget (`max_version_bytes`, web
/// only), the newest versions stay only while their bytes together fit it,
/// and always at least the newest one; then back to a keyframe as before.
#[test]
fn version_prune_keeps_within_a_byte_budget() {
    // Ten versions of 10 bytes each: a 35-byte budget keeps the newest 3.
    assert_eq!(
        version_prune(&metas("KKKKKKKKKK"), 100, Some(35)),
        vec!["v1", "v2", "v3", "v4", "v5", "v6", "v7"]
    );
    // The count still applies when it's the tighter bound.
    assert_eq!(
        version_prune(&metas("KKKKK"), 2, Some(1000)),
        vec!["v1", "v2", "v3"]
    );
    // 0 (keep every version) is bounded by the bytes too.
    assert_eq!(version_prune(&metas("KKKK"), 0, Some(20)), vec!["v1", "v2"]);
    // A newest version larger than the budget is still kept.
    assert_eq!(version_prune(&metas("KKK"), 100, Some(5)), vec!["v1", "v2"]);
    // A kept diff keeps its keyframe, past the budget.
    assert_eq!(version_prune(&metas("KKdd"), 100, Some(20)), vec!["v1"]);
    // Within budget: nothing goes.
    assert!(version_prune(&metas("KKK"), 100, Some(30)).is_empty());
}

#[test]
fn version_prune_keeps_back_to_a_keyframe() {
    assert!(
        version_prune(&metas("KKKKK"), 0, None).is_empty(),
        "0 keeps all"
    );
    assert!(version_prune(&metas("KKK"), 3, None).is_empty());
    assert_eq!(version_prune(&metas("KKKKK"), 3, None), vec!["v1", "v2"]);
    // v3 is a diff on v2, on v1: keeping v3 keeps them.
    assert!(version_prune(&metas("KddKK"), 3, None).is_empty());
    assert_eq!(version_prune(&metas("KKddKK"), 3, None), vec!["v1"]);
    // Diffs before the oldest kept keyframe go.
    assert_eq!(
        version_prune(&metas("dddKK"), 2, None),
        vec!["v1", "v2", "v3"]
    );
    assert!(
        version_prune(&metas("ddddd"), 2, None).is_empty(),
        "no keyframe: nothing is safe to drop"
    );
    // Unordered input.
    let mut m = metas("KKKK");
    m.reverse();
    assert_eq!(version_prune(&m, 1, None), vec!["v1", "v2", "v3"]);
    assert_eq!(parse_version_limit(Some("3")), 3);
}

#[test]
fn limits_are_the_interfaces() {
    let none = LibraryLimits::default();
    let big = "x".repeat(3 * 1024 * 1024);
    let d = json!({"projectId": "p1", "name": big, "type": "postgres", "host": big,
                   "port": 1, "databaseName": "d", "username": "u",
                   "labelIds": vec!["local"; 5000]});
    assert!(
        check_connection_draft(&j(d.clone()), &none).is_ok(),
        "the desktop has no limits"
    );
    let web = LibraryLimits {
        max_name_bytes: Some(1024),
        max_field_bytes: Some(64 * 1024),
        max_query_bytes: Some(2 * 1024 * 1024),
        max_list_items: Some(1000),
        max_connections: Some(10_000),
        max_projects: Some(1000),
        max_saved_queries: Some(50_000),
        max_version_bytes: Some(16 * 1024 * 1024),
    };
    let e = check_connection_draft(&j(d), &web).unwrap_err();
    assert!(e.message.contains("max_name_bytes"), "{e}");
    let q = json!({"projectId": "p1", "name": "Q", "query": big});
    assert!(check_saved_query_draft(&j(q.clone()), &none).is_ok());
    assert!(check_saved_query_draft(&j(q), &web)
        .unwrap_err()
        .message
        .contains("max_query_bytes"));
    let q = json!({"projectId": "p1", "name": "Q", "query": "x", "tags": vec!["t"; 1001]});
    assert!(check_saved_query_draft(&j(q), &web)
        .unwrap_err()
        .message
        .contains("max_list_items"));
    assert!(check_count(9_999, web.max_connections, "max_connections").is_ok());
    assert!(check_count(10_000, web.max_connections, "max_connections").is_err());
    assert!(check_count(u64::MAX, none.max_connections, "max_connections").is_ok());
}

#[test]
fn checks_never_panic() {
    let limits = [
        LibraryLimits::default(),
        LibraryLimits {
            max_name_bytes: Some(0),
            max_field_bytes: Some(0),
            max_query_bytes: Some(0),
            max_list_items: Some(0),
            max_connections: Some(0),
            max_projects: Some(0),
            max_saved_queries: Some(0),
            max_version_bytes: Some(0),
        },
    ];
    let strings = [
        "",
        " ",
        "\u{feff}",
        "\u{0}",
        "\u{10ffff}",
        "\u{fffd}",
        "a\u{301}\u{301}\u{301}",
        "ß",
        "İ",
        "ﬃ",
        "#",
        "#gggggg",
        "🦀🦀",
        "\r\n",
        "\u{2028}",
        "é",
    ];
    for l in &limits {
        for s in strings {
            let _ = name_key(s);
            let _ = free_name(s, &HashSet::from([name_key(s)]));
            let _ = check_colour(s);
            let _ = check_name(s, "x", l);
            let _ = parse_version_limit(Some(s));
            let _ = check_custom_label_id(s, l);
            let d = json!({"projectId": s, "name": s, "type": s, "host": s, "port": 0,
                           "databaseName": s, "username": s, "labelIds": [s], "sslMode": s});
            if let Ok(d) = serde_json::from_value::<ConnectionDraft>(d) {
                let _ = check_connection_draft(&d, l);
                let r = connection_from_draft("c".into(), &d, s);
                let _ = check_connection(&r, &[], l);
            }
            let q = json!({"projectId": s, "name": s, "query": s, "tags": [s],
                           "parameters": [{"name": s, "type": s}], "folder": s});
            if let Ok(q) = serde_json::from_value::<SavedQueryDraft>(q) {
                let _ = check_saved_query_draft(&q, l);
                let _ = check_saved_query(&saved_query_from_draft("q".into(), &q, s), l);
            }
        }
        for port in [f64::NAN, f64::MAX, f64::MIN, -0.0, f64::EPSILON] {
            let _ = check_port(port, "p");
        }
    }
    assert!(version_prune(&[], 5, None).is_empty());
    let _ = version_prune(&metas("K"), u32::MAX, None);
    let nan = vec![VersionMeta {
        id: "a".into(),
        version: f64::NAN,
        keyframe: false,
        bytes: u64::MAX,
    }];
    let _ = version_prune(&nan, 0, None);
    let _ = version_prune(&nan, 1, None);
}
