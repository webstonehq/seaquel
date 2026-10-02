//! The in-memory executor's own tests (phase 8 Task 2). They run twice:
//! natively over `libsqlite3-sys` (`cargo test -p seaquel-storage --lib`),
//! and in wasm32 under Node over `sqlite-wasm-rs`
//! (`cargo test --target wasm32-unknown-unknown -p seaquel-storage --lib`,
//! with `wasm-bindgen-test-runner`). Same C API, same SQLite rules, so the
//! same expectations.
//!
//! Each expectation is what sqlx 0.8.6 gives for the same call, so the
//! facade's two executors agree.

use super::*;

/// One test, on both targets: natively a plain `#[test]` that drives the
/// future to completion (nothing in the executor ever waits), in wasm32 a
/// `#[wasm_bindgen_test]`.
macro_rules! both {
    ($name:ident, $body:block) => {
        #[cfg(not(target_arch = "wasm32"))]
        #[test]
        fn $name() {
            futures::executor::block_on(async $body)
        }
        #[cfg(target_arch = "wasm32")]
        #[wasm_bindgen_test::wasm_bindgen_test]
        async fn $name() $body
    };
}

fn pool() -> SqlitePool {
    SqlitePool::open_in_memory(None).expect("an in-memory database opens")
}

fn db_error(e: Error) -> Box<SqliteError> {
    match e {
        Error::Database(db) => db,
        other => panic!("expected a database error, got {other:?}"),
    }
}

