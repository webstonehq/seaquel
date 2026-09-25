//! Decodes Postgres cells into `Value`.
//!
//! Integers, floats, NUMERIC, BYTEA, JSON and arrays of those decode to their
//! own `Value` kinds. Everything else (dates, times, intervals, network,
//! geometry, enums) decodes to `Text` in the same format as before `Value`,
//! and so do arrays of dates, times, intervals, network addresses and UUIDs.
//! An array this can't decode is `<unsupported: T[]>` text.
//!
//! Dates and times are read from their binary form here, not by sqlx: sqlx
//! panics on ±infinity and on years the `time` crate can't hold, and wraps
//! TIME 24:00 to 00:00. Those come back as text Postgres parses again
//! (`infinity`, `24:00:00`, `0044-03-15 BC`, `10000-01-01`); every other
//! value keeps the `time` crate's format it always had.

use serde_json::Value as JsonValue;
use sqlx::{
    error::BoxDynError,
    postgres::types::{
        PgBox, PgCircle, PgInterval, PgLSeg, PgLine, PgMoney, PgPath, PgPoint, PgPolygon,
    },
    postgres::{types::Oid, PgTypeInfo, PgValueFormat, PgValueRef},
    types::{ipnetwork::IpNetwork, mac_address::MacAddress, BitVec},
    Decode, TypeInfo, Value as _, ValueRef,
};
use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};

use seaquel_engine::{DbError, Value};

use crate::numeric::Numeric;

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
            parts.push(format!(
                "{}{:02}:{:02}:{:02}",
                sign, hours, minutes, seconds
            ));
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

/// Decode as `T`, `None` when sqlx can't. The unchecked variant skips sqlx's
/// type-compatibility check, for enums, domains and the like.
fn get<T>(v: &PgValueRef) -> Option<T>
where
    T: for<'a> Decode<'a, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    ValueRef::to_owned(v).try_decode::<T>().ok()
}

fn get_unchecked<T>(v: &PgValueRef) -> Option<T>
where
    T: for<'a> Decode<'a, sqlx::Postgres>,
{
    ValueRef::to_owned(v).try_decode_unchecked::<T>().ok()
}

/// A decoded value, or `Null` when the cell couldn't be decoded as `T`.
fn map<T>(v: &PgValueRef, f: impl FnOnce(T) -> Value) -> Value
where
    T: for<'a> Decode<'a, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    get::<T>(v).map_or(Value::Null, f)
}

fn text(s: String) -> Value {
    Value::Text(s)
}

