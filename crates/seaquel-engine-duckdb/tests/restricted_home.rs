//! A restricted DuckDB instance (`ConnectConfig::restricted`) writes nothing
//! under `HOME`: an unrestricted one autoinstalls a known extension a query
//! needs into `~/.duckdb/extensions` (json, parquet, icu, httpfs, …). In its
//! own test binary because it points `HOME` at a temp dir for the process.
//!
//! The unrestricted side isn't run here: it would download from the network.

#[path = "common/engine.rs"]
mod engine_switch;

use std::path::Path;

use seaquel_engine::ConnectConfig;
use seaquel_engine_testkit::scratch_name;

fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        out.push(path.display().to_string());
        if path.is_dir() {
            out.extend(files_under(&path));
        }
    }
    out
}

#[tokio::test]
async fn a_restricted_instance_writes_nothing_under_home() {
    let root = std::env::temp_dir().join(scratch_name("seaquel-duckdb-home-"));
    let home = root.join("home");
    let data = root.join("data");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    // Before any DuckDB instance exists in this process (and before the
    // remote driver's helper starts: it inherits the environment). DuckDB
    // reads `USERPROFILE` on Windows.
    std::env::set_var("HOME", &home);
    #[cfg(windows)]
    std::env::set_var("USERPROFILE", &home);

    let config: ConnectConfig = serde_json::from_value(serde_json::json!({
        "driver": "duckdb",
        "path": data.join("app.duckdb").to_str().unwrap(),
        "create_if_missing": true,
        "restricted": true,
    }))
    .unwrap();
    let d = engine_switch::engine().open(&config).await.expect("open");
    d.execute("CREATE TABLE t AS SELECT 1 AS a", vec![])
        .await
        .unwrap();

    // Linked in statically (the duckdb `json` feature): works, and needs
    // nothing from `~/.duckdb`.
    for sql in [
        "SELECT '{\"a\": 1}'::JSON",
        "SELECT json_extract('{\"a\": 1}', '$.a')",
    ] {
        d.query(sql, vec![]).await.unwrap();
        d.query_read_only(sql, vec![], Some(10)).await.unwrap();
    }
    // Each needs an extension this build doesn't link in (icu among them),
    // or a file.
    for sql in [
        "SELECT * FROM read_json('x.json')",
        "SELECT current_setting('TimeZone')",
        "SELECT TIMESTAMPTZ '2024-01-01 00:00:00+00' + INTERVAL 1 DAY",
        "SELECT * FROM read_parquet('x.parquet')",
        "SELECT * FROM 'https://example.com/x.parquet'",
        "SELECT now() AT TIME ZONE 'Europe/Paris'",
        "SELECT * FROM iceberg_scan('x')",
        "SELECT * FROM sqlite_scan('x.db', 't')",
    ] {
        assert!(d.query(sql, vec![]).await.is_err(), "{sql} ran");
        assert!(
            d.query_read_only(sql, vec![], Some(10)).await.is_err(),
            "{sql} ran read-only"
        );
    }
    for sql in [
        "INSTALL json",
        "FORCE INSTALL icu",
        "INSTALL httpfs FROM core",
        "LOAD parquet",
        "SET extension_directory = '.'",
        "SET home_directory = '.'",
    ] {
        assert!(d.execute(sql, vec![]).await.is_err(), "{sql} ran");
    }
    // The database still works after all that.
    let r = d.query("SELECT a FROM t", vec![]).await.unwrap();
    assert_eq!(r.rows.len(), 1);
    d.close().await.unwrap();
    drop(d);

    let written = files_under(&home);
    assert!(written.is_empty(), "written under HOME: {written:?}");
    let _ = std::fs::remove_dir_all(&root);
}