both!(binds_and_reads_every_column_kind, {
    let p = pool();
    query("CREATE TABLE t (i INTEGER, r REAL, s TEXT, b BLOB, n TEXT, f INTEGER)")
        .execute(&p)
        .await
        .unwrap();
    let done = query("INSERT INTO t (i, r, s, b, n, f) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(i64::MAX)
        .bind(1.5_f64)
        .bind("Straße 東京 \0 nul")
        .bind(vec![0u8, 1, 255])
        .bind(None::<String>)
        .bind(true)
        .execute(&p)
        .await
        .unwrap();
    assert_eq!(done.rows_affected(), 1);

    let row: (i64, f64, String, Vec<u8>, Option<String>, bool) =
        query_as("SELECT i, r, s, b, n, f FROM t")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(
        row,
        (
            i64::MAX,
            1.5,
            "Straße 東京 \0 nul".to_string(),
            vec![0, 1, 255],
            None,
            true
        )
    );

    // By name and by index, checked and unchecked.
    let r = query("SELECT i, r, s, b, n FROM t")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(r.try_get::<i64, _>("i").unwrap(), i64::MAX);
    assert_eq!(r.try_get::<f64, _>(1).unwrap(), 1.5);
    assert_eq!(r.try_get::<Option<String>, _>("n").unwrap(), None);
    // sqlx's rules: a text column reads as bytes; an integer doesn't read as
    // text or as a float when checked, and SQLite converts it when not.
    assert_eq!(
        r.try_get::<Vec<u8>, _>("s").unwrap(),
        "Straße 東京 \0 nul".as_bytes()
    );
    assert!(matches!(
        r.try_get::<String, _>("i"),
        Err(Error::ColumnDecode { .. })
    ));
    assert!(matches!(
        r.try_get::<f64, _>("i"),
        Err(Error::ColumnDecode { .. })
    ));
    assert_eq!(
        r.try_get_unchecked::<String, _>("i").unwrap(),
        "9223372036854775807"
    );
    assert_eq!(
        r.try_get_unchecked::<Option<f64>, _>("i").unwrap(),
        Some(9.223372036854776e18)
    );
    assert!(
        matches!(r.try_get::<i64, _>("missing"), Err(Error::ColumnNotFound(c)) if c == "missing")
    );
    assert!(matches!(
        r.try_get::<i64, _>(9),
        Err(Error::ColumnIndexOutOfBounds { index: 9, len: 5 })
    ));
    assert_eq!(cell(&r, "i").unwrap(), Cell::Integer(i64::MAX));
    assert_eq!(cell(&r, "r").unwrap(), Cell::Real(1.5));
    assert_eq!(cell(&r, "s").unwrap(), Cell::Other);
    assert_eq!(cell(&r, "n").unwrap(), Cell::Null);

    // An empty blob is an empty blob, not NULL.
    let (empty, kind): (Vec<u8>, String) = query_as("SELECT ?, typeof(?)")
        .bind(Vec::<u8>::new())
        .bind(Vec::<u8>::new())
        .fetch_one(&p)
        .await
        .unwrap();
    assert!(empty.is_empty());
    assert_eq!(kind, "blob");

    // Numbered parameters, `$N`, and sqlx's arity rules: a missing value is
    // NULL, an extra one is ignored.
    let (a, b, c): (i64, i64, Option<i64>) = query_as("SELECT ?2, ?1, ?3")
        .bind(1_i64)
        .bind(2_i64)
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!((a, b, c), (2, 1, None));
    let n: i64 = query_scalar("SELECT $1 + 1")
        .bind(41_i64)
        .bind("ignored")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(n, 42);

    // Multiple statements: values are consumed in order across them, and
    // `rows_affected` is the sum.
    let done = query("INSERT INTO t (i) VALUES (?); INSERT INTO t (i) VALUES (?);")
        .bind(7_i64)
        .bind(8_i64)
        .execute(&p)
        .await
        .unwrap();
    assert_eq!(done.rows_affected(), 2);
    let all: Vec<(i64,)> = query_as("SELECT i FROM t WHERE i < 10 ORDER BY i")
        .fetch_all(&p)
        .await
        .unwrap();
    assert_eq!(all, vec![(7,), (8,)]);

    // `fetch_optional` returns the first row and stops there: a later
    // statement in the same text never runs.
    let first: Option<(i64,)> = query_as("SELECT i FROM t WHERE i < 10 ORDER BY i; DELETE FROM t")
        .fetch_optional(&p)
        .await
        .unwrap();
    assert_eq!(first, Some((7,)));
    let count: i64 = query_scalar("SELECT COUNT(*) FROM t")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(count, 3);
    let none: Option<(i64,)> = query_as("SELECT i FROM t WHERE 0")
        .fetch_optional(&p)
        .await
        .unwrap();
    assert_eq!(none, None);
    assert!(matches!(
        query_scalar::<i64>("SELECT i FROM t WHERE 0")
            .fetch_one(&p)
            .await,
        Err(Error::RowNotFound)
    ));
    // Empty text and a comment run nothing.
    assert_eq!(
        query("  -- nothing")
            .execute(&p)
            .await
            .unwrap()
            .rows_affected(),
        0
    );
});

both!(a_lone_surrogate_reads_as_5d2_does, {
    let p = pool();
    // `->>` gives an escaped lone surrogate as CESU-8 (`ED A0 80`), and the
    // `name` codecs read it as bytes; reading it as text fails, as sqlx's
    // `from_utf8` does (5d-2 Task 7 review).
    let sql = r#"SELECT '"\ud800x"' ->> '$' AS v"#;
    let bytes: Option<Vec<u8>> = query_scalar(sql).fetch_one(&p).await.unwrap();
    assert_eq!(bytes, Some(vec![0xED, 0xA0, 0x80, b'x']));
    let lossy = String::from_utf8_lossy(bytes.as_deref().unwrap()).into_owned();
    assert_eq!(lossy, "\u{FFFD}\u{FFFD}\u{FFFD}x");
    let row = query(sql).fetch_one(&p).await.unwrap();
    assert!(matches!(
        row.try_get::<String, _>("v"),
        Err(Error::ColumnDecode { .. })
    ));
    assert!(matches!(
        row.try_get_unchecked::<String, _>("v"),
        Err(Error::ColumnDecode { .. })
    ));
});

both!(errors_keep_sqlite_code_and_message, {
    let p = pool();
    query(
        "CREATE TABLE parent (id TEXT PRIMARY KEY);
         CREATE TABLE child (id TEXT PRIMARY KEY, parent TEXT NOT NULL REFERENCES parent(id));
         INSERT INTO parent VALUES ('a');",
    )
    .execute(&p)
    .await
    .unwrap();

    let e = db_error(
        query("INSERT INTO parent VALUES ('a')")
            .execute(&p)
            .await
            .unwrap_err(),
    );
    assert_eq!(e.code().as_deref(), Some("1555"));
    assert_eq!(e.message(), "UNIQUE constraint failed: parent.id");
    assert_eq!(e.kind(), ErrorKind::UniqueViolation);

    // Foreign keys are on.
    let e = db_error(
        query("INSERT INTO child VALUES ('c', 'nope')")
            .execute(&p)
            .await
            .unwrap_err(),
    );
    assert_eq!(e.code().as_deref(), Some("787"));
    assert_eq!(e.kind(), ErrorKind::ForeignKeyViolation);

    let e = db_error(
        query("INSERT INTO child (id) VALUES ('c')")
            .execute(&p)
            .await
            .unwrap_err(),
    );
    assert_eq!(e.code().as_deref(), Some("1299"));
    assert_eq!(e.kind(), ErrorKind::NotNullViolation);

    // Display matches sqlx's: "error returned from database: (code: N) …".
    let e = query("SELEC 1").execute(&p).await.unwrap_err();
    assert_eq!(
        e.to_string(),
        "error returned from database: (code: 1) near \"SELEC\": syntax error"
    );

    // `ROLLBACK` outside a transaction keeps SQLite's wording, which
    // `write.rs` matches.
    let e = db_error(query("ROLLBACK").execute(&p).await.unwrap_err());
    assert!(
        e.message().contains("no transaction is active"),
        "{}",
        e.message()
    );

    // Read-only: SQLITE_READONLY (8).
    query("PRAGMA query_only = ON").execute(&p).await.unwrap();
    let e = db_error(
        query("INSERT INTO parent VALUES ('b')")
            .execute(&p)
            .await
            .unwrap_err(),
    );
    assert_eq!(e.code().as_deref(), Some("8"));
    query("PRAGMA query_only = OFF").execute(&p).await.unwrap();

    // Busy: SQLITE_BUSY (5) from a second connection to one memdb file.
    let a = SqliteConnection::open_uri("file:/busy?vfs=memdb").unwrap();
    let mut b = SqliteConnection::open_uri("file:/busy?vfs=memdb").unwrap();
    let mut a = a;
    query("BEGIN IMMEDIATE").execute(&mut a).await.unwrap();
    let e = db_error(query("BEGIN IMMEDIATE").execute(&mut b).await.unwrap_err());
    assert_eq!(e.code().as_deref(), Some("5"));
    assert!(e.message().contains("locked"), "{}", e.message());

    // An unsigned value binds as an integer.
    let n: Result<i64, _> = query_scalar("SELECT 1 WHERE ? IS NULL")
        .bind(1_u32)
        .fetch_one(&p)
        .await;
    assert!(matches!(n, Err(Error::RowNotFound)));

    // A NUL in the SQL ends it, as SQLite reads it; it never loops.
    let n: i64 = query_scalar("SELECT 1\0; garbage")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(n, 1);
});

both!(
    rollback_after_a_failed_statement_leaves_the_connection_usable,
    {
        let p = pool();
        query("CREATE TABLE t (id INTEGER PRIMARY KEY, v BLOB)")
            .execute(&p)
            .await
            .unwrap();
        let mut conn = p.acquire().await.unwrap();

        // A failed statement inside a transaction: the transaction stays open
        // and ROLLBACK ends it.
        query("BEGIN IMMEDIATE").execute(&mut *conn).await.unwrap();
        query("INSERT INTO t (id) VALUES (1)")
            .execute(&mut *conn)
            .await
            .unwrap();
        query("INSERT INTO t (id) VALUES (1)")
            .execute(&mut *conn)
            .await
            .unwrap_err();
        query("ROLLBACK").execute(&mut *conn).await.unwrap();
        let n: i64 = query_scalar("SELECT COUNT(*) FROM t")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(n, 0);

        // SQLITE_FULL rolls the whole transaction back itself; the ROLLBACK
        // after it finds none, and the connection carries on.
        let pages: i64 = query_scalar("PRAGMA page_count")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        query(&format!("PRAGMA max_page_count = {}", pages + 2))
            .execute(&mut *conn)
            .await
            .unwrap();
        query("BEGIN IMMEDIATE").execute(&mut *conn).await.unwrap();
        let e = db_error(
            query("INSERT INTO t (id, v) VALUES (2, zeroblob(1000000))")
                .execute(&mut *conn)
                .await
                .unwrap_err(),
        );
        assert_eq!(e.code().as_deref(), Some("13"));
        let e = db_error(query("ROLLBACK").execute(&mut *conn).await.unwrap_err());
        assert!(e.message().contains("no transaction is active"));
        query("PRAGMA max_page_count = 1073741823")
            .execute(&mut *conn)
            .await
            .unwrap();
        query("BEGIN IMMEDIATE").execute(&mut *conn).await.unwrap();
        query("INSERT INTO t (id) VALUES (3)")
            .execute(&mut *conn)
            .await
            .unwrap();
        query("COMMIT").execute(&mut *conn).await.unwrap();
        drop(conn);

        // A transaction dropped without commit rolls back at once.
        let mut tx = p.begin_with("BEGIN IMMEDIATE").await.unwrap();
        query("INSERT INTO t (id) VALUES (4)")
            .execute(&mut *tx)
            .await
            .unwrap();
        drop(tx);
        let ids: Vec<(i64,)> = query_as("SELECT id FROM t ORDER BY id")
            .fetch_all(&p)
            .await
            .unwrap();
        assert_eq!(ids, vec![(3,)]);

        // While one caller holds the connection, the next waits for it.
        let held = p.acquire().await.unwrap();
        let mut waiting = Box::pin(p.acquire());
        assert!(futures::poll!(waiting.as_mut()).is_pending());
        drop(held);
        assert!(futures::poll!(waiting.as_mut()).is_ready());
    }
);

both!(serialize_then_deserialize_round_trips, {
    let p = pool();
    query(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, s TEXT); INSERT INTO t VALUES (1, 'Straße 東京')",
    )
    .execute(&p)
    .await
    .unwrap();
    let image = p.snapshot().unwrap();
    assert_eq!(&image[..16], b"SQLite format 3\0");
    // A new file gets SQLite's usual 4096-byte pages, as the desktop's do.
    assert_eq!(u16::from_be_bytes([image[16], image[17]]), 4096);

    let q = SqlitePool::open_in_memory(Some(&image)).unwrap();
    let s: String = query_scalar("SELECT s FROM t WHERE id = 1")
        .fetch_one(&q)
        .await
        .unwrap();
    assert_eq!(s, "Straße 東京");
    // It's writable and grows past the image.
    query("INSERT INTO t (s) SELECT s FROM t; INSERT INTO t (s) SELECT zeroblob(100000)")
        .execute(&q)
        .await
        .unwrap();
    let n: i64 = query_scalar("SELECT COUNT(*) FROM t")
        .fetch_one(&q)
        .await
        .unwrap();
    assert_eq!(n, 3);
    let fk: i64 = query_scalar("PRAGMA foreign_keys")
        .fetch_one(&q)
        .await
        .unwrap();
    assert_eq!(fk, 1);

    // A WAL file (a desktop file's header) opens too.
    let mut wal = image.clone();
    wal[18] = 2;
    wal[19] = 2;
    let w = SqlitePool::open_in_memory(Some(&wal)).unwrap();
    let n: i64 = query_scalar("SELECT COUNT(*) FROM t")
        .fetch_one(&w)
        .await
        .unwrap();
    assert_eq!(n, 1);

    // Bytes that aren't a database open, and SQLite refuses the first read
    // with "file is not a database" (26).
    let junk = SqlitePool::open_in_memory(Some(&[7u8; 4096])).unwrap();
    let e = db_error(
        query("SELECT COUNT(*) FROM sqlite_master")
            .execute(&junk)
            .await
            .unwrap_err(),
    );
    assert_eq!(e.code().as_deref(), Some("26"));

    // An empty image is a new database.
    let e = SqlitePool::open_in_memory(Some(&[])).unwrap();
    let n: i64 = query_scalar("SELECT COUNT(*) FROM sqlite_master")
        .fetch_one(&e)
        .await
        .unwrap();
    assert_eq!(n, 0);
    // A snapshot of an empty database is empty or a header, never an error.
    assert!(e.snapshot().is_ok());

    // No snapshot while a transaction is open: it would hold uncommitted
    // rows.
    let mut conn = p.acquire().await.unwrap();
    query("BEGIN").execute(&mut *conn).await.unwrap();
    assert!(p.snapshot().is_err());
    query("COMMIT").execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(p.snapshot().is_ok());
});

