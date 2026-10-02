//! `tree`: scanning a project's `.seaquel` directory and applying file
//! writes, on real temp directories with real symlinks (Decision 32).

#![cfg(unix)]

use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::sync::Arc;

use seaquel_git::tree::{self, ApplyOptions, OpOutcome, ScanBounds};
use seaquel_workspace::shared::{file_hash, FileOp, SkipReason};

const DIR: &str = ".seaquel/projects/team";

fn put(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn write(rel: &str, text: &str, expect: Option<&str>) -> FileOp {
    FileOp::Write {
        rel_path: rel.into(),
        text: text.into(),
        expect_hash: expect.map(String::from),
    }
}

fn hash(rel: &str, text: &str) -> String {
    file_hash(rel, text).unwrap().0
}

#[tokio::test]
async fn a_symlinked_file_is_skipped_and_a_symlinked_dir_refuses_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(outside.path().join("secret"), "id_ed25519 bytes").unwrap();
    std::fs::create_dir(outside.path().join("dashboards")).unwrap();
    std::fs::write(outside.path().join("dashboards/x.json"), "{\"name\":\"X\"}").unwrap();
    put(
        root,
        &format!("{DIR}/queries/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    symlink(
        outside.path().join("secret"),
        root.join(DIR).join("queries/x.sql"),
    )
    .unwrap();
    symlink(
        outside.path().join("dashboards"),
        root.join(DIR).join("dashboards"),
    )
    .unwrap();

    let scan = tree::scan(root, DIR, ScanBounds::default()).await.unwrap();
    let files: Vec<&str> = scan.files.iter().map(|f| f.rel_path.as_str()).collect();
    assert_eq!(files, [format!("{DIR}/queries/orders.sql")]);
    let mut skipped: Vec<(String, SkipReason)> = scan
        .skipped
        .iter()
        .map(|s| (s.rel_path.clone(), s.why))
        .collect();
    skipped.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        skipped,
        [
            (format!("{DIR}/dashboards"), SkipReason::Symlink),
            (format!("{DIR}/queries/x.sql"), SkipReason::Symlink),
        ]
    );
    assert!(scan.files.iter().all(|f| !f.text.contains("id_ed25519")));

    // A write through the symlinked directory, and over the symlinked file,
    // is refused, and nothing outside the repo changes.
    let out = tree::apply(
        root,
        vec![
            write(&format!("{DIR}/dashboards/x.json"), "{}", None),
            write(&format!("{DIR}/queries/x.sql"), "boom", None),
            FileOp::Delete {
                rel_path: format!("{DIR}/queries/x.sql"),
                expect_hash: None,
            },
        ],
        ApplyOptions::default(),
    )
    .await
    .unwrap();
    assert!(
        out.iter().all(|o| matches!(o, OpOutcome::Refused(_))),
        "{out:?}"
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("dashboards/x.json")).unwrap(),
        "{\"name\":\"X\"}"
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("secret")).unwrap(),
        "id_ed25519 bytes"
    );
    // A symlinked project directory is skipped whole.
    let tmp2 = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp2.path().join(".seaquel/projects")).unwrap();
    symlink(outside.path(), tmp2.path().join(DIR)).unwrap();
    let scan = tree::scan(tmp2.path(), DIR, ScanBounds::default())
        .await
        .unwrap();
    assert!(scan.files.is_empty());
    assert_eq!(scan.skipped.len(), 1);
    assert_eq!(scan.skipped[0].rel_path, DIR);
    assert_eq!(scan.skipped[0].why, SkipReason::Symlink);
}

