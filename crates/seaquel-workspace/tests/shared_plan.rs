//! `seaquel_workspace::shared::plan`: Decision 34's rule table, pairing,
//! skips, Decision 36's publish, and the replay of
//! `fixtures/shared/projection.json` through the pure planner (see
//! `replay` below for exactly what it checks).

use seaquel_types::storage::{PersistedConnection, PersistedDashboard, PersistedSavedQuery};
use seaquel_workspace::library::LibraryLimits;
use seaquel_workspace::shared::format::{parse_query, query_content, write_query, QueryFile};
use seaquel_workspace::shared::plan::{
    row_hash, Limits, NoticeMemory, PublishOutcome, PublishStatus,
};
use seaquel_workspace::shared::{
    content_hash, pick_project_dir, plan_publish, plan_sync, DirScan, FileOp, IdSource, Kind, Link,
    LinkUpdate, LinkedConnection, LinkedDashboard, LinkedQuery, ProjectLink, PublishContext,
    RawFile, RowChange, RowOp, SharedRows, SkipReason, Skipped, SyncNotice,
};

#[path = "shared_plan/replay.rs"]
mod replay;

const ROOT: &str = ".seaquel/projects/team";

/// `plan_sync` under the desktop's limits (none).
fn sync_plan(
    link: &ProjectLink,
    scan: &DirScan,
    rows: &SharedRows,
    ids: &mut dyn IdSource,
) -> seaquel_workspace::shared::SyncPlan {
    plan_sync(link, scan, rows, &Limits::default(), ids)
}

fn link() -> ProjectLink {
    ProjectLink {
        repo_id: "repo-a".into(),
        dir: "team".into(),
    }
}

/// Ids as Core makes them, from a counter.
#[derive(Default)]
struct Ids(u32);

impl IdSource for Ids {
    fn row_id(&mut self, kind: Kind) -> String {
        self.0 += 1;
        let prefix = match kind {
            Kind::SavedQuery => "saved-",
            Kind::Dashboard => "dashboard-",
            Kind::Connection => "conn-",
        };
        format!("{prefix}{}", replay::uuid(self.0))
    }
    fn file_id(&mut self) -> String {
        self.0 += 1;
        replay::uuid(self.0)
    }
}

fn query(id: &str, name: &str, text: &str, shared: bool) -> PersistedSavedQuery {
    PersistedSavedQuery {
        id: id.into(),
        name: name.into(),
        query: text.into(),
        project_id: "p1".into(),
        created_at: "2024-01-01T00:00:00.000Z".into(),
        updated_at: "2024-01-01T00:00:00.000Z".into(),
        parameters: None,
        starred: false,
        shared,
        description: None,
        database_type: None,
        tags: None,
        folder: None,
        shared_path: None,
    }
}

fn qpath(file: &str) -> String {
    format!("{ROOT}/queries/{file}")
}

fn qfile(file: &str, name: &str, text: &str) -> RawFile {
    RawFile {
        rel_path: qpath(file),
        text: format!("---\nname: {name}\n---\n{text}\n"),
    }
}

fn hash_of_query(name: &str, text: &str) -> String {
    let q = QueryFile {
        name: name.into(),
        query: text.into(),
        ..Default::default()
    };
    content_hash(&query_content(&q))
}

fn rows(queries: Vec<LinkedQuery>) -> SharedRows {
    SharedRows {
        project_id: "p1".into(),
        queries,
        dashboards: vec![],
        connections: vec![],
    }
}

fn linked(row: PersistedSavedQuery, path: Option<&str>, base: Option<String>) -> LinkedQuery {
    LinkedQuery {
        row,
        link: Link {
            path: path.map(qpath),
            base,
            file_id: None,
        },
    }
}

fn scan(files: Vec<RawFile>) -> DirScan {
    DirScan {
        files,
        skipped: vec![],
        conflicted: false,
    }
}

fn link_of<'a>(links: &'a [LinkUpdate], id: &str) -> Option<&'a LinkUpdate> {
    links.iter().find(|l| l.id == id)
}

#[test]
fn the_rule_table_nothing_changed() {
    let b = hash_of_query("Orders", "SELECT 1");
    let rows = rows(vec![linked(
        query("q1", "Orders", "SELECT 1", true),
        Some("orders.sql"),
        Some(b.clone()),
    )]);
    let plan = sync_plan(
        &link(),
        &scan(vec![qfile("orders.sql", "Orders", "SELECT 1")]),
        &rows,
        &mut Ids::default(),
    );
    assert!(plan.rows.is_empty() && plan.files.is_empty() && plan.notices.is_empty());
    assert!(plan.links.is_empty(), "the base is already right");
}

#[test]
fn the_rule_table_a_local_change_writes_the_file() {
    let b = hash_of_query("Orders", "SELECT 1");
    let rows = rows(vec![linked(
        query("q1", "Orders", "SELECT 2", true),
        Some("orders.sql"),
        Some(b.clone()),
    )]);
    let plan = sync_plan(
        &link(),
        &scan(vec![qfile("orders.sql", "Orders", "SELECT 1")]),
        &rows,
        &mut Ids::default(),
    );
    assert!(plan.rows.is_empty());
    assert!(plan.notices.is_empty());
    let [FileOp::Write {
        rel_path,
        text,
        expect_hash,
    }] = plan.files.as_slice()
    else {
        panic!("one write: {:?}", plan.files)
    };
    // M1: the write expects the file the scan read.
    assert_eq!(expect_hash.as_deref(), Some(b.as_str()));
    assert_eq!(rel_path, &qpath("orders.sql"));
    assert!(text.contains("SELECT 2") && text.starts_with("---\nid: "));
    // The base waits for the write.
    let l = link_of(&plan.links, "q1").unwrap();
    assert_eq!(l.pending_on.as_deref(), Some(rel_path.as_str()));
    assert_eq!(l.link.base, Some(hash_of_query("Orders", "SELECT 2")));
}

#[test]
fn the_rule_table_a_teammates_change_updates_the_row() {
    let b = hash_of_query("Orders", "SELECT 1");
    let rows = rows(vec![linked(
        query("q1", "Orders", "SELECT 1", true),
        Some("orders.sql"),
        Some(b),
    )]);
    let plan = sync_plan(
        &link(),
        &scan(vec![qfile("orders.sql", "Orders", "SELECT 2")]),
        &rows,
        &mut Ids::default(),
    );
    assert!(plan.files.is_empty());
    assert!(
        plan.notices.is_empty(),
        "no conflict: only the file changed"
    );
    let [RowOp::UpdateQuery { id, patch }] = plan.rows.as_slice() else {
        panic!("{:?}", plan.rows)
    };
    assert_eq!(id, "q1");
    assert_eq!(patch.query.as_deref(), Some("SELECT 2"));
    let l = link_of(&plan.links, "q1").unwrap();
    assert_eq!(l.link.base, Some(hash_of_query("Orders", "SELECT 2")));
    assert!(l.pending_on.is_none());
}

