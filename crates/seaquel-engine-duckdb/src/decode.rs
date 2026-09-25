//! Decodes DuckDB result cells into `Value`, straight from the Arrow chunks
//! DuckDB hands out, without duckdb-rs's `ValueRef` (which panics on Arrow
//! types it doesn't map, e.g. `TIME_NS`, and can't tell some DuckDB types
//! apart once they're in a container).
//!
//! | DuckDB                                | `Value`                                   |
//! |---------------------------------------|-------------------------------------------|
//! | BOOLEAN                               | `Bool`                                    |
//! | TINYINT … BIGINT, UTINYINT … UINTEGER | `Int`                                     |
//! | UBIGINT, HUGEINT, UHUGEINT, BIGNUM    | `Int` when it fits `i64`, else `Decimal` digits |
//! | FLOAT, DOUBLE                         | `Float` (NaN and ±inf included); FLOAT widened exactly (see [`widen`]) |
//! | DECIMAL(p, s)                         | `Decimal`, scale kept (`1.50`); always a leading digit (`-0.05`, where DuckDB prints DECIMAL(2, 2) as `-.05`) |
//! | VARCHAR, UUID, ENUM                   | `Text`                                    |
//! | JSON                                  | `Json` (`Text` if DuckDB let invalid JSON through) |
//! | BLOB, GEOMETRY (WKB)                  | `Bytes`                                   |
//! | BIT                                   | `Text` of 0s and 1s                       |
//! | DATE, TIME*, TIMESTAMP*, INTERVAL     | `Text` as `CAST(v AS VARCHAR)` prints it  |
//! | TIMESTAMPTZ                           | `Text`, UTC with `+00`                    |
//! | LIST, ARRAY                           | `Array` of the element values             |
//! | STRUCT, MAP                           | `Json` (see [`json_of`]), object keys sorted |
//! | UNION                                 | the active member's value                 |
//!
//! Temporal text (not DECIMAL, see above) is exactly what DuckDB prints (`CAST(v AS VARCHAR)` with
//! `TimeZone = 'UTC'`), so it binds back as text and compares equal:
//! `2024-01-01`, `0044-03-15 (BC)`, `infinity`, `12:00:00.5`,
//! `2024-01-01 12:00:00.123456789`, `12:00:00+05:30`,
//! `1 year 2 months -3 days -00:00:01.5`. TIMESTAMPTZ is printed in UTC
//! whatever the session's `TimeZone`: `2024-01-01 12:00:00+00` names one
//! instant, and DuckDB prints it the same way under `TimeZone = 'UTC'`.
//!
//! The driver turns on `arrow_lossless_conversion` for its connection, so
//! TIMETZ keeps its offset, HUGEINT/UHUGEINT/UUID arrive as their own 16
//! bytes and BOOLEAN as an `arrow.bool8` Int8. The lossy forms still decode
//! (a session can `RESET` the setting): HUGEINT and UHUGEINT from
//! `Decimal128(38, 0)`, UUID from text, BOOLEAN from Arrow's Boolean, and
//! TIMETZ from a bare `Time64`, which has lost its offset, so it's printed
//! without one.

use duckdb::arrow::array::{Array, ArrayRef, AsArray, GenericListArray, OffsetSizeTrait};
use duckdb::arrow::datatypes::{
    DataType, Date32Type, Decimal128Type, Decimal32Type, Decimal64Type, Float32Type, Float64Type,
    Int16Type, Int32Type, Int64Type, Int8Type, IntervalMonthDayNanoType, IntervalUnit,
    Time64MicrosecondType, Time64NanosecondType, TimeUnit, TimestampMicrosecondType,
    TimestampMillisecondType, TimestampNanosecondType, TimestampSecondType, UInt16Type, UInt32Type,
    UInt64Type, UInt8Type,
};
use duckdb::core::{LogicalTypeHandle, LogicalTypeId};
use serde_json::Value as Json;

use seaquel_engine::Value;

