//! Decodes MySQL/MariaDB cells into `Value`.
//!
//! The type comes from `type_info().name()` and flags, never from trying
//! decoders in turn: sqlx's `try_decode` checks `compatible()`, and every
//! mismatch used to become a silent NULL (BIT, GEOMETRY, DECIMAL(65,30),
//! negative TIME, zero dates). A number or date that still fails to decode
//! is a `QUERY_ERROR` naming its type (the driver adds the column), never a
//! NULL or protocol bytes; text that isn't UTF-8 comes back as `Bytes`.
//!
//! | Type | `Value` |
//! |---|---|
//! | TINYINT … BIGINT, signed or not, YEAR | `Int`; unsigned above `i64::MAX` is `Decimal` (its digits) |
//! | BOOLEAN (TINYINT(1)) | `Bool` for 0 and 1, `Int` for anything else it holds |
//! | BIT(n) | `Int` of its bits (BIT(1) too), `Decimal` above `i64::MAX` |
//! | DECIMAL | `Decimal`: the server's text, scale kept, up to 65 digits |
//! | FLOAT | `Float` of the f32's shortest text (`0.1`, not `0.10000000149…`) |
//! | DOUBLE | `Float` |
//! | DATE, DATETIME, TIMESTAMP | `Text` as the server prints it (`2024-01-02 03:04:05.5`), zero dates included |
//! | TIME | `Text`, negative and above 24h included (`-838:59:59.999999`) |
//! | JSON (MySQL) | `Json` parsed from the server's text, arrays and scalars included |
//! | GEOMETRY | `Bytes`: MySQL's internal form, a 4-byte SRID then WKB |
//! | BINARY, VARBINARY, BLOB | `Bytes`, or `Text` if clean UTF-8 (see [`binary_string`]) |
//! | CHAR, VARCHAR, TEXT, ENUM, SET | `Text` |
//!
//! TIMESTAMP is UTC wall-clock time: sqlx sets the session `time_zone` to
//! `'+00:00'` at connect, and the server converts TIMESTAMP to the session
//! zone. Don't add `?timezone=` to connection strings: in a zone with DST, an
//! hour repeats each autumn, so two stored instants print the same and a
//! TIMESTAMP key no longer finds a single row.
//!
//! MariaDB's JSON is LONGTEXT with a check constraint and has no type of its
//! own on the wire, so it stays `Text`.

use serde_json::Value as JsonValue;
use sqlx::decode::Decode;
use sqlx::mysql::{MySqlTypeInfo, MySqlValueRef};
use sqlx::{MySql, Type, TypeInfo, ValueRef};

use seaquel_engine::{DbError, Value};

type DecodeResult = Result<Value, sqlx::error::BoxDynError>;

pub fn to_value(v: MySqlValueRef) -> Result<Value, DbError> {
    let ty = v.type_info().into_owned();
    // sqlx reports zero dates (`0000-00-00`, sent as a zero-length binary
    // value) as NULL, but the bytes are there; only a real NULL has none.
    let zero_date = matches!(ty.name(), "DATE" | "DATETIME" | "TIMESTAMP")
        && raw(v.clone()).is_ok_and(|b| b.starts_with(&[0]));
    if v.is_null() && !zero_date {
        return Ok(Value::Null);
    }
    decode(v, &ty).map_err(|e| failed(ty.name(), e))
}

/// A cell the decoder couldn't read. The driver appends the column's name.
fn failed(ty: &str, e: impl std::fmt::Display) -> DbError {
    DbError::query_error(format!("can't decode a {ty} value: {e}"))
}