#[test]
fn the_rule_table_both_changed_alike_moves_only_the_base() {
    let b = hash_of_query("Orders", "SELECT 1");
    let rows = rows(vec![linked(
        query("q1", "Orders", "SELECT 2", true),
        Some("orders.sql"),
        Some(b),
    )]);
    let plan = sync_plan(
        &link(),
        &scan(vec![qfile("orders.sql", "Orders", "SELECT 2")]),
        &rows,
        &mut Ids::default(),
    );
    assert!(plan.rows.is_empty() && plan.files.is_empty() && plan.notices.is_empty());
    let l = link_of(&plan.links, "q1").unwrap();
    assert_eq!(l.link.base, Some(hash_of_query("Orders", "SELECT 2")));
}

#[test]
fn the_rule_table_both_changed_the_file_wins() {
    for base in [Some(hash_of_query("Orders", "SELECT 1")), None] {
        let rows = rows(vec![linked(
            query("q1", "Orders", "SELECT 'mine'", true),
            Some("orders.sql"),
            base,
        )]);
        let plan = sync_plan(
            &link(),
            &scan(vec![qfile("orders.sql", "Orders", "SELECT 'theirs'")]),
            &rows,
            &mut Ids::default(),
        );
        assert!(plan.files.is_empty());
        let [RowOp::UpdateQuery { patch, .. }] = plan.rows.as_slice() else {
            panic!("{:?}", plan.rows)
        };
        assert_eq!(patch.query.as_deref(), Some("SELECT 'theirs'"));
        assert_eq!(
            plan.notices,
            vec![SyncNotice::Conflict {
                kind: Kind::SavedQuery,
                id: "q1".into(),
                replaced: None
            }]
        );
    }
}

#[test]
fn the_rule_table_a_removed_file_unshares_the_row() {
    // R = B and R ≠ B alike: the row stays, unshared, and is named.
    for text in ["SELECT 1", "SELECT 2"] {
        let rows = rows(vec![linked(
            query("q1", "Orders", text, true),
            Some("orders.sql"),
            Some(hash_of_query("Orders", "SELECT 1")),
        )]);
        let plan = sync_plan(&link(), &scan(vec![]), &rows, &mut Ids::default());
        assert!(plan.files.is_empty());
        assert!(matches!(
            plan.rows.as_slice(),
            [RowOp::Unshare { kind: Kind::SavedQuery, id }] if id == "q1"
        ));
        assert_eq!(
            plan.notices,
            vec![SyncNotice::RemovedInRepo {
                kind: Kind::SavedQuery,
                id: "q1".into()
            }]
        );
        assert_eq!(link_of(&plan.links, "q1").unwrap().link, Link::default());
    }
}

#[test]
fn the_rule_table_a_row_never_written_writes_its_file() {
    // A share whose write failed stored the path and no base.
    let rows = rows(vec![linked(
        query("q1", "Orders", "SELECT 1", true),
        Some("orders.sql"),
        None,
    )]);
    let plan = sync_plan(&link(), &scan(vec![]), &rows, &mut Ids::default());
    assert!(plan.rows.is_empty() && plan.notices.is_empty());
    assert!(matches!(
        plan.files.as_slice(),
        [FileOp::Write { rel_path, .. }] if *rel_path == qpath("orders.sql")
    ));
}

#[test]
fn the_rule_table_a_file_without_a_row() {
    // A new shared row under the file's exact name …
    let plan = sync_plan(
        &link(),
        &scan(vec![qfile("revenue.sql", "Revenue", "SELECT 3")]),
        &rows(vec![]),
        &mut Ids::default(),
    );
    let [RowOp::CreateQuery { id, draft }] = plan.rows.as_slice() else {
        panic!("{:?}", plan.rows)
    };
    assert!(draft.shared && draft.name == "Revenue" && draft.query == "SELECT 3");
    assert_eq!(
        link_of(&plan.links, id).unwrap().link.path.as_deref(),
        Some(qpath("revenue.sql").as_str())
    );
    // … unless a row that isn't shared has the name (by `name_key`).
    let plan = sync_plan(
        &link(),
        &scan(vec![qfile("strasse.sql", "STRASSE", "SELECT 1")]),
        &rows(vec![linked(
            query("q1", "Straße", "SELECT 1", false),
            None,
            None,
        )]),
        &mut Ids::default(),
    );
    assert!(plan.rows.is_empty() && plan.files.is_empty());
    assert_eq!(
        plan.notices,
        vec![SyncNotice::NameTaken {
            path: "projects/team/queries/strasse.sql".into(),
            taken_by: "q1".into()
        }]
    );
}

#[test]
fn the_rule_table_an_unreadable_file_changes_nothing() {
    let shared = linked(
        query("q1", "Orders", "SELECT 1", true),
        Some("orders.sql"),
        Some(hash_of_query("Orders", "SELECT 1")),
    );
    for (files, skipped, why) in [
        (
            vec![qfile("orders.sql", "Orders", "SELECT 1")],
            vec![Skipped {
                rel_path: qpath("orders.sql"),
                why: SkipReason::Unreadable,
            }],
            SkipReason::Unreadable,
        ),
        (
            vec![],
            vec![Skipped {
                rel_path: qpath("orders.sql"),
                why: SkipReason::Symlink,
            }],
            SkipReason::Symlink,
        ),
    ] {
        // A skipped file is never read, even when the scan also lists it.
        let files: Vec<RawFile> = files.into_iter().take(0).collect();
        let plan = sync_plan(
            &link(),
            &DirScan {
                files,
                skipped,
                conflicted: false,
            },
            &rows(vec![shared.clone()]),
            &mut Ids::default(),
        );
        assert!(plan.rows.is_empty() && plan.files.is_empty() && plan.links.is_empty());
        assert_eq!(
            plan.notices,
            vec![SyncNotice::Skipped {
                path: "projects/team/queries/orders.sql".into(),
                why
            }]
        );
    }
    // A file that doesn't parse is skipped the same way.
    let dash = LinkedDashboard {
        row: dashboard("d1", "Sales", true),
        link: Link::default(),
    };
    let plan = sync_plan(
        &link(),
        &scan(vec![RawFile {
            rel_path: format!("{ROOT}/dashboards/sales.json"),
            text: "{ \"name\": \"Sales\", \"widgets\": [".into(),
        }]),
        &SharedRows {
            project_id: "p1".into(),
            queries: vec![],
            dashboards: vec![dash],
            connections: vec![],
        },
        &mut Ids::default(),
    );
    assert!(plan.rows.is_empty() && plan.files.is_empty() && plan.links.is_empty());
    assert_eq!(
        plan.notices,
        vec![SyncNotice::Skipped {
            path: "projects/team/dashboards/sales.json".into(),
            why: SkipReason::DoesNotParse
        }]
    );
}

