//! Native values (phase 2 Task 14): each cell decodes to the expected
//! `Value`, binding it back compares equal where `=` supports the type, NULL
//! binds into any column, and CRUD built by `MssqlDialect` finds rows by
//! every kind of key the UI holds. Same environment variable as `smoke.rs`:
//! `SEAQUEL_TEST_MSSQL`.

mod common;

use seaquel_engine::{Dialect, Driver, RowValues, SqlWithBindings, Value};
use seaquel_engine_mssql::MssqlDialect;
use seaquel_engine_testkit::{run_typed_cells, scratch_name, TypedCellCase};

fn dec(s: &str) -> Value {
    Value::Decimal(s.into())
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

/// `SELECT {literal} AS v`, bound back with `@P1 = {literal}` (T-SQL has
/// no boolean expressions in a select list, so through CASE).
fn lit(literal: &str, expected: Value) -> TypedCellCase {
    TypedCellCase::literal(literal, expected).bind_back(&format!(
        "SELECT CASE WHEN @P1 = {literal} THEN 1 ELSE 0 END"
    ))
}

/// A column of `ty` holding `literal`, read back from a scratch table.
/// `compare` is how the bind-back compares `@P1` with the stored `v`, or
/// `None` for no bind-back.
fn column(ty: &str, literal: &str, expected: Value, compare: Option<&str>) -> TypedCellCase {
    let table = scratch_name("t14_values_");
    TypedCellCase {
        name: format!("{ty} column"),
        setup: vec![
            format!("CREATE TABLE dbo.{table} (v {ty})"),
            format!("INSERT INTO dbo.{table} (v) VALUES ({literal})"),
        ],
        teardown: vec![format!("DROP TABLE IF EXISTS dbo.{table}")],
        select: format!("SELECT v FROM dbo.{table}"),
        literal: None,
        expected,
        bind_back: compare
            .map(|c| format!("SELECT CASE WHEN {c} THEN 1 ELSE 0 END FROM dbo.{table}")),
    }
}

fn cases() -> Vec<TypedCellCase> {
    let long = "x".repeat(5000);
    vec![
        // Decoding only: binding NULL is `null_binds_into_every_column_type`.
        TypedCellCase::literal("NULL", Value::Null),
        lit("CAST(1 AS bit)", Value::Bool(true)),
        lit("CAST(0 AS bit)", Value::Bool(false)),
        lit("CAST(255 AS tinyint)", Value::Int(255)),
        lit("CAST(-32768 AS smallint)", Value::Int(-32768)),
        lit("CAST(-2147483648 AS int)", Value::Int(i32::MIN.into())),
        // Beyond 2^53: exact (the wire tags it as a bigint).
        lit(
            "CAST(9007199254740993 AS bigint)",
            Value::Int(9_007_199_254_740_993),
        ),
        lit("CAST(9223372036854775807 AS bigint)", Value::Int(i64::MAX)),
        lit("CAST(-9223372036854775808 AS bigint)", Value::Int(i64::MIN)),
        // decimal/numeric: exact, scale kept, bound back as numeric.
        lit("CAST(-0.5 AS decimal(10,1))", dec("-0.5")),
        lit("CAST(12.5 AS decimal(10,2))", dec("12.50")),
        lit("CAST(0 AS decimal(5,2))", dec("0.00")),
        lit(
            "CAST('9999999999999999999999999999.9999999999' AS decimal(38,10))",
            dec("9999999999999999999999999999.9999999999"),
        ),
        lit(
            "CAST('-99999999999999999999999999999999999999' AS numeric(38,0))",
            dec("-99999999999999999999999999999999999999"),
        ),
        lit(
            "CAST('-0.0000000000000000000000000000000000001' AS numeric(38,37))",
            dec("-0.0000000000000000000000000000000000001"),
        ),
        // money/smallmoney: 4 places, exact below 2^39 (see decode::money).
        lit("CAST(12.5 AS money)", dec("12.5000")),
        lit("CAST(-0.0001 AS money)", dec("-0.0001")),
        lit("CAST(549755813887.9999 AS money)", dec("549755813887.9999")),
        lit(
            "CAST(-549755813887.9999 AS money)",
            dec("-549755813887.9999"),
        ),
        lit("CAST(214748.3647 AS smallmoney)", dec("214748.3647")),
        lit("CAST(-214748.3648 AS smallmoney)", dec("-214748.3648")),
        lit("CAST(0.1 AS float)", Value::Float(0.1)),
        lit("CAST(1e308 AS float)", Value::Float(1e308)),
        // real: the f32's shortest text. `@P1 = real` compares as float,
        // where 0.1 isn't the real 0.1, so real is checked on decoding only.
        TypedCellCase::literal("CAST(0.1 AS real)", Value::Float(0.1)),
        TypedCellCase::literal("CAST(16777217 AS real)", Value::Float(16_777_216.0)),
        lit("CAST(3.5 AS real)", Value::Float(3.5)),
        // Binary is always bytes, also when it holds text.
        lit("0x00FF10", Value::Bytes(vec![0x00, 0xFF, 0x10])),
        lit(
            "CAST('abc' AS varbinary(10))",
            Value::Bytes(b"abc".to_vec()),
        ),
        lit("CAST(0x AS varbinary(10))", Value::Bytes(vec![])),
        lit("CAST(0x01 AS binary(4))", Value::Bytes(vec![1, 0, 0, 0])),
        lit("N'東京 ✓ 🚀'", text("東京 ✓ 🚀")),
        lit("'abc'", text("abc")),
        lit("N''", text("")),
        lit("REPLICATE(CAST(N'x' AS nvarchar(max)), 5000)", text(&long)),
        lit(
            "CAST('6F9619FF-8B86-D011-B42D-00C04FC964FF' AS uniqueidentifier)",
            text("6f9619ff-8b86-d011-b42d-00c04fc964ff"),
        ),
        lit("CAST('2024-01-02' AS date)", text("2024-01-02")),
        lit("CAST('0001-01-01' AS date)", text("0001-01-01")),
        lit("CAST('9999-12-31' AS date)", text("9999-12-31")),
        lit(
            "CAST('03:04:05.1234567' AS time(7))",
            text("03:04:05.1234567"),
        ),
        lit("CAST('23:59:59' AS time(0))", text("23:59:59")),
        lit("CAST('00:00:00.5' AS time(3))", text("00:00:00.5")),
        lit(
            "CAST('2024-01-02 03:04:05.1234567' AS datetime2(7))",
            text("2024-01-02 03:04:05.1234567"),
        ),
        lit(
            "CAST('2024-01-02 03:04:05.1200000' AS datetime2(7))",
            text("2024-01-02 03:04:05.12"),
        ),
        lit(
            "CAST('2024-01-02 03:04:05' AS datetime2(0))",
            text("2024-01-02 03:04:05"),
        ),
        lit(
            "CAST('9999-12-31 23:59:59.9999999' AS datetime2(7))",
            text("9999-12-31 23:59:59.9999999"),
        ),
        lit(
            "CAST('0001-01-01 00:00:00' AS datetime2)",
            text("0001-01-01 00:00:00"),
        ),
        // datetime: 1/300 s ticks, printed to the millisecond as SQL Server does.
        lit(
            "CAST('2024-01-02 03:04:05.123' AS datetime)",
            text("2024-01-02 03:04:05.123"),
        ),
        lit(
            "CAST('2024-01-02 03:04:05.002' AS datetime)",
            text("2024-01-02 03:04:05.003"),
        ),
        lit(
            "CAST('2024-01-02 23:59:59.997' AS datetime)",
            text("2024-01-02 23:59:59.997"),
        ),
        lit(
            "CAST('1753-01-01 00:00:00' AS datetime)",
            text("1753-01-01 00:00:00"),
        ),
        lit(
            "CAST('2024-01-02 03:04:29' AS smalldatetime)",
            text("2024-01-02 03:04:00"),
        ),
        // datetimeoffset: local time and offset, as SQL Server prints it.
        lit(
            "CAST('2024-01-02 03:04:05.1234567 +01:00' AS datetimeoffset(7))",
            text("2024-01-02 03:04:05.1234567 +01:00"),
        ),
        lit(
            "CAST('2024-01-02 23:30:00 -05:30' AS datetimeoffset(0))",
            text("2024-01-02 23:30:00 -05:30"),
        ),
        lit(
            "CAST('9999-12-31 23:59:59.9999999 +14:00' AS datetimeoffset)",
            text("9999-12-31 23:59:59.9999999 +14:00"),
        ),
        lit(
            "CAST('0001-01-01 00:00:00 -14:00' AS datetimeoffset)",
            text("0001-01-01 00:00:00 -14:00"),
        ),
        // xml has no `=`.
        TypedCellCase::literal("CAST(N'<a b=\"1\">é</a>' AS xml)", text("<a b=\"1\">é</a>")),
        // Types a literal can't produce.
        column(
            "text",
            "'hello'",
            text("hello"),
            Some("CAST(v AS varchar(max)) = @P1"),
        ),
        column(
            "ntext",
            "N'héllo 東京'",
            text("héllo 東京"),
            Some("CAST(v AS nvarchar(max)) = @P1"),
        ),
        column(
            "image",
            "0x00FF",
            Value::Bytes(vec![0x00, 0xFF]),
            Some("CAST(v AS varbinary(max)) = @P1"),
        ),
        column(
            "varbinary(max)",
            "0x8081",
            Value::Bytes(vec![0x80, 0x81]),
            Some("v = @P1"),
        ),
        column(
            "varchar(20) COLLATE Latin1_General_CI_AS",
            "'café'",
            text("café"),
            Some("v = @P1"),
        ),
        column(
            "nvarchar(max)",
            &format!("N'{long}'"),
            text(&long),
            Some("v = @P1"),
        ),
        column(
            "decimal(38,10)",
            "-1.5",
            dec("-1.5000000000"),
            Some("v = @P1"),
        ),
        // money beyond 2^39 isn't exact (see decode::money): the maximum
        // reads one unit off, above the money range.
        column(
            "money",
            "922337203685477.5807",
            dec("922337203685477.5808"),
            None,
        )
        .named("money max (nearest, not exact)"),
    ]
}

/// The maximum money value reads as `922337203685477.5808` (see
/// `decode::money`), one unit above the money range. Bound back it is a
/// numeric: compared with the column, SQL Server converts the money to
/// numeric, so the comparison runs and matches nothing; assigned to the
/// column, it overflows (8115).
#[tokio::test]
async fn money_max_binds_back_as_an_out_of_range_numeric() {
    let Some(driver) = common::open().await else {
        return;
    };
    let table = scratch_name("t14_money_");
    driver
        .execute(
            &format!("CREATE TABLE dbo.{table} (m money NOT NULL)"),
            vec![],
        )
        .await
        .expect("create");
    let result = async {
        driver
            .execute(
                &format!("INSERT INTO dbo.{table} VALUES (922337203685477.5807)"),
                vec![],
            )
            .await
            .map_err(|e| format!("insert: {e:?}"))?;
        let read = driver
            .query(&format!("SELECT m FROM dbo.{table}"), vec![])
            .await
            .map_err(|e| format!("select: {e:?}"))?
            .rows
            .remove(0)
            .remove(0);
        if read != dec("922337203685477.5808") {
            return Err(format!("read {read:?}"));
        }
        let matched = driver
            .query(
                &format!("SELECT COUNT(*) FROM dbo.{table} WHERE m = @P1"),
                vec![read.clone()],
            )
            .await
            .map_err(|e| format!("compare: {e:?}"))?;
        if matched.rows != vec![vec![Value::Int(0)]] {
            return Err(format!("compare matched {:?}", matched.rows));
        }
        match driver
            .execute(&format!("UPDATE dbo.{table} SET m = @P1"), vec![read])
            .await
        {
            Err(e) if e.message.contains("8115") || e.message.contains("overflow") => Ok(()),
            other => Err(format!("assign: expected an overflow, got {other:?}")),
        }
    }
    .await;
    let _ = driver
        .execute(&format!("DROP TABLE dbo.{table}"), vec![])
        .await;
    result.unwrap();
}

#[tokio::test]
async fn values_round_trip() {
    let Some(config) = common::config() else {
        return;
    };
    common::drop_stale(
        &seaquel_engine_mssql::MssqlDriver::connect(&config)
            .await
            .expect("connect"),
        "t14_values_",
    )
    .await;
    run_typed_cells(&*seaquel_engine_mssql::engine(), &config, &cases()).await;
}

/// sql_variant and CLR types (geography, hierarchyid) make tiberius panic
/// on the column metadata: a clean `UNSUPPORTED_TYPE`, and the connection
/// works again on the next call. Cast to nvarchar, they read as text.
#[tokio::test]
async fn unsupported_types_fail_cleanly() {
    let Some(driver) = common::open().await else {
        return;
    };
    for sql in [
        "SELECT CAST(1 AS sql_variant) AS v",
        "SELECT geography::Point(47.65, -122.34, 4326) AS v",
        "SELECT hierarchyid::Parse('/1/2/') AS v",
    ] {
        let e = driver.query(sql, vec![]).await.expect_err(sql);
        assert_eq!(e.code, "UNSUPPORTED_TYPE", "{sql}: {e:?}");
        let r = driver.query("SELECT 1 AS one", vec![]).await.expect(sql);
        assert_eq!(r.rows, vec![vec![Value::Int(1)]]);
    }
    let r = driver
        .query(
            "SELECT CAST(CAST(1 AS sql_variant) AS nvarchar(10)), \
             hierarchyid::Parse('/1/2/').ToString(), geography::Point(1, 2, 4326).STAsText()",
            vec![],
        )
        .await
        .expect("cast to text");
    assert_eq!(
        r.rows,
        vec![vec![text("1"), text("/1/2/"), text("POINT (2 1)")]]
    );
}

/// A NULL parameter goes into any column, where a typed NULL wouldn't (an
/// nvarchar NULL into varbinary is error 257), and next to other parameters
/// keeps their numbering.
#[tokio::test]
async fn null_binds_into_every_column_type() {
    let Some(driver) = common::open().await else {
        return;
    };
    let table = scratch_name("t14_nulls_");
    let columns = [
        "vb varbinary(10)",
        "img image",
        "b binary(2)",
        "f float",
        "r real",
        "d decimal(10,2)",
        "m money",
        "dt datetime",
        "dt2 datetime2",
        "dto datetimeoffset",
        "t time",
        "da date",
        "u uniqueidentifier",
        "x xml",
        "nv nvarchar(10)",
        "tx text",
        "nt ntext",
        "bt bit",
        "sv sql_variant",
        "g geography",
        "h hierarchyid",
    ];
    driver
        .execute(
            &format!(
                "CREATE TABLE dbo.{table} (id int PRIMARY KEY, {})",
                columns.join(", ")
            ),
            vec![],
        )
        .await
        .expect("create");
    let result = async {
        driver
            .execute(
                &format!("INSERT INTO dbo.{table} (id, vb, f) VALUES (@P1, @P2, @P3)"),
                vec![Value::Int(1), Value::Bytes(vec![1]), Value::Float(1.5)],
            )
            .await
            .map_err(|e| format!("insert: {e:?}"))?;
        for c in columns {
            let name = c.split(' ').next().unwrap_or(c);
            let r = driver
                .execute(
                    &format!("UPDATE dbo.{table} SET {name} = @P1 WHERE id = @P2"),
                    vec![Value::Null, Value::Int(1)],
                )
                .await
                .map_err(|e| format!("{name} = NULL: {e:?}"))?;
            if r.rows_affected != 1 {
                return Err(format!("{name} = NULL affected {}", r.rows_affected));
            }
        }
        let r = driver
            .query(
                &format!("SELECT vb, f, CASE WHEN @P1 IS NULL THEN 1 ELSE 0 END FROM dbo.{table} WHERE id = @P2"),
                vec![Value::Null, Value::Int(1)],
            )
            .await
            .map_err(|e| format!("select: {e:?}"))?;
        if r.rows != vec![vec![Value::Null, Value::Null, Value::Int(1)]] {
            return Err(format!("read back {:?}", r.rows));
        }
        // An insert of NULL into varbinary, as the grid does.
        driver
            .execute(
                &format!("INSERT INTO dbo.{table} (id, vb) VALUES (@P1, @P2)"),
                vec![Value::Int(2), Value::Null],
            )
            .await
            .map_err(|e| format!("insert NULL: {e:?}"))?;
        Ok::<_, String>(())
    }
    .await;
    let _ = driver
        .execute(&format!("DROP TABLE dbo.{table}"), vec![])
        .await;
    result.unwrap();
}

/// A NULL parameter becomes the literal `NULL` (`bind::inline_nulls`), so
/// passing one as an `OUTPUT` argument fails with 179 (a constant can't be
/// `OUTPUT`), while a procedure's parameter name before `=` is left alone.
#[tokio::test]
async fn null_parameters_in_exec() {
    let Some(driver) = common::open().await else {
        return;
    };
    let proc = scratch_name("t14_proc_");
    driver
        .execute(
            &format!(
                "CREATE PROCEDURE dbo.{proc} @P1 int = 5, @x int = NULL OUTPUT AS SELECT @P1, @x"
            ),
            vec![],
        )
        .await
        .expect("create procedure");
    let result = async {
        let r = driver
            .query(
                &format!("EXEC dbo.{proc} @P1 = @P2, @x = @P1"),
                vec![Value::Null, Value::Int(7)],
            )
            .await
            .map_err(|e| format!("named arguments: {e:?}"))?;
        if r.rows != vec![vec![Value::Int(7), Value::Null]] {
            return Err(format!("named arguments read {:?}", r.rows));
        }
        match driver
            .query(
                &format!("EXEC dbo.{proc} @x = @P1 OUTPUT"),
                vec![Value::Null],
            )
            .await
        {
            Err(e) if e.message.contains("179") => Ok(()),
            other => Err(format!("OUTPUT: expected error 179, got {other:?}")),
        }
    }
    .await;
    let _ = driver
        .execute(&format!("DROP PROCEDURE dbo.{proc}"), vec![])
        .await;
    result.unwrap();
}

// ── CRUD by typed keys ───────────────────────────────────────────────────────

async fn run(driver: &dyn Driver, sql: SqlWithBindings, what: &str) -> Result<(), String> {
    let binds = sql.bind_values.unwrap_or_default();
    let r = driver
        .execute(&sql.sql, binds.clone())
        .await
        .map_err(|e| format!("{what}: {} {binds:?}: {e:?}", sql.sql))?;
    if r.rows_affected == 1 {
        Ok(())
    } else {
        Err(format!(
            "{what}: {} {binds:?} affected {} rows",
            sql.sql, r.rows_affected
        ))
    }
}

/// The only row of `table`, as the UI holds it.
async fn read_row(driver: &dyn Driver, table: &str) -> Result<RowValues, String> {
    let r = driver
        .query(&format!("SELECT * FROM dbo.[{table}]"), vec![])
        .await
        .map_err(|e| format!("reading {table}: {e:?}"))?;
    match r.rows.as_slice() {
        [row] => Ok(r.columns.iter().cloned().zip(row.iter().cloned()).collect()),
        rows => Err(format!("{table} has {} rows", rows.len())),
    }
}

fn cell<'a>(row: &'a RowValues, column: &str) -> &'a Value {
    &row.iter().find(|(c, _)| c == column).expect(column).1
}