#[tokio::test]
async fn a_path_with_dot_dot_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(DIR)).unwrap();
    for bad in [
        format!("{DIR}/../../../escape.sql"),
        format!("{DIR}/queries/./x.sql"),
        "/etc/passwd".to_string(),
        format!("{DIR}/queries/a\\b.sql"),
        format!("{DIR}/queries/con.sql"),
        format!("{DIR}/queries/.hidden.sql"),
        format!("{DIR}/queries/x.sql."),
        "projects/x.sql".to_string(),
    ] {
        let out = tree::apply(root, vec![write(&bad, "x", None)], ApplyOptions::default())
            .await
            .unwrap();
        assert!(matches!(out[0], OpOutcome::Refused(_)), "{bad}: {out:?}");
        let out = tree::apply(
            root,
            vec![FileOp::Delete {
                rel_path: bad.clone(),
                expect_hash: None,
            }],
            ApplyOptions::default(),
        )
        .await
        .unwrap();
        assert!(matches!(out[0], OpOutcome::Refused(_)), "{bad}: {out:?}");
    }
    assert!(!tmp.path().parent().unwrap().join("escape.sql").exists());
    // A project directory with `..` isn't scanned.
    assert!(
        tree::scan(root, ".seaquel/projects/..", ScanBounds::default())
            .await
            .is_err()
    );
    assert!(tree::scan(root, "../x", ScanBounds::default())
        .await
        .is_err());
}

#[tokio::test]
async fn a_file_past_16_mib_is_skipped_and_named() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let big = format!("{DIR}/queries/big.sql");
    let mut text = String::from("---\nname: Big\n---\n");
    text.push_str(&"x".repeat(16 * 1024 * 1024));
    put(root, &big, &text);
    put(root, &format!("{DIR}/queries/small.sql"), "SELECT 1\n");
    put(root, &format!("{DIR}/queries/latin1.sql"), "");
    std::fs::write(
        root.join(DIR).join("queries/latin1.sql"),
        [0xff, 0xfe, b'x'],
    )
    .unwrap();
    put(root, &format!("{DIR}/notes.md"), "not read");
    let scan = tree::scan(root, DIR, ScanBounds::default()).await.unwrap();
    let files: Vec<&str> = scan.files.iter().map(|f| f.rel_path.as_str()).collect();
    assert_eq!(files, [format!("{DIR}/queries/small.sql")]);
    let mut skipped: Vec<(String, SkipReason)> = scan
        .skipped
        .iter()
        .map(|s| (s.rel_path.clone(), s.why))
        .collect();
    skipped.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        skipped,
        [
            (big, SkipReason::TooLarge),
            (format!("{DIR}/queries/latin1.sql"), SkipReason::NotUtf8),
        ]
    );
}

#[tokio::test]
async fn too_many_files_skip_the_whole_project() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for i in 0..12 {
        put(root, &format!("{DIR}/queries/q{i}.sql"), "SELECT 1\n");
    }
    let bounds = ScanBounds {
        max_files: 10,
        ..ScanBounds::default()
    };
    let scan = tree::scan(root, DIR, bounds).await.unwrap();
    assert!(scan.files.is_empty());
    assert_eq!(scan.skipped.len(), 1);
    assert_eq!(scan.skipped[0].rel_path, DIR);
    assert_eq!(scan.skipped[0].why, SkipReason::TooMany);
    let bounds = ScanBounds {
        max_total_bytes: 20,
        ..ScanBounds::default()
    };
    let scan = tree::scan(root, DIR, bounds).await.unwrap();
    assert!(scan.files.is_empty());
    assert_eq!(scan.skipped[0].why, SkipReason::TooLarge);
}

#[tokio::test]
async fn an_unreadable_directory_is_skipped_not_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    put(root, &format!("{DIR}/queries/a.sql"), "SELECT 1\n");
    put(
        root,
        &format!("{DIR}/dashboards/d.json"),
        "{\"name\":\"D\"}",
    );
    let locked = root.join(DIR).join("queries");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let scan = tree::scan(root, DIR, ScanBounds::default()).await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    let scan = scan.unwrap();
    assert_eq!(scan.files.len(), 1);
    assert_eq!(scan.skipped.len(), 1);
    assert_eq!(scan.skipped[0].rel_path, format!("{DIR}/queries"));
    assert_eq!(scan.skipped[0].why, SkipReason::Unreadable);
}

