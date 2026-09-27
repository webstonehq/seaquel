//! `ConnectConfig::restricted` (phase 4 security review): the MCP server's
//! own DuckDB instances open with `enable_external_access`,
//! `autoinstall_known_extensions` and `autoload_known_extensions` off and
//! `lock_configuration` on. A query then reaches the database file and
//! nothing else: no other file, no other database, no extension.
//!
//! Without the flag nothing changes (the last tests): the editor's DuckDB
//! keeps its file functions, `ATTACH` and `SET`.
//!
//! Every test points `HOME` at a temp dir first, so nothing DuckDB might
//! write under `~/.duckdb` lands in the real one. `restricted_home.rs`
//! checks that a restricted instance writes nothing there.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};

use seaquel_engine::{ConnectConfig, Driver, Value};
use seaquel_engine_testkit::scratch_name;

static HOME: Once = Once::new();

/// Points `HOME` at a temp dir, once per process, before any test opens a
/// database.
fn temp_home() {
    HOME.call_once(|| {
        let home = std::env::temp_dir().join(scratch_name("seaquel-duckdb-home-"));
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("HOME", &home);
    });
}

/// A temp dir holding a DuckDB file with a table, a text file and an SQLite
/// file next to it. Removed on drop.
struct Files {
    dir: PathBuf,
}

impl Files {
    async fn new() -> Self {
        temp_home();
        let dir = std::env::temp_dir().join(scratch_name("seaquel-duckdb-restricted-"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("secret.txt"), "top secret\n").unwrap();
        std::fs::write(dir.join("rows.csv"), "a,b\n1,2\n").unwrap();
        // The header is enough: every read below is refused before parsing.
        let mut sqlite = b"SQLite format 3\0".to_vec();
        sqlite.resize(4096, 0);
        std::fs::write(dir.join("other.sqlite"), sqlite).unwrap();
        let files = Self { dir };

        // The database, written by an unrestricted instance (as the app does).
        let d = open(&files.config(false)).await;
        d.execute(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name VARCHAR)",
            vec![],
        )
        .await
        .unwrap();
        d.execute("INSERT INTO items VALUES (1, 'one'), (2, 'two')", vec![])
            .await
            .unwrap();
        d.execute("CREATE VIEW named AS SELECT name FROM items", vec![])
            .await
            .unwrap();
        d.close().await.unwrap();
        files
    }

    fn path(&self, name: &str) -> String {
        self.dir.join(name).to_str().unwrap().to_string()
    }

