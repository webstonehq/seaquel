//! Native values on both servers (phase 2 Task 6): each cell decodes to the
//! expected `Value`, binding it back compares equal where `=` supports the
//! type, and CRUD built by `MysqlDialect` finds rows by BIGINT UNSIGNED and
//! VARBINARY keys. Same environment variables as `smoke.rs`:
//! `SEAQUEL_TEST_MYSQL` and `SEAQUEL_TEST_MARIADB`.

use seaquel_engine::{ConnectConfig, Dialect, Driver, RowValues, SqlWithBindings, Value};
use seaquel_engine_mysql::MysqlDialect;
use seaquel_engine_testkit::{config_from_env, run_typed_cells, scratch_name, TypedCellCase};
use serde_json::json;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Server {
    Mysql,
    Mariadb,
}

impl Server {
    fn config(self) -> Option<ConnectConfig> {
        config_from_env(match self {
            Server::Mysql => "SEAQUEL_TEST_MYSQL",
            Server::Mariadb => "SEAQUEL_TEST_MARIADB",
        })
    }

    /// An `INSERT` that stores zero dates. MySQL's default `sql_mode` has
    /// `NO_ZERO_DATE` and `NO_ZERO_IN_DATE`, and a `SET_VAR` hint doesn't lift
    /// them, but `INSERT IGNORE` stores `0000-00-00` (with a warning). The pool
    /// rules out a session `SET sql_mode`.
    fn relaxed_insert(self, rest: &str) -> String {
        match self {
            Server::Mysql => format!("INSERT IGNORE {rest}"),
            Server::Mariadb => format!("SET STATEMENT sql_mode='' FOR INSERT {rest}"),
        }
    }
}

