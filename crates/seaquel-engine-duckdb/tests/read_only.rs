//! `query_read_only` (AI safety, Task 4): every attack goes through the
//! read-only path and a normal session then checks it left no trace. Runs on
//! an in-memory database and on a temp copy of the seeded file (skipped when
//! it hasn't been seeded; `npm run e2e:db:seed`).
//!
//! The mechanism: each call gets its own connection (`try_clone()`, dropped
//! after the call), `BEGIN TRANSACTION READ ONLY`, the SQL as the one
//! parameter of `SELECT * FROM query(?) LIMIT <cap + 1>`, `ROLLBACK`.
//!
//! **The read-only session doesn't follow the user's.** It's a fresh
//! connection every time, so what the user set on the editor's connection
//! doesn't apply there: `SET search_path`, `USE`, `SET TimeZone` and other
//! session settings, temp tables and temp macros, `SET VARIABLE`. Global
//! settings (`SET threads`, `SET GLOBAL …`), persistent macros and attached
//! databases do apply.
//!
//! **Core runs the AI's token check first** (`seaquel_sql::read_only`), and
//! the demo runs the same check through `seaquel-wasm`. The harness calls the
//! driver directly, so the attacks below meet the driver alone. A few
//! SELECTs change state the driver can't contain; the token check refuses
//! them on DuckDB (`state_functions_are_refused_by_the_token_check`):
//! `enable_logging()` (global: `duckdb_logs` then shows the user's editor
//! SQL, `CREATE SECRET` included), `enable_profiling()`, `checkpoint()`,
//! `query()` and `json_execute_serialized_sql()`, whose SQL argument the
//! check can't read, the scanners' `postgres_execute`/`mysql_execute`/…
//! (SQL on an attached database, outside the read-only transaction), the UI
//! server and `load_aws_credentials`. A table macro the user created that
//! wraps one of them still runs (plan: Decision 1, gaps). The same check
//! admits only statements starting with SELECT or WITH, so `FROM t`,
//! `VALUES`, `SUMMARIZE`, `DESCRIBE` and `SHOW`, which the driver allows,
//! don't reach it from the AI.
//!
//! Accepted gaps (plan, "Probe results", DuckDB column), not asserted:
//!
//! - **Local file reads and network egress.** `read_text('/etc/hosts')` and
//!   `read_csv('https://…')` are SELECTs and run. `enable_external_access`
//!   and `disabled_filesystems` can't be switched back on once off, so they
//!   can't be turned off for one call. The user-facing note names this.
//! - **Extension autoinstall.** A SELECT that needs a known extension
//!   downloads and installs it (`autoinstall_known_extensions`), writing
//!   into the extension directory: reading `current_setting('TimeZone')`
//!   installed `icu` in these tests. That setting is global, so turning it
//!   off for the AI would turn it off in the editor too; the download is
//!   the same egress as `read_csv('https://…')`. `INSTALL` itself is
//!   refused (below). Plan: Follow-ups.
//! - **At the driver alone** (Core's token check refuses them):
//!   `checkpoint()` folds the WAL into the database file (no data changes;
//!   DuckDB does the same by itself past `checkpoint_threshold` and on
//!   close), and `enable_logging()` turns on logging for the whole database.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use seaquel_engine::{ConnectConfig, Driver, Value};
use seaquel_engine_testkit::{
    run_read_only, same_value, scratch_name, Attack, Check, ReadOnlySpec,
};
use seaquel_sql::read_only::{read_only_error, READ_ONLY_MESSAGE};
use seaquel_sql::SqlEngine;

#[path = "common/cells.rs"]
mod cells;
#[path = "common/seeded.rs"]
mod seeded;

use seeded::SeededCopy;

fn memory() -> ConnectConfig {
    serde_json::from_value(serde_json::json!({ "driver": "duckdb", "path": ":memory:" })).unwrap()
}

async fn open(config: &ConnectConfig) -> Arc<dyn Driver> {
    seaquel_engine_duckdb::engine()
        .open(config)
        .await
        .expect("open")
}