both!(the_commit_counter_moves_on_every_commit_and_only_then, {
    let p = pool();
    let start = p.commits();
    query("CREATE TABLE t (id INTEGER PRIMARY KEY)")
        .execute(&p)
        .await
        .unwrap();
    assert_eq!(p.commits(), start + 1, "DDL in autocommit commits");

    let _: i64 = query_scalar("SELECT COUNT(*) FROM t")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(p.commits(), start + 1, "a read commits nothing");

    query("INSERT INTO t VALUES (1)").execute(&p).await.unwrap();
    assert_eq!(p.commits(), start + 2);

    query("INSERT INTO t VALUES (1)")
        .execute(&p)
        .await
        .unwrap_err();
    assert_eq!(p.commits(), start + 2, "a failed write commits nothing");

    let mut conn = p.acquire().await.unwrap();
    query("BEGIN IMMEDIATE").execute(&mut *conn).await.unwrap();
    query("INSERT INTO t VALUES (2)")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(p.commits(), start + 2, "nothing until COMMIT");
    query("COMMIT").execute(&mut *conn).await.unwrap();
    assert_eq!(p.commits(), start + 3);

    query("BEGIN IMMEDIATE").execute(&mut *conn).await.unwrap();
    query("INSERT INTO t VALUES (3)")
        .execute(&mut *conn)
        .await
        .unwrap();
    query("ROLLBACK").execute(&mut *conn).await.unwrap();
    assert_eq!(p.commits(), start + 3, "a rollback commits nothing");

    // A read transaction commits nothing; a write transaction counts when
    // it commits, even one that changed nothing (SQLite's commit hook): the
    // count can run ahead of real changes, never behind them.
    query("BEGIN; COMMIT;").execute(&mut *conn).await.unwrap();
    assert_eq!(p.commits(), start + 3);
    query("BEGIN IMMEDIATE; COMMIT;")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(p.commits(), start + 4);
    drop(conn);

    // A COMMIT that fails (a deferred foreign key) commits nothing.
    query(
        "CREATE TABLE p (id INTEGER PRIMARY KEY);
         CREATE TABLE c (id INTEGER PRIMARY KEY,
             p INTEGER REFERENCES p(id) DEFERRABLE INITIALLY DEFERRED)",
    )
    .execute(&p)
    .await
    .unwrap();
    let before = p.commits();
    let mut conn = p.acquire().await.unwrap();
    query("BEGIN IMMEDIATE").execute(&mut *conn).await.unwrap();
    query("INSERT INTO c VALUES (1, 99)")
        .execute(&mut *conn)
        .await
        .unwrap();
    let e = db_error(query("COMMIT").execute(&mut *conn).await.unwrap_err());
    assert_eq!(e.code().as_deref(), Some("787"));
    query("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);
    assert_eq!(p.commits(), before, "a failed COMMIT commits nothing");

    // A write stopped at its first row in autocommit (`fetch_optional` on
    // `… RETURNING`) commits when its statement is finalized, and counts.
    let first: Option<(i64,)> = query_as("INSERT INTO t VALUES (10), (11) RETURNING id")
        .fetch_optional(&p)
        .await
        .unwrap();
    assert_eq!(first, Some((10,)));
    assert_eq!(p.commits(), before + 1, "the finalize's commit counts");
    let n: i64 = query_scalar("SELECT COUNT(*) FROM t WHERE id >= 10")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(n, 2);
    let before = p.commits();

    // A dropped transaction rolls back without counting.
    let mut tx = p.begin_with("BEGIN IMMEDIATE").await.unwrap();
    query("INSERT INTO t VALUES (4)")
        .execute(&mut *tx)
        .await
        .unwrap();
    drop(tx);
    assert_eq!(p.commits(), before);
});