/// What the Arrow type alone doesn't say about a column (or an element of
/// one): DuckDB types that share an Arrow carrier with another type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// The Arrow type says it all.
    Plain,
    /// `arrow.bool8` (an Int8) with `arrow_lossless_conversion`.
    Bool,
    HugeInt,
    UHugeInt,
    Uuid,
    Json,
    Bit,
    Bignum,
    TimeTz,
    /// LIST and ARRAY.
    List(Box<Kind>),
    Struct(Vec<Kind>),
    Map(Box<Kind>, Box<Kind>),
    /// Members in declaration order, as Arrow's union fields are.
    Union(Vec<Kind>),
}

impl Kind {
    /// The kind of a result column's DuckDB type. duckdb-rs panics on type
    /// ids it doesn't know; the caller catches that and uses [`Kind::Plain`],
    /// so decoding goes by the Arrow type alone.
    pub(crate) fn of(t: &LogicalTypeHandle) -> Kind {
        Self::walk(t)
    }

    fn walk(t: &LogicalTypeHandle) -> Kind {
        let children = || {
            (0..t.num_children())
                .map(|i| Self::walk(&t.child(i)))
                .collect()
        };
        match t.id() {
            LogicalTypeId::Boolean => Kind::Bool,
            LogicalTypeId::Hugeint => Kind::HugeInt,
            LogicalTypeId::UHugeint => Kind::UHugeInt,
            LogicalTypeId::Uuid => Kind::Uuid,
            LogicalTypeId::Bit => Kind::Bit,
            LogicalTypeId::Bignum => Kind::Bignum,
            LogicalTypeId::TimeTZ => Kind::TimeTz,
            LogicalTypeId::Varchar if t.get_alias().as_deref() == Some("JSON") => Kind::Json,
            LogicalTypeId::List | LogicalTypeId::Array => {
                Kind::List(Box::new(Self::walk(&t.child(0))))
            }
            LogicalTypeId::Struct => Kind::Struct(children()),
            LogicalTypeId::Union => Kind::Union(children()),
            LogicalTypeId::Map => Kind::Map(
                Box::new(Self::walk(&t.child(0))),
                Box::new(Self::walk(&t.child(1))),
            ),
            _ => Kind::Plain,
        }
    }

    fn child(&self, i: usize) -> &Kind {
        match self {
            Kind::Struct(k) | Kind::Union(k) => k.get(i).unwrap_or(&Kind::Plain),
            _ => &Kind::Plain,
        }
    }
}

/// DuckDB's ±infinity for DATE (`date_t::infinity()`) and the TIMESTAMP
/// types (`timestamp_t::infinity()`, in every unit).
const DATE_INFINITY: i32 = i32::MAX;
const TIMESTAMP_INFINITY: i64 = i64::MAX;