#[tokio::test]
async fn writes_are_atomic_and_checked_against_the_expected_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let rel = format!("{DIR}/queries/orders.sql");
    let v1 = "---\nname: Orders\n---\nSELECT 1\n";
    let v2 = "---\nname: Orders\n---\nSELECT 2\n";
    // A new path: no file may be there.
    let out = tree::apply(root, vec![write(&rel, v1, None)], ApplyOptions::default())
        .await
        .unwrap();
    assert_eq!(out, [OpOutcome::Done]);
    assert_eq!(std::fs::read_to_string(root.join(&rel)).unwrap(), v1);
    // Again with `None`: the file is there now, so the write is stale.
    let out = tree::apply(root, vec![write(&rel, v2, None)], ApplyOptions::default())
        .await
        .unwrap();
    assert_eq!(out, [OpOutcome::Stale]);
    // The wrong hash is stale; the right one writes.
    let out = tree::apply(
        root,
        vec![write(&rel, v2, Some(&hash(&rel, v2)))],
        ApplyOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(out, [OpOutcome::Stale]);
    let out = tree::apply(
        root,
        vec![write(&rel, v2, Some(&hash(&rel, v1)))],
        ApplyOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(out, [OpOutcome::Done]);
    assert_eq!(std::fs::read_to_string(root.join(&rel)).unwrap(), v2);
    // No temp file is left behind.
    let names: Vec<String> = std::fs::read_dir(root.join(DIR).join("queries"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["orders.sql"]);
    // A delete keeps the bytes, so they can be put back.
    let out = tree::apply(
        root,
        vec![FileOp::Delete {
            rel_path: rel.clone(),
            expect_hash: Some(hash(&rel, v2)),
        }],
        ApplyOptions::default(),
    )
    .await
    .unwrap();
    let OpOutcome::Deleted(bytes) = &out[0] else {
        panic!("{out:?}")
    };
    assert!(!root.join(&rel).exists());
    tree::restore(root, &rel, bytes.clone()).await.unwrap();
    assert_eq!(std::fs::read_to_string(root.join(&rel)).unwrap(), v2);
}

#[tokio::test]
async fn a_sequence_stops_at_the_first_failure_and_the_hook_fails_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let a = format!("{DIR}/queries/a.sql");
    let b = format!("{DIR}/queries/b.sql");
    let failing = root.join(&a);
    let hook: tree::WriteHook = Arc::new(move |p: &Path| {
        if p == failing {
            Err("EACCES".to_string())
        } else {
            Ok(())
        }
    });
    let opts = ApplyOptions {
        stop_at_first_failure: true,
        hook: Some(hook.clone()),
    };
    let out = tree::apply(
        root,
        vec![write(&a, "SELECT 1\n", None), write(&b, "SELECT 2\n", None)],
        opts,
    )
    .await
    .unwrap();
    assert!(matches!(out[0], OpOutcome::Failed(_)), "{out:?}");
    assert_eq!(out[1], OpOutcome::NotRun);
    assert!(!root.join(&b).exists());
    // Independent writes go on past a failure.
    let opts = ApplyOptions {
        stop_at_first_failure: false,
        hook: Some(hook),
    };
    let out = tree::apply(
        root,
        vec![write(&a, "SELECT 1\n", None), write(&b, "SELECT 2\n", None)],
        opts,
    )
    .await
    .unwrap();
    assert!(matches!(out[0], OpOutcome::Failed(_)));
    assert_eq!(out[1], OpOutcome::Done);
}

#[tokio::test]
async fn project_dirs_read_their_names_and_skip_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = tmp.path();
    put(
        root,
        ".seaquel/projects/team/project.yaml",
        "name: Team A\n",
    );
    std::fs::create_dir_all(root.join(".seaquel/projects/bare")).unwrap();
    put(
        root,
        ".seaquel/projects/yml/project.yml",
        "name: From yml\ndescription: d\n",
    );
    symlink(outside.path(), root.join(".seaquel/projects/linked")).unwrap();
    let dirs = tree::project_dirs(root).await.unwrap();
    let got: Vec<(String, String)> = dirs
        .iter()
        .map(|d| (d.dir.clone(), d.name.clone()))
        .collect();
    assert_eq!(
        got,
        [
            ("bare".to_string(), "bare".to_string()),
            ("team".to_string(), "Team A".to_string()),
            ("yml".to_string(), "From yml".to_string()),
        ]
    );
    assert_eq!(dirs[2].description.as_deref(), Some("d"));
    // Probe fix 8: the symlinked directory is named as skipped.
    let listing = tree::project_dirs_listing(root).await.unwrap();
    assert_eq!(listing.dirs.len(), 3);
    let skipped: Vec<(String, SkipReason)> = listing
        .skipped
        .iter()
        .map(|s| (s.rel_path.clone(), s.why))
        .collect();
    assert_eq!(skipped, [("linked".to_string(), SkipReason::Symlink)]);
    // No `.seaquel` at all: none.
    let empty = tempfile::tempdir().unwrap();
    assert!(tree::project_dirs(empty.path()).await.unwrap().is_empty());
}

