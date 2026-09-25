//! Native values (phase 2 Task 16): each DuckDB type decodes to its own
//! `Value` kind; temporal text is exactly what DuckDB prints; binding a
//! decoded value back compares equal; and rows keyed by any of these types
//! are found again by `WHERE k = ?`. No server needed: in-memory databases.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use seaquel_engine::{ConnectConfig, Driver, Value};
use seaquel_engine_testkit::{run_typed_cells, scratch_name, TypedCellCase};
use serde_json::json;

fn config() -> ConnectConfig {
    serde_json::from_value(json!({ "driver": "duckdb", "path": ":memory:" })).unwrap()
}

async fn open() -> Arc<dyn Driver> {
    seaquel_engine_duckdb::engine()
        .open(&config())
        .await
        .unwrap()
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn dec(s: &str) -> Value {
    Value::Decimal(s.into())
}

/// Queries whose rows are `(v, CAST(v AS VARCHAR))`. `v` must decode to
/// that exact text (numbers: to digits of it), under any session time zone
/// for everything but TIMESTAMPTZ, which prints in UTC.
const CAST_CASES: &[&str] = &[
    // DATE: every era, both calendars' edges, the limits and infinity.
    "SELECT DATE '1970-01-01' + (i * 7919)::INTEGER AS v FROM range(-270000, 270000, 997) t(i)",
    "SELECT DATE '0001-01-01' + i::INTEGER AS v FROM range(-800, 800) t(i)",
    "SELECT v::DATE AS v FROM (VALUES ('infinity'), ('-infinity'), ('5881580-07-10'), \
     ('5877642-06-25 (BC)'), ('0044-03-15 (BC)'), ('9999-12-31'), ('10000-01-01'), \
     ('2000-02-29'), ('1900-03-01')) t(v)",
    // TIME, TIME_NS, TIMETZ.
    "SELECT TIME '00:00:00' + to_microseconds(i * 7_654_321_987) AS v FROM range(0, 12000) t(i)",
    "SELECT v::TIME AS v FROM (VALUES ('24:00:00'), ('23:59:59.999999'), ('00:00:00.000001'), \
     ('12:00:00.1')) t(v)",
    "SELECT v::TIME_NS AS v FROM (VALUES ('12:00:00.123456789'), ('00:00:00.000000001'), \
     ('23:59:59.999999999'), ('12:00:00.5'), ('12:00:00'), ('24:00:00')) t(v)",
    "SELECT v::TIMETZ AS v FROM (VALUES ('12:00:00+00'), ('12:00:00+02'), ('12:00:00-05:30'), \
     ('23:59:59.999999+15:59:59'), ('00:00:00-15:59:59'), ('12:00:00.5+01:02:03'), \
     ('24:00:00-01'), ('00:00:00+00')) t(v)",
    // TIMESTAMP in every unit.
    "SELECT TIMESTAMP '1970-01-01' + to_microseconds(i * 987_654_321_987_654) AS v \
     FROM range(-9000, 9000, 7) t(i)",
    "SELECT TIMESTAMP '0001-01-01' + to_microseconds(i * 3_600_000_001) AS v FROM range(-500, 500) t(i)",
    "SELECT v::TIMESTAMP AS v FROM (VALUES ('infinity'), ('-infinity'), \
     ('294247-01-10 04:00:54.775806'), ('290309-12-22 (BC) 00:00:00'), \
     ('0044-03-15 (BC) 12:30:00.25')) t(v)",
    "SELECT (TIMESTAMP '1970-01-01' + to_microseconds(i * 987_654_321_987))::TIMESTAMP_S AS v \
     FROM range(-9000, 9000, 7) t(i)",
    "SELECT (TIMESTAMP '1970-01-01' + to_microseconds(i * 987_654_321_987))::TIMESTAMP_MS AS v \
     FROM range(-9000, 9000, 7) t(i)",
    "SELECT (TIMESTAMP_NS '1970-01-01' + to_microseconds(i * 987_654_321))::TIMESTAMP_NS \
     + to_microseconds(0) AS v FROM range(-9000, 9000, 7) t(i)",
    "SELECT v::TIMESTAMP_NS AS v FROM (VALUES ('2024-01-01 00:00:00.123456789'), \
     ('1677-09-22 00:00:00'), ('2262-04-11 23:47:16.854775806'), ('1969-12-31 23:59:59.999999999'), \
     ('infinity'), ('-infinity')) t(v)",
    "SELECT v::TIMESTAMP_S AS v FROM (VALUES ('infinity'), ('-infinity'), ('0044-03-15 (BC)')) t(v)",
    "SELECT v::TIMESTAMP_MS AS v FROM (VALUES ('infinity'), ('-infinity'), \
     ('2024-01-01 00:00:00.5')) t(v)",
    // TIMESTAMPTZ (the session is UTC for these cases).
    "SELECT (TIMESTAMP '1970-01-01' + to_microseconds(i * 987_654_321_987_654))::TIMESTAMPTZ AS v \
     FROM range(-9000, 9000, 7) t(i)",
    "SELECT v::TIMESTAMPTZ AS v FROM (VALUES ('infinity'), ('-infinity'), \
     ('2024-01-01 12:00:00+05:30'), ('0044-03-15 (BC) 12:30:00.25+00')) t(v)",
    // INTERVAL: each part's sign on its own, micros, hours past 24.
    "SELECT to_months(m) + to_days(d) + to_microseconds(us) AS v \
     FROM range(-25, 26, 7) a(m), range(-3, 4) b(d), \
     (VALUES (0), (1), (-1), (1_500_000), (-1_500_000), (86_400_000_000), (-130_000_000_001), \
     (123_456)) c(us)",
    "SELECT v::INTERVAL AS v FROM (VALUES ('1 year'), ('-1 year'), ('1 month'), ('2 years 1 month'), \
     ('00:00:00'), ('1 day'), ('-1 day'), ('1000000 hours'), ('-1 year -2 months -3 days -04:05:06.789')) t(v)",
    // Integers and exact numbers.
    "SELECT v::HUGEINT AS v FROM (VALUES ('0'), ('-1'), ('9223372036854775807'), \
     ('9223372036854775808'), ('-9223372036854775809'), ('170141183460469231731687303715884105727'), \
     ('-170141183460469231731687303715884105728')) t(v)",
    "SELECT v::UHUGEINT AS v FROM (VALUES ('0'), ('9223372036854775808'), \
     ('170141183460469231731687303715884105728'), ('340282366920938463463374607431768211455')) t(v)",
    "SELECT v::UBIGINT AS v FROM (VALUES ('0'), ('9223372036854775807'), ('9223372036854775808'), \
     ('18446744073709551615')) t(v)",
    "SELECT v::BIGNUM AS v FROM (VALUES ('0'), ('7'), ('-7'), ('255'), ('-255'), ('256'), ('-256'), \
     ('9223372036854775807'), ('-9223372036854775808'), ('-9223372036854775809'), \
     ('340282366920938463463374607431768211455'), ('-170141183460469231731687303715884105729'), \
     ('123456789012345678901234567890123456789012345678901234567890'), \
     ('-123456789012345678901234567890123456789012345678901234567890')) t(v)",
    "SELECT v AS v FROM (VALUES (1.50::DECIMAL(4,2)), (-0.05), (0.00), (123456.789), (-1)) t(v)",
    "SELECT v::DECIMAL(38,10) AS v FROM (VALUES ('-9999999999999999999999999999.9999999999'), \
     ('0.0000000001'), ('0')) t(v)",
    "SELECT v::DECIMAL(18,3) AS v FROM (VALUES ('-123456789012345.678'), ('0.001')) t(v)",
    // Text-like.
    "SELECT gen_random_uuid() AS v FROM range(50)",
    "SELECT v::UUID AS v FROM (VALUES ('00000000-0000-0000-0000-000000000000'), \
     ('ffffffff-ffff-ffff-ffff-ffffffffffff'), ('80000000-0000-0000-0000-000000000001'), \
     ('7fffffff-ffff-ffff-ffff-ffffffffffff')) t(v)",
    "SELECT v::BIT AS v FROM (VALUES ('0'), ('1'), ('101010'), ('11111111'), ('100000000'), \
     ('0000000000000000001'), ('10101010101010101010101010101010101')) t(v)",
    "SELECT v::mood AS v FROM (VALUES ('sad'), ('ok'), ('happy')) t(v)",
];

/// The decoded text of `v` for comparing with DuckDB's cast.
fn as_text(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) | Value::Decimal(s) => Some(s.clone()),
        Value::Int(i) => Some(i.to_string()),
        _ => None,
    }
}