pub fn to_value(v: PgValueRef) -> Result<Value, DbError> {
    if v.is_null() {
        return Ok(Value::Null);
    }

    let type_name = v.type_info().name().to_string();
    let res = match type_name.as_str() {
        "UUID" => map::<Uuid>(&v, |u| Value::Text(u.0)),
        "CHAR" | "VARCHAR" | "TEXT" | "NAME" | "XML" | "TSVECTOR" | "TSQUERY" | "CITEXT" => {
            get::<String>(&v)
                .or_else(|| get_unchecked::<String>(&v))
                .map_or(Value::Null, Value::Text)
        }
        "FLOAT4" => map::<f32>(&v, |f| Value::Float(f.into())),
        "FLOAT8" => map::<f64>(&v, Value::Float),
        "INT2" => map::<i16>(&v, |n| Value::Int(n.into())),
        "INT4" => map::<i32>(&v, |n| Value::Int(n.into())),
        "INT8" => map::<i64>(&v, Value::Int),
        // Binary OIDs are 4 raw bytes; read as text they were garbage.
        "OID" => map::<Oid>(&v, |o| Value::Int(o.0.into())),
        "BOOL" => map::<bool>(&v, Value::Bool),
        "DATE" => map::<PgDate>(&v, |d| text(d.0)),
        "TIME" => map::<PgTime>(&v, |t| text(t.0)),
        "TIMETZ" => map::<PgTimeTzText>(&v, |t| text(t.0)),
        "TIMESTAMP" => map::<PgTimestamp>(&v, |t| text(t.0)),
        "TIMESTAMPTZ" => map::<PgTimestampTz>(&v, |t| text(t.0)),
        "INTERVAL" => map::<PgInterval>(&v, |i| text(format_interval(&i))),
        "JSON" | "JSONB" => map::<JsonValue>(&v, Value::Json),
        "BYTEA" => map::<Vec<u8>>(&v, Value::Bytes),
        "NUMERIC" => map::<Numeric>(&v, |n| Value::Decimal(n.to_text())),
        "INET" | "CIDR" => map::<IpNetwork>(&v, |n| text(n.to_string())),
        "MACADDR" => map::<MacAddress>(&v, |m| text(m.to_string())),
        "BIT" | "VARBIT" => map::<BitVec>(&v, |b| text(format_bitvec(&b))),
        "MONEY" => map::<PgMoney>(&v, |m| text(format_money(&m))),
        "POINT" => map::<PgPoint>(&v, |p| text(format!("({},{})", p.x, p.y))),
        "LINE" => map::<PgLine>(&v, |l| text(format!("{{{},{},{}}}", l.a, l.b, l.c))),
        "LSEG" => map::<PgLSeg>(&v, |l| {
            text(format!(
                "[({},{}),({},{})]",
                l.start_x, l.start_y, l.end_x, l.end_y
            ))
        }),
        "BOX" => map::<PgBox>(&v, |b| {
            text(format!(
                "({},{}),({},{})",
                b.upper_right_x, b.upper_right_y, b.lower_left_x, b.lower_left_y
            ))
        }),
        "PATH" => map::<PgPath>(&v, |p| {
            let points = format_points(&p.points);
            text(if p.closed {
                format!("({points})")
            } else {
                format!("[{points}]")
            })
        }),
        "POLYGON" => map::<PgPolygon>(&v, |p| text(format!("({})", format_points(&p.points)))),
        "CIRCLE" => map::<PgCircle>(&v, |c| text(format!("<({},{}),{}>", c.x, c.y, c.radius))),
        "INT2[]" => array::<i16>(&v, |n| Value::Int(n.into())),
        "INT4[]" => array::<i32>(&v, |n| Value::Int(n.into())),
        "INT8[]" => array::<i64>(&v, Value::Int),
        "FLOAT4[]" => array::<f32>(&v, |f| Value::Float(f.into())),
        "FLOAT8[]" => array::<f64>(&v, Value::Float),
        "NUMERIC[]" => array::<Numeric>(&v, |n| Value::Decimal(n.to_text())),
        "BOOL[]" => array::<bool>(&v, Value::Bool),
        "TEXT[]" | "VARCHAR[]" | "CHAR[]" | "NAME[]" => array::<String>(&v, Value::Text),
        "UUID[]" => array::<Uuid>(&v, |u| Value::Text(u.0)),
        "DATE[]" => array::<PgDate>(&v, |d| text(d.0)),
        "TIME[]" => array::<PgTime>(&v, |t| text(t.0)),
        "TIMETZ[]" => array::<PgTimeTzText>(&v, |t| text(t.0)),
        "TIMESTAMP[]" => array::<PgTimestamp>(&v, |t| text(t.0)),
        "TIMESTAMPTZ[]" => array::<PgTimestampTz>(&v, |t| text(t.0)),
        "INTERVAL[]" => array::<PgInterval>(&v, |i| text(format_interval(&i))),
        "INET[]" | "CIDR[]" => array::<IpNetwork>(&v, |n| text(n.to_string())),
        "MACADDR[]" => array::<MacAddress>(&v, |m| text(m.to_string())),
        "JSON[]" | "JSONB[]" => array::<JsonValue>(&v, Value::Json),
        "BYTEA[]" => array::<Vec<u8>>(&v, Value::Bytes),
        "VOID" => Value::Null,
        // Other arrays: their binary form isn't text.
        name if name.ends_with("[]") => unsupported(name),
        // Custom types (enums, domains, ...): try their text.
        _ => get_unchecked::<String>(&v).map_or_else(|| unsupported(&type_name), Value::Text),
    };

    Ok(res)
}

fn format_points(points: &[PgPoint]) -> String {
    points
        .iter()
        .map(|p| format!("({},{})", p.x, p.y))
        .collect::<Vec<_>>()
        .join(",")
}

fn unsupported(type_name: &str) -> Value {
    Value::Text(format!("<unsupported: {type_name}>"))
}

/// A one-dimensional, 1-based array; NULL elements stay `Null`. An array
/// sqlx can't decode (multi-dimensional, or a lower bound other than 1) is
/// `<unsupported: T[]>` text rather than `Null`, which would read as SQL NULL.
fn array<T>(v: &PgValueRef, f: impl Fn(T) -> Value) -> Value
where
    T: for<'a> Decode<'a, sqlx::Postgres>
        + sqlx::Type<sqlx::Postgres>
        + sqlx::postgres::PgHasArrayType,
{
    match get::<Vec<Option<T>>>(v) {
        Some(items) => Value::Array(
            items
                .into_iter()
                .map(|i| i.map_or(Value::Null, &f))
                .collect(),
        ),
        None => unsupported(v.type_info().name()),
    }
}

/// A UUID in Postgres's text form. sqlx's `uuid` feature is off, and a UUID
/// isn't a `String` to sqlx, so this reads the 16 bytes itself.
struct Uuid(String);