#[test]
fn a_conflicted_scan_plans_nothing() {
    let mut s = scan(vec![qfile("orders.sql", "Orders", "<<<<<<< HEAD")]);
    s.conflicted = true;
    let plan = sync_plan(
        &link(),
        &s,
        &rows(vec![linked(
            query("q1", "Orders", "SELECT 1", true),
            None,
            None,
        )]),
        &mut Ids::default(),
    );
    assert!(plan.conflicted);
    assert!(plan.rows.is_empty() && plan.files.is_empty() && plan.links.is_empty());
    assert!(plan.notices.is_empty());
}

#[test]
fn a_skipped_file_is_never_missing() {
    // A row whose stored path, or (without one) whose slug path, lies under
    // a skipped directory is left alone, and so is a legacy row (no stored
    // path) while anything of its kind was skipped.
    for (path, skip) in [
        (Some("reports/daily.sql"), "reports"),
        (None, "reports"),
        (Some("daily.sql"), "daily.sql"),
        (None, "elsewhere.sql"),
    ] {
        let mut row = query("q1", "Daily", "SELECT 1", true);
        if path.is_some_and(|p| p.starts_with("reports/")) || skip == "reports" {
            row.folder = Some("reports".into());
        }
        let base = path.map(|_| hash_of_query("Daily", "SELECT 1"));
        let plan = sync_plan(
            &link(),
            &DirScan {
                files: vec![],
                skipped: vec![Skipped {
                    rel_path: qpath(skip),
                    why: SkipReason::Unreadable,
                }],
                conflicted: false,
            },
            &rows(vec![linked(row, path, base)]),
            &mut Ids::default(),
        );
        assert!(plan.rows.is_empty(), "{path:?} {skip}: {:?}", plan.rows);
        assert!(plan.links.is_empty() && plan.files.is_empty());
        assert_eq!(plan.notices.len(), 1);
    }
}

#[test]
fn a_query_takes_its_folder_from_its_files_path() {
    // The row says `reports`; its file (paired by its stored path) is one
    // level deeper: the folder follows the file, the text stays.
    let mut row = query("q1", "Daily", "SELECT 1", true);
    row.folder = Some("reports".into());
    let b = hash_of_query("Daily", "SELECT 1");
    let plan = sync_plan(
        &link(),
        &scan(vec![qfile("reports/2024/daily.sql", "Daily", "SELECT 1")]),
        &rows(vec![linked(
            row.clone(),
            Some("reports/2024/daily.sql"),
            Some(b.clone()),
        )]),
        &mut Ids::default(),
    );
    let [RowOp::UpdateQuery { patch, .. }] = plan.rows.as_slice() else {
        panic!("{:?}", plan.rows)
    };
    assert_eq!(patch.folder, Some(Some("reports/2024".into())));
    assert!(patch.query.is_none() && plan.files.is_empty() && plan.notices.is_empty());
    // A legacy row (no stored path) pairs by name only within its folder:
    // a file of that name in another folder is another query.
    let plan = sync_plan(
        &link(),
        &scan(vec![qfile("other/daily.sql", "Daily", "SELECT 1")]),
        &rows(vec![linked(row, None, None)]),
        &mut Ids::default(),
    );
    assert!(plan
        .rows
        .iter()
        .any(|r| matches!(r, RowOp::CreateQuery { draft, .. } if draft.folder.as_deref() == Some("other"))));
}

#[test]
fn each_file_pairs_with_one_row() {
    // Two files name one row: the slug path wins, the other is unpaired.
    let rows1 = rows(vec![linked(
        query("q1", "Orders", "SELECT 1", true),
        None,
        None,
    )]);
    let plan = sync_plan(
        &link(),
        &scan(vec![
            qfile("orders-copy.sql", "Orders", "SELECT 3"),
            qfile("orders.sql", "Orders", "SELECT 2"),
        ]),
        &rows1,
        &mut Ids::default(),
    );
    assert_eq!(
        link_of(&plan.links, "q1").unwrap().link.path.as_deref(),
        Some(qpath("orders.sql").as_str())
    );
    assert!(plan.notices.contains(&SyncNotice::Unpaired {
        path: "projects/team/queries/orders-copy.sql".into(),
        claims: "q1".into()
    }));
    assert_eq!(plan.rows.len(), 1, "one update, no create: {:?}", plan.rows);

    // The stored path beats a slug-path claim.
    let b = hash_of_query("Orders", "SELECT 1");
    let rows2 = rows(vec![linked(
        query("q1", "Orders", "SELECT 1", true),
        Some("legacy-orders.sql"),
        Some(b.clone()),
    )]);
    let plan = sync_plan(
        &link(),
        &scan(vec![
            qfile("legacy-orders.sql", "Orders", "SELECT 1"),
            qfile("orders.sql", "Orders", "SELECT 9"),
        ]),
        &rows2,
        &mut Ids::default(),
    );
    assert!(plan.rows.is_empty());
    assert_eq!(
        plan.notices,
        vec![SyncNotice::Unpaired {
            path: "projects/team/queries/orders.sql".into(),
            claims: "q1".into()
        }]
    );

    // The id beats the stored path and the name; a duplicated id pairs
    // nothing by id.
    let id = "1b4e28ba-2fa1-41d2-883f-0016dc9b6b30";
    let mut l = linked(
        query("q1", "Orders", "SELECT 1", true),
        Some("orders.sql"),
        Some(b.clone()),
    );
    l.link.file_id = Some(id.into());
    let renamed = RawFile {
        rel_path: qpath("totals.sql"),
        text: format!("---\nid: {id}\nname: Order totals\n---\nSELECT 1\n"),
    };
    let plan = sync_plan(
        &link(),
        &scan(vec![renamed.clone()]),
        &rows(vec![l.clone()]),
        &mut Ids::default(),
    );
    assert!(plan.notices.is_empty(), "{:?}", plan.notices);
    let lu = link_of(&plan.links, "q1").unwrap();
    assert_eq!(lu.link.path.as_deref(), Some(qpath("totals.sql").as_str()));
    assert_eq!(lu.link.file_id.as_deref(), Some(id));
    let copy = RawFile {
        rel_path: qpath("totals-copy.sql"),
        ..renamed.clone()
    };
    let plan = sync_plan(
        &link(),
        &scan(vec![renamed, copy]),
        &rows(vec![l]),
        &mut Ids::default(),
    );
    assert!(
        !plan
            .links
            .iter()
            .any(|u| u.id == "q1" && u.link.file_id.as_deref() == Some(id)),
        "a duplicated id pairs nothing by id"
    );
}

