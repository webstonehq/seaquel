//! Helpers shared by the storage integration tests: loading the frozen
//! fixtures into temp files, and reading a file's schema as a structure.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, Row, SqliteConnection};

/// sqlx's own bookkeeping table. It is in every file `Storage::open` has
/// touched and in no fixture, so schema comparisons skip it.
pub const SQLX_MIGRATIONS: &str = "_sqlx_migrations";

/// Where `Storage::open` records the data steps it ran. Like
/// `_sqlx_migrations`, it is in every file `Storage::open` has touched and in
/// no fixture, so schema comparisons skip it.
pub const DATA_STEPS: &str = seaquel_storage::DATA_STEPS_TABLE;

pub fn fixture_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

pub fn fixture(rel: &str) -> String {
    std::fs::read_to_string(fixture_path(rel)).unwrap_or_else(|e| panic!("reading {rel}: {e}"))
}

/// A plain connection to `path`, created if missing, with none of
/// `Storage::open`'s checks.
pub async fn raw_connect(path: &Path) -> SqliteConnection {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .connect()
        .await
        .unwrap()
}

/// Run `sql` (one or more statements) against the file at `path`.
pub async fn exec_file(path: &Path, sql: &str) {
    let mut conn = raw_connect(path).await;
    sqlx::raw_sql(sql).execute(&mut conn).await.unwrap();
    conn.close().await.unwrap();
}

/// A new file at `path` holding the frozen schema fixture `rel`.
pub async fn load_fixture(path: &Path, rel: &str) {
    exec_file(path, &fixture(rel)).await;
}

/// Every numbered migration in `migrations/` (sqlx's `NNNN_name.sql`), in
/// version order.
pub fn migrations() -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect()
}

/// A new file at `path` holding the frozen schema fixture `rel` with the
/// numbered migrations applied after it: the schema `Storage::open` gives a
/// file whose baseline is `rel`. The fixtures are frozen at the baseline;
/// migrations come after it.
pub async fn load_migrated(path: &Path, rel: &str) {
    load_fixture(path, rel).await;
    for sql in migrations() {
        exec_file(path, &sql).await;
    }
}

/// [`fixture_shape`] after the numbered migrations ([`load_migrated`]).
pub async fn migrated_shape(rel: &str) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("expected.db");
    load_migrated(&path, rel).await;
    schema_shape_of(&path).await
}

/// Columns the numbered migrations add, which the frozen data fixtures
/// (recorded at the baseline) don't list.
pub const MIGRATION_COLUMNS: &[(&str, &str)] = &[
    ("connections", "name_key"),
    ("projects", "name_key"),
    ("saved_queries", "name_key"),
    ("dashboards", "name_key"),
];

/// A file's schema as sorted lines, one per fact, so two schemas compare
/// structurally: whitespace and `CREATE` text don't matter, but column order
/// and every column's type, NOT NULL, default and primary key position do, as
/// do foreign keys, indexes (with their columns, uniqueness and origin) and
/// the set of views and triggers, and every CHECK clause (read from the
/// `CREATE` text with whitespace collapsed, since no pragma reports them).
/// `_sqlx_migrations` and `_seaquel_data_steps` are left out.
pub async fn schema_shape(conn: &mut SqliteConnection) -> Vec<String> {
    let mut lines = BTreeSet::new();
    let objects = sqlx::query(
        "SELECT type, name, tbl_name, sql FROM sqlite_master \
         WHERE name NOT LIKE 'sqlite_%' AND tbl_name NOT IN (?, ?)",
    )
    .bind(SQLX_MIGRATIONS)
    .bind(DATA_STEPS)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let mut tables = Vec::new();
    for o in &objects {
        let (kind, name, tbl): (String, String, String) = (o.get(0), o.get(1), o.get(2));
        match kind.as_str() {
            "table" => {
                let sql: String = o.get(3);
                for check in check_clauses(&sql) {
                    lines.insert(format!("{name} check {check}"));
                }
                tables.push(name);
            }
            // Indexes are listed per table below, with their columns.
            "index" => {}
            _ => {
                lines.insert(format!("{kind} {name} on {tbl}"));
            }
        }
    }
    for table in tables {
        lines.insert(format!("table {table}"));
        let cols = sqlx::query(&format!("PRAGMA table_xinfo(\"{table}\")"))
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        for c in cols {
            let cid: i64 = c.get("cid");
            let name: String = c.get("name");
            let ty: String = c.get("type");
            let notnull: i64 = c.get("notnull");
            let dflt: Option<String> = c.get("dflt_value");
            let pk: i64 = c.get("pk");
            let hidden: i64 = c.get("hidden");
            lines.insert(format!(
                "{table} column {cid:02} {name} type={ty} notnull={notnull} default={dflt:?} pk={pk} hidden={hidden}"
            ));
        }
        let fks = sqlx::query(&format!("PRAGMA foreign_key_list(\"{table}\")"))
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        for f in fks {
            let id: i64 = f.get("id");
            let seq: i64 = f.get("seq");
            let to_table: String = f.get("table");
            let from: String = f.get("from");
            let to: Option<String> = f.get("to");
            let on_update: String = f.get("on_update");
            let on_delete: String = f.get("on_delete");
            let matching: String = f.get("match");
            lines.insert(format!(
                "{table} fk {id}.{seq} {from} -> {to_table}({to:?}) on_update={on_update} on_delete={on_delete} match={matching}"
            ));
        }
        let indexes = sqlx::query(&format!("PRAGMA index_list(\"{table}\")"))
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        for i in indexes {
            let name: String = i.get("name");
            let unique: i64 = i.get("unique");
            let origin: String = i.get("origin");
            let partial: i64 = i.get("partial");
            lines.insert(format!(
                "{table} index {name} unique={unique} origin={origin} partial={partial}"
            ));
            let cols = sqlx::query(&format!("PRAGMA index_xinfo(\"{name}\")"))
                .fetch_all(&mut *conn)
                .await
                .unwrap();
            for c in cols {
                let seqno: i64 = c.get("seqno");
                let col: Option<String> = c.get("name");
                let desc: i64 = c.get("desc");
                let coll: Option<String> = c.get("coll");
                let key: i64 = c.get("key");
                lines.insert(format!(
                    "{table} index {name} col {seqno} {col:?} desc={desc} coll={coll:?} key={key}"
                ));
            }
        }
    }
    lines.into_iter().collect()
}