impl sqlx::Type<sqlx::Postgres> for Uuid {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(Oid(2950))
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        ty.name() == "UUID"
    }
}

impl sqlx::postgres::PgHasArrayType for Uuid {
    fn array_type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(Oid(2951))
    }

    fn array_compatible(ty: &PgTypeInfo) -> bool {
        ty.name() == "UUID[]"
    }
}

impl Decode<'_, sqlx::Postgres> for Uuid {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        if value.format() == PgValueFormat::Text {
            return Ok(Uuid(value.as_str()?.to_string()));
        }
        let b = value.as_bytes()?;
        if b.len() != 16 {
            return Err(format!("a UUID has 16 bytes, got {}", b.len()).into());
        }
        let hex =
            |r: std::ops::Range<usize>| b[r].iter().map(|x| format!("{x:02x}")).collect::<String>();
        Ok(Uuid(format!(
            "{}-{}-{}-{}-{}",
            hex(0..4),
            hex(4..6),
            hex(6..8),
            hex(8..10),
            hex(10..16)
        )))
    }
}

// ── Dates and times ─────────────────────────────────────────────────────────

/// Microseconds in a day.
const DAY_US: i64 = 86_400_000_000;
/// 2000-01-01, Postgres's epoch, in days since 1970-01-01.
const PG_EPOCH_UNIX_DAYS: i64 = 10_957;

/// Days since 1970-01-01 → proleptic Gregorian (year, month, day), for any
/// day count (Howard Hinnant's `civil_from_days`). Year 0 is 1 BC.
fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A `time::Date` when the `time` crate can hold it and the year is AD.
fn time_date(year: i64, month: u8, day: u8) -> Option<Date> {
    if !(1..=9999).contains(&year) {
        return None;
    }
    Date::from_calendar_date(year as i32, Month::try_from(month).ok()?, day).ok()
}

/// `YYYY-MM-DD`, and whether the year is BC (Postgres's spelling).
fn pg_date(year: i64, month: u8, day: u8) -> (String, bool) {
    if year <= 0 {
        (format!("{:04}-{month:02}-{day:02}", 1 - year), true)
    } else {
        (format!("{year:04}-{month:02}-{day:02}"), false)
    }
}

/// `HH:MM:SS[.ffffff]`, fraction without trailing zeros, as Postgres prints.
fn pg_clock(us: i64) -> String {
    let secs = us / 1_000_000;
    let frac = us % 1_000_000;
    let clock = format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
    if frac == 0 {
        clock
    } else {
        format!("{clock}.{}", format!("{frac:06}").trim_end_matches('0'))
    }
}

fn be_i64(b: &[u8]) -> Result<i64, BoxDynError> {
    Ok(i64::from_be_bytes(b.try_into().map_err(|_| format!("expected 8 bytes, got {}", b.len()))?))
}

fn time_of_day(us: i64) -> Result<Time, BoxDynError> {
    if !(0..DAY_US).contains(&us) {
        return Err(format!("time of day out of range: {us}µs").into());
    }
    let secs = us / 1_000_000;
    Ok(Time::from_hms_micro(
        (secs / 3600) as u8,
        (secs / 60 % 60) as u8,
        (secs % 60) as u8,
        (us % 1_000_000) as u32,
    )?)
}

fn decode_date(b: &[u8]) -> Result<String, BoxDynError> {
    let days = i32::from_be_bytes(b.try_into().map_err(|_| format!("expected 4 bytes, got {}", b.len()))?);
    match days {
        i32::MAX => return Ok("infinity".into()),
        i32::MIN => return Ok("-infinity".into()),
        _ => {}
    }
    let (y, m, d) = civil_from_days(i64::from(days) + PG_EPOCH_UNIX_DAYS);
    Ok(match time_date(y, m, d) {
        Some(date) => date.to_string(),
        None => match pg_date(y, m, d) {
            (date, true) => format!("{date} BC"),
            (date, false) => date,
        },
    })
}

fn decode_timestamp(b: &[u8], utc: bool) -> Result<String, BoxDynError> {
    let us = be_i64(b)?;
    match us {
        i64::MAX => return Ok("infinity".into()),
        i64::MIN => return Ok("-infinity".into()),
        _ => {}
    }
    let of_day = us.rem_euclid(DAY_US);
    let (y, m, d) = civil_from_days(us.div_euclid(DAY_US) + PG_EPOCH_UNIX_DAYS);
    if let Some(date) = time_date(y, m, d) {
        let t = PrimitiveDateTime::new(date, time_of_day(of_day)?);
        return Ok(if utc { t.assume_utc().to_string() } else { t.to_string() });
    }
    let (date, bc) = pg_date(y, m, d);
    Ok(format!(
        "{date} {}{}{}",
        pg_clock(of_day),
        if utc { "+00" } else { "" },
        if bc { " BC" } else { "" }
    ))
}