/// Row `row` of `array`, as `kind` says to read it. An Arrow type this
/// doesn't read is an error naming it.
pub(crate) fn decode(kind: &Kind, array: &dyn Array, row: usize) -> Result<Value, String> {
    if array.is_null(row) {
        return Ok(Value::Null);
    }
    let dt = array.data_type();
    let unexpected = || format!("unexpected Arrow type {dt}");
    // `as_*_opt` only fails when the Arrow type lied about its array.
    macro_rules! prim {
        ($t:ty) => {
            array
                .as_primitive_opt::<$t>()
                .ok_or_else(unexpected)?
                .value(row)
        };
    }
    Ok(match dt {
        DataType::Null => Value::Null,
        DataType::Boolean => Value::Bool(array.as_boolean_opt().ok_or_else(unexpected)?.value(row)),
        DataType::Int8 if *kind == Kind::Bool => Value::Bool(prim!(Int8Type) != 0),
        DataType::Int8 => Value::Int(prim!(Int8Type).into()),
        DataType::Int16 => Value::Int(prim!(Int16Type).into()),
        DataType::Int32 => Value::Int(prim!(Int32Type).into()),
        DataType::Int64 => Value::Int(prim!(Int64Type)),
        DataType::UInt8 => Value::Int(prim!(UInt8Type).into()),
        DataType::UInt16 => Value::Int(prim!(UInt16Type).into()),
        DataType::UInt32 => Value::Int(prim!(UInt32Type).into()),
        DataType::UInt64 => unsigned(prim!(UInt64Type).into()),
        DataType::Float32 => Value::Float(widen(prim!(Float32Type))),
        DataType::Float64 => Value::Float(prim!(Float64Type)),
        DataType::Decimal32(_, s) => Value::Decimal(decimal_text(prim!(Decimal32Type).into(), *s)),
        DataType::Decimal64(_, s) => Value::Decimal(decimal_text(prim!(Decimal64Type).into(), *s)),
        // HUGEINT and UHUGEINT share DECIMAL(38, 0)'s carrier without
        // `arrow_lossless_conversion`.
        DataType::Decimal128(_, s) => {
            let v = prim!(Decimal128Type);
            match kind {
                Kind::HugeInt => signed(v),
                Kind::UHugeInt => unsigned(v as u128),
                _ => Value::Decimal(decimal_text(v, *s)),
            }
        }
        DataType::Utf8 => text(
            kind,
            array
                .as_string_opt::<i32>()
                .ok_or_else(unexpected)?
                .value(row),
        ),
        DataType::LargeUtf8 => text(
            kind,
            array
                .as_string_opt::<i64>()
                .ok_or_else(unexpected)?
                .value(row),
        ),
        DataType::Utf8View => text(
            kind,
            array
                .as_string_view_opt()
                .ok_or_else(unexpected)?
                .value(row),
        ),
        DataType::Binary => binary(
            kind,
            array
                .as_binary_opt::<i32>()
                .ok_or_else(unexpected)?
                .value(row),
        )?,
        DataType::LargeBinary => binary(
            kind,
            array
                .as_binary_opt::<i64>()
                .ok_or_else(unexpected)?
                .value(row),
        )?,
        DataType::BinaryView => binary(
            kind,
            array
                .as_binary_view_opt()
                .ok_or_else(unexpected)?
                .value(row),
        )?,
        DataType::FixedSizeBinary(_) => {
            let b = array
                .as_fixed_size_binary_opt()
                .ok_or_else(unexpected)?
                .value(row);
            match (kind, <[u8; 16]>::try_from(b), <[u8; 8]>::try_from(b)) {
                (Kind::HugeInt, Ok(b), _) => signed(i128::from_le_bytes(b)),
                (Kind::UHugeInt, Ok(b), _) => unsigned(u128::from_le_bytes(b)),
                (Kind::Uuid, Ok(b), _) => Value::Text(uuid_text(&b)),
                (Kind::TimeTz, _, Ok(b)) => Value::Text(time_tz_text(u64::from_le_bytes(b))),
                _ => binary(kind, b)?,
            }
        }
        DataType::Date32 => Value::Text(date_text(prim!(Date32Type))),
        DataType::Time64(TimeUnit::Microsecond) => {
            Value::Text(time_text(prim!(Time64MicrosecondType), 6))
        }
        DataType::Time64(TimeUnit::Nanosecond) => {
            Value::Text(time_text(prim!(Time64NanosecondType), 9))
        }
        DataType::Timestamp(unit, tz) => {
            let (v, per_second) = match unit {
                TimeUnit::Second => (prim!(TimestampSecondType), 1),
                TimeUnit::Millisecond => (prim!(TimestampMillisecondType), 1_000),
                TimeUnit::Microsecond => (prim!(TimestampMicrosecondType), 1_000_000),
                TimeUnit::Nanosecond => (prim!(TimestampNanosecondType), 1_000_000_000),
            };
            Value::Text(timestamp_text(v, per_second, tz.is_some()))
        }
        DataType::Interval(IntervalUnit::MonthDayNano) => {
            let v = prim!(IntervalMonthDayNanoType);
            Value::Text(interval_text(v.months, v.days, v.nanoseconds))
        }
        DataType::List(_) => list(
            kind,
            array.as_list_opt::<i32>().ok_or_else(unexpected)?,
            row,
        )?,
        DataType::LargeList(_) => list(
            kind,
            array.as_list_opt::<i64>().ok_or_else(unexpected)?,
            row,
        )?,
        DataType::FixedSizeList(..) => {
            let a = array.as_fixed_size_list_opt().ok_or_else(unexpected)?;
            let len = a.value_length() as usize;
            let start = a.value_offset(row) as usize;
            elements(kind, a.values(), start..start + len)?
        }
        DataType::Struct(fields) => {
            let a = array.as_struct_opt().ok_or_else(unexpected)?;
            let mut obj = serde_json::Map::new();
            for (i, (field, column)) in fields.iter().zip(a.columns()).enumerate() {
                obj.insert(
                    field.name().clone(),
                    json_of(&decode(kind.child(i), column, row)?),
                );
            }
            Value::Json(Json::Object(obj))
        }
        DataType::Map(..) => {
            let a = array.as_map_opt().ok_or_else(unexpected)?;
            let (key_kind, value_kind) = match kind {
                Kind::Map(k, v) => (&**k, &**v),
                _ => (&Kind::Plain, &Kind::Plain),
            };
            let offsets = a.value_offsets();
            let range = offsets[row] as usize..offsets[row + 1] as usize;
            let mut entries = Vec::with_capacity(range.len());
            for i in range {
                entries.push((
                    decode(key_kind, a.keys(), i)?,
                    decode(value_kind, a.values(), i)?,
                ));
            }
            Value::Json(map_json(entries))
        }
        DataType::Union(fields, _) => {
            let a = array.as_union_opt().ok_or_else(unexpected)?;
            let id = a.type_id(row);
            let member = fields
                .iter()
                .position(|(i, _)| i == id)
                .ok_or_else(unexpected)?;
            decode(kind.child(member), a.child(id), a.value_offset(row))?
        }
        DataType::Dictionary(key, _) => {
            let label = match **key {
                DataType::UInt8 => dictionary_label::<UInt8Type>(array, row),
                DataType::UInt16 => dictionary_label::<UInt16Type>(array, row),
                DataType::UInt32 => dictionary_label::<UInt32Type>(array, row),
                _ => None,
            };
            Value::Text(label.ok_or_else(unexpected)?.to_string())
        }
        _ => return Err(format!("unsupported Arrow type {dt}")),
    })
}

