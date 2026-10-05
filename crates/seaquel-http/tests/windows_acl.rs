//! Windows: what an install sets on the helper's folders and file.
//! A protected DACL of the user
//! and SYSTEM on `bin`, `duckdb`, the version folder and the file; the app's
//! folder (`root`) is left as the profile made it unless another principal
//! can write it, and refused when someone else owns it. Run by CI's Windows
//! leg; nothing here runs elsewhere.
#![cfg(windows)]

use std::path::Path;
use std::process::Command;

use seaquel_http::release_asset::testing::{digest, gzip};
use seaquel_http::release_asset::{install_file, prepare_target, InstallErrorKind, InstallTarget};
use seaquel_runtime::acl::{self, Level, Problem};

const VERSION: &str = "2026.10.1";

fn target(root: &Path) -> InstallTarget {
    InstallTarget {
        root: root.join("app.seaquel.test"),
        folders: vec!["bin".into(), "duckdb".into(), VERSION.into()],
        file_name: "seaquel-duckdb.exe".into(),
    }
}

fn icacls(path: &Path, args: &[&str]) {
    let out = Command::new("icacls")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "icacls {args:?} failed (setting an owner needs the elevated runner): {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A gzipped stand-in in `dir` and its SHA-256.
fn asset(dir: &Path) -> (std::path::PathBuf, String) {
    let gz = gzip(b"MZ a helper stand-in");
    let path = dir.join("helper.gz");
    std::fs::write(&path, &gz).unwrap();
    (path, digest(&gz)["sha256:".len()..].to_string())
}

/// `bin`, `duckdb`, the version folder and the file.
fn privates(t: &InstallTarget) -> Vec<std::path::PathBuf> {
    let bin = t.root.join("bin");
    let duckdb = bin.join("duckdb");
    vec![bin, duckdb, t.dir(), t.path()]
}

fn assert_private(path: &Path) {
    let user = acl::current_user().unwrap();
    let entry = acl::inspect(path).unwrap();
    assert!(
        entry.security.is_private(&user),
        "{path:?}: {:?}",
        entry.security
    );
}

#[test]
fn an_install_leaves_each_folder_and_the_file_private() {
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let (gz, sha) = asset(tmp.path());
    install_file(&gz, &sha, &t).unwrap();
    for path in privates(&t) {
        assert_private(&path);
    }
    // The app's folder keeps the profile's DACL: it is safe as it is.
    let user = acl::current_user().unwrap();
    let root = acl::inspect(&t.root).unwrap();
    assert_eq!(root.problem(&user, true, Level::Root), None);
    assert!(!root.security.protected, "left as inherited");
}

/// A version folder given `Everyone:(M)` is unsafe until an install
/// (or the offline repair, `prepare_target`) makes it private again.
#[test]
fn a_loosened_folder_is_repaired() {
    let user = acl::current_user().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let (gz, sha) = asset(tmp.path());
    install_file(&gz, &sha, &t).unwrap();
    for folder in &privates(&t)[..3] {
        icacls(folder, &["/grant", "*S-1-1-0:(M)"]);
        assert_eq!(
            acl::inspect(folder)
                .unwrap()
                .problem(&user, true, Level::Private),
            Some(Problem::OthersCanWrite)
        );
    }
    prepare_target(&t).unwrap();
    for folder in &privates(&t)[..3] {
        assert_private(folder);
    }
    icacls(&t.dir(), &["/grant", "*S-1-1-0:(M)"]);
    install_file(&gz, &sha, &t).unwrap();
    assert_private(&t.dir());
}

/// The file itself loosened: a new install puts a private file in place.
#[test]
fn a_loosened_file_is_replaced_by_a_private_one() {
    let user = acl::current_user().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let (gz, sha) = asset(tmp.path());
    install_file(&gz, &sha, &t).unwrap();
    icacls(&t.path(), &["/grant", "*S-1-1-0:(M)"]);
    assert_eq!(
        acl::inspect(&t.path())
            .unwrap()
            .problem(&user, false, Level::Private),
        Some(Problem::OthersCanWrite)
    );
    install_file(&gz, &sha, &t).unwrap();
    assert_private(&t.path());
}

/// A shared app folder (`SEAQUEL_DATA_DIR` where `Users` may modify) is
/// repaired in place (decision 3): `Users` keeps reading and adding, loses
/// delete; the other entries stay; the result is protected.
#[test]
fn a_root_others_can_replace_in_is_repaired_in_place() {
    let user = acl::current_user().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    std::fs::create_dir(&t.root).unwrap();
    icacls(&t.root, &["/grant", "*S-1-5-32-545:(OI)(CI)(M)"]);
    let before = acl::inspect(&t.root).unwrap();
    assert_eq!(
        before.problem(&user, true, Level::Root),
        Some(Problem::OthersCanWrite)
    );
    prepare_target(&t).unwrap();
    let root = acl::inspect(&t.root).unwrap();
    assert_eq!(root.problem(&user, true, Level::Root), None);
    assert!(root.security.protected);
    assert!(!root.security.is_private(&user), "not replaced wholesale");
    let aces = root.security.dacl.unwrap();
    assert!(
        aces.iter().any(|a| a.sid == acl::ADMINISTRATORS),
        "{aces:?}"
    );
    let users = aces
        .iter()
        .find(|a| a.sid == "S-1-5-32-545")
        .expect("Users keeps an entry");
    assert_eq!(users.mask & acl::rights::REPLACE, 0);
    assert_ne!(users.mask & acl::rights::FILE_WRITE_DATA, 0, "adding stays");
    // And below it, everything is private as usual.
    for folder in &privates(&t)[..3] {
        assert_private(folder);
    }
}

/// Others adding files to the app's folder is left alone.
#[test]
fn a_root_others_may_only_add_to_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    std::fs::create_dir(&t.root).unwrap();
    icacls(&t.root, &["/grant", "*S-1-5-11:(OI)(CI)(RX,WD,AD)"]);
    let before = acl::inspect(&t.root).unwrap().security;
    prepare_target(&t).unwrap();
    assert_eq!(acl::inspect(&t.root).unwrap().security, before);
}

/// Another principal's folder is refused, never changed (as Unix refuses
/// another user's). Administrators and SYSTEM as owner are accepted.
#[test]
fn a_folder_owned_by_someone_else_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    std::fs::create_dir(&t.root).unwrap();
    icacls(&t.root, &["/setowner", "*S-1-5-32-544"]);
    prepare_target(&t).expect("Administrators may own it");
    let bin = t.root.join("bin");
    icacls(&bin, &["/setowner", "*S-1-5-18"]);
    prepare_target(&t).expect("SYSTEM may own it");
    icacls(&bin, &["/setowner", "*S-1-5-32-545"]);
    let before = acl::inspect(&bin).unwrap().security;
    let e = prepare_target(&t).unwrap_err();
    assert_eq!(e.kind, InstallErrorKind::UnsafeFolder, "{e}");
    assert!(e.message.contains("SEAQUEL_DATA_DIR"), "{e}");
    assert_eq!(acl::inspect(&bin).unwrap().security, before, "unchanged");
}

/// A junction where a folder belongs is refused and not followed.
#[test]
fn a_junction_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    std::fs::create_dir(&t.root).unwrap();
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let out = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(t.root.join("bin"))
        .arg(&elsewhere)
        .output()
        .unwrap();
    assert!(out.status.success(), "mklink /J");
    let e = prepare_target(&t).unwrap_err();
    assert_eq!(e.kind, InstallErrorKind::UnsafeFolder, "{e}");
    assert!(!acl::inspect(&elsewhere).unwrap().security.protected);
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
}