fn decode(v: MySqlValueRef, ty: &MySqlTypeInfo) -> DecodeResult {
    let unsigned = <u64 as Type<MySql>>::compatible(ty);
    Ok(match ty.name() {
        "NULL" => Value::Null,
        "BOOLEAN" => {
            let n = if unsigned {
                i128::from(<u64 as Decode<MySql>>::decode(v)?)
            } else {
                i128::from(<i64 as Decode<MySql>>::decode(v)?)
            };
            match n {
                0 => Value::Bool(false),
                1 => Value::Bool(true),
                n => Value::Int(n as i64),
            }
        }
        "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "BIGINT" => {
            Value::Int(<i64 as Decode<MySql>>::decode(v)?)
        }
        "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED" | "INT UNSIGNED"
        | "BIGINT UNSIGNED" | "YEAR" | "BIT" => unsigned_value(<u64 as Decode<MySql>>::decode(v)?),
        "FLOAT" => Value::Float(widen(<f32 as Decode<MySql>>::decode(v)?)),
        "DOUBLE" => Value::Float(<f64 as Decode<MySql>>::decode(v)?),
        "DECIMAL" => Value::Decimal(<&str as Decode<MySql>>::decode(v)?.to_string()),
        "DATE" => Value::Text(temporal(raw(v)?, false)?),
        "DATETIME" | "TIMESTAMP" => Value::Text(temporal(raw(v)?, true)?),
        "TIME" => Value::Text(time_text(raw(v)?)?),
        "JSON" => {
            let raw = raw(v)?;
            match serde_json::from_slice::<JsonValue>(raw) {
                Ok(j) => Value::Json(j),
                Err(_) => binary_string(raw),
            }
        }
        "GEOMETRY" => Value::Bytes(raw(v)?.to_vec()),
        "BINARY" | "VARBINARY" | "TINYBLOB" | "BLOB" | "MEDIUMBLOB" | "LONGBLOB" => {
            binary_string(raw(v)?)
        }
        // CHAR (also SET), VARCHAR, *TEXT, ENUM: text in the connection's
        // character set (utf8mb4). Bytes that aren't UTF-8 stay bytes.
        _ => {
            let raw = raw(v)?;
            match std::str::from_utf8(raw) {
                Ok(s) => Value::Text(s.to_string()),
                Err(_) => Value::Bytes(raw.to_vec()),
            }
        }
    })
}

fn raw<'r>(v: MySqlValueRef<'r>) -> Result<&'r [u8], sqlx::error::BoxDynError> {
    <&[u8] as Decode<MySql>>::decode(v)
}

/// `Int`, or the digits as `Decimal` above `i64::MAX` (the wire format has
/// no unsigned 64-bit integer; the UI shows an integral decimal as an integer).
fn unsigned_value(n: u64) -> Value {
    match i64::try_from(n) {
        Ok(i) => Value::Int(i),
        Err(_) => Value::Decimal(n.to_string()),
    }
}

/// The f64 closest to the f32's shortest decimal text, so FLOAT 0.1 shows
/// as 0.1 (casting the f32 gives 0.10000000149011612).
fn widen(f: f32) -> f64 {
    f.to_string().parse().unwrap_or(f64::from(f))
}

/// A binary-collation string. MySQL flags `*_bin` text columns (e.g. every
/// `information_schema` name on MySQL 8, or `VARCHAR … COLLATE utf8mb4_bin`)
/// exactly like BINARY/VARBINARY/BLOB, and sqlx doesn't expose the collation
/// that would tell them apart. So the bytes decide: clean UTF-8 text (no
/// control characters besides tab, newline and carriage return) is `Text`;
/// anything else (`\0` padding, invalid UTF-8, most binary data) is `Bytes`.
/// Either binds back to the same bytes.
pub(crate) fn binary_string(raw: &[u8]) -> Value {
    match std::str::from_utf8(raw) {
        Ok(s)
            if !s
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r')) =>
        {
            Value::Text(s.to_string())
        }
        _ => Value::Bytes(raw.to_vec()),
    }
}