#[tokio::test]
async fn text_matches_what_duckdb_prints() {
    compare_with_cast(false).await;
}

/// The same with `arrow_lossless_conversion` reset, as a session may do:
/// HUGEINT, UHUGEINT and UUID arrive in their lossy Arrow forms and still
/// decode exactly. TIMETZ loses its offset there, so it's left out (see
/// `lossy_timetz_has_no_offset`).
#[tokio::test]
async fn text_matches_what_duckdb_prints_without_lossless_arrow() {
    compare_with_cast(true).await;
}

async fn compare_with_cast(lossy: bool) {
    let driver = open().await;
    driver
        .execute("CREATE TYPE mood AS ENUM ('sad', 'ok', 'happy')", vec![])
        .await
        .unwrap();
    // TIMESTAMPTZ prints in the session's zone; the decoder always prints UTC.
    driver
        .execute("SET TimeZone = 'UTC'", vec![])
        .await
        .unwrap();
    if lossy {
        driver
            .execute("RESET arrow_lossless_conversion", vec![])
            .await
            .unwrap();
    }
    let mut failures = Vec::new();
    let mut checked = 0;
    for select in CAST_CASES {
        if lossy && select.contains("TIMETZ") {
            continue;
        }
        let sql = format!("SELECT v, CAST(v AS VARCHAR) AS s FROM ({select}) q");
        let r = match driver.query(&sql, vec![]).await {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("{select}: {e:?}"));
                continue;
            }
        };
        assert!(!r.rows.is_empty(), "{select}: no rows");
        for row in &r.rows {
            checked += 1;
            let expected = row[1]
                .as_str()
                .unwrap_or_else(|| panic!("{select}: {row:?}"));
            if as_text(&row[0]).as_deref() != Some(expected) {
                failures.push(format!(
                    "{select}\n  decoded {:?}, DuckDB prints {expected:?}",
                    row[0]
                ));
            }
        }
    }
    eprintln!("{checked} values compared with CAST(v AS VARCHAR) (lossy Arrow: {lossy})");
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Without `arrow_lossless_conversion`, DuckDB sends a TIMETZ as its local
/// time alone, and booleans as Arrow booleans.
#[tokio::test]
async fn lossy_timetz_has_no_offset() {
    let driver = open().await;
    driver
        .execute("RESET arrow_lossless_conversion", vec![])
        .await
        .unwrap();
    let r = driver
        .query(
            "SELECT TIMETZ '12:00:00+02' AS t, true AS b, [false] AS l",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(
        r.rows[0],
        vec![
            text("12:00:00"),
            Value::Bool(true),
            Value::Array(vec![Value::Bool(false)])
        ]
    );
}

/// The one deliberate difference from DuckDB's own text: a DECIMAL whose
/// precision equals its scale. DuckDB prints DECIMAL(2, 2) -0.05 as `-.05`;
/// the decoder writes `-0.05`, which is the same number and binds back
/// (`typed_cells` checks both).
#[tokio::test]
async fn decimal_precision_equal_to_scale_keeps_a_leading_zero() {
    let driver = open().await;
    let r = driver
        .query(
            "SELECT v, CAST(v AS VARCHAR) FROM (VALUES (-0.05::DECIMAL(2,2)), (0.5::DECIMAL(2,2))) t(v)",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.rows[0], vec![dec("-0.05"), text("-.05")]);
    assert_eq!(r.rows[1], vec![dec("0.50"), text(".50")]);
}

/// A BLOB inside a STRUCT is DuckDB's escaped text; the rest of the nested
/// scalars are their top-level text.
#[tokio::test]
async fn nested_blobs_are_duckdbs_text() {
    let driver = open().await;
    let r = driver
        .query(
            "SELECT {'b': b} AS v, CAST(b AS VARCHAR) AS s FROM (VALUES ('\\x00\\xFFa\\x5C\\x27\\x22 ~'::BLOB), \
             (''::BLOB), ('plain'::BLOB)) t(b)",
            vec![],
        )
        .await
        .unwrap();
    for row in &r.rows {
        let Value::Json(obj) = &row[0] else {
            panic!("{row:?}")
        };
        assert_eq!(obj["b"].as_str(), row[1].as_str(), "{row:?}");
    }
}

// ── Typed cells ──────────────────────────────────────────────────────────────

fn lit(literal: &str, expected: Value) -> TypedCellCase {
    TypedCellCase::literal(literal, expected).bind_back_eq("?")
}

/// duckdb-rs can't bind LIST parameters, so lists aren't bound back.
fn list(literal: &str, expected: Value) -> TypedCellCase {
    TypedCellCase::literal(literal, expected)
}

/// A table keyed by `ty` holding `literal`: the decoded key must find its
/// row with `WHERE k = ?`, which is what CRUD sends.
fn key_case(ty: &str, literal: &str, expected: Value) -> TypedCellCase {
    let table = scratch_name("values_");
    TypedCellCase {
        name: format!("{ty} key"),
        setup: vec![
            // DuckDB can't index an INTERVAL.
            format!(
                "CREATE TABLE {table} (k {ty}{}, n INTEGER)",
                if ty == "INTERVAL" { "" } else { " PRIMARY KEY" }
            ),
            format!("INSERT INTO {table} VALUES ({literal}, 1)"),
        ],
        teardown: vec![format!("DROP TABLE IF EXISTS {table}")],
        select: format!("SELECT k FROM {table}"),
        literal: None,
        expected,
        bind_back: Some(format!("SELECT count(*) = 1 FROM {table} WHERE k = ?")),
    }
}

fn cases() -> Vec<TypedCellCase> {
    let json_value = |j: serde_json::Value| Value::Json(j);
    vec![
        TypedCellCase::literal("NULL", Value::Null).bind_back("SELECT ? IS NULL"),
        lit("true", Value::Bool(true)),
        lit("42::TINYINT", Value::Int(42)),
        lit("(-32768)::SMALLINT", Value::Int(-32768)),
        lit("2147483647::INTEGER", Value::Int(2_147_483_647)),
        lit("(-9223372036854775808)::BIGINT", Value::Int(i64::MIN)),
        lit("255::UTINYINT", Value::Int(255)),
        lit("4294967295::UINTEGER", Value::Int(4_294_967_295)),
        lit("9223372036854775807::UBIGINT", Value::Int(i64::MAX)),
        lit("18446744073709551615::UBIGINT", dec("18446744073709551615")),
        // HUGEINT/UHUGEINT: Int when it fits, digits otherwise.
        lit("12::HUGEINT", Value::Int(12)),
        lit("-9223372036854775809::HUGEINT", dec("-9223372036854775809")),
        lit(
            "'170141183460469231731687303715884105727'::HUGEINT",
            dec("170141183460469231731687303715884105727"),
        ),
        lit(
            "'-170141183460469231731687303715884105728'::HUGEINT",
            dec("-170141183460469231731687303715884105728"),
        ),
        lit("7::UHUGEINT", Value::Int(7)),
        // 39 digits: binds as text, which DuckDB casts.
        lit(
            "'340282366920938463463374607431768211455'::UHUGEINT",
            dec("340282366920938463463374607431768211455"),
        ),
        lit("-7::BIGNUM", Value::Int(-7)),
        lit(
            "'-123456789012345678901234567890123456789012'::BIGNUM",
            dec("-123456789012345678901234567890123456789012"),
        ),
        // DECIMAL keeps its scale and binds exactly.
        lit("1.50::DECIMAL(4,2)", dec("1.50")),
        lit("-0.05::DECIMAL(3,2)", dec("-0.05")),
        lit("0::DECIMAL(10,3)", dec("0.000")),
        // Precision = scale: DuckDB prints `-.05` (see
        // `decimal_precision_equal_to_scale_keeps_a_leading_zero`).
        lit("-0.05::DECIMAL(2,2)", dec("-0.05")),
        lit(
            "'99999999999999999999999999999999999999'::DECIMAL(38,0)",
            dec("99999999999999999999999999999999999999"),
        ),
        lit(
            "'-9999999999999999999999999999.9999999999'::DECIMAL(38,10)",
            dec("-9999999999999999999999999999.9999999999"),
        ),
        // Floats, NaN and infinities included.
        lit("1.5::DOUBLE", Value::Float(1.5)),
        lit("0.1::DOUBLE", Value::Float(0.1)),
        // FLOAT: widened exactly, so it binds back (see `decode::widen`).
        lit("0.1::FLOAT", Value::Float(f64::from(0.1_f32))),
        lit("1e-40::FLOAT", Value::Float(f64::from(1e-40_f32))),
        lit("3.4028235e38::FLOAT", Value::Float(f64::from(f32::MAX))),
        lit("16777217::FLOAT", Value::Float(16_777_216.0)),
        lit("'nan'::DOUBLE", Value::Float(f64::NAN)),
        lit("'inf'::DOUBLE", Value::Float(f64::INFINITY)),
        lit("'-inf'::FLOAT", Value::Float(f64::NEG_INFINITY)),
        // Text.
        lit("'héllo €'", text("héllo €")),
        lit("''", text("")),
        lit("'ok'::ENUM('sad', 'ok', 'happy')", text("ok")),
        lit(
            "'00000000-0000-4000-8000-000000000000'::UUID",
            text("00000000-0000-4000-8000-000000000000"),
        ),
        lit("'101010'::BIT", text("101010")),
        TypedCellCase::literal("'{\"a\": [1, 2.5, null]}'::JSON", json_value(json!({"a": [1, 2.5, null]})))
            .bind_back("SELECT ?::JSON = '{\"a\":[1,2.5,null]}'::JSON"),
        // Bytes.
        lit("'\\x00\\xFFa'::BLOB", Value::Bytes(vec![0, 0xff, b'a'])),
        lit("''::BLOB", Value::Bytes(vec![])),
        // Dates and times: DuckDB's own text.
        lit("DATE '2024-01-01'", text("2024-01-01")),
        lit("DATE '0044-03-15 (BC)'", text("0044-03-15 (BC)")),
        lit("DATE '10000-01-01'", text("10000-01-01")),
        lit("'infinity'::DATE", text("infinity")),
        lit("'-infinity'::DATE", text("-infinity")),
        lit("TIME '12:00:00.5'", text("12:00:00.5")),
        lit("TIME '24:00:00'", text("24:00:00")),
        lit("TIME_NS '12:00:00.123456789'", text("12:00:00.123456789")),
        lit("TIMETZ '12:00:00+02'", text("12:00:00+02")),
        lit("TIMETZ '12:00:00.25-05:30'", text("12:00:00.25-05:30")),
        lit("TIMESTAMP '2024-01-01 12:00:00'", text("2024-01-01 12:00:00")),
        lit("TIMESTAMP '2024-01-01 12:00:00.123456'", text("2024-01-01 12:00:00.123456")),
        lit("TIMESTAMP '0044-03-15 (BC) 12:00:00'", text("0044-03-15 (BC) 12:00:00")),
        lit("'infinity'::TIMESTAMP", text("infinity")),
        lit("'-infinity'::TIMESTAMP", text("-infinity")),
        lit("TIMESTAMP_S '2024-01-01 12:00:01'", text("2024-01-01 12:00:01")),
        lit("TIMESTAMP_MS '2024-01-01 12:00:00.5'", text("2024-01-01 12:00:00.5")),
        lit("TIMESTAMP_NS '2024-01-01 12:00:00.123456789'", text("2024-01-01 12:00:00.123456789")),
        lit("'infinity'::TIMESTAMP_NS", text("infinity")),
        // TIMESTAMPTZ: UTC, whatever the session's zone.
        lit("TIMESTAMPTZ '2024-01-01 12:00:00+00'", text("2024-01-01 12:00:00+00")),
        lit("TIMESTAMPTZ '2024-01-01 12:00:00.5+05:30'", text("2024-01-01 06:30:00.5+00")),
        lit("TIMESTAMPTZ '0044-03-15 (BC) 12:00:00+00'", text("0044-03-15 (BC) 12:00:00+00")),
        lit("'infinity'::TIMESTAMPTZ", text("infinity")),
        // Intervals.
        lit("INTERVAL '1 month 2 days 00:00:01'", text("1 month 2 days 00:00:01")),
        lit("INTERVAL '14 months'", text("1 year 2 months")),
        lit(
            "-INTERVAL '1 year 2 months 3 days 04:05:06.789'",
            text("-1 year -2 months -3 days -04:05:06.789"),
        ),
        lit("INTERVAL '0 seconds'", text("00:00:00")),
        lit("INTERVAL '36 hours'", text("36:00:00")),
        lit("to_days(1) - to_microseconds(1)", text("1 day -00:00:00.000001")),
        // LIST and ARRAY: arrays of element values, recursively.
        list("[1, 2, NULL]", Value::Array(vec![Value::Int(1), Value::Int(2), Value::Null])),
        list("[]::INTEGER[]", Value::Array(vec![])),
        list(
            "[[1], [2, NULL], []]",
            Value::Array(vec![
                Value::Array(vec![Value::Int(1)]),
                Value::Array(vec![Value::Int(2), Value::Null]),
                Value::Array(vec![]),
            ]),
        ),
        list("[1.50, -2.25]::DECIMAL(4,2)[]", Value::Array(vec![dec("1.50"), dec("-2.25")])),
        list(
            "[DATE '2024-01-01', NULL]",
            Value::Array(vec![text("2024-01-01"), Value::Null]),
        ),
        list(
            "['\\x01'::BLOB]",
            Value::Array(vec![Value::Bytes(vec![1])]),
        ),
        list(
            "[340282366920938463463374607431768211455::UHUGEINT, 1]",
            Value::Array(vec![dec("340282366920938463463374607431768211455"), Value::Int(1)]),
        ),
        list(
            "[TIMETZ '12:00:00+02', TIME '00:00:01'::TIMETZ]",
            Value::Array(vec![text("12:00:00+02"), text("00:00:01+00")]),
        ),
        TypedCellCase::literal("[TIME_NS '12:00:00.123456789']", Value::Array(vec![text("12:00:00.123456789")])),
        TypedCellCase::literal(
            "['ok', 'sad']::ENUM('sad', 'ok', 'happy')[]",
            Value::Array(vec![text("ok"), text("sad")]),
        ),
        TypedCellCase::literal("[1, 2, 3]::INTEGER[3]", Value::Array(vec![Value::Int(1), Value::Int(2), Value::Int(3)])),
        TypedCellCase::literal(
            "['nan'::DOUBLE, 1]",
            Value::Array(vec![Value::Float(f64::NAN), Value::Float(1.0)]),
        ),
        TypedCellCase::literal(
            "[{'a': 1, 'b': 1.50}, NULL]",
            Value::Array(vec![json_value(json!({"a": 1, "b": "1.50"})), Value::Null]),
        ),
        TypedCellCase::literal(
            "['{\"x\": 1}'::JSON]",
            Value::Array(vec![json_value(json!({"x": 1}))]),
        ),
        // STRUCT: JSON, non-JSON-exact scalars as their text.
        TypedCellCase::literal(
            "{'i': 1, 'big': 9007199254740993, 'd': 1.50, 'f': 'inf'::DOUBLE, 'b': '\\x00a'::BLOB, \
             'dt': DATE '2024-01-01', 'l': [1, 2], 's': {'x': NULL}, 'j': '[1]'::JSON, \
             'h': 170141183460469231731687303715884105727::HUGEINT}",
            json_value(json!({
                "i": 1, "big": "9007199254740993", "d": "1.50", "f": "inf", "b": "\\x00a",
                "dt": "2024-01-01", "l": [1, 2], "s": {"x": null}, "j": [1],
                "h": "170141183460469231731687303715884105727"
            })),
        ),
        TypedCellCase::literal(
            "{'i': INTERVAL 1 YEAR, 't': TIME_NS '12:00:00.123456789'}",
            json_value(json!({"i": "1 year", "t": "12:00:00.123456789"})),
        ),
        // MAP: an object for text and number keys, entries otherwise.
        TypedCellCase::literal("MAP {'a': 1, 'b': 2}", json_value(json!({"a": 1, "b": 2}))),
        TypedCellCase::literal("MAP {1: INTERVAL 1 DAY}", json_value(json!({"1": "1 day"}))),
        TypedCellCase::literal("MAP {DATE '2024-01-01': [1.5::DECIMAL(2,1)]}", json_value(json!({"2024-01-01": ["1.5"]}))),
        TypedCellCase::literal(
            "MAP {1.5::DOUBLE: 'x', 2: 'y'}",
            json_value(json!([{"key": 1.5, "value": "x"}, {"key": 2.0, "value": "y"}])),
        ),
        TypedCellCase::literal(
            "MAP {[1]: true}",
            json_value(json!([{"key": [1], "value": true}])),
        ),
        TypedCellCase::literal("MAP {}::MAP(VARCHAR, INTEGER)", json_value(json!({}))),
        // UNION: the active member's value.
        TypedCellCase::literal("union_value(num := 2)", Value::Int(2)),
        TypedCellCase::literal("union_value(i := INTERVAL 1 YEAR)", text("1 year")),
        TypedCellCase::literal(
            "union_value(t := TIME_NS '12:00:00')::UNION(n INTEGER, t TIME_NS)",
            text("12:00:00"),
        ),
        TypedCellCase::literal(
            "[union_value(n := 1)::UNION(n INTEGER, s VARCHAR), union_value(s := 'x')]",
            Value::Array(vec![Value::Int(1), text("x")]),
        ),
        // GEOMETRY: its WKB.
        TypedCellCase::literal(
            "'POINT(1 2)'::GEOMETRY",
            Value::Bytes(vec![
                1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 240, 63, 0, 0, 0, 0, 0, 0, 0, 64,
            ]),
        ),
        // CRUD: rows keyed by each type are found again by the decoded key.
        key_case("DATE", "DATE '0044-03-15 (BC)'", text("0044-03-15 (BC)")),
        key_case("TIMESTAMP", "TIMESTAMP '2024-01-01 12:00:00.123456'", text("2024-01-01 12:00:00.123456")),
        key_case("TIMESTAMP_NS", "TIMESTAMP_NS '2024-01-01 12:00:00.123456789'", text("2024-01-01 12:00:00.123456789")),
        key_case("TIMESTAMPTZ", "TIMESTAMPTZ '2024-01-01 12:00:00.5+05:30'", text("2024-01-01 06:30:00.5+00")),
        key_case("TIME", "TIME '23:59:59.999999'", text("23:59:59.999999")),
        key_case("TIMETZ", "TIMETZ '12:00:00-05:30'", text("12:00:00-05:30")),
        key_case("INTERVAL", "INTERVAL '1 month -1 day 00:00:00.5'", text("1 month -1 day 00:00:00.5")),
        key_case("DECIMAL(2,2)", "-0.05", dec("-0.05")),
        key_case("DECIMAL(18,3)", "123456789012345.678", dec("123456789012345.678")),
        key_case("DECIMAL(38,10)", "'1234567890123456789012345678.0123456789'", dec("1234567890123456789012345678.0123456789")),
        key_case("HUGEINT", "'-170141183460469231731687303715884105728'", dec("-170141183460469231731687303715884105728")),
        key_case("UHUGEINT", "'340282366920938463463374607431768211455'", dec("340282366920938463463374607431768211455")),
        key_case("UBIGINT", "18446744073709551615", dec("18446744073709551615")),
        key_case("BIGINT", "9007199254740993", Value::Int(9_007_199_254_740_993)),
        key_case("BLOB", "'\\x00\\xFF\\x80'::BLOB", Value::Bytes(vec![0, 0xff, 0x80])),
        key_case("UUID", "'8e0a3b1c-7d4f-4e2a-9b6c-1f2e3d4c5b6a'", text("8e0a3b1c-7d4f-4e2a-9b6c-1f2e3d4c5b6a")),
        key_case("DOUBLE", "0.1", Value::Float(0.1)),
        key_case("FLOAT", "0.1", Value::Float(f64::from(0.1_f32))),
        key_case("FLOAT", "1e-40", Value::Float(f64::from(1e-40_f32))),
        key_case("FLOAT", "3.4028235e38", Value::Float(f64::from(f32::MAX))),
        key_case("FLOAT", "-1.17549435e-38", Value::Float(f64::from(-f32::MIN_POSITIVE))),
        key_case("VARCHAR", "'x'", text("x")),
    ]
}

#[tokio::test]
async fn typed_cells() {
    run_typed_cells(&*seaquel_engine_duckdb::engine(), &config(), &cases()).await;
}

/// Binding back works in any session time zone: TIMESTAMPTZ text carries
/// its offset.
#[tokio::test]
async fn timestamptz_binds_back_in_another_zone() {
    let driver = open().await;
    driver
        .execute("SET TimeZone = 'America/St_Johns'", vec![])
        .await
        .unwrap();
    let r = driver
        .query(
            "SELECT TIMESTAMPTZ '2024-07-01 12:00:00.25+00' AS v",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(r.rows[0][0], text("2024-07-01 12:00:00.25+00"));
    let r = driver
        .query(
            "SELECT ? = TIMESTAMPTZ '2024-07-01 12:00:00.25+00'",
            vec![r.rows[0][0].clone()],
        )
        .await
        .unwrap();
    assert_eq!(r.rows[0][0], Value::Bool(true));
}

/// Decimals bind as DECIMAL where they fit 38 digits: `? + 0` would be text
/// arithmetic otherwise.
#[tokio::test]
async fn decimals_bind_exactly() {
    let driver = open().await;
    for (param, expected) in [
        ("1.50", "1.50"),
        ("-0.05", "-0.05"),
        ("+7", "7"),
        (".5", "0.5"),
        (
            "12345678901234567890123456789012345678",
            "12345678901234567890123456789012345678",
        ),
    ] {
        let r = driver
            .query(
                "SELECT typeof(?) AS t, CAST(? AS VARCHAR) AS s",
                vec![dec(param), dec(param)],
            )
            .await
            .unwrap();
        assert!(
            r.rows[0][0].as_str().unwrap().starts_with("DECIMAL"),
            "{param}: {:?}",
            r.rows[0]
        );
        assert_eq!(r.rows[0][1], text(expected), "{param}");
    }
    // Beyond 38 digits, and not a number DuckDB's DECIMAL holds: text.
    for param in ["340282366920938463463374607431768211455", "NaN", "1e5"] {
        let r = driver
            .query("SELECT typeof(?) AS t", vec![dec(param)])
            .await
            .unwrap();
        assert_eq!(r.rows[0][0], text("VARCHAR"), "{param}");
    }
}

/// A 100k-row result with a LIST column decodes in linear time (the old
/// decoder printed the whole column chunk for every cell).
#[tokio::test]
async fn list_columns_decode_in_linear_time() {
    let driver = open().await;
    let mut timings = Vec::new();
    for rows in [25_000, 50_000, 100_000] {
        let sql = format!(
            "SELECT i, [i, i + 1, NULL] AS l, {{'a': i, 'b': [i]}} AS s FROM range({rows}) t(i)"
        );
        let started = Instant::now();
        let r = driver.query(&sql, vec![]).await.unwrap();
        let took = started.elapsed();
        assert_eq!(r.rows.len(), rows);
        assert_eq!(
            r.rows[rows - 1][1],
            Value::Array(vec![
                Value::Int(rows as i64 - 1),
                Value::Int(rows as i64),
                Value::Null
            ])
        );
        eprintln!("LIST/STRUCT decode: {rows} rows in {took:?}");
        timings.push(took);
    }
    // Doubling the rows must not quadruple the time. Generous: shared CI.
    let per_row = |i: usize, rows: u32| timings[i] / rows;
    assert!(
        per_row(2, 100_000) < per_row(0, 25_000) * 3 + Duration::from_micros(5),
        "not linear: {timings:?}"
    );
}