    fn config(&self, restricted: bool) -> ConnectConfig {
        config(&self.path("app.duckdb"), restricted)
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn config(path: &str, restricted: bool) -> ConnectConfig {
    serde_json::from_value(serde_json::json!({
        "driver": "duckdb",
        "path": path,
        "create_if_missing": true,
        "restricted": restricted,
    }))
    .unwrap()
}

async fn open(config: &ConnectConfig) -> Arc<dyn Driver> {
    seaquel_engine_duckdb::engine()
        .open(config)
        .await
        .expect("open")
}

fn sql_str(path: &str) -> String {
    format!("'{}'", path.replace('\'', "''"))
}

/// Each of these must fail on a restricted instance, on the editor's path and
/// on the read-only one.
fn escapes(files: &Files) -> Vec<String> {
    let secret = sql_str(&files.path("secret.txt"));
    let csv = sql_str(&files.path("rows.csv"));
    let sqlite = sql_str(&files.path("other.sqlite"));
    let glob = sql_str(&format!("{}/*", files.dir.to_str().unwrap()));
    vec![
        // A file outside the database.
        format!("SELECT * FROM read_text({secret})"),
        format!("SELECT * FROM read_blob({secret})"),
        format!("SELECT * FROM read_csv({csv})"),
        format!("SELECT * FROM {csv}"),
        format!("SELECT * FROM glob({glob})"),
        // Another SQLite file.
        format!("SELECT * FROM read_blob({sqlite})"),
        format!("SELECT * FROM sqlite_scan({sqlite}, 'items')"),
        // A URL.
        "SELECT * FROM read_csv('https://example.com/x.csv')".into(),
    ]
}

/// Statements that must fail on a restricted instance through the editor's
/// path (the read-only path refuses them for other reasons already).
fn statements(files: &Files) -> Vec<String> {
    let sqlite = sql_str(&files.path("other.sqlite"));
    let other = sql_str(&files.path("other.duckdb"));
    let out = sql_str(&files.path("out.csv"));
    vec![
        format!("ATTACH {sqlite} AS lite (TYPE sqlite)"),
        format!("ATTACH {other} AS other"),
        format!("COPY items TO {out}"),
        "INSTALL json".into(),
        // Not `LOAD json`: json is linked in statically and already loaded,
        // so that LOAD is a no-op that succeeds. Parquet isn't linked.
        "LOAD parquet".into(),
        "INSTALL httpfs".into(),
        "LOAD httpfs".into(),
        // Undoing the lock.
        "SET autoload_known_extensions = true".into(),
        "SET autoinstall_known_extensions = true".into(),
        "SET GLOBAL autoload_known_extensions = true".into(),
        "SET enable_external_access = true".into(),
        "SET lock_configuration = false".into(),
        "RESET lock_configuration".into(),
    ]
}

#[tokio::test]
async fn a_restricted_instance_reaches_no_file_but_its_own() {
    let files = Files::new().await;
    let d = open(&files.config(true)).await;

    for sql in escapes(&files) {
        let e = d.query(&sql, vec![]).await;
        assert!(e.is_err(), "{sql} ran: {:?}", e.unwrap());
        let e = d.query_read_only(&sql, vec![], Some(10)).await;
        assert!(e.is_err(), "{sql} ran read-only: {:?}", e.unwrap());
    }
    for sql in statements(&files) {
        let e = d.execute(&sql, vec![]).await;
        assert!(e.is_err(), "{sql} ran");
    }
    // Session settings aren't locked, but none of them reaches a file: the
    // profiler's output file and the query log are refused.
    let profile = files.path("profile.json");
    let log = files.path("queries.log");
    for sql in [
        format!("SET profiling_output = {}", sql_str(&profile)),
        format!("SET log_query_path = {}", sql_str(&log)),
        format!("PRAGMA log_query_path = {}", sql_str(&log)),
    ] {
        let _ = d.execute(&sql, vec![]).await;
    }
    let _ = d.execute("PRAGMA enable_profiling = 'json'", vec![]).await;
    let _ = d.execute("SET enable_profiling = 'json'", vec![]).await;
    d.query("SELECT count(*) FROM items", vec![]).await.unwrap();
    let _ = d
        .query_read_only(
            "SELECT * FROM enable_profiling(save_location := 'x.json')",
            vec![],
            None,
        )
        .await;
    assert!(!Path::new(&profile).exists(), "profiling wrote {profile}");
    assert!(!Path::new(&log).exists(), "the query log wrote {log}");

    // Nothing was written or attached.
    assert!(!Path::new(&files.path("out.csv")).exists());
    assert!(!Path::new(&files.path("other.duckdb")).exists());
    let dbs = d
        .query(
            "SELECT database_name FROM duckdb_databases() WHERE NOT internal",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(dbs.rows, vec![vec![Value::Text("app".into())]]);

    // The settings, as DuckDB reports them after all that.
    let settings = d
        .query(
            "SELECT current_setting('enable_external_access'), \
                    current_setting('autoinstall_known_extensions'), \
                    current_setting('autoload_known_extensions'), \
                    current_setting('lock_configuration'), \
                    current_setting('arrow_lossless_conversion')",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(
        settings.rows,
        vec![vec![
            Value::Bool(false),
            Value::Bool(false),
            Value::Bool(false),
            Value::Bool(true),
            Value::Bool(true),
        ]]
    );
    // Even listing extensions is refused: it reads the extension directory.
    assert!(d
        .query("SELECT * FROM duckdb_extensions()", vec![])
        .await
        .is_err());
    d.close().await.unwrap();
}

#[tokio::test]
async fn a_restricted_instance_still_uses_its_own_database() {
    let files = Files::new().await;
    let d = open(&files.config(true)).await;

    let rows = d
        .query("SELECT id, name FROM items ORDER BY id", vec![])
        .await
        .unwrap();
    assert_eq!(
        rows.rows,
        vec![
            vec![Value::Int(1), Value::Text("one".into())],
            vec![Value::Int(2), Value::Text("two".into())],
        ]
    );
    let r = d
        .query_read_only("SELECT name FROM named ORDER BY name", vec![], Some(1))
        .await
        .unwrap();
    assert_eq!(r.rows, vec![vec![Value::Text("one".into())]]);
    assert!(r.truncated);

    // What the MCP tools read.
    let info = d
        .query(
            "SELECT table_name, table_type FROM information_schema.tables \
             WHERE table_schema = 'main' ORDER BY 1",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(info.rows.len(), 2, "{:?}", info.rows);
    let described = d.query("DESCRIBE items", vec![]).await.unwrap();
    assert_eq!(described.rows.len(), 2);
    let r = d
        .query_read_only("DESCRIBE items", vec![], Some(10))
        .await
        .unwrap();
    assert_eq!(r.rows.len(), 2);
    assert_eq!(d.list_schemas().await.unwrap(), vec!["main".to_string()]);
    let tables = d.schema_tables().await.unwrap();
    assert!(tables.iter().any(|t| t.name == "items"), "{tables:?}");
    let (columns, _) = d.table_metadata("main", "items").await.unwrap();
    assert_eq!(columns.len(), 2);
    assert!(columns[0].is_primary_key);
    let plan = d
        .explain(
            "SELECT name FROM items WHERE id = ?",
            vec![Value::Int(1)],
            false,
        )
        .await
        .unwrap();
    assert!(!plan.is_analyze);
    d.explain("SELECT count(*) FROM items", vec![], true)
        .await
        .unwrap();
    d.statistics().await.unwrap();

    // Writes to the database itself still work, and survive a reopen (the WAL
    // and the checkpoint are the database's own files).
    d.execute("INSERT INTO items VALUES (3, 'three')", vec![])
        .await
        .unwrap();
    d.execute("CREATE TABLE more AS SELECT * FROM range(3) r(i)", vec![])
        .await
        .unwrap();
    d.close().await.unwrap();
    drop(d);
    let d = open(&files.config(true)).await;
    let n = d
        .query(
            "SELECT (SELECT count(*) FROM items), (SELECT count(*) FROM more)",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(n.rows, vec![vec![Value::Int(3), Value::Int(3)]]);
    d.close().await.unwrap();
}

#[tokio::test]
async fn a_restricted_in_memory_instance_is_locked_down_too() {
    let files = Files::new().await;
    let d = open(&config(":memory:", true)).await;
    d.execute("CREATE TABLE t AS SELECT 1 AS a", vec![])
        .await
        .unwrap();
    assert_eq!(
        d.query("SELECT a FROM t", vec![]).await.unwrap().rows,
        vec![vec![Value::Int(1)]]
    );
    for sql in escapes(&files) {
        assert!(d.query(&sql, vec![]).await.is_err(), "{sql} ran");
    }
    for sql in statements(&files) {
        assert!(d.execute(&sql, vec![]).await.is_err(), "{sql} ran");
    }
}

/// The JSON functions are linked into the binary (the workspace's duckdb
/// `json` feature) and loaded at startup, so they work with autoload off,
/// on both paths, restricted or not.
#[tokio::test]
async fn json_functions_work_restricted_or_not() {
    let files = Files::new().await;
    for restricted in [true, false] {
        let d = open(&files.config(restricted)).await;
        let sql = "SELECT json_extract('{\"a\": {\"b\": 7}}', '$.a.b')::INTEGER AS b, \
                          '{\"x\": \"y\"}'::JSON ->> 'x' AS x, \
                          json_array_length('[1, 2, 3]') AS n, \
                          to_json({k: 1})::VARCHAR AS j";
        let expected = vec![vec![
            Value::Int(7),
            Value::Text("y".into()),
            Value::Int(3),
            Value::Text("{\"k\":1}".into()),
        ]];
        let r = d.query(sql, vec![]).await.unwrap_or_else(|e| {
            panic!("restricted={restricted}: {e:?}");
        });
        assert_eq!(r.rows, expected, "restricted={restricted}");
        let r = d.query_read_only(sql, vec![], Some(10)).await.unwrap();
        assert_eq!(r.rows, expected, "restricted={restricted}, read-only");
        // A JSON column in the database reads back as JSON.
        let r = d.query("SELECT '[1,2]'::JSON AS j", vec![]).await.unwrap();
        assert_eq!(r.rows.len(), 1, "restricted={restricted}");
        d.close().await.unwrap();
    }
    // Nothing was installed for them, restricted or not.
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert!(
        !home.join(".duckdb").exists(),
        "DuckDB wrote under the temp HOME {}",
        home.display()
    );
}

/// The ICU extension (time zones, `current_setting('TimeZone')`,
/// TIMESTAMPTZ arithmetic) is not linked in: duckdb-rs's `icu` feature
/// needs `bundled-cmake`, which can't build from the crates.io package
/// (it has no `duckdb-sources`). A restricted instance can't autoload it,
/// so these fail there: `current_setting` with DuckDB's "icu extension"
/// message, the arithmetic as a binder error (no `+` for TIMESTAMPTZ and
/// INTERVAL without icu). The
/// unrestricted side isn't run: it would download icu from the network.
/// When icu gets linked in, this test flips.
#[tokio::test]
async fn time_zone_functions_need_icu_which_a_restricted_instance_lacks() {
    let files = Files::new().await;
    let d = open(&files.config(true)).await;
    for (sql, message) in [
        ("SELECT current_setting('TimeZone')", "icu"),
        (
            "SELECT TIMESTAMPTZ '2024-01-01 00:00:00+00' + INTERVAL 1 DAY",
            "+(TIMESTAMP WITH TIME ZONE, INTERVAL)",
        ),
    ] {
        let e =
            d.query(sql, vec![]).await.err().unwrap_or_else(|| {
                panic!("{sql} ran: is icu linked in now? Then update this test")
            });
        assert!(e.message.contains(message), "{sql}: {}", e.message);
        let e = d.query_read_only(sql, vec![], Some(10)).await.err();
        assert!(e.is_some(), "{sql} ran read-only");
    }
    d.close().await.unwrap();
}

/// A restricted instance still refuses a file that doesn't exist yet the
/// usual way.
#[tokio::test]
async fn a_restricted_missing_file_is_not_created() {
    temp_home();
    let dir = std::env::temp_dir().join(scratch_name("seaquel-duckdb-restricted-"));
    let path = dir.join("missing.duckdb");
    let mut c = config(path.to_str().unwrap(), true);
    c.create_if_missing = None;
    let e = seaquel_engine_duckdb::engine()
        .open(&c)
        .await
        .err()
        .unwrap();
    assert_eq!(e.code, "FILE_NOT_FOUND");
    assert!(!dir.exists());
}

/// Without the flag (and with it `false`), the instance is the one the
/// editor has always had: file functions, `SET` and `ATTACH` work.
#[tokio::test]
async fn an_unrestricted_instance_is_unchanged() {
    let files = Files::new().await;
    let unset = {
        let mut c = files.config(false);
        c.restricted = None;
        c
    };
    for config in [unset, files.config(false)] {
        let d = open(&config).await;
        let secret = sql_str(&files.path("secret.txt"));
        let r = d
            .query(&format!("SELECT content FROM read_text({secret})"), vec![])
            .await
            .unwrap();
        assert_eq!(r.rows, vec![vec![Value::Text("top secret\n".into())]]);
        // The read-only path too: its known gap in the app.
        let r = d
            .query_read_only(
                &format!(
                    "SELECT size FROM read_blob({})",
                    sql_str(&files.path("other.sqlite"))
                ),
                vec![],
                None,
            )
            .await
            .unwrap();
        assert_eq!(r.rows, vec![vec![Value::Int(4096)]]);
        let csv = d
            .query(
                &format!("SELECT a, b FROM {}", sql_str(&files.path("rows.csv"))),
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(csv.rows, vec![vec![Value::Int(1), Value::Int(2)]]);

        let settings = d
            .query(
                "SELECT current_setting('enable_external_access'), \
                        current_setting('autoinstall_known_extensions'), \
                        current_setting('autoload_known_extensions'), \
                        current_setting('lock_configuration'), \
                        current_setting('arrow_lossless_conversion')",
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(
            settings.rows,
            vec![vec![
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(false),
                Value::Bool(true),
            ]]
        );
        d.execute("SET threads = 2", vec![]).await.unwrap();
        let other = sql_str(&files.path("other.duckdb"));
        d.execute(&format!("ATTACH {other} AS other"), vec![])
            .await
            .unwrap();
        d.execute("DETACH other", vec![]).await.unwrap();
        d.close().await.unwrap();
    }
}