/// A scratch directory for the files attacks try to write. Removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(scratch_name("seaquel-duckdb-ro-"));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The read-only connection's session matches a fresh one: the settings an
/// attack could change with `SET`/`USE`/`PRAGMA` read the same through
/// `query_read_only` as through the main connection, and the read-only
/// connection has no temp table.
fn session_is_default() -> Check {
    const SETTINGS: &str = "SELECT current_database(), current_schema(), \
        current_setting('TimeZone'), current_setting('search_path'), \
        current_setting('threads'), current_setting('memory_limit'), \
        current_setting('enable_profiling'), current_setting('extension_directory'), \
        current_setting('arrow_lossless_conversion'), \
        (SELECT count(*) FROM duckdb_tables() WHERE temporary) AS temp_tables, \
        (SELECT count(*) FROM duckdb_databases()) AS databases";
    Check::custom("the read-only session is default", |driver| async move {
        let main = driver
            .query(SETTINGS, vec![])
            .await
            .map_err(|e| format!("{e:?}"))?;
        let ro = driver
            .query_read_only(SETTINGS, vec![])
            .await
            .map_err(|e| format!("{e:?}"))?;
        if main.rows == ro.rows {
            Ok(())
        } else {
            Err(format!("main {:?}, read-only {:?}", main.rows, ro.rows))
        }
    })
}

/// The main connection isn't in a transaction (a `BEGIN` there would fail)
/// and still writes.
fn main_is_not_in_a_transaction() -> Check {
    Check::custom(
        "the main connection isn't in a transaction",
        |driver| async move {
            driver
                .execute("BEGIN TRANSACTION", vec![])
                .await
                .map_err(|e| format!("BEGIN failed: {e:?}"))?;
            driver
                .execute("ROLLBACK", vec![])
                .await
                .map_err(|e| format!("ROLLBACK failed: {e:?}"))?;
            Ok(())
        },
    )
}

/// `sql` returns the one integer `n` on both connections: for state that
/// may live in either session (secrets, temp objects).
fn count_on_both(sql: &'static str, n: i64) -> Check {
    Check::custom(
        format!("{sql} is {n} on both connections"),
        move |driver| async move {
            for (which, r) in [
                ("main", driver.query(sql, vec![]).await),
                ("read-only", driver.query_read_only(sql, vec![]).await),
            ] {
                let r = r.map_err(|e| format!("{which}: {e:?}"))?;
                let got = r
                    .rows
                    .first()
                    .and_then(|r| r.first())
                    .and_then(Value::as_i64);
                if got != Some(n) {
                    return Err(format!("{which}: {:?}", r.rows));
                }
            }
            Ok(())
        },
    )
}

/// No `{name}.duckdb_extension` file anywhere under `dir` (the setup's
/// `extension_directory`). Other extensions may appear there: reading
/// `current_setting('TimeZone')` autoinstalls `icu` (see the accepted gaps).
fn no_extension_file(dir: &str, name: &'static str) -> Check {
    let file = format!("{name}.duckdb_extension");
    fn find(dir: &std::path::Path, file: &str) -> Option<PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find(&path, file) {
                    return Some(found);
                }
            } else if path.file_name().is_some_and(|f| *f == *file) {
                return Some(path);
            }
        }
        None
    }
    let dir = PathBuf::from(dir);
    Check::custom(format!("{name} wasn't installed"), move |_| {
        let found = find(&dir, &file);
        async move {
            match found {
                Some(path) => Err(format!("{} exists", path.display())),
                None => Ok(()),
            }
        }
    })
}

/// A later read-only call writes no profile to `path`: whatever the attack
/// itself wrote is removed first.
fn profiling_ended(path: String) -> Check {
    Check::custom("no later call writes a profile", move |driver| {
        let path = PathBuf::from(&path);
        async move {
            let _ = std::fs::remove_file(&path);
            driver
                .query_read_only("SELECT 42 AS answer", vec![])
                .await
                .map_err(|e| format!("{e:?}"))?;
            if path.exists() {
                Err(format!("{} was written again", path.display()))
            } else {
                Ok(())
            }
        }
    })
}

