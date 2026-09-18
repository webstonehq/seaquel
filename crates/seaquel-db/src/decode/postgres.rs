use rust_decimal::prelude::ToPrimitive;
use serde_json::Value as JsonValue;
use sqlx::{
    postgres::types::{
        PgBox, PgCircle, PgInterval, PgLine, PgLSeg, PgMoney, PgPath, PgPoint, PgPolygon,
        PgTimeTz,
    },
    postgres::PgValueRef,
    types::{ipnetwork::IpNetwork, mac_address::MacAddress, BitVec},
    TypeInfo, Value, ValueRef,
};
use time::{Date, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

use crate::DbError;

fn format_interval(i: &PgInterval) -> String {
    let mut parts: Vec<String> = Vec::new();
    if i.months != 0 {
        parts.push(format!("{} mons", i.months));
    }
    if i.days != 0 {
        parts.push(format!("{} days", i.days));
    }
    if i.microseconds != 0 || parts.is_empty() {
        let total = i.microseconds.unsigned_abs();
        let hours = total / 3_600_000_000;
        let minutes = (total % 3_600_000_000) / 60_000_000;
        let seconds = (total % 60_000_000) / 1_000_000;
        let micros = total % 1_000_000;
        let sign = if i.microseconds < 0 { "-" } else { "" };
        if micros != 0 {
            parts.push(format!(
                "{}{:02}:{:02}:{:02}.{:06}",
                sign, hours, minutes, seconds, micros
            ));
        } else {
            parts.push(format!("{}{:02}:{:02}:{:02}", sign, hours, minutes, seconds));
        }
    }
    parts.join(" ")
}

fn format_money(m: &PgMoney) -> String {
    let sign = if m.0 < 0 { "-" } else { "" };
    let abs = m.0.checked_abs().unwrap_or(i64::MAX) as u64;
    format!("{}{}.{:02}", sign, abs / 100, abs % 100)
}

fn format_bitvec(b: &BitVec) -> String {
    b.iter().map(|bit| if bit { '1' } else { '0' }).collect()
}

pub fn to_json(v: PgValueRef) -> Result<JsonValue, DbError> {
    if v.is_null() {
        return Ok(JsonValue::Null);
    }

    let type_name = v.type_info().name().to_string();
    let res = match type_name.as_str() {
        "CHAR" | "VARCHAR" | "TEXT" | "NAME" | "UUID" | "XML" | "TSVECTOR" | "TSQUERY"
        | "CITEXT" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<String>() {
                JsonValue::String(v)
            } else if let Ok(v) = ValueRef::to_owned(&v).try_decode_unchecked::<String>() {
                JsonValue::String(v)
            } else {
                JsonValue::Null
            }
        }
        "FLOAT4" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<f32>() {
                JsonValue::from(v)
            } else {
                JsonValue::Null
            }
        }
        "FLOAT8" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<f64>() {
                JsonValue::from(v)
            } else {
                JsonValue::Null
            }
        }
        "INT2" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<i16>() {
                JsonValue::Number(v.into())
            } else {
                JsonValue::Null
            }
        }
        "INT4" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<i32>() {
                JsonValue::Number(v.into())
            } else {
                JsonValue::Null
            }
        }
        "INT8" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<i64>() {
                JsonValue::Number(v.into())
            } else {
                JsonValue::Null
            }
        }
        "BOOL" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode() {
                JsonValue::Bool(v)
            } else {
                JsonValue::Null
            }
        }
        "DATE" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<Date>() {
                JsonValue::String(v.to_string())
            } else {
                JsonValue::Null
            }
        }
        "TIME" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<Time>() {
                JsonValue::String(v.to_string())
            } else {
                JsonValue::Null
            }
        }
        "TIMETZ" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<PgTimeTz<Time, UtcOffset>>() {
                JsonValue::String(format!("{}{}", v.time, v.offset))
            } else {
                JsonValue::Null
            }
        }
        "TIMESTAMP" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<PrimitiveDateTime>() {
                JsonValue::String(v.to_string())
            } else {
                JsonValue::Null
            }
        }
        "TIMESTAMPTZ" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<OffsetDateTime>() {
                JsonValue::String(v.to_string())
            } else {
                JsonValue::Null
            }
        }
        "INTERVAL" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<PgInterval>() {
                JsonValue::String(format_interval(&v))
            } else {
                JsonValue::Null
            }
        }
        "JSON" | "JSONB" => ValueRef::to_owned(&v).try_decode().unwrap_or_default(),
        "BYTEA" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<Vec<u8>>() {
                JsonValue::Array(v.into_iter().map(|n| JsonValue::Number(n.into())).collect())
            } else {
                JsonValue::Null
            }
        }
        "NUMERIC" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<rust_decimal::Decimal>() {
                if let Some(n) = v.to_f64().and_then(serde_json::Number::from_f64) {
                    JsonValue::Number(n)
                } else {
                    JsonValue::String(v.to_string())
                }
            } else {
                JsonValue::Null
            }
        }
        "INET" | "CIDR" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<IpNetwork>() {
                JsonValue::String(v.to_string())
            } else {
                JsonValue::Null
            }
        }
        "MACADDR" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<MacAddress>() {
                JsonValue::String(v.to_string())
            } else {
                JsonValue::Null
            }
        }
        "BIT" | "VARBIT" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<BitVec>() {
                JsonValue::String(format_bitvec(&v))
            } else {
                JsonValue::Null
            }
        }
        "MONEY" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<PgMoney>() {
                JsonValue::String(format_money(&v))
            } else {
                JsonValue::Null
            }
        }
        "POINT" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<PgPoint>() {
                JsonValue::String(format!("({},{})", v.x, v.y))
            } else {
                JsonValue::Null
            }
        }
        "LINE" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<PgLine>() {
                JsonValue::String(format!("{{{},{},{}}}", v.a, v.b, v.c))
            } else {
                JsonValue::Null
            }
        }
        "LSEG" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<PgLSeg>() {
                JsonValue::String(format!(
                    "[({},{}),({},{})]",
                    v.start_x, v.start_y, v.end_x, v.end_y
                ))
            } else {
                JsonValue::Null
            }
        }
        "BOX" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<PgBox>() {
                JsonValue::String(format!(
                    "({},{}),({},{})",
                    v.upper_right_x, v.upper_right_y, v.lower_left_x, v.lower_left_y
                ))
            } else {
                JsonValue::Null
            }
        }
        "PATH" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<PgPath>() {
                let points = v
                    .points
                    .iter()
                    .map(|p| format!("({},{})", p.x, p.y))
                    .collect::<Vec<_>>()
                    .join(",");
                let formatted = if v.closed {
                    format!("({})", points)
                } else {
                    format!("[{}]", points)
                };
                JsonValue::String(formatted)
            } else {
                JsonValue::Null
            }
        }
        "POLYGON" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<PgPolygon>() {
                let points = v
                    .points
                    .iter()
                    .map(|p| format!("({},{})", p.x, p.y))
                    .collect::<Vec<_>>()
                    .join(",");
                JsonValue::String(format!("({})", points))
            } else {
                JsonValue::Null
            }
        }
        "CIRCLE" => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode::<PgCircle>() {
                JsonValue::String(format!("<({},{}),{}>", v.x, v.y, v.radius))
            } else {
                JsonValue::Null
            }
        }
        "INT2[]" => decode_array::<i16>(&v, |n| JsonValue::Number(n.into())),
        "INT4[]" => decode_array::<i32>(&v, |n| JsonValue::Number(n.into())),
        "INT8[]" => decode_array::<i64>(&v, |n| JsonValue::Number(n.into())),
        "FLOAT4[]" => decode_array::<f32>(&v, JsonValue::from),
        "FLOAT8[]" => decode_array::<f64>(&v, JsonValue::from),
        "BOOL[]" => decode_array::<bool>(&v, JsonValue::Bool),
        "TEXT[]" | "VARCHAR[]" | "CHAR[]" | "NAME[]" | "UUID[]" => {
            decode_array::<String>(&v, JsonValue::String)
        }
        "VOID" => JsonValue::Null,
        // Handle custom types (enums, domains, etc.) by trying to decode as string
        _ => {
            if let Ok(v) = ValueRef::to_owned(&v).try_decode_unchecked::<String>() {
                JsonValue::String(v)
            } else {
                JsonValue::String(format!("<unsupported: {}>", type_name))
            }
        }
    };

    Ok(res)
}

fn decode_array<T>(v: &PgValueRef, map: impl Fn(T) -> JsonValue) -> JsonValue
where
    T: for<'a> sqlx::Decode<'a, sqlx::Postgres>
        + sqlx::Type<sqlx::Postgres>
        + sqlx::postgres::PgHasArrayType,
{
    if let Ok(items) = ValueRef::to_owned(v).try_decode::<Vec<T>>() {
        JsonValue::Array(items.into_iter().map(map).collect())
    } else {
        JsonValue::Null
    }
}