/// A FLOAT as the f64 holding exactly the same number, so FLOAT 0.1 is
/// 0.10000000149011612, not the 0.1 of its shortest text (MySQL's decoder
/// uses that). DuckDB types the parameter in `k = ?` as the bound DOUBLE
/// and compares the FLOAT column widened to DOUBLE, so only the exact value
/// finds the row again: 0.1 matched nothing (`tests/values.rs` checks 0.1,
/// the subnormal 1e-40 and 3.4028235e38 as FLOAT keys). The binder can't
/// narrow instead: it doesn't know the column's type, and binding short
/// doubles as FLOAT would break DOUBLE keys.
fn widen(f: f32) -> f64 {
    f64::from(f)
}

fn signed(v: i128) -> Value {
    i64::try_from(v).map_or_else(|_| Value::Decimal(v.to_string()), Value::Int)
}

fn unsigned(v: u128) -> Value {
    i64::try_from(v).map_or_else(|_| Value::Decimal(v.to_string()), Value::Int)
}

fn text(kind: &Kind, s: &str) -> Value {
    match kind {
        Kind::Json => {
            serde_json::from_str(s).map_or_else(|_| Value::Text(s.to_string()), Value::Json)
        }
        _ => Value::Text(s.to_string()),
    }
}

fn binary(kind: &Kind, b: &[u8]) -> Result<Value, String> {
    Ok(match kind {
        Kind::Bit => Value::Text(bit_text(b)?),
        Kind::Bignum => bignum(b)?,
        _ => Value::Bytes(b.to_vec()),
    })
}

fn list<O: OffsetSizeTrait>(
    kind: &Kind,
    a: &GenericListArray<O>,
    row: usize,
) -> Result<Value, String> {
    let offsets = a.value_offsets();
    elements(
        kind,
        a.values(),
        offsets[row].as_usize()..offsets[row + 1].as_usize(),
    )
}