/// DATE (`with_time` false), DATETIME or TIMESTAMP as MySQL prints it:
/// `2024-01-02`, `2024-01-02 03:04:05`, `2024-01-02 03:04:05.5`, and zero
/// dates as `0000-00-00[ 00:00:00]`. Fractional seconds lose trailing zeros
/// (the column's precision isn't on the wire); the text casts back to the
/// same value either way. TIMESTAMP is in the session time zone, as the
/// server sends it; there is no offset to show.
///
/// Binary form: a length byte (0, 4, 7 or 11), then year (u16), month, day,
/// hour, minute, second and microseconds (u32), all little-endian. A value
/// in the text protocol is already the server's text.
fn temporal(raw: &[u8], with_time: bool) -> Result<String, sqlx::error::BoxDynError> {
    let Some((&len, rest)) = raw.split_first() else {
        return Err("empty temporal value".into());
    };
    if !matches!(len, 0 | 4 | 7 | 11) || rest.len() != len as usize {
        if len.is_ascii_digit() {
            return Ok(std::str::from_utf8(raw)?.to_string());
        }
        return Err(format!("unexpected {}-byte date/time value", raw.len()).into());
    }
    let byte = |i: usize| rest.get(i).copied().unwrap_or(0);
    let year = u16::from_le_bytes([byte(0), byte(1)]);
    let micros = u32::from_le_bytes([byte(7), byte(8), byte(9), byte(10)]);
    let mut out = format!("{year:04}-{:02}-{:02}", byte(2), byte(3));
    if with_time {
        out.push_str(&format!(" {:02}:{:02}:{:02}", byte(4), byte(5), byte(6)));
        push_micros(&mut out, micros);
    }
    Ok(out)
}

/// TIME as MySQL prints it: `-01:02:03.25`, `838:59:59.999999`, `00:00:00`.
///
/// Binary form: a length byte (0, 8 or 12), then the sign (1 = negative),
/// days (u32), hours, minutes, seconds and microseconds (u32), little-endian.
/// Read directly: sqlx's `MySqlTime` rejects MariaDB's `838:59:59.999999`
/// and its `is_negative()` returns `is_positive()` (0.8.6). A value in the
/// text protocol is already the server's text.
fn time_text(raw: &[u8]) -> Result<String, sqlx::error::BoxDynError> {
    let Some((&len, rest)) = raw.split_first() else {
        return Err("empty TIME value".into());
    };
    if !matches!(len, 0 | 8 | 12) || rest.len() != len as usize {
        if raw
            .first()
            .is_some_and(|b| b.is_ascii_digit() || *b == b'-')
        {
            return Ok(std::str::from_utf8(raw)?.to_string());
        }
        return Err(format!("unexpected {}-byte TIME value", raw.len()).into());
    }
    let byte = |i: usize| rest.get(i).copied().unwrap_or(0);
    let u32_at = |i: usize| u32::from_le_bytes([byte(i), byte(i + 1), byte(i + 2), byte(i + 3)]);
    let sign = if byte(0) == 1 { "-" } else { "" };
    let hours = u64::from(u32_at(1)) * 24 + u64::from(byte(5));
    let mut out = format!("{sign}{hours:02}:{:02}:{:02}", byte(6), byte(7));
    push_micros(&mut out, u32_at(8));
    Ok(out)
}

