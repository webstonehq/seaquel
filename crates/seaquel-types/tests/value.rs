//! The `Value` wire format. See "Value wire format" in
//! docs/plans/2026-09-25-rust-core-phase-1-plan.md: plain JSON wherever
//! JavaScript holds the value exactly, a `{"$sq": kind, "v": …}` tag otherwise.

use seaquel_types::{SqlWithBindings, Value};
use serde_json::{from_value, json, to_string, to_value};

const MAX_SAFE: i64 = (1 << 53) - 1;

/// Serialize, check the JSON, then decode it with `from_wire` and `Deserialize`
/// and check both give the value back.
fn round_trip(v: Value, wire: serde_json::Value) {
    assert_eq!(to_value(&v).unwrap(), wire, "serialize {v:?}");
    assert_eq!(Value::from_wire(wire.clone()).unwrap(), v, "from_wire {wire}");
    assert_eq!(from_value::<Value>(wire.clone()).unwrap(), v, "deserialize {wire}");
}

#[test]
fn null() {
    round_trip(Value::Null, json!(null));
}

#[test]
fn bool() {
    round_trip(Value::Bool(true), json!(true));
    round_trip(Value::Bool(false), json!(false));
}

#[test]
fn safe_int_is_a_number() {
    round_trip(Value::Int(0), json!(0));
    round_trip(Value::Int(-42), json!(-42));
    round_trip(Value::Int(MAX_SAFE), json!(MAX_SAFE));
    round_trip(Value::Int(-MAX_SAFE), json!(-MAX_SAFE));
}

#[test]
fn unsafe_int_is_tagged() {
    round_trip(
        Value::Int(MAX_SAFE + 1),
        json!({ "$sq": "bigint", "v": "9007199254740992" }),
    );
    round_trip(
        Value::Int(-MAX_SAFE - 1),
        json!({ "$sq": "bigint", "v": "-9007199254740992" }),
    );
    round_trip(
        Value::Int(i64::MIN),
        json!({ "$sq": "bigint", "v": "-9223372036854775808" }),
    );
    round_trip(
        Value::Int(i64::MAX),
        json!({ "$sq": "bigint", "v": "9223372036854775807" }),
    );
}

#[test]
fn finite_float_is_a_number() {
    round_trip(Value::Float(1.5), json!(1.5));
    round_trip(Value::Float(-0.25), json!(-0.25));
}

#[test]
fn non_finite_float_is_tagged() {
    assert_eq!(
        to_value(Value::Float(f64::NAN)).unwrap(),
        json!({ "$sq": "float", "v": "NaN" })
    );
    let nan = Value::from_wire(json!({ "$sq": "float", "v": "NaN" })).unwrap();
    assert!(matches!(nan, Value::Float(f) if f.is_nan()));
    round_trip(Value::Float(f64::INFINITY), json!({ "$sq": "float", "v": "inf" }));
    round_trip(
        Value::Float(f64::NEG_INFINITY),
        json!({ "$sq": "float", "v": "-inf" }),
    );
}

#[test]
fn decimal_is_tagged_and_keeps_its_text() {
    round_trip(
        Value::Decimal("12.50".into()),
        json!({ "$sq": "decimal", "v": "12.50" }),
    );
    round_trip(Value::Decimal("NaN".into()), json!({ "$sq": "decimal", "v": "NaN" }));
}

#[test]
fn text_is_a_string() {
    round_trip(Value::Text("hello".into()), json!("hello"));
    round_trip(Value::Text("".into()), json!(""));
}

#[test]
fn bytes_are_base64() {
    round_trip(
        Value::Bytes(vec![0x00, 0xff, 0x10]),
        json!({ "$sq": "bytes", "v": "AP8Q" }),
    );
    round_trip(Value::Bytes(vec![]), json!({ "$sq": "bytes", "v": "" }));
}

#[test]
fn json_is_tagged() {
    round_trip(
        Value::Json(json!({ "a": [1, 2], "b": null })),
        json!({ "$sq": "json", "v": { "a": [1, 2], "b": null } }),
    );
    round_trip(Value::Json(json!("s")), json!({ "$sq": "json", "v": "s" }));
}

#[test]
fn array_is_a_json_array_of_encoded_elements() {
    round_trip(
        Value::Array(vec![Value::Int(1), Value::Text("x".into()), Value::Null]),
        json!([1, "x", null]),
    );
}

#[test]
fn nested_arrays_of_tagged_values() {
    round_trip(
        Value::Array(vec![
            Value::Array(vec![Value::Int(i64::MAX), Value::Bytes(vec![1])]),
            Value::Array(vec![Value::Decimal("1.0".into())]),
        ]),
        json!([
            [{ "$sq": "bigint", "v": "9223372036854775807" }, { "$sq": "bytes", "v": "AQ==" }],
            [{ "$sq": "decimal", "v": "1.0" }]
        ]),
    );
}

#[test]
fn plain_u64_beyond_i64_becomes_decimal() {
    assert_eq!(
        Value::from_wire(json!(u64::MAX)).unwrap(),
        Value::Decimal("18446744073709551615".into())
    );
}