/// Rows `range` of a list's child array: only this cell's elements, so a
/// column of lists decodes in linear time.
fn elements(
    kind: &Kind,
    values: &ArrayRef,
    range: std::ops::Range<usize>,
) -> Result<Value, String> {
    let element = match kind {
        Kind::List(k) => &**k,
        _ => &Kind::Plain,
    };
    range
        .map(|i| decode(element, values, i))
        .collect::<Result<_, _>>()
        .map(Value::Array)
}

fn dictionary_label<K: duckdb::arrow::datatypes::ArrowDictionaryKeyType>(
    array: &dyn Array,
    row: usize,
) -> Option<&str> {
    let a = array.as_dictionary_opt::<K>()?;
    let key = a.key(row)?;
    let values = a.values().as_string_opt::<i32>()?;
    (key < values.len()).then(|| values.value(key))
}

// ── Nested values as JSON ────────────────────────────────────────────────────

/// A STRUCT field's or MAP entry's value as JSON. JSON holds booleans,
/// strings and numbers up to 2^53 exactly; everything else is a string in
/// the text DuckDB prints for it, so nothing is rounded:
///
/// - integers beyond ±(2^53 − 1) and DECIMALs: their digits (`"12.50"`);
/// - NaN and ±inf: `"nan"`, `"inf"`, `"-inf"`;
/// - BLOBs: DuckDB's escaped text (`"a\\x00"`, as `CAST(b AS VARCHAR)`);
/// - dates, times, intervals, UUIDs: their text, as at the top level;
/// - LISTs: arrays; STRUCTs and MAPs: nested the same way; JSON: as is.
///
/// The wire has no tags inside `json` values, which is why this doesn't use
/// them.
///
/// Object keys come out **sorted**, not in the STRUCT's field order (nor a
/// MAP's entry order): the workspace's serde_json has no `preserve_order`,
/// so `serde_json::Map` is a BTreeMap. Enabling it is a workspace-wide
/// follow-up in the phase 2 plan.
pub(crate) fn json_of(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) if i.unsigned_abs() <= seaquel_engine::MAX_SAFE_INTEGER as u64 => (*i).into(),
        Value::Int(i) => Json::String(i.to_string()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map_or_else(|| Json::String(float_text(*f).to_string()), Json::Number),
        Value::Decimal(s) | Value::Text(s) => Json::String(s.clone()),
        Value::Bytes(b) => Json::String(blob_text(b)),
        Value::Json(j) => j.clone(),
        Value::Array(items) => Json::Array(items.iter().map(json_of).collect()),
    }
}

/// DuckDB's text for a non-finite float.
fn float_text(f: f64) -> &'static str {
    if f.is_nan() {
        "nan"
    } else if f > 0.0 {
        "inf"
    } else {
        "-inf"
    }
}

/// A MAP as JSON: an object when every key is text or a number (keyed by
/// its text, as DuckDB's `to_json` does: `MAP {1: 'a'}` is `{"1": "a"}`),
/// otherwise an array of `{"key": …, "value": …}` entries, since a float,
/// BLOB, boolean or nested key has no unambiguous object key.
fn map_json(entries: Vec<(Value, Value)>) -> Json {
    let keyed = entries
        .iter()
        .all(|(k, _)| matches!(k, Value::Text(_) | Value::Int(_) | Value::Decimal(_)));
    if keyed {
        let mut obj = serde_json::Map::new();
        for (k, v) in entries {
            let key = match k {
                Value::Text(s) | Value::Decimal(s) => s,
                Value::Int(i) => i.to_string(),
                _ => unreachable!("checked above"),
            };
            obj.insert(key, json_of(&v));
        }
        Json::Object(obj)
    } else {
        Json::Array(
            entries
                .iter()
                .map(|(k, v)| serde_json::json!({ "key": json_of(k), "value": json_of(v) }))
                .collect(),
        )
    }
}

// ── Text formats ─────────────────────────────────────────────────────────────

/// `v × 10^-scale`, scale kept: `150, 2` is `1.50`, `-5, 2` is `-0.05`.
fn decimal_text(v: i128, scale: i8) -> String {
    let digits = v.unsigned_abs().to_string();
    let sign = if v < 0 { "-" } else { "" };
    let Ok(scale @ 1..) = usize::try_from(scale) else {
        return format!("{sign}{digits}");
    };
    let digits = format!("{digits:0>width$}", width = scale + 1);
    let (int, frac) = digits.split_at(digits.len() - scale);
    format!("{sign}{int}.{frac}")
}