fn same(actual: &Value, expected: &Value) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("expected {expected:?}, got {actual:?}"))
    }
}

/// For each key type: insert a row, read it back as the UI does, update,
/// set a column to NULL and to its default, delete, all by the key as read.
async fn crud_by_key(
    driver: &dyn Driver,
    ty: &str,
    key: Value,
    expect_read: Value,
) -> Result<(), String> {
    let d = MssqlDialect;
    let table = scratch_name("t14_crud_");
    let pk = vec!["k".to_string()];
    driver
        .execute(
            &format!(
                "CREATE TABLE dbo.[{table}] (k {ty} NOT NULL PRIMARY KEY, n int NULL DEFAULT 7, \
                 b varbinary(16) NULL, amount decimal(38,10) NULL, m money NULL)"
            ),
            vec![],
        )
        .await
        .map_err(|e| format!("create {ty}: {e:?}"))?;
    let result = async {
        run(
            driver,
            d.build_insert(
                "dbo",
                &table,
                &[
                    ("k".into(), key.clone()),
                    ("n".into(), Value::Int(1)),
                    ("b".into(), Value::Bytes(vec![0x00, 0xff])),
                ],
                None,
            ),
            "insert",
        )
        .await?;
        let row = read_row(driver, &table).await?;
        same(cell(&row, "k"), &expect_read)?;
        same(cell(&row, "b"), &Value::Bytes(vec![0x00, 0xff]))?;
        let amount = dec("-123456789012345678.1234567891");
        for (column, value) in [
            ("n", Value::Int(2)),
            ("b", Value::Bytes(vec![0x80, 0x00, 0xfe])),
            ("amount", amount.clone()),
            ("m", dec("12.3456")),
        ] {
            run(
                driver,
                d.build_update("dbo", &table, column, value, &pk, &row, None),
                &format!("update {column} by {ty}"),
            )
            .await?;
        }
        let after = read_row(driver, &table).await?;
        same(cell(&after, "n"), &Value::Int(2))?;
        same(cell(&after, "b"), &Value::Bytes(vec![0x80, 0x00, 0xfe]))?;
        same(cell(&after, "amount"), &amount)?;
        same(cell(&after, "m"), &dec("12.3456"))?;
        for column in ["b", "amount", "m"] {
            run(
                driver,
                d.build_update("dbo", &table, column, Value::Null, &pk, &after, None),
                &format!("set {column} NULL by {ty}"),
            )
            .await?;
        }
        run(
            driver,
            d.build_set_default("dbo", &table, "n", &pk, &after, None),
            &format!("set default by {ty}"),
        )
        .await?;
        let nulls = read_row(driver, &table).await?;
        for column in ["b", "amount", "m"] {
            same(cell(&nulls, column), &Value::Null)?;
        }
        same(cell(&nulls, "n"), &Value::Int(7))?;
        run(
            driver,
            d.build_delete("dbo", &table, &pk, &nulls, None),
            &format!("delete by {ty}"),
        )
        .await
    }
    .await;
    let _ = driver
        .execute(&format!("DROP TABLE dbo.[{table}]"), vec![])
        .await;
    result.map_err(|e| format!("{ty}: {e}"))
}

