//! A temp copy of the seeded DuckDB file, shared by `smoke.rs` and
//! `read_only.rs`.

use std::path::{Path, PathBuf};

use seaquel_engine::ConnectConfig;
use seaquel_engine_testkit::scratch_name;

/// A copy of the seeded database (and its WAL) in a fresh directory, under
/// the same file name: DuckDB names the catalog after it, and the scratch
/// setup runs `USE seaquel_test`. Removed on drop.
///
/// The source is `SEAQUEL_TEST_DUCKDB_FILE` when set (a file named
/// `seaquel_test.duckdb`), else the seeded file in `e2e/test-databases`.
/// Missing, the tests skip, or fail under `SEAQUEL_TEST_REQUIRE_ENGINES`
/// (CI seeds it with `node e2e/test-databases/seed.mjs duckdb`).
pub struct SeededCopy(pub PathBuf);

impl SeededCopy {
    pub fn new() -> Option<Self> {
        let source = std::env::var_os("SEAQUEL_TEST_DUCKDB_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../e2e/test-databases/duckdb/seaquel_test.duckdb")
            });
        if !source.exists() {
            let msg = format!(
                "{} isn't seeded (node e2e/test-databases/seed.mjs duckdb)",
                source.display()
            );
            if std::env::var_os(seaquel_engine_testkit::REQUIRE_ENGINES).is_some() {
                panic!("{msg}");
            }
            eprintln!("skipping: {msg}");
            return None;
        }
        let dir = std::env::temp_dir().join(scratch_name("seaquel-duckdb-"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(&source, dir.join("seaquel_test.duckdb")).unwrap();
        let wal = source.with_extension("duckdb.wal");
        if wal.exists() {
            std::fs::copy(&wal, dir.join("seaquel_test.duckdb.wal")).unwrap();
        }
        Some(Self(dir))
    }

    pub fn config(&self) -> ConnectConfig {
        serde_json::from_value(serde_json::json!({
            "driver": "duckdb",
            "path": self.0.join("seaquel_test.duckdb").to_str().unwrap(),
        }))
        .unwrap()
    }
}

impl Drop for SeededCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