/// Proleptic Gregorian date of a day count since 1970-01-01 (Howard
/// Hinnant's `civil_from_days`). Year 0 is 1 BC.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `2024-01-01`, `0044-03-15 (BC)`, `infinity`.
fn date_text(days: i32) -> String {
    match days {
        DATE_INFINITY => "infinity".into(),
        d if d == -DATE_INFINITY => "-infinity".into(),
        d => {
            let (y, m, d) = civil_from_days(d.into());
            date_parts(y, m, d)
        }
    }
}

fn date_parts(y: i64, m: u32, d: u32) -> String {
    if y <= 0 {
        format!("{:04}-{m:02}-{d:02} (BC)", 1 - y)
    } else {
        format!("{y:04}-{m:02}-{d:02}")
    }
}

/// `.5`, `.123456789`: a fraction of `digits` digits, trailing zeros
/// dropped; nothing for zero.
fn fraction(v: i64, digits: usize) -> String {
    if v == 0 {
        return String::new();
    }
    let f = format!("{v:0digits$}");
    format!(".{}", f.trim_end_matches('0'))
}

/// Time of day from a count of `10^-digits` seconds: `12:00:00`,
/// `12:00:00.5`, `24:00:00`.
fn time_text(v: i64, digits: usize) -> String {
    let per_second = 10_i64.pow(digits as u32);
    let secs = v.div_euclid(per_second);
    format!(
        "{:02}:{:02}:{:02}{}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        fraction(v.rem_euclid(per_second), digits)
    )
}

/// DuckDB's `dtime_tz_t`: microseconds of the day in the high 40 bits, and
/// the offset in the low 24 as `16:00:00 − 1 s − offset` seconds.
fn time_tz_text(bits: u64) -> String {
    const MAX_OFFSET: i64 = 16 * 60 * 60 - 1;
    let micros = (bits >> 24) as i64;
    let offset = MAX_OFFSET - (bits & 0xFF_FFFF) as i64;
    let sign = if offset < 0 { '-' } else { '+' };
    let o = offset.abs();
    let mut text = format!("{}{sign}{:02}", time_text(micros, 6), o / 3600);
    if o % 3600 != 0 {
        text += &format!(":{:02}", o / 60 % 60);
        if o % 60 != 0 {
            text += &format!(":{:02}", o % 60);
        }
    }
    text
}

/// `2024-01-01 12:00:00.5`, `0044-03-15 (BC) 12:00:00`, `infinity`; with
/// `utc`, `+00` at the end. `v` counts `1 / per_second` seconds.
fn timestamp_text(v: i64, per_second: i64, utc: bool) -> String {
    match v {
        TIMESTAMP_INFINITY => return "infinity".into(),
        v if v == -TIMESTAMP_INFINITY => return "-infinity".into(),
        _ => {}
    }
    let per_day = per_second * 86_400;
    let (y, m, d) = civil_from_days(v.div_euclid(per_day));
    let digits = if per_second == 1_000_000_000 { 9 } else { 6 };
    let of_day = v.rem_euclid(per_day);
    // As micro- or nanoseconds, for `time_text`.
    let of_day = if digits == 9 {
        of_day
    } else {
        of_day * (1_000_000 / per_second)
    };
    let offset = if utc { "+00" } else { "" };
    format!(
        "{} {}{offset}",
        date_parts(y, m, d),
        time_text(of_day, digits)
    )
}