fn dashboard(id: &str, name: &str, shared: bool) -> PersistedDashboard {
    PersistedDashboard {
        id: id.into(),
        project_id: "p1".into(),
        name: name.into(),
        viewport: r#"{"x":0,"y":0,"zoom":1}"#.into(),
        widgets: "[]".into(),
        date_filter: None,
        starred: false,
        shared,
        description: None,
        created_at: "2024-01-01T00:00:00.000Z".into(),
        updated_at: "2024-01-01T00:00:00.000Z".into(),
        shared_path: None,
    }
}

fn connection(id: &str, name: &str, link_path: Option<&str>) -> PersistedConnection {
    serde_json::from_value(serde_json::json!({
        "id": id, "projectId": "p1", "name": name, "type": "postgres",
        "host": "db.internal", "port": 5432, "databaseName": "warehouse",
        "username": "me", "savePassword": false, "saveSshPassword": false,
        "saveSshKeyPassphrase": false, "labelIds": [],
        "sharedConnectionId": link_path.map(|p| format!("repo-a:{ROOT}/connections/{p}")),
    }))
    .unwrap()
}

#[test]
fn a_template_changed_on_both_sides_names_what_it_replaced() {
    let mut c = connection("c1", "Warehouse", Some("warehouse.yaml"));
    let base = row_hash(&RowChange::Connection {
        row: Some(&c),
        link: &Link::default(),
        renamed: false,
        shared_now: false,
    });
    c.host = "db-local.internal".into();
    c.username = "secret-user".into();
    let plan = sync_plan(
        &link(),
        &scan(vec![RawFile {
            rel_path: format!("{ROOT}/connections/warehouse.yaml"),
            text: "name: Warehouse\ntype: postgres\nhost: db2.internal\nport: 6543\ndatabaseName: warehouse\n".into(),
        }]),
        &SharedRows {
            project_id: "p1".into(),
            queries: vec![],
            dashboards: vec![],
            connections: vec![LinkedConnection {
                link: Link {
                    path: Some(format!("{ROOT}/connections/warehouse.yaml")),
                    base,
                    file_id: None,
                },
                row: c,
            }],
        },
        &mut Ids::default(),
    );
    let [SyncNotice::Conflict {
        kind: Kind::Connection,
        replaced: Some(r),
        ..
    }] = plan.notices.as_slice()
    else {
        panic!("{:?}", plan.notices)
    };
    let shown = serde_json::to_value(r).unwrap();
    assert_eq!(
        shown,
        serde_json::json!({"host": "db-local.internal", "port": 5432})
    );
    assert!(!format!("{shown}").contains("secret-user"));
}

#[test]
fn publish_writes_moves_and_deletes() {
    let taken = |_: &str| false;
    let ctx = PublishContext {
        taken: &taken,
        existing: None,
    };
    let row = query("q1", "Orders", "SELECT 1", true);
    // A first share writes a free path; a failed write still stores it.
    let plan = plan_publish(
        &link(),
        &RowChange::Query {
            row: Some(&row),
            link: &Link::default(),
            renamed: false,
        },
        &ctx,
        &mut Ids::default(),
    )
    .unwrap();
    assert!(
        matches!(plan.files.as_slice(), [FileOp::Write { rel_path, .. }] if *rel_path == qpath("orders.sql"))
    );
    let on_failure = plan.on_failure.expect("the share keeps its path");
    assert_eq!(on_failure.link.path, Some(qpath("orders.sql")));
    assert_eq!(on_failure.link.base, None);

    // Nothing changed in the content: nothing to write.
    let stored = Link {
        path: Some(qpath("orders.sql")),
        base: Some(hash_of_query("Orders", "SELECT 1")),
        file_id: Some(replay::uuid(1)),
    };
    let plan = plan_publish(
        &link(),
        &RowChange::Query {
            row: Some(&row),
            link: &stored,
            renamed: false,
        },
        &ctx,
        &mut Ids::default(),
    )
    .unwrap();
    assert!(plan.files.is_empty() && plan.on_success.is_none());

    // A rename writes the new path, then deletes the old one.
    let mut renamed = row.clone();
    renamed.name = "All orders".into();
    let plan = plan_publish(
        &link(),
        &RowChange::Query {
            row: Some(&renamed),
            link: &stored,
            renamed: true,
        },
        &ctx,
        &mut Ids::default(),
    )
    .unwrap();
    let [FileOp::Write {
        rel_path: new,
        text,
        expect_hash,
    }, FileOp::Delete { rel_path: old, .. }] = plan.files.as_slice()
    else {
        panic!("{:?}", plan.files)
    };
    assert_eq!(expect_hash, &None, "a new path expects no file");
    assert_eq!(new, &qpath("all-orders.sql"));
    assert_eq!(old, &qpath("orders.sql"));
    assert!(text.contains(&replay::uuid(1)), "the file keeps its id");
    assert!(plan.on_failure.is_none(), "a failed move keeps the link");

    // A case-only rename keeps the stored path, even a mixed-case one.
    let stored_mixed = Link {
        path: Some(qpath("Orders.sql")),
        ..stored.clone()
    };
    let mut upper = row.clone();
    upper.name = "ORDERS".into();
    let plan = plan_publish(
        &link(),
        &RowChange::Query {
            row: Some(&upper),
            link: &stored_mixed,
            renamed: true,
        },
        &ctx,
        &mut Ids::default(),
    )
    .unwrap();
    assert!(
        matches!(plan.files.as_slice(), [FileOp::Write { rel_path, .. }] if *rel_path == qpath("Orders.sql"))
    );

    // A rename keeps the file's own folder (a deep one, as the sync made
    // the row's folder follow it) …
    let deep = Link {
        path: Some(qpath("reports/2024/daily.sql")),
        base: Some(hash_of_query("Daily", "SELECT 1")),
        file_id: None,
    };
    let mut daily = query("q2", "Daily totals", "SELECT 1", true);
    daily.folder = Some("reports/2024".into());
    let plan = plan_publish(
        &link(),
        &RowChange::Query {
            row: Some(&daily),
            link: &deep,
            renamed: true,
        },
        &ctx,
        &mut Ids::default(),
    )
    .unwrap();
    assert!(
        matches!(&plan.files[0], FileOp::Write { rel_path, .. } if *rel_path == qpath("reports/2024/daily-totals.sql"))
    );
    // … and a moved query goes to its new folder (I1), under its stem.
    let mut moved = query("q2", "Daily", "SELECT 1", true);
    moved.folder = Some("archive".into());
    let plan = plan_publish(
        &link(),
        &RowChange::Query {
            row: Some(&moved),
            link: &deep,
            renamed: true,
        },
        &ctx,
        &mut Ids::default(),
    )
    .unwrap();
    let [FileOp::Write { rel_path: to, .. }, FileOp::Delete { rel_path: from, .. }] =
        plan.files.as_slice()
    else {
        panic!("{:?}", plan.files)
    };
    assert_eq!(
        (to, from),
        (
            &qpath("archive/daily.sql"),
            &qpath("reports/2024/daily.sql")
        )
    );
    // Moved back to the root.
    moved.folder = None;
    let plan = plan_publish(
        &link(),
        &RowChange::Query {
            row: Some(&moved),
            link: &deep,
            renamed: true,
        },
        &ctx,
        &mut Ids::default(),
    )
    .unwrap();
    assert!(
        matches!(&plan.files[0], FileOp::Write { rel_path, .. } if *rel_path == qpath("daily.sql"))
    );

    // Unsharing or removing deletes the stored file.
    let mut unshared = row.clone();
    unshared.shared = false;
    for r in [Some(&unshared), None] {
        let plan = plan_publish(
            &link(),
            &RowChange::Query {
                row: r,
                link: &stored,
                renamed: false,
            },
            &ctx,
            &mut Ids::default(),
        )
        .unwrap();
        assert!(
            matches!(plan.files.as_slice(), [FileOp::Delete { rel_path, .. }] if *rel_path == qpath("orders.sql"))
        );
        // An unshared row's link is cleared; a removed row has none left.
        match r {
            Some(_) => assert_eq!(plan.on_success.unwrap().link, Link::default()),
            None => assert!(plan.on_success.is_none()),
        }
    }

    // A folder that fails Decision 32 is refused.
    let mut bad = row.clone();
    bad.folder = Some("../outside".into());
    assert!(plan_publish(
        &link(),
        &RowChange::Query {
            row: Some(&bad),
            link: &Link::default(),
            renamed: false
        },
        &ctx,
        &mut Ids::default(),
    )
    .is_err());
}

