//! The typed-cell cases, shared by `values.rs` (decoded through `query`) and
//! `read_only.rs` (the same cells through `query_read_only`).

use seaquel_engine::Value;
use seaquel_engine_testkit::{scratch_name, TypedCellCase};
use serde_json::json;

pub fn text(s: &str) -> Value {
    Value::Text(s.into())
}

pub fn dec(s: &str) -> Value {
    Value::Decimal(s.into())
}

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

pub fn cases() -> Vec<TypedCellCase> {
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