fn dec(s: &str) -> Value {
    Value::Decimal(s.into())
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

/// A column of `ty` holding `literal`, read back from a scratch table in
/// `seaquel_test`. Bind-back compares `?` with the stored value.
fn column_case(
    name: &str,
    ty: &str,
    insert: impl FnOnce(&str) -> String,
    expected: Value,
    bind_back: bool,
) -> TypedCellCase {
    let table = scratch_name("seaquel_values_");
    TypedCellCase {
        name: format!("{name} column"),
        setup: vec![
            format!("CREATE TABLE seaquel_test.{table} (v {ty})"),
            insert(&format!("INTO seaquel_test.{table} (v) VALUES")),
        ],
        teardown: vec![format!("DROP TABLE IF EXISTS seaquel_test.{table}")],
        select: format!("SELECT v FROM seaquel_test.{table}"),
        literal: None,
        expected,
        bind_back: bind_back
            .then(|| format!("SELECT ? = (SELECT v FROM seaquel_test.{table}) AS eq")),
    }
}

fn plain(name: &str, ty: &str, literal: &str, expected: Value, bind_back: bool) -> TypedCellCase {
    let literal = literal.to_string();
    column_case(
        name,
        ty,
        move |into| format!("INSERT {into} ({literal})"),
        expected,
        bind_back,
    )
}

fn cases(server: Server) -> Vec<TypedCellCase> {
    let lit = |literal: &str, expected: Value| {
        TypedCellCase::literal(literal, expected).bind_back_eq("?")
    };
    let dec65 = "12345678901234567890123456789012345.123456789012345678901234567890";
    let zero_ok = server == Server::Mariadb;
    let time_max = match server {
        Server::Mysql => "838:59:59",
        Server::Mariadb => "838:59:59.999999",
    };
    let mut cases = vec![
        lit("NULL", Value::Null).bind_back("SELECT ? IS NULL AS eq"),
        lit("CAST(-5 AS SIGNED)", Value::Int(-5)),
        lit("CAST(-9223372036854775808 AS SIGNED)", Value::Int(i64::MIN)),
        lit(
            "CAST(9223372036854775807 AS UNSIGNED)",
            Value::Int(i64::MAX),
        ),
        // Above i64: the digits, exactly (the UI shows an integral decimal as an integer).
        lit(
            "CAST(18446744073709551615 AS UNSIGNED)",
            dec("18446744073709551615"),
        ),
        // DECIMAL is the server's text: scale kept, no 28-digit limit.
        lit(
            "CAST('1.5' AS DECIMAL(65,30))",
            dec("1.500000000000000000000000000000"),
        ),
        lit(&format!("CAST('{dec65}' AS DECIMAL(65,30))"), dec(dec65)),
        lit("CAST('-0.5' AS DECIMAL(10,2))", dec("-0.50")),
        lit("CAST(10000 AS DECIMAL(10,0))", dec("10000")),
        // DOUBLE is exact; FLOAT shows the f32's shortest text (0.1, not
        // 0.10000000149011612). `? = FLOAT` compares as double and is false
        // for 0.1 on the server itself, so FLOAT is checked on decoding only.
        lit("CAST(0.1 AS DOUBLE)", Value::Float(0.1)),
        lit("CAST(1e308 AS DOUBLE)", Value::Float(1e308)),
        TypedCellCase::literal("CAST(0.1 AS FLOAT)", Value::Float(0.1)),
        TypedCellCase::literal("CAST(16777217 AS FLOAT)", Value::Float(16_777_216.0)),
        // Binary strings: Bytes unless they are UTF-8 text without control
        // characters (a `*_bin` text column looks the same on the wire).
        lit("x'00ff10'", Value::Bytes(vec![0x00, 0xff, 0x10])),
        lit("x'0102'", Value::Bytes(vec![1, 2])),
        lit("x'e282ac'", text("€")),
        lit("CAST('abc' AS BINARY)", text("abc")),
        lit("CAST('héllo' AS CHAR) COLLATE utf8mb4_bin", text("héllo")),
        lit("'tab\there'", text("tab\there")),
        lit("_latin1 x'e9'", text("é")),
        lit("DATE'2024-01-02'", text("2024-01-02")),
        lit("DATE'0999-12-31'", text("0999-12-31")),
        lit(
            "CAST('2024-01-02 03:04:05' AS DATETIME)",
            text("2024-01-02 03:04:05"),
        ),
        lit(
            "CAST('2024-01-02 03:04:05.5' AS DATETIME(6))",
            text("2024-01-02 03:04:05.5"),
        ),
        lit(
            "CAST('2024-01-02 00:00:00' AS DATETIME)",
            text("2024-01-02 00:00:00"),
        ),
        lit(
            "CAST('2024-01-02 03:04:05.000001' AS DATETIME(6))",
            text("2024-01-02 03:04:05.000001"),
        ),
        lit("CAST('01:02:03' AS TIME)", text("01:02:03")),
        lit("CAST('-01:02:03.25' AS TIME(2))", text("-01:02:03.25")),
        lit("CAST('25:00:00' AS TIME)", text("25:00:00")),
        lit("CAST('838:59:59' AS TIME)", text("838:59:59")),
        lit("CAST('-838:59:59' AS TIME)", text("-838:59:59")),
        lit("CAST('00:00:00' AS TIME)", text("00:00:00")),
        lit("CAST('-00:00:01' AS TIME)", text("-00:00:01")),
    ];

    // Table columns: types a literal can't produce.
    cases.extend([
        plain(
            "DECIMAL(65,30)",
            "DECIMAL(65,30)",
            "'0.000000000000000000000000000001'",
            dec("0.000000000000000000000000000001"),
            true,
        ),
        plain(
            "BIGINT UNSIGNED max",
            "BIGINT UNSIGNED",
            "18446744073709551615",
            dec("18446744073709551615"),
            true,
        ),
        plain(
            "INT UNSIGNED",
            "INT UNSIGNED",
            "4294967295",
            Value::Int(4_294_967_295),
            true,
        ),
        plain("TINYINT", "TINYINT", "-128", Value::Int(-128), true),
        // BIT(n) is its number, BIT(1) included.
        plain("BIT(1) one", "BIT(1)", "b'1'", Value::Int(1), true),
        plain("BIT(1) zero", "BIT(1)", "b'0'", Value::Int(0), true),
        plain(
            "BIT(12)",
            "BIT(12)",
            "b'101010101010'",
            Value::Int(0b1010_1010_1010),
            true,
        ),
        plain(
            "BIT(64) max",
            "BIT(64)",
            "b'1111111111111111111111111111111111111111111111111111111111111111'",
            dec("18446744073709551615"),
            true,
        ),
        // TINYINT(1) is BOOLEAN to sqlx: 0/1 are booleans, anything else keeps its number.
        plain(
            "TINYINT(1) true",
            "TINYINT(1)",
            "1",
            Value::Bool(true),
            true,
        ),
        plain(
            "TINYINT(1) false",
            "TINYINT(1)",
            "0",
            Value::Bool(false),
            true,
        ),
        plain("TINYINT(1) 5", "TINYINT(1)", "5", Value::Int(5), true),
        plain("TINYINT(1) -1", "TINYINT(1)", "-1", Value::Int(-1), true),
        plain(
            "TINYINT(1) UNSIGNED 200",
            "TINYINT(1) UNSIGNED",
            "200",
            Value::Int(200),
            true,
        ),
        plain("FLOAT", "FLOAT", "0.1", Value::Float(0.1), false),
        plain("DOUBLE", "DOUBLE", "0.1", Value::Float(0.1), true),
        plain(
            "GEOMETRY",
            "GEOMETRY",
            "ST_GeomFromText('POINT(1 2)')",
            Value::Bytes(
                [
                    &[0u8, 0, 0, 0, 1, 1, 0, 0, 0][..],
                    &1f64.to_le_bytes(),
                    &2f64.to_le_bytes(),
                ]
                .concat(),
            ),
            true,
        ),
        plain(
            "BLOB",
            "BLOB",
            "x'00ff'",
            Value::Bytes(vec![0x00, 0xff]),
            true,
        ),
        plain("BLOB with UTF-8 text", "BLOB", "'abc'", text("abc"), true),
        plain(
            "VARBINARY non-UTF-8",
            "VARBINARY(10)",
            "x'8081'",
            Value::Bytes(vec![0x80, 0x81]),
            true,
        ),
        plain(
            "BINARY(4) padded",
            "BINARY(4)",
            "'ab'",
            Value::Bytes(b"ab\0\0".to_vec()),
            true,
        ),
        plain(
            "utf8mb4_bin VARCHAR",
            "VARCHAR(10) COLLATE utf8mb4_bin",
            "'Ab'",
            text("Ab"),
            true,
        ),
        plain("TEXT", "TEXT", "'hello'", text("hello"), true),
        plain("DATE", "DATE", "'2024-01-02'", text("2024-01-02"), true),
        plain(
            "DATETIME(6)",
            "DATETIME(6)",
            "'2024-01-02 03:04:05.123456'",
            text("2024-01-02 03:04:05.123456"),
            true,
        ),
        plain(
            "DATETIME",
            "DATETIME",
            "'2024-01-02 23:59:59'",
            text("2024-01-02 23:59:59"),
            true,
        ),
        plain(
            "TIMESTAMP",
            "TIMESTAMP NULL",
            "'2024-01-02 03:04:05'",
            text("2024-01-02 03:04:05"),
            true,
        ),
        plain(
            "TIMESTAMP(6)",
            "TIMESTAMP(6) NULL",
            "'2024-01-02 03:04:05.5'",
            text("2024-01-02 03:04:05.5"),
            true,
        ),
        plain(
            "TIME(6) negative",
            "TIME(6)",
            "'-12:34:56.000789'",
            text("-12:34:56.000789"),
            true,
        ),
        plain(
            "TIME over 24h",
            "TIME",
            "'100:00:00'",
            text("100:00:00"),
            true,
        ),
        // The TIME range ends, fractions included (sqlx's MySqlTime rejects
        // MariaDB's 838:59:59.999999), and fractions of negative values.
        // MySQL's range stops at 838:59:59.000000.
        plain(
            "TIME(6) max",
            "TIME(6)",
            &format!("'{time_max}'"),
            text(time_max),
            true,
        ),
        plain(
            "TIME(6) min",
            "TIME(6)",
            &format!("'-{time_max}'"),
            text(&format!("-{time_max}")),
            true,
        ),
        plain(
            "TIME(6) near max",
            "TIME(6)",
            "'-838:59:58.999999'",
            text("-838:59:58.999999"),
            true,
        ),
        plain(
            "TIME(6) -0.5s",
            "TIME(6)",
            "'-00:00:00.5'",
            text("-00:00:00.5"),
            true,
        ),
        plain(
            "TIME(6) -1µs",
            "TIME(6)",
            "'-00:00:00.000001'",
            text("-00:00:00.000001"),
            true,
        ),
        plain(
            "TIME(3) -1.25s",
            "TIME(3)",
            "'-00:00:01.25'",
            text("-00:00:01.25"),
            true,
        ),
        plain(
            "TIME -34 days",
            "TIME",
            "'-816:00:00'",
            text("-816:00:00"),
            true,
        ),
        plain("YEAR", "YEAR", "2024", Value::Int(2024), true),
        plain("ENUM", "ENUM('x','y')", "'y'", text("y"), true),
        plain("SET", "SET('p','q')", "'p,q'", text("p,q"), true),
        // Zero dates: stored with a relaxed sql_mode, read as the server prints
        // them. They bind back only where sql_mode allows zero dates: MySQL's
        // default NO_ZERO_DATE turns `'0000-00-00'` into NULL, so `=` is NULL.
        column_case(
            "zero DATE",
            "DATE",
            |into| server.relaxed_insert(&format!("{into} ('0000-00-00')")),
            text("0000-00-00"),
            zero_ok,
        ),
        column_case(
            "zero DATETIME",
            "DATETIME",
            |into| server.relaxed_insert(&format!("{into} ('0000-00-00 00:00:00')")),
            text("0000-00-00 00:00:00"),
            zero_ok,
        ),
        column_case(
            "zero TIMESTAMP",
            "TIMESTAMP NULL",
            |into| server.relaxed_insert(&format!("{into} ('0000-00-00 00:00:00')")),
            text("0000-00-00 00:00:00"),
            zero_ok,
        ),
    ]);

    // A zero month: MySQL's NO_ZERO_IN_DATE turns it into 0000-00-00 even
    // with INSERT IGNORE, so only MariaDB stores it.
    if server == Server::Mariadb {
        cases.push(column_case(
            "zero-month DATE",
            "DATE",
            |into| server.relaxed_insert(&format!("{into} ('2024-00-15')")),
            text("2024-00-15"),
            true,
        ));
    }

    // JSON: MySQL's JSON type is `Json` from the server's text (arrays and
    // scalars included). MariaDB's JSON is LONGTEXT with a check constraint,
    // indistinguishable from text on the wire: it stays `Text`.
    match server {
        Server::Mysql => {
            let json_lit = |literal: &str, expected: serde_json::Value| {
                TypedCellCase::literal(&format!("CAST('{literal}' AS JSON)"), Value::Json(expected))
                    .bind_back(&format!(
                        "SELECT CAST(? AS JSON) = CAST('{literal}' AS JSON) AS eq"
                    ))
            };
            cases.extend([
                json_lit(
                    r#"{"a":1,"b":[true,null]}"#,
                    json!({"a": 1, "b": [true, null]}),
                ),
                json_lit("[1,2]", json!([1, 2])),
                json_lit("12345678901234567890", json!(12_345_678_901_234_567_890u64)),
                json_lit(r#"{"n":-9223372036854775808}"#, json!({"n": i64::MIN})),
                json_lit(r#""x""#, json!("x")),
                json_lit("null", serde_json::Value::Null),
                json_lit("0.1", json!(0.1)),
            ]);
            cases.push(column_case(
                "JSON",
                "JSON",
                |into| format!(r#"INSERT {into} ('{{"k":[1,"two"]}}')"#),
                Value::Json(json!({"k": [1, "two"]})),
                false,
            ));
        }
        Server::Mariadb => cases.extend([
            plain(
                "JSON",
                "JSON",
                r#"'{"a":12345678901234567890}'"#,
                text(r#"{"a":12345678901234567890}"#),
                true,
            ),
            lit("JSON_ARRAY(1,2)", text("[1, 2]")),
        ]),
    }
    cases
}

async fn values_round_trip(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    run_typed_cells(&*seaquel_engine_mysql::engine(), &config, &cases(server)).await;
}

#[tokio::test]
async fn mysql_values_round_trip() {
    values_round_trip(Server::Mysql).await;
}

#[tokio::test]
async fn mariadb_values_round_trip() {
    values_round_trip(Server::Mariadb).await;
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
        .query(&format!("SELECT * FROM seaquel_test.`{table}`"), vec![])
        .await
        .map_err(|e| format!("reading {table}: {e:?}"))?;
    match r.rows.as_slice() {
        [row] => Ok(r.columns.iter().cloned().zip(row.iter().cloned()).collect()),
        rows => Err(format!("{table} has {} rows", rows.len())),
    }
}

fn same(actual: &Value, expected: &Value) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("expected {expected:?}, got {actual:?}"))
    }
}

fn cell<'a>(row: &'a RowValues, column: &str) -> &'a Value {
    &row.iter().find(|(c, _)| c == column).expect(column).1
}

