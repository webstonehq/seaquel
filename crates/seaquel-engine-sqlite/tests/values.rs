//! Native values (phase 2 Task 10): each cell decodes by its storage class
//! (`typeof`), never by the declared column type; binding it back compares
//! equal; and CRUD built by `SqliteDialect` finds rows by BLOB and 64-bit
//! integer keys. No server needed: a temp-file database.

use std::path::PathBuf;
use std::sync::Arc;

use seaquel_engine::{ConnectConfig, Dialect, Driver, RowValues, SqlWithBindings, Value};
use seaquel_engine_sqlite::SqliteDialect;
use seaquel_engine_testkit::{run_typed_cells, scratch_name, TypedCellCase};

/// A fresh database file, deleted on drop.
struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("seaquel-values-{}.sqlite", uuid::Uuid::new_v4())))
    }

    fn config(&self) -> ConnectConfig {
        serde_json::from_value(serde_json::json!({
            "driver": "sqlite",
            "connection_string": format!("sqlite:{}", self.0.display()),
            "create_if_missing": true
        }))
        .unwrap()
    }

    async fn open(&self) -> Arc<dyn Driver> {
        seaquel_engine_sqlite::engine()
            .open(&self.config())
            .await
            .expect("open")
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

/// A column declared `ty` (empty: no type) holding `literal`. Bind-back
/// compares `?` with the stored value, whose storage class the column's
/// affinity chose.
fn column_case(name: &str, ty: &str, literal: &str, expected: Value) -> TypedCellCase {
    let table = scratch_name("values_");
    TypedCellCase {
        name: format!("{name} column"),
        setup: vec![
            format!("CREATE TABLE {table} (v {ty})"),
            format!("INSERT INTO {table} (v) VALUES ({literal})"),
        ],
        teardown: vec![format!("DROP TABLE IF EXISTS {table}")],
        select: format!("SELECT v FROM {table}"),
        literal: None,
        expected,
        bind_back: Some(format!("SELECT ? = (SELECT v FROM {table})")),
    }
}

fn cases() -> Vec<TypedCellCase> {
    let lit = |literal: &str, expected: Value| {
        TypedCellCase::literal(literal, expected).bind_back_eq("?")
    };
    vec![
        TypedCellCase::literal("NULL", Value::Null).bind_back("SELECT ? IS NULL"),
        lit("0", Value::Int(0)),
        lit("-9223372036854775808", Value::Int(i64::MIN)),
        lit("9223372036854775807", Value::Int(i64::MAX)),
        // Beyond 2^53: exact (the wire tags it `bigint`).
        lit("9007199254740993", Value::Int(9_007_199_254_740_993)),
        lit("-9007199254740993", Value::Int(-9_007_199_254_740_993)),
        lit("1.5", Value::Float(1.5)),
        lit("0.1", Value::Float(0.1)),
        lit("1e308", Value::Float(1e308)),
        // Infinity is a REAL (the wire tags it `float`).
        lit("9e999", Value::Float(f64::INFINITY)),
        lit("-9e999", Value::Float(f64::NEG_INFINITY)),
        // NaN can't be stored or computed: SQLite turns it into NULL.
        TypedCellCase::literal("9e999 - 9e999", Value::Null).named("NaN is NULL"),
        lit("'abc'", text("abc")),
        lit("'héllo €'", text("héllo €")),
        lit("''", text("")),
        lit("'12'", text("12")),
        lit("x'00ff10'", Value::Bytes(vec![0x00, 0xff, 0x10])),
        lit("x''", Value::Bytes(vec![])),
        // A BLOB of UTF-8 text is still a BLOB.
        lit("CAST('abc' AS BLOB)", Value::Bytes(b"abc".to_vec())),
        // TEXT that isn't UTF-8 is lossy text instead of failing the query
        // (not Bytes, which would bind back as a BLOB); it matches nothing.
        TypedCellCase::literal("CAST(x'ff41' AS TEXT)", text("\u{fffd}A")),
        lit("CAST(x'4142' AS TEXT)", text("AB")),
        // Storage class, not declared type.
        column_case("untyped holding text", "", "'123'", text("123")),
        column_case("untyped holding an integer", "", "123", Value::Int(123)),
        column_case(
            "TEXT holding a BLOB",
            "TEXT",
            "x'8081'",
            Value::Bytes(vec![0x80, 0x81]),
        ),
        column_case("TEXT holding a number (affinity)", "TEXT", "42", text("42")),
        column_case(
            "INTEGER holding numeric text (affinity)",
            "INTEGER",
            "'12'",
            Value::Int(12),
        ),
        column_case(
            "INTEGER holding other text",
            "INTEGER",
            "'abc'",
            text("abc"),
        ),
        column_case(
            "BIGINT beyond 2^53",
            "BIGINT",
            "9007199254740993",
            Value::Int(9_007_199_254_740_993),
        ),
        column_case(
            "REAL holding an integer (affinity)",
            "REAL",
            "1",
            Value::Float(1.0),
        ),
        column_case(
            "NUMERIC holding decimal text (affinity)",
            "NUMERIC(10,2)",
            "'1.5'",
            Value::Float(1.5),
        ),
        column_case(
            "NUMERIC holding an integral real (affinity)",
            "NUMERIC",
            "2.0",
            Value::Int(2),
        ),
        column_case("BLOB holding text", "BLOB", "'abc'", text("abc")),
        column_case(
            "BLOB holding a BLOB",
            "BLOB",
            "x'00ff'",
            Value::Bytes(vec![0x00, 0xff]),
        ),
        column_case("BOOLEAN", "BOOLEAN", "TRUE", Value::Int(1)),
        column_case(
            "DATETIME",
            "DATETIME",
            "'2024-01-02 03:04:05'",
            text("2024-01-02 03:04:05"),
        ),
        column_case("DATE holding a number", "DATE", "'2024'", Value::Int(2024)),
        column_case("JSON", "JSON", "'{\"a\":1}'", text("{\"a\":1}")),
    ]
}

#[tokio::test]
async fn values_round_trip() {
    let db = TempDb::new();
    run_typed_cells(&*seaquel_engine_sqlite::engine(), &db.config(), &cases()).await;
}

// ── CRUD by typed keys ───────────────────────────────────────────────────────

async fn run(driver: &dyn Driver, sql: SqlWithBindings, what: &str) {
    let binds = sql.bind_values.unwrap_or_default();
    let r = driver
        .execute(&sql.sql, binds.clone())
        .await
        .unwrap_or_else(|e| panic!("{what}: {} {binds:?}: {e:?}", sql.sql));
    assert_eq!(r.rows_affected, 1, "{what}: {} {binds:?}", sql.sql);
}

/// Every row of `table`, as the UI holds them.
async fn read_rows(driver: &dyn Driver, table: &str) -> Vec<RowValues> {
    let r = driver
        .query(&format!("SELECT * FROM \"{table}\" ORDER BY note"), vec![])
        .await
        .unwrap();
    r.rows
        .into_iter()
        .map(|row| r.columns.iter().cloned().zip(row).collect())
        .collect()
}

/// Update, set to default and delete the row whose key `SELECT` returned,
/// so the key's value binds back as it decoded. A BLOB key must bind as a
/// BLOB: the same bytes as TEXT compare unequal.
async fn crud_by_key(key_type: &str, key_literal: &str, expected_key: Value) {
    let db = TempDb::new();
    let driver = db.open().await;
    let table = "typed keys";
    driver
        .execute(
            &format!("CREATE TABLE \"{table}\" (k {key_type} PRIMARY KEY, note TEXT DEFAULT 'd') WITHOUT ROWID"),
            vec![],
        )
        .await
        .unwrap();
    driver
        .execute(
            &format!("INSERT INTO \"{table}\" (k, note) VALUES ({key_literal}, 'a')"),
            vec![],
        )
        .await
        .unwrap();
    // The same bytes as TEXT, which a key bound as TEXT would match. An
    // INTEGER key column converts it back to the same integer, a conflict.
    driver
        .execute(
            &format!("INSERT OR IGNORE INTO \"{table}\" (k, note) VALUES (CAST({key_literal} AS TEXT), 'decoy')"),
            vec![],
        )
        .await
        .unwrap();
    let rows = read_rows(&*driver, table).await;
    let row = rows
        .iter()
        .find(|r| r[1].1 == text("a"))
        .expect("the inserted row")
        .clone();
    assert_eq!(row[0].1, expected_key, "{key_type} key decodes");

    let d = SqliteDialect;
    let pk = vec!["k".to_string()];
    run(
        &*driver,
        d.build_update("main", table, "note", text("b"), &pk, &row, None),
        "update",
    )
    .await;
    run(
        &*driver,
        d.build_set_default_expr("main", table, "note", Some("'d'"), &pk, &row, None),
        "set default",
    )
    .await;
    let after = read_rows(&*driver, table).await;
    assert!(
        after
            .iter()
            .any(|r| r[0].1 == expected_key && r[1].1 == text("d")),
        "{after:?}"
    );
    run(
        &*driver,
        d.build_delete("main", table, &pk, &row, None),
        "delete",
    )
    .await;
    let after = read_rows(&*driver, table).await;
    assert!(after.iter().all(|r| r[0].1 != expected_key), "{after:?}");
}

#[tokio::test]
async fn crud_by_blob_key() {
    crud_by_key("BLOB", "x'00ff10'", Value::Bytes(vec![0x00, 0xff, 0x10])).await;
}

/// A BLOB in a TEXT-declared key column (TEXT affinity doesn't convert blobs).
#[tokio::test]
async fn crud_by_blob_in_text_key() {
    crud_by_key("TEXT", "x'4142'", Value::Bytes(b"AB".to_vec())).await;
}

#[tokio::test]
async fn crud_by_bigint_key() {
    crud_by_key(
        "INTEGER",
        "9007199254740993",
        Value::Int(9_007_199_254_740_993),
    )
    .await;
}

/// An INSERT with a BLOB parameter stores a BLOB, and one with an integer
/// beyond 2^53 stores it exactly.
#[tokio::test]
async fn insert_binds_bytes_and_big_integers() {
    let db = TempDb::new();
    let driver = db.open().await;
    driver
        .execute("CREATE TABLE t (b, i)", vec![])
        .await
        .unwrap();
    let values = vec![
        ("b".to_string(), Value::Bytes(vec![0, 1, 2])),
        ("i".to_string(), Value::Int(i64::MAX)),
    ];
    run(
        &*driver,
        SqliteDialect.build_insert("main", "t", &values, None),
        "insert",
    )
    .await;
    let r = driver
        .query("SELECT typeof(b), b, typeof(i), i FROM t", vec![])
        .await
        .unwrap();
    assert_eq!(
        r.rows[0],
        vec![
            text("blob"),
            Value::Bytes(vec![0, 1, 2]),
            text("integer"),
            Value::Int(i64::MAX)
        ]
    );
}