fn decode_time(b: &[u8]) -> Result<String, BoxDynError> {
    let us = be_i64(b)?;
    Ok(if us == DAY_US { "24:00:00".into() } else { time_of_day(us)?.to_string() })
}

fn decode_timetz(b: &[u8]) -> Result<String, BoxDynError> {
    if b.len() != 12 {
        return Err(format!("a TIMETZ has 12 bytes, got {}", b.len()).into());
    }
    let us = be_i64(&b[..8])?;
    // Stored as seconds west of UTC.
    let west = i32::from_be_bytes(b[8..].try_into()?);
    let offset = -UtcOffset::from_whole_seconds(west)?;
    let time = if us == DAY_US { "24:00:00".into() } else { time_of_day(us)?.to_string() };
    Ok(format!("{time}{offset}"))
}

/// A date/time type decoded to text by `decode` (binary) or taken as Postgres
/// printed it (text format).
macro_rules! temporal {
    ($name:ident, $pg:literal, $oid:literal, $array_oid:literal, $decode:expr) => {
        struct $name(String);

        impl sqlx::Type<sqlx::Postgres> for $name {
            fn type_info() -> PgTypeInfo {
                PgTypeInfo::with_oid(Oid($oid))
            }

            fn compatible(ty: &PgTypeInfo) -> bool {
                ty.name() == $pg
            }
        }

        impl sqlx::postgres::PgHasArrayType for $name {
            fn array_type_info() -> PgTypeInfo {
                PgTypeInfo::with_oid(Oid($array_oid))
            }

            fn array_compatible(ty: &PgTypeInfo) -> bool {
                ty.name() == concat!($pg, "[]")
            }
        }

        impl Decode<'_, sqlx::Postgres> for $name {
            fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
                if value.format() == PgValueFormat::Text {
                    return Ok($name(value.as_str()?.to_string()));
                }
                let decode: fn(&[u8]) -> Result<String, BoxDynError> = $decode;
                decode(value.as_bytes()?).map($name)
            }
        }
    };
}

temporal!(PgDate, "DATE", 1082, 1182, decode_date);
temporal!(PgTime, "TIME", 1083, 1183, decode_time);
temporal!(PgTimeTzText, "TIMETZ", 1266, 1270, decode_timetz);
temporal!(PgTimestamp, "TIMESTAMP", 1114, 1115, |b| decode_timestamp(b, false));
temporal!(PgTimestampTz, "TIMESTAMPTZ", 1184, 1185, |b| decode_timestamp(b, true));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(PG_EPOCH_UNIX_DAYS), (2000, 1, 1));
        assert_eq!(civil_from_days(-719_528), (0, 1, 1)); // 1 BC
        assert_eq!(civil_from_days(2_932_896), (9999, 12, 31));
        assert_eq!(civil_from_days(2_932_897), (10000, 1, 1));
    }

    #[test]
    fn dates_from_binary() {
        let d = |days: i32| decode_date(&days.to_be_bytes()).unwrap();
        assert_eq!(d(0), "2000-01-01");
        assert_eq!(d(i32::MAX), "infinity");
        assert_eq!(d(i32::MIN), "-infinity");
        assert_eq!(d(-730_120), "0001-12-31 BC");
        assert_eq!(d(-730_485), "0001-01-01 BC");
        assert_eq!(d(2_921_940), "10000-01-01");
    }

    #[test]
    fn timestamps_from_binary() {
        let t = |us: i64, utc| decode_timestamp(&us.to_be_bytes(), utc).unwrap();
        assert_eq!(t(1_500_000, false), "2000-01-01 0:00:01.5");
        assert_eq!(t(0, true), "2000-01-01 0:00:00.0 +00:00:00");
        assert_eq!(t(i64::MAX, true), "infinity");
        assert_eq!(t(i64::MIN, false), "-infinity");
        // One microsecond before 1 AD.
        let bc = -730_119 * DAY_US - 1;
        assert_eq!(t(bc, false), "0001-12-31 23:59:59.999999 BC");
        assert_eq!(t(bc, true), "0001-12-31 23:59:59.999999+00 BC");
    }

    #[test]
    fn times_from_binary() {
        assert_eq!(decode_time(&DAY_US.to_be_bytes()).unwrap(), "24:00:00");
        assert_eq!(decode_time(&0i64.to_be_bytes()).unwrap(), "0:00:00.0");
        assert!(decode_time(&(DAY_US + 1).to_be_bytes()).is_err());
        let mut tz = DAY_US.to_be_bytes().to_vec();
        tz.extend((-19_800i32).to_be_bytes());
        assert_eq!(decode_timetz(&tz).unwrap(), "24:00:00+05:30:00");
    }
}