/// `1 year 2 months 3 days 04:05:06.789`, `-1 day -00:00:01`, `00:00:00`.
/// DuckDB keeps months, days and microseconds apart, each with its own
/// sign; nanoseconds that aren't whole microseconds (DuckDB doesn't make
/// them) are printed to the nanosecond.
fn interval_text(months: i32, days: i32, nanos: i64) -> String {
    fn unit(n: i64, name: &str) -> String {
        format!("{n} {name}{}", if n == 1 || n == -1 { "" } else { "s" })
    }
    let mut parts = Vec::new();
    let (years, months) = (months / 12, months % 12);
    if years != 0 {
        parts.push(unit(years.into(), "year"));
    }
    if months != 0 {
        parts.push(unit(months.into(), "month"));
    }
    if days != 0 {
        parts.push(unit(days.into(), "day"));
    }
    if nanos != 0 || parts.is_empty() {
        let sign = if nanos < 0 { "-" } else { "" };
        let abs = nanos.unsigned_abs() as i64;
        let (v, digits) = if abs % 1000 == 0 {
            (abs / 1000, 6)
        } else {
            (abs, 9)
        };
        // Not `time_text`'s day wrap: an interval has hours past 24.
        let per_second = 10_i64.pow(digits as u32);
        let secs = v / per_second;
        parts.push(format!(
            "{sign}{:02}:{:02}:{:02}{}",
            secs / 3600,
            secs / 60 % 60,
            secs % 60,
            fraction(v % per_second, digits)
        ));
    }
    parts.join(" ")
}