/// A table exists (1) or not (0) in any catalog, temp included.
fn table_count(name: &str) -> String {
    format!("SELECT count(*) FROM duckdb_tables() WHERE table_name = '{name}'")
}

/// The spec for one database. `wal` is the database's WAL file, for the
/// checkpoint traces (file databases only).
fn spec(dir: &Scratch, wal: Option<PathBuf>) -> ReadOnlySpec {
    let t = scratch_name("ro_t_");
    let w = scratch_name("ro_w_");
    let s = scratch_name("ro_s_");
    let x = scratch_name("ro_x_");
    let v = scratch_name("ro_v_");
    let sc = scratch_name("ro_sc_");
    let m = scratch_name("ro_m_");
    let use_schema = scratch_name("ro_use_");
    let count_t = format!("SELECT count(*) FROM {t}");
    let one_row = || Check::count(count_t.clone(), 1);
    let columns_of_t = Check::count(
        format!("SELECT count(*) FROM duckdb_columns() WHERE table_name = '{t}'"),
        1,
    );
    let no_table = |name: &str| Check::count(table_count(name), 0);
    let threads = Check::value("SELECT current_setting('threads')::BIGINT", Value::Int(3));
    let no_database = |name: &str| {
        Check::count(
            format!("SELECT count(*) FROM duckdb_databases() WHERE database_name = '{name}'"),
            0,
        )
    };
    // Refused by `query()`'s parser: the SQL never ran.
    let refused = |name: &str, sql: String| {
        Attack::new(name, sql)
            .refused_with("READ_ONLY")
            .message_contains("Expected a single SELECT statement")
    };
    // A checkpoint folds the WAL into the file and removes it; the setup's
    // writes leave one behind.
    let wal_kept = |a: Attack| match &wal {
        Some(wal) => {
            let wal = wal.clone();
            a.trace(Check::custom("the WAL is still there", move |_| {
                let wal = wal.clone();
                async move {
                    match std::fs::metadata(&wal) {
                        Ok(m) if m.len() > 0 => Ok(()),
                        Ok(_) => Err(format!("{} is empty", wal.display())),
                        Err(e) => Err(format!("{}: {e}", wal.display())),
                    }
                }
            }))
        }
        None => a,
    };
    let ext_dir = dir.path("extensions");

    let attacks = vec![
        // ── What the plan's probes found a read-only transaction allows ──
        // duckdb-rs runs every statement but the last while preparing; the
        // COMMIT would end the read-only transaction.
        refused(
            "COMMIT, then a write",
            format!("COMMIT; INSERT INTO {t} VALUES (2); SELECT 1"),
        )
        .trace(one_row()),
        refused(
            "ROLLBACK, then a write",
            format!("ROLLBACK; INSERT INTO {t} VALUES (2); SELECT 1"),
        )
        .trace(one_row()),
        refused(
            "BEGIN, then a write",
            format!("BEGIN TRANSACTION; INSERT INTO {t} VALUES (2); COMMIT"),
        )
        .trace(one_row()),
        refused(
            "a SELECT, then a write",
            format!("SELECT 1; INSERT INTO {t} VALUES (2)"),
        )
        .trace(one_row()),
        refused(
            "a write, then a SELECT",
            format!("INSERT INTO {t} VALUES (2); SELECT 1"),
        )
        .trace(one_row()),
        refused(
            "COPY TO",
            format!("COPY (SELECT 1) TO '{}'", dir.path("x.csv")),
        )
        .trace(Check::no_file(dir.path("x.csv"))),
        refused(
            "COPY a table TO parquet",
            format!("COPY {t} TO '{}' (FORMAT parquet)", dir.path("t.parquet")),
        )
        .trace(Check::no_file(dir.path("t.parquet"))),
        refused("INSTALL", "INSTALL httpfs".into()).trace(no_extension_file(&ext_dir, "httpfs")),
        refused("FORCE INSTALL", "FORCE INSTALL json".into())
            .trace(no_extension_file(&ext_dir, "json")),
        refused("LOAD", "LOAD json".into()).trace(Check::count(
            "SELECT count(*) FROM duckdb_extensions() WHERE extension_name = 'json' AND loaded",
            0,
        )),
        refused("ATTACH in memory", "ATTACH ':memory:' AS ro_m".into()).trace(no_database("ro_m")),
        refused(
            "ATTACH a new file",
            format!("ATTACH '{}' AS ro_f", dir.path("new.duckdb")),
        )
        .trace(Check::no_file(dir.path("new.duckdb")))
        .trace(no_database("ro_f")),
        refused("SET a global", "SET threads = 1".into()).trace(threads.clone()),
        refused("SET GLOBAL", "SET GLOBAL threads = 1".into()).trace(threads.clone()),
        refused("PRAGMA", "PRAGMA threads = 1".into()).trace(threads.clone()),
        refused("SELECT, then SET", "SELECT 1; SET threads = 1".into()).trace(threads.clone()),
        // Session settings on the read-only connection: `after_each` compares
        // them with the main connection's.
        refused(
            "SET a session setting",
            "SET TimeZone = 'Asia/Kolkata'".into(),
        )
        .trace(session_is_default()),
        refused("RESET", "RESET threads".into()).trace(threads.clone()),
        refused("USE", format!("USE {use_schema}")).trace(session_is_default()),
        refused("SET VARIABLE", "SET VARIABLE ro_v = 1".into()).trace(Check::custom(
            "the variable isn't set",
            |driver| async move {
                let r = driver
                    .query_read_only("SELECT getvariable('ro_v') IS NULL", vec![])
                    .await
                    .map_err(|e| format!("{e:?}"))?;
                match r.rows.first().and_then(|r| r.first()) {
                    Some(Value::Bool(true)) => Ok(()),
                    other => Err(format!("getvariable: {other:?}")),
                }
            },
        )),
        refused(
            "EXPORT DATABASE",
            format!("EXPORT DATABASE '{}'", dir.path("export")),
        )
        .trace(Check::no_file(dir.path("export"))),
        wal_kept(refused("CHECKPOINT", "CHECKPOINT".into()).trace(one_row())),
        wal_kept(refused("FORCE CHECKPOINT", "FORCE CHECKPOINT".into()).trace(one_row())),
        // `force_checkpoint()` is a SELECT; DuckDB refuses it inside a
        // transaction. (`checkpoint()` runs: see the accepted gaps.)
        wal_kept(
            Attack::new("force_checkpoint()", "SELECT * FROM force_checkpoint()")
                .refused()
                .trace(one_row()),
        ),
        // `enable_profiling()` is a SELECT the driver runs (Core's token
        // check refuses it). It must not outlive its call: on a shared
        // read-only connection every later call rewrote the file.
        Attack::new(
            "enable_profiling()",
            format!(
                "SELECT * FROM enable_profiling(format := 'json', save_location := '{}')",
                dir.path("profile.json")
            ),
        )
        .trace(profiling_ended(dir.path("profile.json"))),
        Attack::new(
            "enable_profiling() inside json_execute_serialized_sql()",
            format!(
                "SELECT * FROM json_execute_serialized_sql(json_serialize_sql(\
                 'SELECT * FROM enable_profiling(format := ''json'', save_location := ''{}'')'))",
                dir.path("profile3.json")
            ),
        )
        .trace(profiling_ended(dir.path("profile3.json"))),
        Attack::new(
            "enable_profiling() inside query()",
            format!(
                "SELECT * FROM query('SELECT * FROM enable_profiling(format := ''json'', \
                 save_location := ''{}'')')",
                dir.path("profile2.json")
            ),
        )
        .trace(profiling_ended(dir.path("profile2.json"))),
        refused(
            "CREATE SECRET",
            "CREATE SECRET ro_sec (TYPE s3, KEY_ID 'k', SECRET 's')".into(),
        )
        .trace(count_on_both("SELECT count(*) FROM duckdb_secrets()", 0)),
        // ── DDL ──
        refused("CREATE TABLE AS", format!("CREATE TABLE {x} AS SELECT 1")).trace(no_table(&x)),
        refused("CREATE TABLE", format!("CREATE TABLE {x} (a INT)")).trace(no_table(&x)),
        refused(
            "CREATE TEMP TABLE",
            format!("CREATE TEMP TABLE {x} AS SELECT 1"),
        )
        .trace(no_table(&x))
        .trace(session_is_default()),
        refused("DROP TABLE", format!("DROP TABLE {t}")).trace(one_row()),
        refused(
            "ALTER TABLE",
            format!("ALTER TABLE {t} ADD COLUMN z INTEGER"),
        )
        .trace(columns_of_t.clone()),
        refused("CREATE VIEW", format!("CREATE VIEW {v} AS SELECT 1")).trace(Check::count(
            format!("SELECT count(*) FROM duckdb_views() WHERE view_name = '{v}'"),
            0,
        )),
        refused("CREATE SCHEMA", format!("CREATE SCHEMA {sc}")).trace(Check::count(
            format!("SELECT count(*) FROM duckdb_schemas() WHERE schema_name = '{sc}'"),
            0,
        )),
        refused("CREATE MACRO", format!("CREATE MACRO {m}() AS 1")).trace(Check::count(
            format!("SELECT count(*) FROM duckdb_functions() WHERE function_name = '{m}'"),
            0,
        )),
        // ── DML ──
        refused("INSERT", format!("INSERT INTO {t} VALUES (2)")).trace(one_row()),
        refused(
            "INSERT … RETURNING",
            format!("INSERT INTO {t} VALUES (2) RETURNING n"),
        )
        .trace(one_row()),
        refused("UPDATE", format!("UPDATE {t} SET n = 5")).trace(Check::count(
            format!("SELECT count(*) FROM {t} WHERE n = 1"),
            1,
        )),
        refused("DELETE", format!("DELETE FROM {t}")).trace(one_row()),
        // A write inside a CTE: DuckDB's parser has no data-modifying CTEs,
        // and a CTE in front of an INSERT is still an INSERT.
        Attack::new(
            "DELETE inside a CTE",
            format!("WITH d AS (DELETE FROM {t} RETURNING *) SELECT * FROM d"),
        )
        .refused()
        .trace(one_row()),
        refused(
            "INSERT after a CTE",
            format!("WITH c AS (SELECT 2 AS n) INSERT INTO {t} SELECT n FROM c"),
        )
        .trace(one_row()),
        // query() nested in the SELECT: the inner call is refused too.
        refused(
            "query() of a write",
            format!("SELECT * FROM query('INSERT INTO {t} VALUES (2)')"),
        )
        .trace(one_row()),
        // nextval is a SELECT; the read-only transaction refuses it.
        Attack::new("nextval", format!("SELECT nextval('{s}')"))
            .refused_with("READ_ONLY")
            .message_contains("transaction is launched in read-only mode")
            .trace(Check::value(
                format!("SELECT last_value FROM duckdb_sequences() WHERE sequence_name = '{s}'"),
                Value::Null,
            )),
        // ── Parameters: the one parameter is the SQL ──
        Attack::new("a bind value", "SELECT ? AS v")
            .params(vec![Value::Int(1)])
            .refused_with("READ_ONLY")
            .message_contains("take no bind values")
            .trace(one_row()),
        Attack::new(
            "a write with a bind value",
            format!("INSERT INTO {t} VALUES (?)"),
        )
        .params(vec![Value::Int(2)])
        .refused_with("READ_ONLY")
        .message_contains("take no bind values")
        .trace(one_row()),
        refused("an empty query", String::new()).trace(one_row()),
        // ── Allowed ──
        Attack::allowed("SELECT", format!("SELECT n FROM {t}")).returns(vec![vec![Value::Int(1)]]),
        Attack::allowed(
            "WITH",
            format!("WITH c AS (SELECT n FROM {t}) SELECT n + 1 FROM c"),
        )
        .returns(vec![vec![Value::Int(2)]]),
        Attack::allowed("FROM", format!("FROM {t}")).returns(vec![vec![Value::Int(1)]]),
        Attack::allowed("a trailing semicolon", "SELECT 1;").returns(vec![vec![Value::Int(1)]]),
        Attack::allowed("a trailing comment", "SELECT 1; -- note")
            .returns(vec![vec![Value::Int(1)]]),
        Attack::allowed("VALUES", "VALUES (1)").returns(vec![vec![Value::Int(1)]]),
        Attack::allowed("SUMMARIZE", format!("SUMMARIZE {t}")),
        Attack::allowed("DESCRIBE", format!("DESCRIBE {t}")),
        Attack::allowed(
            "SUMMARIZE as a subquery",
            format!("SELECT * FROM (SUMMARIZE {t})"),
        ),
        Attack::allowed("SHOW TABLES", "SHOW TABLES"),
        Attack::allowed("a quote in the SQL", "SELECT 'it''s' AS q")
            .returns(vec![vec![Value::Text("it's".into())]]),
    ];

    ReadOnlySpec {
        setup: vec![
            format!("CREATE TABLE {t} (n INTEGER)"),
            format!("INSERT INTO {t} VALUES (1)"),
            format!("CREATE TABLE {w} (n INTEGER)"),
            format!("CREATE SEQUENCE {s}"),
            format!("CREATE SCHEMA {use_schema}"),
            // Global settings, so the traces know their value.
            "SET threads = 3".into(),
            format!("SET extension_directory = '{ext_dir}'"),
        ],
        teardown: vec![
            format!("DROP TABLE IF EXISTS {t}"),
            format!("DROP TABLE IF EXISTS {w}"),
            format!("DROP SEQUENCE IF EXISTS {s}"),
            format!("DROP SCHEMA IF EXISTS {use_schema}"),
            format!("DROP TABLE IF EXISTS {x}"),
            format!("DROP VIEW IF EXISTS {v}"),
            format!("DROP SCHEMA IF EXISTS {sc}"),
            format!("DROP MACRO IF EXISTS {m}"),
        ],
        attacks,
        after_each: vec![
            Check::executes(format!("INSERT INTO {w} VALUES (1)")),
            one_row(),
            columns_of_t,
            threads,
            main_is_not_in_a_transaction(),
            session_is_default(),
        ],
        slow_query: "SELECT sum(a.range * b.range) FROM range(300000) a, range(300000) b".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn read_only_in_memory() {
    let dir = Scratch::new();
    run_read_only(
        &*seaquel_engine_duckdb::engine(),
        &memory(),
        &spec(&dir, None),
    )
    .await;
}

#[tokio::test]
async fn read_only_on_the_seeded_file() {
    let Some(db) = SeededCopy::new() else {
        return;
    };
    let dir = Scratch::new();
    let wal = db.0.join("seaquel_test.duckdb.wal");
    run_read_only(
        &*seaquel_engine_duckdb::engine(),
        &db.config(),
        &spec(&dir, Some(wal)),
    )
    .await;
}

/// A transaction the user opened on the main connection is untouched: the
/// AI query runs meanwhile (on its own connection, seeing only committed
/// rows), and the user's transaction is still open afterwards with its row.
#[tokio::test]
async fn the_users_transaction_is_untouched() {
    let driver = open(&memory()).await;
    let t = scratch_name("ro_tx_");
    driver
        .execute(&format!("CREATE TABLE {t} (n INTEGER)"), vec![])
        .await
        .unwrap();
    driver.execute("BEGIN TRANSACTION", vec![]).await.unwrap();
    driver
        .execute(&format!("INSERT INTO {t} VALUES (1)"), vec![])
        .await
        .unwrap();

    let count = format!("SELECT count(*) FROM {t}");
    let ro = driver.query_read_only(&count, vec![]).await.unwrap();
    assert_eq!(
        ro.rows,
        vec![vec![Value::Int(0)]],
        "the AI sees committed rows only"
    );
    let e = driver
        .query_read_only(&format!("INSERT INTO {t} VALUES (2)"), vec![])
        .await
        .unwrap_err();
    assert_eq!(e.code, "READ_ONLY", "{e:?}");
    assert!(
        !e.message.contains("query(?)"),
        "the message points at the wrapper: {e:?}"
    );

    let main = driver.query(&count, vec![]).await.unwrap();
    assert_eq!(
        main.rows,
        vec![vec![Value::Int(1)]],
        "the user's row is still there"
    );
    // Still open: COMMIT fails when no transaction is active.
    driver.execute("COMMIT", vec![]).await.unwrap();
    let main = driver.query(&count, vec![]).await.unwrap();
    assert_eq!(main.rows, vec![vec![Value::Int(1)]]);
}

/// Cancelling a read-only query interrupts the read-only connection only: a
/// slow query on the main connection, running at the same time, finishes.
#[tokio::test]
async fn cancel_interrupts_only_the_read_only_connection() {
    let driver = open(&memory()).await;
    let started = tokio::time::Instant::now();
    let main = async {
        let r = driver
            .query("SELECT sum(i % 7) FROM range(600000000) t(i)", vec![])
            .await;
        (r, started.elapsed())
    };
    let read_only = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let slow = tokio::time::timeout(
            Duration::from_millis(200),
            driver.query_read_only(
                "SELECT sum(a.range * b.range) FROM range(300000) a, range(300000) b",
                vec![],
            ),
        )
        .await;
        (slow, started.elapsed())
    };
    let ((r, main_took), (slow, dropped_at)) = tokio::join!(main, read_only);
    assert!(slow.is_err(), "the slow read-only query finished: {slow:?}");
    assert!(
        main_took > dropped_at,
        "the main query ({main_took:?}) was done before the read-only one was dropped \
         ({dropped_at:?}), so this proves nothing; make it slower"
    );
    assert!(
        r.is_ok(),
        "the main connection's query was interrupted: {r:?}"
    );
    let r = driver.query_read_only("SELECT 1", vec![]).await.unwrap();
    assert_eq!(r.rows, vec![vec![Value::Int(1)]]);
}

/// Every typed cell decodes the same through `query_read_only` as through
/// `query`: the read-only connection has `arrow_lossless_conversion` too,
/// and the `query()` wrapper keeps the column types.
#[tokio::test]
async fn typed_cells_decode_the_same() {
    let driver = open(&memory()).await;
    let mut failures = Vec::new();
    for case in cells::cases() {
        for sql in &case.setup {
            driver.execute(sql, vec![]).await.unwrap();
        }
        let main = driver.query(&case.select, vec![]).await;
        let ro = driver.query_read_only(&case.select, vec![]).await;
        match (main, ro) {
            (Ok(main), Ok(ro)) => {
                let cell = |r: &seaquel_engine::QueryResult| r.rows[0][0].clone();
                if !same_value(&cell(&main), &cell(&ro)) || main.columns != ro.columns {
                    failures.push(format!(
                        "{}: query {:?} {:?}, read-only {:?} {:?}",
                        case.name,
                        main.columns,
                        cell(&main),
                        ro.columns,
                        cell(&ro)
                    ));
                }
            }
            (main, ro) => failures.push(format!("{}: query {main:?}, read-only {ro:?}", case.name)),
        }
        for sql in &case.teardown {
            let _ = driver.execute(sql, vec![]).await;
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Duplicate column names: `query()` renames them itself (`a`, `a_1`), where
/// `query` keeps `a`, `a` for the TS to dedupe (`a`, `a_2`). Every widget
/// run and every AI query goes through the read-only path, so a widget's
/// chart config and the rows it gets always agree; only the same SQL run in
/// the editor names the second column differently. Pinned here so a change
/// in DuckDB's renaming shows up.
#[tokio::test]
async fn duplicate_column_names() {
    let driver = open(&memory()).await;
    let r = driver
        .query_read_only("SELECT 1 AS a, 2 AS a", vec![])
        .await
        .unwrap();
    assert_eq!(r.columns, vec!["a", "a_1"]);
    let r = driver.query("SELECT 1 AS a, 2 AS a", vec![]).await.unwrap();
    assert_eq!(r.columns, vec!["a", "a"]);
}

/// The SELECTs the driver can't contain (see the header) are refused by the
/// token check Core runs before the driver, nested in `query()` too.
#[test]
fn state_functions_are_refused_by_the_token_check() {
    for sql in [
        "SELECT * FROM enable_logging()",
        "SELECT * FROM enable_logging(storage := 'file', storage_path := '/tmp/x')",
        "SELECT * FROM disable_logging()",
        "SELECT * FROM truncate_duckdb_logs()",
        "SELECT * FROM enable_profiling(format := 'json', save_location := '/tmp/x.json')",
        "SELECT * FROM disable_profiling()",
        "SELECT * FROM checkpoint()",
        "SELECT * FROM force_checkpoint()",
        "SELECT * FROM query('SELECT * FROM enable_logging()')",
        "SELECT * FROM query('SELECT * FROM enable_profiling(format := ''json'')')",
        "SELECT * FROM query('SELECT 1')",
        "SELECT * FROM json_execute_serialized_sql(json_serialize_sql('SELECT * FROM enable_logging()'))",
        "SELECT * FROM json_execute_serialized_sql(json_serialize_sql('SELECT * FROM checkpoint()'))",
        // SQL on an attached database, outside the read-only transaction:
        // `mysql_execute('my', 'CREATE TABLE …')` created the table.
        "SELECT * FROM postgres_execute('pg', 'DROP TABLE t')",
        "SELECT * FROM mysql_execute('my', 'CREATE TABLE t (a int)')",
        "SELECT * FROM mysql_query('my', 'SELECT 1')",
        "SELECT * FROM postgres_query('pg', 'SELECT 1')",
        "SELECT * FROM sqlite_query('s', 'INSERT INTO t VALUES (1) RETURNING a')",
        // The UI extension's HTTP server, which serves the whole database.
        "SELECT * FROM start_ui_server()",
        "SELECT * FROM load_aws_credentials(redact_secret := false)",
    ] {
        assert_eq!(
            read_only_error(sql, SqlEngine::Duckdb),
            Some(READ_ONLY_MESSAGE),
            "{sql}"
        );
    }
    assert_eq!(
        read_only_error("SELECT * FROM query_table('t')", SqlEngine::Duckdb),
        None
    );
    // Only SELECT and WITH start a statement the check admits, so the other
    // forms `query()` allows never reach the driver from the AI.
    for sql in [
        "FROM t",
        "VALUES (1)",
        "SUMMARIZE t",
        "DESCRIBE t",
        "SHOW TABLES",
    ] {
        assert_eq!(
            read_only_error(sql, SqlEngine::Duckdb),
            Some(READ_ONLY_MESSAGE),
            "{sql}"
        );
    }
    // As a subquery they're a SELECT.
    assert_eq!(
        read_only_error("SELECT * FROM (SUMMARIZE t)", SqlEngine::Duckdb),
        None
    );
}

/// The wrapper's `LIMIT` stops DuckDB one row past the cap, so a huge result
/// fails fast with `RESULT_TOO_LARGE` instead of being materialized first.
#[tokio::test]
async fn a_huge_result_fails_fast() {
    let driver = open(&memory()).await;
    let started = tokio::time::Instant::now();
    let e = driver
        .query_read_only("SELECT * FROM range(2000000000)", vec![])
        .await
        .unwrap_err();
    assert_eq!(e.code, "RESULT_TOO_LARGE", "{e:?}");
    let took = started.elapsed();
    assert!(took < Duration::from_secs(3), "took {took:?}");
}