#[test]
fn a_connection_never_shared_publishes_nothing() {
    let taken = |_: &str| false;
    let ctx = PublishContext {
        taken: &taken,
        existing: None,
    };
    let c = connection("c4", "Billing", None);
    let none = Link::default();
    let change = |shared_now| RowChange::Connection {
        row: Some(&c),
        link: &none,
        renamed: false,
        shared_now,
    };
    let plan = plan_publish(&link(), &change(false), &ctx, &mut Ids::default()).unwrap();
    assert!(plan.files.is_empty() && plan.on_success.is_none() && plan.on_failure.is_none());
    let plan = plan_publish(&link(), &change(true), &ctx, &mut Ids::default()).unwrap();
    let [FileOp::Write { text, .. }] = plan.files.as_slice() else {
        panic!()
    };
    assert!(!text.contains("me\n"), "no user name in a template");
    // The share stores the link even when the write fails (`*` (4a)).
    assert!(plan.on_failure.unwrap().link.path.is_some());
}

#[test]
fn a_project_rename_rewrites_only_the_name() {
    let taken = |_: &str| false;
    let ctx = PublishContext {
        taken: &taken,
        existing: Some("name: Team\ndescription: Shared KPIs\n"),
    };
    let plan = plan_publish(
        &link(),
        &RowChange::Project { name: "Platform" },
        &ctx,
        &mut Ids::default(),
    )
    .unwrap();
    assert!(
        matches!(plan.files.as_slice(), [FileOp::Write { rel_path, text, .. }]
        if *rel_path == format!("{ROOT}/project.yaml") && text == "name: Platform\ndescription: Shared KPIs\n")
    );
    let same = PublishContext {
        taken: &taken,
        existing: Some("name: Platform\n"),
    };
    assert!(plan_publish(
        &link(),
        &RowChange::Project { name: "Platform" },
        &same,
        &mut Ids::default()
    )
    .unwrap()
    .files
    .is_empty());
}

#[test]
fn the_directory_is_picked_by_name_then_freed() {
    let dirs = vec![
        ("team".to_string(), "Team".to_string()),
        ("analytics".to_string(), "Analytics Team".to_string()),
    ];
    assert_eq!(pick_project_dir("ANALYTICS TEAM", &dirs), "analytics");
    assert_eq!(pick_project_dir("Team", &dirs), "team");
    assert_eq!(pick_project_dir("Ops", &dirs), "ops");
    // A directory named like the stem but holding another project.
    let dirs = vec![("team".to_string(), "Platform".to_string())];
    assert_eq!(pick_project_dir("Team", &dirs), "team-2");
}

#[test]
fn notices_name_a_file_once_per_session() {
    let mut memory = NoticeMemory::default();
    let n = SyncNotice::NameTaken {
        path: "projects/team/queries/a.sql".into(),
        taken_by: "q1".into(),
    };
    let c = SyncNotice::Conflict {
        kind: Kind::SavedQuery,
        id: "q1".into(),
        replaced: None,
    };
    assert_eq!(
        memory.filter(vec![n.clone(), c.clone()]),
        vec![n.clone(), c.clone()]
    );
    assert_eq!(memory.filter(vec![n, c.clone()]), vec![c]);
}