#[test]
fn plain_huge_numbers_stay_floats() {
    // JS tags its bigints, so a plain huge number is a float (JSON.stringify(1e30) = "1e+30").
    let big: serde_json::Value = serde_json::from_str("1e+30").unwrap();
    assert_eq!(Value::from_wire(big).unwrap(), Value::Float(1e30));
    let two_pow_64: serde_json::Value = serde_json::from_str("18446744073709551616").unwrap();
    assert_eq!(Value::from_wire(two_pow_64).unwrap(), Value::Float(18446744073709551616.0));

    // Driver float cells keep their float form on the wire.
    let cell = Value::from_json_cell(json!(1e20));
    assert_eq!(cell, Value::Float(1e20));
    assert_eq!(to_string(&cell).unwrap(), to_string(&json!(1e20)).unwrap());
    assert_eq!(Value::from_json_cell(json!(1e300)), Value::Float(1e300));
}

#[test]
fn bigint_tag_beyond_i64_becomes_decimal() {
    assert_eq!(
        Value::from_wire(json!({ "$sq": "bigint", "v": "18446744073709551615" })).unwrap(),
        Value::Decimal("18446744073709551615".into())
    );
    assert!(Value::from_wire(json!({ "$sq": "bigint", "v": "12x" })).is_err());
}

#[test]
fn plain_object_without_tag_is_json() {
    assert_eq!(
        Value::from_wire(json!({ "a": 1 })).unwrap(),
        Value::Json(json!({ "a": 1 }))
    );
}

#[test]
fn unknown_tag_is_an_error() {
    let err = Value::from_wire(json!({ "$sq": "date", "v": "2026-01-01" })).unwrap_err();
    assert!(err.contains("date"), "{err}");
    assert!(from_value::<Value>(json!({ "$sq": "date", "v": "x" })).is_err());
}

#[test]
fn malformed_tags_are_errors() {
    assert!(Value::from_wire(json!({ "$sq": "bytes", "v": "not base64!" })).is_err());
    assert!(Value::from_wire(json!({ "$sq": "decimal" })).is_err());
    assert!(Value::from_wire(json!({ "$sq": 1, "v": "1" })).is_err());
    assert!(Value::from_wire(json!({ "$sq": "float", "v": "fast" })).is_err());
}

#[test]
fn from_json_cell_never_interprets_tags() {
    let tagged = json!({ "$sq": "bigint", "v": "1" });
    assert_eq!(Value::from_json_cell(tagged.clone()), Value::Json(tagged));
    assert_eq!(
        Value::from_json_cell(json!([1, 2.5, "a", true, null, { "k": 1 }])),
        Value::Array(vec![
            Value::Int(1),
            Value::Float(2.5),
            Value::Text("a".into()),
            Value::Bool(true),
            Value::Null,
            Value::Json(json!({ "k": 1 })),
        ])
    );
    assert_eq!(
        Value::from_json_cell(json!(u64::MAX)),
        Value::Decimal("18446744073709551615".into())
    );
}

#[test]
fn as_i64_accepts_integers_in_any_form() {
    assert_eq!(Value::Int(7).as_i64(), Some(7));
    assert_eq!(Value::Float(7.0).as_i64(), Some(7));
    assert_eq!(Value::Float(7.5).as_i64(), None);
    assert_eq!(Value::Decimal("-12".into()).as_i64(), Some(-12));
    assert_eq!(Value::Text("12".into()).as_i64(), Some(12));
    assert_eq!(Value::Text("x".into()).as_i64(), None);
    assert_eq!(Value::Null.as_i64(), None);
}

#[test]
fn as_str_reads_text() {
    assert_eq!(Value::Text("a".into()).as_str(), Some("a"));
    assert_eq!(Value::Int(1).as_str(), None);
}

#[test]
fn float_wire_bytes_match_serde_json() {
    // Cells that used to be serde_json numbers must serialize identically.
    for f in [1.0, 0.1, 1e300, -2.5e-8] {
        assert_eq!(to_string(&Value::Float(f)).unwrap(), to_string(&json!(f)).unwrap());
    }
    assert_eq!(to_string(&Value::Int(5)).unwrap(), "5");
}

#[test]
fn sql_with_bindings_shape() {
    let s = SqlWithBindings {
        sql: "UPDATE t SET a = $1".into(),
        bind_values: Some(vec![Value::Int(1), Value::Bytes(vec![1])]),
    };
    assert_eq!(
        to_value(&s).unwrap(),
        json!({ "sql": "UPDATE t SET a = $1", "bindValues": [1, { "$sq": "bytes", "v": "AQ==" }] })
    );
    let bare = SqlWithBindings { sql: "DELETE".into(), bind_values: None };
    assert_eq!(to_value(&bare).unwrap(), json!({ "sql": "DELETE" }));
    assert_eq!(
        from_value::<SqlWithBindings>(json!({ "sql": "x", "bindValues": [{ "$sq": "bigint", "v": "9007199254740993" }] }))
            .unwrap()
            .bind_values,
        Some(vec![Value::Int(9007199254740993)])
    );
}