/// `00000000-0000-4000-8000-000000000000`, from Arrow's (big-endian) bytes.
fn uuid_text(b: &[u8; 16]) -> String {
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// DuckDB's BIT: a byte holding how many leading bits of the next byte are
/// padding, then the bits, most significant first.
fn bit_text(b: &[u8]) -> Result<String, String> {
    let (&padding, bits) = b.split_first().ok_or("empty BIT value")?;
    Ok(bits
        .iter()
        .flat_map(|byte| {
            (0..8)
                .rev()
                .map(move |i| if byte >> i & 1 == 1 { '1' } else { '0' })
        })
        .skip(padding.into())
        .collect())
}

/// DuckDB's BIGNUM (VARINT): a 3-byte header whose top bit is set for a
/// non-negative number, then the magnitude's bytes, big-endian. A negative
/// number has every byte, header included, inverted.
fn bignum(b: &[u8]) -> Result<Value, String> {
    if b.len() < 4 {
        return Err(format!("BIGNUM of {} bytes", b.len()));
    }
    let negative = b[0] & 0x80 == 0;
    let magnitude: Vec<u8> = b[3..]
        .iter()
        .map(|&x| if negative { !x } else { x })
        .skip_while(|&x| x == 0)
        .collect();
    if magnitude.len() <= 16 {
        let mut buf = [0u8; 16];
        buf[16 - magnitude.len()..].copy_from_slice(&magnitude);
        let m = u128::from_be_bytes(buf);
        if negative {
            if let Some(v) = i128::try_from(m).ok().map(|v| -v) {
                return Ok(signed(v));
            }
        } else {
            return Ok(unsigned(m));
        }
    }
    let digits = big_decimal_digits(&magnitude);
    Ok(Value::Decimal(if negative {
        format!("-{digits}")
    } else {
        digits
    }))
}

/// The decimal digits of a big-endian unsigned magnitude of any length.
fn big_decimal_digits(be: &[u8]) -> String {
    // Little-endian base-2^32 limbs, divided by 10^9 until zero.
    let mut limbs: Vec<u32> = be
        .rchunks(4)
        .map(|c| c.iter().fold(0u32, |acc, &x| acc << 8 | u32::from(x)))
        .collect();
    let mut chunks = Vec::new();
    while limbs.iter().any(|&l| l != 0) {
        let mut rem = 0u64;
        for l in limbs.iter_mut().rev() {
            let cur = rem << 32 | u64::from(*l);
            *l = (cur / 1_000_000_000) as u32;
            rem = cur % 1_000_000_000;
        }
        chunks.push(rem as u32);
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
    }
    let Some((last, rest)) = chunks.split_last() else {
        return "0".into();
    };
    let mut s = last.to_string();
    for c in rest.iter().rev() {
        s += &format!("{c:09}");
    }
    s
}

/// DuckDB's text for a BLOB: printable ASCII as is, every other byte (and
/// `\`, `'`, `"`) as `\xHH`.
fn blob_text(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len());
    for &x in b {
        if (32..=126).contains(&x) && !matches!(x, b'\\' | b'\'' | b'"') {
            s.push(x as char);
        } else {
            s += &format!("\\x{x:02X}");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_US: i64 = 86_400_000_000;

    #[test]
    fn decimals_keep_their_scale() {
        assert_eq!(decimal_text(150, 2), "1.50");
        assert_eq!(decimal_text(-5, 2), "-0.05");
        assert_eq!(decimal_text(0, 3), "0.000");
        assert_eq!(decimal_text(42, 0), "42");
        assert_eq!(decimal_text(i128::MIN, 0), i128::MIN.to_string());
    }

    #[test]
    fn dates() {
        assert_eq!(date_text(0), "1970-01-01");
        assert_eq!(date_text(19_723), "2024-01-01");
        assert_eq!(date_text(-719_528), "0001-01-01 (BC)");
        assert_eq!(date_text(-719_162), "0001-01-01");
        assert_eq!(date_text(i32::MAX), "infinity");
        assert_eq!(date_text(-i32::MAX), "-infinity");
    }

    #[test]
    fn times() {
        assert_eq!(time_text(0, 6), "00:00:00");
        assert_eq!(time_text(DAY_US, 6), "24:00:00");
        assert_eq!(time_text(43_200_500_000, 6), "12:00:00.5");
        assert_eq!(time_text(43_200_123_456_789, 9), "12:00:00.123456789");
        // 12:00:00+02
        assert_eq!(time_tz_text(0x0A0E_EBB0_0000_C4DF), "12:00:00+02");
    }

    #[test]
    fn timestamps() {
        assert_eq!(timestamp_text(0, 1, false), "1970-01-01 00:00:00");
        assert_eq!(
            timestamp_text(1_500, 1_000, true),
            "1970-01-01 00:00:01.5+00"
        );
        assert_eq!(
            timestamp_text(-1, 1_000_000_000, false),
            "1969-12-31 23:59:59.999999999"
        );
        assert_eq!(timestamp_text(i64::MAX, 1, false), "infinity");
        assert_eq!(timestamp_text(-i64::MAX, 1_000_000, true), "-infinity");
    }

    #[test]
    fn intervals() {
        assert_eq!(interval_text(0, 0, 0), "00:00:00");
        assert_eq!(
            interval_text(14, 1, 1_000_000_000),
            "1 year 2 months 1 day 00:00:01"
        );
        assert_eq!(
            interval_text(-14, -3, -1_500_000_000),
            "-1 year -2 months -3 days -00:00:01.5"
        );
        assert_eq!(interval_text(0, 0, 36 * 3_600_000_000_000), "36:00:00");
        assert_eq!(interval_text(0, 0, 1), "00:00:00.000000001");
    }

    #[test]
    fn bits_and_bignums() {
        assert_eq!(bit_text(&[2, 0b0010_1010]).unwrap(), "101010");
        assert_eq!(bignum(&[0x80, 0x00, 0x01, 0x07]).unwrap(), Value::Int(7));
        assert_eq!(bignum(&[0x7F, 0xFF, 0xFE, 0xF8]).unwrap(), Value::Int(-7));
        assert_eq!(
            big_decimal_digits(&[0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            "340282366920938463463374607431768211456"
        );
        assert_eq!(big_decimal_digits(&[]), "0");
    }

    #[test]
    fn blobs_as_text() {
        assert_eq!(blob_text(b"a\x00\\'\"~"), "a\\x00\\x5C\\x27\\x22~");
    }

    #[test]
    fn nested_json() {
        assert_eq!(
            json_of(&Value::Int(1 << 60)),
            Json::String((1_i64 << 60).to_string())
        );
        assert_eq!(json_of(&Value::Float(f64::NAN)), Json::String("nan".into()));
        assert_eq!(
            json_of(&Value::Decimal("1.50".into())),
            Json::String("1.50".into())
        );
        assert_eq!(
            map_json(vec![(Value::Float(1.5), Value::Int(1))]),
            serde_json::json!([{ "key": 1.5, "value": 1 }])
        );
        assert_eq!(
            map_json(vec![(Value::Int(1), Value::Text("a".into()))]),
            serde_json::json!({ "1": "a" })
        );
    }
}