/// Every `CHECK (…)` in `sql`, whitespace collapsed. Good enough for the
/// baseline's DDL, which has no CHECK inside a string literal.
pub fn check_clauses(sql: &str) -> Vec<String> {
    let flat = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let upper = flat.to_ascii_uppercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = upper[from..].find("CHECK") {
        let start = from + at;
        let Some(open) = flat[start..].find('(').map(|i| start + i) else {
            break;
        };
        let mut depth = 0;
        let mut end = open;
        for (i, ch) in flat[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.push(flat[start..=end].to_string());
        from = end + 1;
    }
    out
}

pub async fn schema_shape_of(path: &Path) -> Vec<String> {
    let mut conn = raw_connect(path).await;
    let shape = schema_shape(&mut conn).await;
    conn.close().await.unwrap();
    shape
}

/// The structure a frozen schema fixture loads to, from a scratch file of
/// its own.
pub async fn fixture_shape(rel: &str) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("expected.db");
    load_fixture(&path, rel).await;
    schema_shape_of(&path).await
}

/// The lines only `a` has, and the lines only `b` has.
pub fn shape_diff(a: &[String], b: &[String]) -> (Vec<String>, Vec<String>) {
    let sa: BTreeSet<&String> = a.iter().collect();
    let sb: BTreeSet<&String> = b.iter().collect();
    (
        sa.difference(&sb).map(|s| s.to_string()).collect(),
        sb.difference(&sa).map(|s| s.to_string()).collect(),
    )
}

/// Panic with the lines only one side has, if the shapes differ.
pub fn assert_same_shape(actual: &[String], expected: &[String], label: &str) {
    if actual == expected {
        return;
    }
    let a: BTreeSet<_> = actual.iter().collect();
    let e: BTreeSet<_> = expected.iter().collect();
    let extra: Vec<_> = a.difference(&e).collect();
    let missing: Vec<_> = e.difference(&a).collect();
    panic!(
        "{label}: schemas differ\n  only in actual:\n    {}\n  only in expected:\n    {}",
        extra
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n    "),
        missing
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n    "),
    );
}

/// `sqlite_master`'s `(type, name, sql)` in creation order, without
/// `_sqlx_migrations` and `_seaquel_data_steps`: the schema as text.
pub async fn schema_text(conn: &mut SqliteConnection) -> Vec<(String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT type, name, sql FROM sqlite_master WHERE tbl_name NOT IN (?, ?) ORDER BY rowid",
    )
    .bind(SQLX_MIGRATIONS)
    .bind(DATA_STEPS)
    .fetch_all(&mut *conn)
    .await
    .unwrap()
}

pub async fn versions(conn: &mut SqliteConnection) -> Vec<i64> {
    sqlx::query_scalar("SELECT version FROM schema_version ORDER BY rowid")
        .fetch_all(&mut *conn)
        .await
        .unwrap()
}

/// `table`'s `columns`, every row as `[values, typeof per value]`, sorted
/// by every column in order (the fixtures' `ORDER BY 1, 2, …`).
pub async fn rows(
    conn: &mut SqliteConnection,
    table: &str,
    columns: &[String],
) -> Vec<(serde_json::Value, serde_json::Value)> {
    let quoted: Vec<String> = columns.iter().map(|c| format!("\"{c}\"")).collect();
    let types: Vec<String> = quoted.iter().map(|c| format!("typeof({c})")).collect();
    // JSON can't hold a BLOB (`_sqlx_migrations.checksum`), so BLOBs go in
    // as hex; `typeof` still says `blob`.
    let values: Vec<String> = quoted
        .iter()
        .map(|c| format!("CASE WHEN typeof({c}) = 'blob' THEN hex({c}) ELSE {c} END"))
        .collect();
    let sql = format!(
        "SELECT json_array({}), json_array({}) FROM \"{table}\" ORDER BY {}",
        values.join(", "),
        types.join(", "),
        quoted.join(", "),
    );
    sqlx::query(&sql)
        .fetch_all(&mut *conn)
        .await
        .unwrap()
        .into_iter()
        .map(|r| {
            let values: String = r.get(0);
            let types: String = r.get(1);
            (
                serde_json::from_str(&values).unwrap(),
                serde_json::from_str(&types).unwrap(),
            )
        })
        .collect()
}

/// Every object's DDL and every table's rows (`_sqlx_migrations` included),
/// in a stable order: enough to tell whether a file changed at all.
pub async fn snapshot(path: &Path) -> Vec<String> {
    let mut conn = raw_connect(path).await;
    let mut out = Vec::new();
    let objects: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT type, name, sql FROM sqlite_master ORDER BY rowid")
            .fetch_all(&mut conn)
            .await
            .unwrap();
    for (kind, name, sql) in objects {
        out.push(format!("{kind} {name}: {sql:?}"));
    }
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    for table in tables {
        let columns: Vec<String> =
            sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .fetch_all(&mut conn)
                .await
                .unwrap();
        for (values, types) in rows(&mut conn, &table, &columns).await {
            out.push(format!("{table}: {values} {types}"));
        }
    }
    conn.close().await.unwrap();
    out
}