async fn typed_crud(driver: &dyn Driver, server: Server, u: &str, vb: &str) -> Result<(), String> {
    let d = MysqlDialect;
    let schema = "seaquel_test";
    let pk = vec!["id".to_string()];
    let dec65 = "-12345678901234567890123456789012345.123456789012345678901234567890";

    // BIGINT UNSIGNED key above i64: the row holds Decimal digits.
    let row = read_row(driver, u).await?;
    same(cell(&row, "id"), &dec("18446744073709551615"))?;
    same(cell(&row, "b"), &Value::Bytes(vec![0x00, 0xff]))?;
    run(
        driver,
        d.build_update(schema, u, "n", Value::Int(2), &pk, &row, None),
        "update n by u64 key",
    )
    .await?;
    run(
        driver,
        d.build_update(schema, u, "amount", dec(dec65), &pk, &row, None),
        "update DECIMAL(65,30)",
    )
    .await?;
    run(
        driver,
        d.build_update(
            schema,
            u,
            "flags",
            Value::Int(0b1010_1010_1010),
            &pk,
            &row,
            None,
        ),
        "update BIT(12)",
    )
    .await?;
    run(
        driver,
        d.build_update(
            schema,
            u,
            "b",
            Value::Bytes(vec![0x80, 0x00, 0xfe]),
            &pk,
            &row,
            None,
        ),
        "update VARBINARY",
    )
    .await?;
    let geom = cell(&row, "g").clone();
    if !matches!(geom, Value::Bytes(_)) {
        return Err(format!("geometry decoded as {geom:?}"));
    }
    run(
        driver,
        d.build_update(schema, u, "g", geom.clone(), &pk, &row, None),
        "update GEOMETRY with its bytes",
    )
    .await?;
    if server == Server::Mysql {
        run(
            driver,
            d.build_update(
                schema,
                u,
                "j",
                Value::Json(json!({"big": 18_446_744_073_709_551_615u64, "a": [1]})),
                &pk,
                &row,
                None,
            ),
            "update JSON",
        )
        .await?;
    }
    let after = read_row(driver, u).await?;
    same(cell(&after, "n"), &Value::Int(2))?;
    same(cell(&after, "amount"), &dec(dec65))?;
    same(cell(&after, "flags"), &Value::Int(0b1010_1010_1010))?;
    same(cell(&after, "b"), &Value::Bytes(vec![0x80, 0x00, 0xfe]))?;
    same(cell(&after, "g"), &geom)?;
    if server == Server::Mysql {
        same(
            cell(&after, "j"),
            &Value::Json(json!({"a": [1], "big": 18_446_744_073_709_551_615u64})),
        )?;
    }

    // NULL into binary, geometry, BIT and JSON columns; then the default.
    for column in ["b", "g", "flags", "j", "amount"] {
        run(
            driver,
            d.build_update(schema, u, column, Value::Null, &pk, &after, None),
            "set NULL",
        )
        .await?;
    }
    run(
        driver,
        d.build_set_default(schema, u, "n", &pk, &after, None),
        "set default by u64 key",
    )
    .await?;
    let nulls = read_row(driver, u).await?;
    for column in ["b", "g", "flags", "j", "amount"] {
        same(cell(&nulls, column), &Value::Null)?;
    }
    same(cell(&nulls, "n"), &Value::Int(7))?;
    run(
        driver,
        d.build_delete(schema, u, &pk, &nulls, None),
        "delete by u64 key",
    )
    .await?;

    // Insert by the same key, as the UI would send it back.
    run(
        driver,
        d.build_insert(
            schema,
            u,
            &[
                ("id".into(), dec("18446744073709551615")),
                ("n".into(), Value::Int(1)),
            ],
            None,
        ),
        "insert u64 key",
    )
    .await?;
    same(
        cell(&read_row(driver, u).await?, "id"),
        &dec("18446744073709551615"),
    )?;

    // VARBINARY key that isn't UTF-8.
    let row = read_row(driver, vb).await?;
    same(cell(&row, "id"), &Value::Bytes(vec![0x00, 0xff, 0x80]))?;
    run(
        driver,
        d.build_update(schema, vb, "n", Value::Int(2), &pk, &row, None),
        "update by VARBINARY key",
    )
    .await?;
    run(
        driver,
        d.build_set_default(schema, vb, "n", &pk, &row, None),
        "set default by VARBINARY key",
    )
    .await?;
    run(
        driver,
        d.build_delete(schema, vb, &pk, &row, None),
        "delete by VARBINARY key",
    )
    .await?;
    Ok(())
}

