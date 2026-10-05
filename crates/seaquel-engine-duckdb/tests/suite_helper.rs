//! Where the suites find the helper (`common/engine.rs`): the variable,
//! else the binary beside the test binary, else a failure naming the
//! build, never a skip.

use std::ffi::OsString;

#[path = "common/engine.rs"]
mod engine;

fn deps(target: &std::path::Path) -> std::path::PathBuf {
    let deps = target.join("debug").join("deps");
    std::fs::create_dir_all(&deps).unwrap();
    deps.join("values-0123456789abcdef")
}

#[test]
fn the_variable_wins() {
    let dir = tempfile::tempdir().unwrap();
    let got = engine::helper_from(Some(OsString::from("/x/seaquel-duckdb")), &deps(dir.path()));
    assert_eq!(got.unwrap(), std::path::PathBuf::from("/x/seaquel-duckdb"));
}

#[test]
fn the_built_binary_beside_the_test_is_next() {
    let dir = tempfile::tempdir().unwrap();
    let exe = deps(dir.path());
    let bin = dir
        .path()
        .join("debug")
        .join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&bin, b"").unwrap();
    assert_eq!(engine::helper_from(None, &exe).unwrap(), bin);
}

#[test]
fn without_either_the_suite_fails_with_the_build_hint() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine::helper_from(None, &deps(dir.path())).unwrap_err();
    assert!(e.contains("cargo build -p seaquel-duckdb"), "{e}");
    assert!(e.contains(engine::HELPER_VAR), "{e}");
}