#[tokio::test]
async fn crud_on_typed_keys() {
    let Some(driver) = common::open().await else {
        return;
    };
    let cases = [
        ("nvarchar(20)", text("東京 ✓ 🚀"), text("東京 ✓ 🚀")),
        (
            "uniqueidentifier",
            text("6F9619FF-8B86-D011-B42D-00C04FC964FF"),
            text("6f9619ff-8b86-d011-b42d-00c04fc964ff"),
        ),
        (
            "datetime2(7)",
            text("2026-01-02 03:04:05.1234567"),
            text("2026-01-02 03:04:05.1234567"),
        ),
        (
            "datetimeoffset(7)",
            text("2026-01-02 03:04:05.1234567 -05:30"),
            text("2026-01-02 03:04:05.1234567 -05:30"),
        ),
        (
            "datetime",
            text("2026-01-02 03:04:05.002"),
            text("2026-01-02 03:04:05.003"),
        ),
        (
            "decimal(38,10)",
            dec("9999999999999999999999999999.9999999999"),
            dec("9999999999999999999999999999.9999999999"),
        ),
        (
            "bigint",
            Value::Int(9_007_199_254_740_993),
            Value::Int(9_007_199_254_740_993),
        ),
        (
            "varbinary(16)",
            Value::Bytes(vec![0x00, 0xff, 0x80]),
            Value::Bytes(vec![0x00, 0xff, 0x80]),
        ),
        (
            "money",
            dec("-549755813887.9999"),
            dec("-549755813887.9999"),
        ),
    ];
    let mut failures = Vec::new();
    for (ty, key, read) in cases {
        if let Err(e) = crud_by_key(&driver, ty, key, read).await {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