async fn crud_on_typed_keys(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let driver = seaquel_engine_mysql::engine()
        .open(&config)
        .await
        .expect("open");
    let u = scratch_name("seaquel_crud_u_");
    let vb = scratch_name("seaquel_crud_vb_");
    // MariaDB's JSON is LONGTEXT; the column exists on both so NULL is tried on both.
    for sql in [
        format!(
            "CREATE TABLE seaquel_test.`{u}` (id BIGINT UNSIGNED PRIMARY KEY, n INT DEFAULT 7, \
             amount DECIMAL(65,30), flags BIT(12), b VARBINARY(16), g GEOMETRY, j JSON)"
        ),
        format!(
            "INSERT INTO seaquel_test.`{u}` VALUES (18446744073709551615, 1, '1.5', b'1', x'00ff', \
             ST_GeomFromText('POINT(1 2)'), '{{\"a\":1}}')"
        ),
        format!("CREATE TABLE seaquel_test.`{vb}` (id VARBINARY(16) PRIMARY KEY, n INT DEFAULT 7)"),
        format!("INSERT INTO seaquel_test.`{vb}` VALUES (x'00ff80', 1)"),
    ] {
        driver
            .execute(&sql, vec![])
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    let result = typed_crud(&*driver, server, &u, &vb).await;
    for table in [&u, &vb] {
        let _ = driver
            .execute(
                &format!("DROP TABLE IF EXISTS seaquel_test.`{table}`"),
                vec![],
            )
            .await;
    }
    driver.close().await.expect("close");
    result.unwrap_or_else(|e| panic!("{e}"));
}

#[tokio::test]
async fn mysql_crud_on_typed_keys() {
    crud_on_typed_keys(Server::Mysql).await;
}

#[tokio::test]
async fn mariadb_crud_on_typed_keys() {
    crud_on_typed_keys(Server::Mariadb).await;
}

// ── DECIMAL parameters compare as DECIMAL ────────────────────────────────────

/// A `Decimal` parameter is a DECIMAL, so `=` is exact. As a plain string,
/// MariaDB would compare it as DOUBLE, where 0.1 equals 0.1000…0001 (MySQL
/// 8.4 compares that string exactly).
async fn decimal_binds_as_decimal(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let driver = seaquel_engine_mysql::engine()
        .open(&config)
        .await
        .expect("open");
    let eq = |p: Value, literal: &'static str| {
        let driver = driver.clone();
        async move {
            let r = driver
                .query(&format!("SELECT ? = {literal} AS eq"), vec![p])
                .await
                .expect("compare");
            r.rows[0][0].clone()
        }
    };
    let close = "CAST('0.100000000000000000000000000001' AS DECIMAL(65,30))";
    let as_decimal = eq(dec("0.1"), close).await;
    let as_text = eq(text("0.1"), close).await;
    let exact = eq(dec("0.100000000000000000000000000001"), close).await;
    driver.close().await.expect("close");
    assert_eq!(
        as_decimal,
        Value::Int(0),
        "a Decimal parameter compares exactly"
    );
    if server == Server::Mariadb {
        assert_eq!(
            as_text,
            Value::Int(1),
            "text compares as DOUBLE (why Decimal isn't bound as text)"
        );
    }
    assert_eq!(exact, Value::Int(1));
}

#[tokio::test]
async fn mysql_decimal_binds_as_decimal() {
    decimal_binds_as_decimal(Server::Mysql).await;
}

#[tokio::test]
async fn mariadb_decimal_binds_as_decimal() {
    decimal_binds_as_decimal(Server::Mariadb).await;
}