/// The planner never panics on scans and rows made of awkward pieces.
#[test]
fn plans_never_panic() {
    let mut rng = replay::Rng(0x0ddb_a112_0261_001f);
    const NAMES: &[&str] = &[
        "Orders", "orders", "ORDERS", "Straße", "STRASSE", "", " ", "\u{feff}", "a/b", "..", "con",
        "売上", "x\u{0}", "\"q\"", "it's", "a:b",
    ];
    const TEXTS: &[&str] = &[
        "---\nname: Orders\n---\nSELECT 1\n",
        "---\nid: x\nname: Orders\n---\nSELECT 2\n",
        "---\nid: 1b4e28ba-2fa1-41d2-883f-0016dc9b6b30\nname: STRASSE\n---\n\n",
        "SELECT 1",
        "",
        "{\"name\":\"Orders\",\"widgets\":[]}",
        "{\"widgets\":[null]}",
        "name: Orders\ntype: mysql\n",
        "name: Orders\nport: 99999999999999999999\nsshTunnel:\n  enabled: true\n",
        "\u{feff}\r\n",
    ];
    const DIRS: &[&str] = &[
        "queries",
        "queries/a",
        "dashboards",
        "connections",
        "",
        "queries/..",
    ];
    const EXTS: &[&str] = &[".sql", ".json", ".yaml", ".yml", ".SQL", ""];
    for round in 0..3_000 {
        let n_files = rng.below(6);
        let files: Vec<RawFile> = (0..n_files)
            .map(|_| RawFile {
                rel_path: format!(
                    "{ROOT}/{}/{}{}",
                    rng.pick(DIRS),
                    rng.pick(NAMES),
                    rng.pick(EXTS)
                ),
                text: rng.pick(TEXTS).to_string(),
            })
            .collect();
        let skipped = (0..rng.below(2))
            .map(|_| Skipped {
                rel_path: format!("{ROOT}/{}", rng.pick(DIRS)),
                why: SkipReason::Symlink,
            })
            .collect();
        let pick_link = |rng: &mut replay::Rng| Link {
            path: (rng.below(2) == 0).then(|| {
                format!(
                    "{ROOT}/{}/{}{}",
                    rng.pick(DIRS),
                    rng.pick(NAMES),
                    rng.pick(EXTS)
                )
            }),
            base: (rng.below(2) == 0).then(|| content_hash(rng.pick(TEXTS))),
            file_id: (rng.below(3) == 0).then(|| "x".to_string()),
        };
        let queries = (0..rng.below(4))
            .map(|i| LinkedQuery {
                row: query(
                    &format!("q{i}"),
                    rng.pick(NAMES),
                    rng.pick(TEXTS),
                    rng.below(3) > 0,
                ),
                link: pick_link(&mut rng),
            })
            .collect();
        let dashboards = (0..rng.below(3))
            .map(|i| LinkedDashboard {
                row: dashboard(&format!("d{i}"), rng.pick(NAMES), rng.below(2) == 0),
                link: pick_link(&mut rng),
            })
            .collect();
        let connections = (0..rng.below(3))
            .map(|i| {
                let l = pick_link(&mut rng);
                LinkedConnection {
                    row: connection(
                        &format!("c{i}"),
                        rng.pick(NAMES),
                        l.path.as_deref().and_then(|p| p.rsplit('/').next()),
                    ),
                    link: l,
                }
            })
            .collect();
        let rows = SharedRows {
            project_id: "p1".into(),
            queries,
            dashboards,
            connections,
        };
        let s = DirScan {
            files,
            skipped,
            conflicted: round % 50 == 0,
        };
        let plan = sync_plan(&link(), &s, &rows, &mut Ids::default());
        let _ = format!("{plan:?}");
        let taken = |p: &str| p.len().is_multiple_of(2);
        for q in &rows.queries {
            let ctx = PublishContext {
                taken: &taken,
                existing: Some(rng.pick(TEXTS)),
            };
            let _ = plan_publish(
                &link(),
                &RowChange::Query {
                    row: Some(&q.row),
                    link: &q.link,
                    renamed: rng.below(2) == 0,
                },
                &ctx,
                &mut Ids::default(),
            );
        }
    }
}

#[test]
fn debug_shows_no_names_paths_or_text() {
    const CANARY: &str = "canary-Zq9";
    let row = query(
        "q1",
        &format!("{CANARY} name"),
        &format!("SELECT '{CANARY}'"),
        true,
    );
    let mut d = dashboard("d1", &format!("{CANARY} dash"), true);
    d.widgets = format!("[{{\"title\":\"{CANARY}\"}}]");
    let mut c = connection("c1", &format!("{CANARY} conn"), Some("w.yaml"));
    c.host = format!("{CANARY}.host");
    let files = vec![
        RawFile {
            rel_path: qpath(&format!("{CANARY}.sql")),
            text: format!("---\nname: {CANARY} file\n---\nSELECT '{CANARY}'\n"),
        },
        RawFile {
            rel_path: format!("{ROOT}/connections/{CANARY}.yaml"),
            text: format!("name: {CANARY}\ntype: postgres\nhost: {CANARY}2\n"),
        },
    ];
    let lk = Link {
        path: Some(qpath(&format!("{CANARY}-old.sql"))),
        base: Some(hash_of_query("x", "y")),
        file_id: None,
    };
    let rows = SharedRows {
        project_id: "p1".into(),
        queries: vec![LinkedQuery {
            row: row.clone(),
            link: lk.clone(),
        }],
        dashboards: vec![LinkedDashboard {
            row: d,
            link: Link::default(),
        }],
        connections: vec![LinkedConnection {
            row: c.clone(),
            link: Link {
                path: Some(format!("{ROOT}/connections/w.yaml")),
                base: Some("0".repeat(64)),
                file_id: None,
            },
        }],
    };
    let s = DirScan {
        files: files.clone(),
        skipped: vec![Skipped {
            rel_path: qpath(CANARY),
            why: SkipReason::Unreadable,
        }],
        conflicted: false,
    };
    let plan = sync_plan(&link(), &s, &rows, &mut Ids::default());
    let taken = |_: &str| false;
    let publish = plan_publish(
        &link(),
        &RowChange::Query {
            row: Some(&row),
            link: &lk,
            renamed: true,
        },
        &PublishContext {
            taken: &taken,
            existing: Some(&files[0].text),
        },
        &mut Ids::default(),
    );
    let parsed = parse_query(
        &files[0].text,
        &files[0].rel_path,
        ".seaquel/projects/team/queries",
    );
    let shown = format!(
        "{plan:?} {publish:?} {s:?} {rows:?} {lk:?} {:?} {parsed:?} {:?} {:?}",
        files[0],
        link(),
        RowChange::Connection {
            row: Some(&c),
            link: &Link::default(),
            renamed: false,
            shared_now: true
        }
    );
    let outcome = PublishOutcome {
        status: PublishStatus::Failed,
        code: Some("FILE_ERROR".into()),
        message: Some(format!("{CANARY}.sql couldn't be written")),
    };
    let shown = format!("{shown} {outcome:?}");
    assert!(!shown.contains(CANARY), "{shown}");
    assert!(!shown.contains("SELECT"), "{shown}");
    assert!(
        !shown.contains("team"),
        "the directory is a project name: {shown}"
    );
    // The writer is exercised too, so the canary really was in the inputs.
    assert!(write_query(&parsed).contains(CANARY));
}

fn tfile(file: &str, text: &str) -> RawFile {
    RawFile {
        rel_path: format!("{ROOT}/connections/{file}"),
        text: text.into(),
    }
}

fn dfile(file: &str, text: &str) -> RawFile {
    RawFile {
        rel_path: format!("{ROOT}/dashboards/{file}"),
        text: text.into(),
    }
}

