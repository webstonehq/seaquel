//! DuckDB for Core's tests: the `seaquel-duckdb` helper, installed into a
//! folder of the test's own as a real install is laid out. Since the
//! native driver went, DuckDB
//! is reached only through the helper, as every interface reaches it.
//!
//! The helper is `SEAQUEL_TEST_DUCKDB_HELPER`, else the `seaquel-duckdb`
//! built beside the test binary (`target/<profile>/`, which `cargo test
//! --workspace` builds). With neither, the DuckDB cases are skipped, or
//! fail under `SEAQUEL_TEST_REQUIRE_ENGINES`. The variable set to a path
//! that isn't a file always fails.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use seaquel_core::{CoreBuilder, DuckdbHelper};

/// The built helper, if there is one.
pub fn built_helper() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER") {
        let path = PathBuf::from(path);
        // Set but wrong: a mistake, never a quiet skip.
        assert!(
            path.is_file(),
            "SEAQUEL_TEST_DUCKDB_HELPER names no file: {}",
            path.display()
        );
        return Some(path);
    }
    let sibling = std::env::current_exe().ok().and_then(|exe| {
        // target/<profile>/deps/<test>
        let profile = exe.parent()?.parent()?;
        let bin = profile.join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX));
        bin.is_file().then_some(bin)
    });
    if sibling.is_some() {
        return sibling;
    }
    let msg = "no DuckDB helper: set SEAQUEL_TEST_DUCKDB_HELPER or run \
               cargo build -p seaquel-duckdb";
    if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() {
        panic!("{msg}");
    }
    eprintln!("skipping DuckDB: {msg}");
    None
}

/// The app version the helper reports.
pub fn helper_version(bin: &Path) -> String {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .strip_prefix("seaquel-duckdb ")
        .unwrap()
        .to_string()
}

/// `bin/duckdb/<version>/seaquel-duckdb` in a folder of its own, 0700.
pub fn install(bin: &Path) -> (tempfile::TempDir, DuckdbHelper) {
    let dir = tempfile::Builder::new()
        .prefix("duckdb-remote-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let version = helper_version(bin);
    let root = dir.path().join("bin").join("duckdb");
    let folder = root.join(&version);
    std::fs::create_dir_all(&folder).unwrap();
    #[cfg(unix)]
    for d in [root.parent().unwrap(), &root, &folder] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let to = folder.join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX));
    if std::fs::hard_link(bin, &to).is_err() {
        std::fs::copy(bin, &to).unwrap();
    }
    (dir, DuckdbHelper { dir: root, version })
}

/// `with_default_plugins()` plus DuckDB through the helper when there is
/// one. Keep the folder for as long as the Core.
pub fn default_plugins() -> (CoreBuilder, Option<tempfile::TempDir>) {
    let builder = seaquel_core::with_default_plugins();
    match built_helper() {
        Some(bin) => {
            let (dir, helper) = install(&bin);
            (builder.duckdb_helper(helper), Some(dir))
        }
        None => (builder, None),
    }
}