#[tokio::test]
async fn reading_a_file_never_follows_a_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = tmp.path();
    put(root, &format!("{DIR}/queries/a.sql"), "SELECT 1\n");
    std::fs::write(outside.path().join("s"), "secret").unwrap();
    symlink(
        outside.path().join("s"),
        root.join(DIR).join("queries/b.sql"),
    )
    .unwrap();
    assert_eq!(
        tree::read_file(root, &format!("{DIR}/queries/a.sql"))
            .await
            .unwrap()
            .as_deref(),
        Some("SELECT 1\n")
    );
    assert!(tree::read_file(root, &format!("{DIR}/queries/b.sql"))
        .await
        .is_err());
    assert_eq!(
        tree::read_file(root, &format!("{DIR}/queries/none.sql"))
            .await
            .unwrap(),
        None
    );
    let taken = tree::paths_under(root, DIR).await.unwrap();
    assert!(taken.contains(&format!("{DIR}/queries/a.sql")));
    assert!(taken.contains(&format!("{DIR}/queries/b.sql")));
}

#[tokio::test]
async fn conflicts_are_read_from_the_index() {
    let tmp = tempfile::tempdir().unwrap();
    // Not a repo: no conflicts.
    assert!(!tree::conflicted(tmp.path()).await.unwrap());
    let repo = git2::Repository::init(tmp.path()).unwrap();
    assert!(!tree::conflicted(tmp.path()).await.unwrap());
    // Stage a conflict by hand: three stages of one path.
    put(tmp.path(), "f.txt", "x\n");
    let mut index = repo.index().unwrap();
    let blob = repo.blob(b"x\n").unwrap();
    for stage in 1..=3u16 {
        let entry = git2::IndexEntry {
            ctime: git2::IndexTime::new(0, 0),
            mtime: git2::IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o100644,
            uid: 0,
            gid: 0,
            file_size: 2,
            id: blob,
            flags: stage << 12 | 5,
            flags_extended: 0,
            path: b"f.txt".to_vec(),
        };
        index.add(&entry).unwrap();
    }
    index.write().unwrap();
    assert!(tree::conflicted(tmp.path()).await.unwrap());
}

/// Review M4: a rewrite keeps the file's mode, and a temp file a crash left
/// behind is removed at the next scan, so a commit never stages it.
#[tokio::test]
async fn a_rewrite_keeps_the_mode_and_a_scan_clears_stale_temp_files() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let rel = format!("{DIR}/queries/orders.sql");
    let v1 = "---\nname: Orders\n---\nSELECT 1\n";
    put(root, &rel, v1);
    std::fs::set_permissions(root.join(&rel), std::fs::Permissions::from_mode(0o640)).unwrap();
    let out = tree::apply(
        root,
        vec![write(
            &rel,
            "---\nname: Orders\n---\nSELECT 2\n",
            Some(&hash(&rel, v1)),
        )],
        ApplyOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(out, [OpOutcome::Done]);
    let mode = std::fs::metadata(root.join(&rel))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o640);

    let stale = root
        .join(DIR)
        .join("queries/.orders.sql.4242-7.seaquel-tmp");
    std::fs::write(&stale, "half a write").unwrap();
    let other = root.join(DIR).join("queries/.keep-me");
    std::fs::write(&other, "not ours").unwrap();
    // Only a scan that asks (a sync, under the repo lock) clears them.
    tree::scan(root, DIR, ScanBounds::default()).await.unwrap();
    assert!(stale.exists(), "a preview scan removed a file");
    let bounds = ScanBounds {
        clear_temp_files: true,
        ..ScanBounds::default()
    };
    tree::scan(root, DIR, bounds).await.unwrap();
    assert!(!stale.exists(), "a stale temp file survived the scan");
    assert!(
        other.exists(),
        "the scan removed a file that isn't Seaquel's"
    );
}