/// C1: a file whose content the library would refuse changes nothing on
/// either side: it is skipped as `invalid`, the row it stands for is left
/// alone, and nothing is created from it.
#[test]
fn a_file_the_library_would_refuse_is_skipped() {
    let web = Limits {
        library: LibraryLimits {
            max_name_bytes: Some(16),
            ..Default::default()
        },
        ..Default::default()
    };
    let cases: Vec<(&str, RawFile, Limits)> = vec![
        (
            "a bogus parameter type",
            RawFile {
                rel_path: qpath("orders.sql"),
                text: "---\nname: Orders\nparameters:\n  - name: a\n    type: blob\n---\nSELECT {{a}}\n"
                    .into(),
            },
            Limits::default(),
        ),
        (
            "duplicate parameter names",
            RawFile {
                rel_path: qpath("orders.sql"),
                text: "---\nname: Orders\nparameters:\n  - name: a\n    type: text\n  - name: a\n    type: number\n---\nSELECT 1\n"
                    .into(),
            },
            Limits::default(),
        ),
        (
            "a NUL in a name",
            qfile("orders.sql", "Ord\u{0}ers", "SELECT 1"),
            Limits::default(),
        ),
        (
            "an over-limit name under web limits",
            qfile("orders.sql", "Orders from every region", "SELECT 1"),
            web,
        ),
        (
            "a dashboard viewport that isn't an object",
            dfile(
                "orders.json",
                "{\"name\":\"Orders\",\"widgets\":[],\"viewport\":5}",
            ),
            Limits::default(),
        ),
        (
            "type: oracle",
            tfile("orders.yaml", "name: Orders\ntype: oracle\nhost: h\n"),
            Limits::default(),
        ),
        (
            "port 70000",
            tfile("orders.yaml", "name: Orders\ntype: postgres\nport: 70000\n"),
            Limits::default(),
        ),
        (
            "port -5",
            tfile("orders.yaml", "name: Orders\ntype: postgres\nport: -5\n"),
            Limits::default(),
        ),
    ];
    for (what, file, limits) in cases {
        let kind = if file.rel_path.contains("/queries/") {
            Kind::SavedQuery
        } else if file.rel_path.contains("/dashboards/") {
            Kind::Dashboard
        } else {
            Kind::Connection
        };
        let path = file.rel_path.clone();
        let notice = SyncNotice::Skipped {
            path: path.trim_start_matches(".seaquel/").into(),
            why: SkipReason::Invalid,
        };
        // A row of the file's kind at its stored path …
        let at = |k: Kind| Link {
            path: (k == kind).then(|| path.clone()),
            base: Some("0".repeat(64)),
            file_id: None,
        };
        let mut rows = SharedRows {
            project_id: "p1".into(),
            ..Default::default()
        };
        match kind {
            Kind::SavedQuery => rows.queries.push(LinkedQuery {
                row: query("q1", "Orders", "SELECT 1", true),
                link: at(kind),
            }),
            Kind::Dashboard => rows.dashboards.push(LinkedDashboard {
                row: dashboard("d1", "Orders", true),
                link: at(kind),
            }),
            Kind::Connection => rows.connections.push(LinkedConnection {
                row: connection("c1", "Orders", Some("orders.yaml")),
                link: at(kind),
            }),
        }
        // … is left alone.
        let plan = plan_sync(
            &link(),
            &scan(vec![file.clone()]),
            &rows,
            &limits,
            &mut Ids::default(),
        );
        assert!(
            plan.rows.is_empty() && plan.links.is_empty() && plan.files.is_empty(),
            "{what}: {:?}",
            plan.rows
        );
        assert_eq!(plan.notices, vec![notice.clone()], "{what}");
        // … and with no row, nothing is created.
        let plan = plan_sync(
            &link(),
            &scan(vec![file]),
            &SharedRows {
                project_id: "p1".into(),
                ..Default::default()
            },
            &limits,
            &mut Ids::default(),
        );
        assert!(
            plan.rows.is_empty() && plan.links.is_empty(),
            "{what}: {:?}",
            plan.rows
        );
        assert_eq!(plan.notices, vec![notice], "{what}");
    }
}

/// I5: a row whose slug path was skipped doesn't pair with another file of
/// its name; that file claims it and isn't imported.
#[test]
fn a_row_at_a_skipped_path_is_left_out_of_pairing() {
    let rows = rows(vec![linked(query("q1", "X", "SELECT 1", true), None, None)]);
    let plan = sync_plan(
        &link(),
        &DirScan {
            files: vec![qfile("copy-of-x.sql", "X", "SELECT 2")],
            skipped: vec![Skipped {
                rel_path: qpath("x.sql"),
                why: SkipReason::Symlink,
            }],
            conflicted: false,
        },
        &rows,
        &mut Ids::default(),
    );
    assert!(
        plan.rows.is_empty() && plan.links.is_empty() && plan.files.is_empty(),
        "{:?}",
        plan.rows
    );
    assert!(plan.notices.contains(&SyncNotice::Unpaired {
        path: "projects/team/queries/copy-of-x.sql".into(),
        claims: "q1".into()
    }));
}