/// `.5` for 500000 µs, `.000001` for 1, nothing for 0.
fn push_micros(out: &mut String, micros: u32) {
    if micros != 0 {
        let digits = format!("{micros:06}");
        out.push('.');
        out.push_str(digits.trim_end_matches('0'));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datetime(year: u16, rest: &[u8], micros: Option<u32>) -> Vec<u8> {
        let mut body = year.to_le_bytes().to_vec();
        body.extend_from_slice(rest);
        if let Some(m) = micros {
            body.extend_from_slice(&m.to_le_bytes());
        }
        let mut out = vec![body.len() as u8];
        out.extend(body);
        out
    }

    #[test]
    fn temporal_binary_forms() {
        assert_eq!(temporal(&[0], false).unwrap(), "0000-00-00");
        assert_eq!(temporal(&[0], true).unwrap(), "0000-00-00 00:00:00");
        assert_eq!(
            temporal(&datetime(2024, &[1, 2], None), false).unwrap(),
            "2024-01-02"
        );
        assert_eq!(
            temporal(&datetime(999, &[12, 31], None), false).unwrap(),
            "0999-12-31"
        );
        assert_eq!(
            temporal(&datetime(2024, &[0, 15], None), false).unwrap(),
            "2024-00-15"
        );
        assert_eq!(
            temporal(&datetime(2024, &[1, 2], None), true).unwrap(),
            "2024-01-02 00:00:00"
        );
        assert_eq!(
            temporal(&datetime(2024, &[1, 2, 3, 4, 5], None), true).unwrap(),
            "2024-01-02 03:04:05"
        );
        assert_eq!(
            temporal(&datetime(2024, &[1, 2, 3, 4, 5], Some(500_000)), true).unwrap(),
            "2024-01-02 03:04:05.5"
        );
        assert_eq!(
            temporal(&datetime(2024, &[1, 2, 3, 4, 5], Some(1)), true).unwrap(),
            "2024-01-02 03:04:05.000001"
        );
    }

    #[test]
    fn temporal_text_passes_through() {
        assert_eq!(
            temporal(b"2024-01-02 03:04:05", true).unwrap(),
            "2024-01-02 03:04:05"
        );
        assert_eq!(temporal(b"0000-00-00", false).unwrap(), "0000-00-00");
    }

    fn time(negative: bool, days: u32, hms: [u8; 3], micros: Option<u32>) -> Vec<u8> {
        let mut body = vec![u8::from(negative)];
        body.extend_from_slice(&days.to_le_bytes());
        body.extend_from_slice(&hms);
        if let Some(m) = micros {
            body.extend_from_slice(&m.to_le_bytes());
        }
        let mut out = vec![body.len() as u8];
        out.extend(body);
        out
    }

    #[test]
    fn time_forms() {
        let t = |raw: Vec<u8>| time_text(&raw).unwrap();
        assert_eq!(t(vec![0]), "00:00:00");
        assert_eq!(t(time(false, 0, [1, 2, 3], None)), "01:02:03");
        assert_eq!(t(time(true, 0, [1, 2, 3], Some(250_000))), "-01:02:03.25");
        assert_eq!(
            t(time(false, 34, [22, 59, 59], Some(999_999))),
            "838:59:59.999999"
        );
        assert_eq!(
            t(time(true, 34, [22, 59, 59], Some(999_999))),
            "-838:59:59.999999"
        );
        assert_eq!(t(time(true, 0, [0, 0, 0], Some(1))), "-00:00:00.000001");
        assert_eq!(t(time(false, 4, [4, 0, 0], None)), "100:00:00");
        assert_eq!(time_text(b"-01:02:03").unwrap(), "-01:02:03");
    }

    #[test]
    fn malformed_values_are_errors() {
        // A truncated TIME or DATETIME is an error, not protocol bytes.
        assert!(time_text(&[5, 0, 1, 0, 0, 0]).is_err());
        assert!(time_text(&[]).is_err());
        assert!(temporal(&[], true).is_err());
        assert!(temporal(&[7, 0xe8, 7, 1], true).is_err());
        let e = failed("TIME", time_text(&[5, 0, 1]).unwrap_err());
        assert_eq!(e.code, "QUERY_ERROR");
        assert!(
            e.message.contains("can't decode a TIME value"),
            "{}",
            e.message
        );
    }

    #[test]
    fn binary_strings() {
        assert_eq!(binary_string(b"abc"), Value::Text("abc".into()));
        assert_eq!(
            binary_string("tab\tnew\nline\r".as_bytes()),
            Value::Text("tab\tnew\nline\r".into())
        );
        assert_eq!(binary_string("€".as_bytes()), Value::Text("€".into()));
        assert_eq!(binary_string(b""), Value::Text(String::new()));
        assert_eq!(binary_string(b"ab\0\0"), Value::Bytes(b"ab\0\0".to_vec()));
        assert_eq!(binary_string(&[0x80, 0x81]), Value::Bytes(vec![0x80, 0x81]));
        assert_eq!(binary_string(&[1, 2]), Value::Bytes(vec![1, 2]));
        assert_eq!(binary_string(&[0x7f]), Value::Bytes(vec![0x7f]));
    }

    #[test]
    fn numbers() {
        assert_eq!(
            unsigned_value(u64::MAX),
            Value::Decimal("18446744073709551615".into())
        );
        assert_eq!(unsigned_value(i64::MAX as u64), Value::Int(i64::MAX));
        assert_eq!(widen(0.1), 0.1);
        assert_eq!(widen(16_777_217.0), 16_777_216.0);
        assert_eq!(widen(f32::MAX), 3.4028235e38);
    }
}
