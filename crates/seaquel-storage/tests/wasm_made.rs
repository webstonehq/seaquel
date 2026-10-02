//! A metadata file made by the browser's storage (phase 8 Decision 4)
//! opens natively with nothing pending: `tests/wasm.rs` writes
//! `fixtures/wasm-made/meta.db` from wasm32 (with
//! `SEAQUEL_RECORD_WASM_FIXTURE=1`), and this opens it read-only, which
//! refuses any file with a baseline step, a migration or a data step left
//! to do (`STORAGE_NEEDS_UPGRADE`), and refuses a recorded migration whose
//! checksum differs (`VersionMismatch`).
//!
//! A migration or data step added after the fixture was made is pending in
//! it, which says nothing about the browser's migrator; the test then
//! checks everything the fixture did record and asks for a fresh fixture.

#![cfg(not(target_arch = "wasm32"))]

use seaquel_storage::{app_state, projects, Storage, StorageError, StorageOptions};
use sqlx::{Connection, SqliteConnection};

/// A stale fixture only warns locally, but fails in CI or with
/// `SEAQUEL_STRICT_FIXTURES=1`, so it can't go stale unnoticed.
fn strict() -> bool {
    let set = |k: &str| std::env::var(k).is_ok_and(|v| !v.is_empty() && v != "0" && v != "false");
    set("CI") || set("SEAQUEL_STRICT_FIXTURES")
}

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/wasm-made/meta.db"
);

#[tokio::test]
async fn a_file_made_in_wasm_opens_with_nothing_pending() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    std::fs::copy(FIXTURE, &path).expect("the wasm-made fixture exists");

    // Every migration the browser recorded is sqlx's, byte for byte.
    let ours = sqlx::migrate!("./migrations");
    let mut raw = SqliteConnection::connect(&format!("sqlite://{}?mode=ro", path.display()))
        .await
        .unwrap();
    let recorded: Vec<(i64, String, Vec<u8>, bool)> = sqlx::query_as(
        "SELECT version, description, checksum, success FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut raw)
    .await
    .unwrap();
    raw.close().await.unwrap();
    assert!(!recorded.is_empty());
    for (version, description, checksum, success) in &recorded {
        let m = ours
            .iter()
            .find(|m| m.version == *version)
            .unwrap_or_else(|| panic!("migration {version} is one of this build's"));
        assert_eq!(description, &*m.description, "migration {version}");
        assert_eq!(checksum.as_slice(), &*m.checksum, "migration {version}");
        assert!(success, "migration {version}");
    }
    let current = recorded.len() == ours.iter().count();

    match Storage::open(
        &path,
        StorageOptions {
            read_only: true,
            ..StorageOptions::default()
        },
    )
    .await
    {
        Ok(st) => {
            let p = projects::get(&st, "p-wasm").await.unwrap().unwrap();
            assert_eq!(p.name, "Straße 東京");
            assert_eq!(
                app_state::get(&st, "madeBy").await.unwrap().as_deref(),
                Some("wasm")
            );
            st.close().await;
        }
        Err(e) if strict() => panic!(
            "the wasm-made fixture is stale or unreadable (CI or SEAQUEL_STRICT_FIXTURES is set; \
             regenerate it, tests/fixtures/README.md): {e}"
        ),
        Err(e @ StorageError::DataStepPending { .. }) => {
            eprintln!(
                "the wasm-made fixture predates a data step; regenerate it (tests/fixtures/README.md): {e}"
            );
        }
        Err(e) if !current && e.code() == "STORAGE_NEEDS_UPGRADE" => {
            eprintln!(
                "the wasm-made fixture predates a migration; regenerate it (tests/fixtures/README.md): {e}"
            );
        }
        Err(e) => panic!("a file the browser made should open read-only: {e}"),
    }

    // The desktop's writable open takes it as it is.
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let p = projects::get(&st, "p-wasm").await.unwrap().unwrap();
    assert_eq!(p.name, "Straße 東京");
    let mut tx = st.write().await.unwrap();
    projects::delete_with_orphans(&mut tx, "p-wasm")
        .await
        .unwrap();
    tx.commit().await.unwrap();
}