/// Flag 4: an update from a file whose name another row holds keeps the
/// row's name, takes the rest, says so, and stores the row's own hash as
/// the base, so each later sync tries again.
#[test]
fn a_taken_name_is_withheld_from_an_update() {
    let rows1 = rows(vec![
        linked(
            query("q1", "Orders", "SELECT 1", true),
            Some("orders.sql"),
            Some(hash_of_query("Orders", "SELECT 1")),
        ),
        linked(
            query("q2", "Totals", "SELECT 2", true),
            Some("totals.sql"),
            Some(hash_of_query("Totals", "SELECT 2")),
        ),
    ]);
    let plan = sync_plan(
        &link(),
        &scan(vec![
            qfile("orders.sql", "Orders", "SELECT 1"),
            qfile("totals.sql", "Orders", "SELECT 9"),
        ]),
        &rows1,
        &mut Ids::default(),
    );
    let [RowOp::UpdateQuery { id, patch }] = plan.rows.as_slice() else {
        panic!("{:?}", plan.rows)
    };
    assert_eq!(id, "q2");
    assert!(patch.name.is_none(), "the name is withheld");
    assert_eq!(patch.query.as_deref(), Some("SELECT 9"));
    assert!(plan.files.is_empty(), "nothing is written back");
    assert_eq!(
        plan.notices,
        vec![SyncNotice::NameTaken {
            path: "projects/team/queries/totals.sql".into(),
            taken_by: "q1".into()
        }]
    );
    assert_eq!(
        link_of(&plan.links, "q2").unwrap().link.base,
        Some(hash_of_query("Totals", "SELECT 9")),
        "the base is the row after the partial patch"
    );

    // Two rows swapping names in one pull: both are refused.
    let plan = sync_plan(
        &link(),
        &scan(vec![
            qfile("orders.sql", "Totals", "SELECT 1"),
            qfile("totals.sql", "Orders", "SELECT 2"),
        ]),
        &rows1,
        &mut Ids::default(),
    );
    assert!(
        plan.rows.is_empty(),
        "nothing but the names changed, and both are withheld: {:?}",
        plan.rows
    );
    assert_eq!(plan.notices.len(), 2, "{:?}", plan.notices);

    // A name a rename earlier in the same plan frees can be taken.
    let plan = sync_plan(
        &link(),
        &scan(vec![
            qfile("orders.sql", "Old orders", "SELECT 1"),
            qfile("totals.sql", "Orders", "SELECT 2"),
        ]),
        &rows1,
        &mut Ids::default(),
    );
    let names: Vec<Option<&str>> = plan
        .rows
        .iter()
        .map(|op| match op {
            RowOp::UpdateQuery { patch, .. } => patch.name.as_deref(),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec![Some("Old orders"), Some("Orders")]);
    assert!(plan.notices.is_empty(), "{:?}", plan.notices);
}

/// M6: a connection linked to a template of another repo isn't this
/// repo's: it neither pairs nor is removed.
#[test]
fn a_link_to_another_repo_is_ignored() {
    let mut c = connection("c1", "Warehouse", Some("warehouse.yaml"));
    c.shared_connection_id = Some(format!("repo-other:{ROOT}/connections/warehouse.yaml"));
    let plan = sync_plan(
        &link(),
        &scan(vec![]),
        &SharedRows {
            project_id: "p1".into(),
            queries: vec![],
            dashboards: vec![],
            connections: vec![LinkedConnection {
                row: c,
                link: Link {
                    path: Some(format!("{ROOT}/connections/warehouse.yaml")),
                    base: Some("0".repeat(64)),
                    file_id: None,
                },
            }],
        },
        &mut Ids::default(),
    );
    assert!(
        plan.rows.is_empty() && plan.links.is_empty() && plan.notices.is_empty(),
        "{:?}",
        plan.rows
    );
}

/// M4: 20,000 files and as many rows plan within a time box (keys
/// computed once, pairing through indexes).
#[test]
#[allow(clippy::disallowed_types, clippy::disallowed_methods)] // Instant, in a native-only test
fn a_large_project_plans_quickly() {
    const N: usize = 20_000;
    let mut files = Vec::with_capacity(N);
    let mut queries = Vec::with_capacity(N);
    for i in 0..N {
        let name = format!("Query {i}");
        files.push(qfile(&format!("q{i}.sql"), &name, &format!("SELECT {i}")));
        // Half pair by stored path, half by name; every tenth changed here.
        let text = if i % 10 == 0 {
            format!("SELECT -{i}")
        } else {
            format!("SELECT {i}")
        };
        let path = (i % 2 == 0).then(|| format!("q{i}.sql"));
        let base = path
            .as_ref()
            .map(|_| hash_of_query(&name, &format!("SELECT {i}")));
        queries.push(LinkedQuery {
            row: query(&format!("q{i}"), &name, &text, true),
            link: Link {
                path: path.map(|p| qpath(&p)),
                base,
                file_id: None,
            },
        });
    }
    let started = std::time::Instant::now();
    let plan = sync_plan(&link(), &scan(files), &rows(queries), &mut Ids::default());
    let took = started.elapsed();
    assert!(took.as_secs() < 30, "took {took:?}");
    // The odd rows get their first link; every tenth row (all even, with
    // a stored path) changed here and is written; the rest are as stored.
    assert_eq!(plan.links.len(), N / 2 + N / 10);
    assert_eq!(plan.files.len(), N / 10);
    assert!(!plan
        .rows
        .iter()
        .any(|op| matches!(op, RowOp::CreateQuery { .. } | RowOp::Unshare { .. })));
}

/// A connection linked to a template in another project's directory of
/// the same repo (bug 7's damage) isn't this directory's: it neither pairs
/// nor is removed.
#[test]
fn a_link_to_another_directory_is_ignored() {
    let mut c = connection("c1", "Metrics", None);
    let path = ".seaquel/projects/ops/connections/metrics.yaml";
    c.shared_connection_id = Some(format!("repo-a:{path}"));
    let plan = sync_plan(
        &link(),
        &scan(vec![tfile(
            "metrics.yaml",
            "name: Metrics\ntype: postgres\nhost: other\n",
        )]),
        &SharedRows {
            project_id: "p1".into(),
            queries: vec![],
            dashboards: vec![],
            connections: vec![LinkedConnection {
                row: c,
                link: Link {
                    path: Some(path.into()),
                    base: Some("0".repeat(64)),
                    file_id: None,
                },
            }],
        },
        &mut Ids::default(),
    );
    assert!(
        !plan.rows.iter().any(|op| matches!(op,
            RowOp::Unshare { id, .. } | RowOp::UpdateConnection { id, .. } if id == "c1")),
        "{:?}",
        plan.rows
    );
    assert!(!plan.links.iter().any(|l| l.id == "c1"), "{:?}", plan.links);
}

/// Two rows that already share a name key both hold it; `NameTaken` names
/// the first by id, whatever the order of the rows.
#[test]
fn every_holder_of_a_name_is_kept() {
    let dup = |order: [&str; 2]| {
        let mut qs: Vec<LinkedQuery> = order
            .iter()
            .map(|id| linked(query(id, "Orders", "SELECT 1", false), None, None))
            .collect();
        qs.push(linked(
            query("q9", "Totals", "SELECT 2", true),
            Some("totals.sql"),
            Some(hash_of_query("Totals", "SELECT 2")),
        ));
        rows(qs)
    };
    for order in [["q1", "q2"], ["q2", "q1"]] {
        let plan = sync_plan(
            &link(),
            &scan(vec![
                qfile("totals.sql", "Orders", "SELECT 2"),
                qfile("orders.sql", "ORDERS", "SELECT 3"),
            ]),
            &dup(order),
            &mut Ids::default(),
        );
        assert!(plan.rows.is_empty(), "{:?}", plan.rows);
        let mut notices = plan.notices.clone();
        notices.sort_by_key(|n| format!("{n:?}"));
        assert_eq!(
            plan.notices.len(),
            2,
            "the rename and the new file: {:?}",
            plan.notices
        );
        for n in &plan.notices {
            assert!(
                matches!(n, SyncNotice::NameTaken { taken_by, .. } if taken_by == "q1"),
                "{n:?}"
            );
        }
    }
}

/// R1 through the planner: a deep dashboard file is skipped, not followed,
/// on a 2 MiB stack.
#[test]
fn a_deep_file_plans_on_a_small_stack() {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            let deep = format!("{}1{}", "{\"a\":".repeat(100_000), "}".repeat(100_000));
            let plan = sync_plan(
                &link(),
                &scan(vec![dfile(
                    "deep.json",
                    &format!("{{\"name\":\"Deep\",\"widgets\":[{deep}]}}"),
                )]),
                &rows(vec![]),
                &mut Ids::default(),
            );
            assert_eq!(
                plan.notices,
                vec![SyncNotice::Skipped {
                    path: "projects/team/dashboards/deep.json".into(),
                    why: SkipReason::DoesNotParse
                }]
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
