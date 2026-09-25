//! Typed cell and parameter values.
//!
//! On the wire a `Value` is plain JSON wherever JavaScript holds it exactly,
//! and a tagged object `{"$sq": kind, "v": …}` otherwise:
//!
//! | `Value`                    | wire                                  |
//! |----------------------------|---------------------------------------|
//! | `Null`, `Bool`, `Text`     | `null`, `true`/`false`, string        |
//! | `Int`, \|i\| ≤ 2^53−1      | number                                |
//! | `Int`, otherwise           | `{"$sq":"bigint","v":"<digits>"}`     |
//! | `Float`, finite            | number                                |
//! | `Float`, NaN/±inf          | `{"$sq":"float","v":"NaN"\|"inf"\|"-inf"}` |
//! | `Decimal(s)`               | `{"$sq":"decimal","v":"<s>"}`         |
//! | `Bytes`                    | `{"$sq":"bytes","v":"<base64>"}`      |
//! | `Json(j)`                  | `{"$sq":"json","v":<j>}`              |
//! | `Array`                    | JSON array of encoded elements        |
//!
//! `src/lib/values.ts` is the TypeScript half of this contract.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as JsonValue;

/// The largest integer JavaScript's `number` holds exactly (2^53 − 1).
pub const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// `i64` as a float range: [-2^63, 2^63). Both bounds are exact in `f64`.
const I64_RANGE: std::ops::Range<f64> = (i64::MIN as f64)..-(i64::MIN as f64);

/// Tag key on the wire.
const TAG: &str = "$sq";

/// A cell read from a database, or a parameter bound into a query.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Exact decimal text as the database printed it (scale kept, e.g. `12.50`).
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    Json(JsonValue),
    Array(Vec<Value>),
}

impl Value {
    /// Parameters and anything else arriving from a client: interprets `$sq`
    /// tags. A plain object becomes `Json`.
    pub fn from_wire(j: JsonValue) -> Result<Value, String> {
        match j {
            JsonValue::Array(items) => items
                .into_iter()
                .map(Value::from_wire)
                .collect::<Result<_, _>>()
                .map(Value::Array),
            JsonValue::Object(mut map) => {
                let Some(kind) = map.remove(TAG) else {
                    return Ok(Value::Json(JsonValue::Object(map)));
                };
                let JsonValue::String(kind) = kind else {
                    return Err(format!("\"{TAG}\" must be a string, got {kind}"));
                };
                let v = map
                    .remove("v")
                    .ok_or_else(|| format!("tagged value \"{kind}\" has no \"v\""))?;
                decode_tag(&kind, v)
            }
            other => Ok(Value::from_json_cell(other)),
        }
    }

    /// Cells produced by a driver's own JSON decoder: never interprets `$sq`.
    /// Objects become `Json`, arrays `Array`, numbers as in [`Value::from_wire`].
    pub fn from_json_cell(j: JsonValue) -> Value {
        match j {
            JsonValue::Null => Value::Null,
            JsonValue::Bool(b) => Value::Bool(b),
            JsonValue::Number(n) => from_number(&n),
            JsonValue::String(s) => Value::Text(s),
            JsonValue::Array(items) => {
                Value::Array(items.into_iter().map(Value::from_json_cell).collect())
            }
            obj @ JsonValue::Object(_) => Value::Json(obj),
        }
    }

    /// Integer view for tests and parsers: `Int`, an integral `Float`, or
    /// `Decimal`/`Text` holding integer digits.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) if f.fract() == 0.0 && I64_RANGE.contains(f) => Some(*f as i64),
            Value::Decimal(s) | Value::Text(s) => s.parse().ok(),
            _ => None,
        }
    }

    /// The string of a `Text` value.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

/// A JSON number: `Int` when it's an integer that fits `i64`, `Decimal`
/// digits for a `u64` beyond `i64`, `Float` for everything serde_json parsed
/// as a float. A huge plain number is always a float: JS tags its bigints,
/// and drivers' float cells (`1e20`) must stay floats.
fn from_number(n: &serde_json::Number) -> Value {
    if let Some(i) = n.as_i64() {
        Value::Int(i)
    } else if let Some(u) = n.as_u64() {
        Value::Decimal(u.to_string())
    } else {
        Value::Float(n.as_f64().unwrap_or(f64::NAN))
    }
}

fn is_integer_text(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

fn decode_tag(kind: &str, v: JsonValue) -> Result<Value, String> {
    let text = |v: JsonValue| match v {
        JsonValue::String(s) => Ok(s),
        other => Err(format!("\"{kind}\" value must be a string, got {other}")),
    };
    match kind {
        "bigint" => {
            let s = text(v)?;
            if !is_integer_text(&s) {
                return Err(format!("invalid bigint \"{s}\""));
            }
            // Beyond i64 (e.g. MySQL BIGINT UNSIGNED): keep the digits exactly.
            Ok(s.parse().map(Value::Int).unwrap_or(Value::Decimal(s)))
        }
        "float" => {
            let s = text(v)?;
            match s.as_str() {
                "NaN" => Ok(Value::Float(f64::NAN)),
                "inf" => Ok(Value::Float(f64::INFINITY)),
                "-inf" => Ok(Value::Float(f64::NEG_INFINITY)),
                _ => Err(format!("invalid float \"{s}\"")),
            }
        }
        "decimal" => Ok(Value::Decimal(text(v)?)),
        "bytes" => STANDARD
            .decode(text(v)?)
            .map(Value::Bytes)
            .map_err(|e| format!("invalid base64 in bytes value: {e}")),
        "json" => Ok(Value::Json(v)),
        other => Err(format!("unknown value tag \"{other}\"")),
    }
}

struct Tagged<'a, V: Serialize + ?Sized>(&'a str, &'a V);

impl<V: Serialize + ?Sized> Serialize for Tagged<'_, V> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(2))?;
        map.serialize_entry(TAG, self.0)?;
        map.serialize_entry("v", self.1)?;
        map.end()
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Null => s.serialize_unit(),
            Value::Bool(b) => s.serialize_bool(*b),
            Value::Int(i) if i.unsigned_abs() <= MAX_SAFE_INTEGER as u64 => s.serialize_i64(*i),
            Value::Int(i) => Tagged("bigint", &i.to_string()).serialize(s),
            Value::Float(f) if f.is_finite() => s.serialize_f64(*f),
            Value::Float(f) => {
                let v = if f.is_nan() {
                    "NaN"
                } else if *f > 0.0 {
                    "inf"
                } else {
                    "-inf"
                };
                Tagged("float", v).serialize(s)
            }
            Value::Decimal(d) => Tagged("decimal", d.as_str()).serialize(s),
            Value::Text(t) => s.serialize_str(t),
            Value::Bytes(b) => Tagged("bytes", &STANDARD.encode(b)).serialize(s),
            Value::Json(j) => Tagged("json", j).serialize(s),
            Value::Array(items) => s.collect_seq(items),
        }
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let j = JsonValue::deserialize(d)?;
        Value::from_wire(j).map_err(serde::de::Error::custom)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Int(i)
    }
}

impl From<i32> for Value {
    fn from(i: i32) -> Self {
        Value::Int(i.into())
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Self {
        Value::Float(f)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Text(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Text(s)
    }
}

impl From<Vec<u8>> for Value {
    fn from(b: Vec<u8>) -> Self {
        Value::Bytes(b)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Value::Null, Into::into)
    }
}
