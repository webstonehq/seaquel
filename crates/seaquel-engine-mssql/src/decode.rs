//! Decodes SQL Server cells into `Value`.
//!
//! Each cell is read from tiberius's `ColumnData` variant, never by trying
//! decoders in turn; the column's type only matters for money, which
//! tiberius hands over as an `f64`.
//!
//! | Type | `Value` |
//! |---|---|
//! | bit | `Bool` |
//! | tinyint, smallint, int, bigint | `Int` (the wire tags bigints beyond 2^53) |
//! | decimal, numeric | `Decimal`, exact, scale kept: `12.50`, `-0.5`, `0.0000000001` |
//! | money, smallmoney | `Decimal` with 4 places: `12.5000` (see [`money`]) |
//! | float | `Float` |
//! | real | `Float` of the f32's shortest text (`0.1`, not `0.10000000149…`) |
//! | char, varchar, nchar, nvarchar, text, ntext, xml | `Text` |
//! | binary, varbinary, image, timestamp/rowversion | `Bytes` |
//! | uniqueidentifier | `Text`, lower case |
//! | date | `Text`: `2024-01-02` |
//! | time(n) | `Text`: `03:04:05.1234567`, trailing zeros dropped |
//! | datetime2(n), datetime | `Text`: `2024-01-02 03:04:05.5` (datetime as SQL Server prints it, to the millisecond) |
//! | smalldatetime | `Text`: `2024-01-02 03:04:00` |
//! | datetimeoffset(n) | `Text`: `2024-01-02 03:04:05.5 +01:00`, local time and offset |
//!
//! Dates and times are SQL Server's own text form (`CONVERT(…, 121)` and
//! `CAST(… AS nvarchar)`), with trailing zeros of the fraction dropped as
//! the MySQL and Postgres decoders do. Every form converts back to the same
//! value, so a date/time key bound back as text finds its row.
//!
//! sql_variant and CLR types (geography, geometry, hierarchyid) never get
//! here: tiberius 0.12 panics on their column metadata, which the session
//! reports as `UNSUPPORTED_TYPE` (see `session::Failure`).

use tiberius::{ColumnData, ColumnType, Row};

use seaquel_engine::Value;

/// The cells of `row`, by position: two columns with the same name
/// (`SELECT a.id, b.id`) or none (`SELECT 1, 2`) each keep their own value.
pub(crate) fn row_to_values(row: &Row) -> Vec<Value> {
    row.cells()
        .map(|(column, data)| cell_value(column.column_type(), data))
        .collect()
}

/// One cell of a column of type `ty`.
pub(crate) fn cell_value(ty: ColumnType, data: &ColumnData<'_>) -> Value {
    match data {
        ColumnData::U8(v) => opt(*v, |v| Value::Int(v.into())),
        ColumnData::I16(v) => opt(*v, |v| Value::Int(v.into())),
        ColumnData::I32(v) => opt(*v, |v| Value::Int(v.into())),
        ColumnData::I64(v) => opt(*v, Value::Int),
        ColumnData::F32(v) => opt(*v, |v| Value::Float(widen(v))),
        ColumnData::F64(v) => opt(*v, |v| match ty {
            ColumnType::Money | ColumnType::Money4 => Value::Decimal(money(v)),
            _ => Value::Float(v),
        }),
        ColumnData::Bit(v) => opt(*v, Value::Bool),
        ColumnData::String(v) => opt(v.as_ref(), |s| Value::Text(s.to_string())),
        ColumnData::Guid(v) => opt(*v, |u| Value::Text(u.to_string())),
        ColumnData::Binary(v) => opt(v.as_ref(), |b| Value::Bytes(b.to_vec())),
        ColumnData::Numeric(v) => opt(*v, |n| Value::Decimal(decimal_text(n.value(), n.scale()))),
        ColumnData::Xml(v) => opt(v.as_ref(), |x| Value::Text(x.as_ref().as_ref().to_string())),
        ColumnData::DateTime(v) => opt(*v, |d| {
            Value::Text(datetime(d.days(), d.seconds_fragments()))
        }),
        ColumnData::SmallDateTime(v) => opt(*v, |d| {
            let minutes = u64::from(d.seconds_fragments());
            Value::Text(format!(
                "{} {:02}:{:02}:00",
                date_text(i64::from(d.days()) + DAYS_0001_TO_1900),
                minutes / 60,
                minutes % 60
            ))
        }),
        ColumnData::Time(v) => opt(*v, |t| Value::Text(time_text(t.increments(), t.scale()))),
        ColumnData::Date(v) => opt(*v, |d| Value::Text(date_text(i64::from(d.days())))),
        ColumnData::DateTime2(v) => opt(*v, |d| {
            let t = d.time();
            Value::Text(format!(
                "{} {}",
                date_text(i64::from(d.date().days())),
                time_text(t.increments(), t.scale())
            ))
        }),
        ColumnData::DateTimeOffset(v) => opt(*v, |d| {
            let dt = d.datetime2();
            let t = dt.time();
            Value::Text(datetimeoffset(
                i64::from(dt.date().days()),
                t.increments(),
                t.scale(),
                d.offset(),
            ))
        }),
    }
}

fn opt<T>(v: Option<T>, f: impl FnOnce(T) -> Value) -> Value {
    v.map_or(Value::Null, f)
}

/// The f64 closest to the f32's shortest decimal text, so REAL 0.1 shows as
/// 0.1 (widening the f32 gives 0.10000000149011612).
fn widen(f: f32) -> f64 {
    f.to_string().parse().unwrap_or(f64::from(f))
}

/// `value / 10^scale` as exact decimal text: `-0.5`, `12.50`, `100`.
/// (tiberius's own `Display` prints −0.5 as `0.-5`.)
pub(crate) fn decimal_text(value: i128, scale: u8) -> String {
    let digits = value.unsigned_abs().to_string();
    let sign = if value < 0 { "-" } else { "" };
    let scale = usize::from(scale);
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let digits = format!("{digits:0>width$}", width = scale + 1);
    let (int, frac) = digits.split_at(digits.len() - scale);
    format!("{sign}{int}.{frac}")
}

/// money/smallmoney as `Decimal` text with 4 places.
///
/// SQL Server stores money as an integer count of 1/10000 units, but
/// tiberius 0.12 turns it into an `f64` while reading the row (the high and
/// low 32 bits summed as floats, then divided by 10⁴) and keeps nothing
/// else. The count is recovered as the integer whose quotient is that
/// `f64`:
///
/// - exact while the f64's spacing is below 0.0001, that is for |value| <
///   2^39 ≈ 5.5·10¹¹ (about 2^52.3 units); smallmoney always;
/// - above that, the nearest count to what tiberius handed over. Above
///   2^53 units tiberius's own sum already rounds, so near the ends of the
///   range (±9.2·10¹⁴) the value is off by up to about 1536 units (0.1536).
///   The maximum, 922337203685477.5807, reads as `922337203685477.5808`,
///   which is above the money range: bound back, it compares as numeric
///   (no row matches) and doesn't assign to a money column (overflow).
pub(crate) fn money(f: f64) -> String {
    let guess = (f * 10_000.0).round();
    // `guess` is at most one unit off; take the neighbour that divides back to `f`.
    let units = [guess, guess - 1.0, guess + 1.0]
        .into_iter()
        .find(|n| n / 10_000.0 == f)
        .unwrap_or(guess);
    decimal_text(units as i128, 4)
}

/// Days from 0001-01-01 to 1900-01-01 (the epoch of datetime and smalldatetime).
const DAYS_0001_TO_1900: i64 = 693_595;

/// `YYYY-MM-DD` for `days` since 0001-01-01 (proleptic Gregorian).
fn date_text(days: i64) -> String {
    // Howard Hinnant's civil_from_days, shifted from 1970-01-01.
    let z = days - 719_162 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// `hh:mm:ss[.fffffff]` for `increments` of 10^-`scale` seconds since
/// midnight, trailing zeros of the fraction dropped.
fn time_text(increments: u64, scale: u8) -> String {
    let unit = 10u64.pow(u32::from(scale));
    let secs = increments / unit;
    let mut out = format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
    push_fraction(&mut out, increments % unit, usize::from(scale));
    out
}

/// `.5` for 5 at scale 1, `.0000001` for 1 at scale 7, nothing for 0.
fn push_fraction(out: &mut String, fraction: u64, scale: usize) {
    if fraction != 0 {
        let digits = format!("{fraction:0scale$}");
        out.push('.');
        out.push_str(digits.trim_end_matches('0'));
    }
}

/// datetime: `days` since 1900-01-01 (negative back to 1753) and 1/300 s
/// ticks, printed to the millisecond as SQL Server does (`.003`, `.007`):
/// the text converts back to the same ticks.
fn datetime(days: i32, ticks: u32) -> String {
    let ms = (u64::from(ticks) * 10 + 1) / 3; // round(ticks * 10 / 3)
    let secs = ms / 1000;
    let mut out = format!(
        "{} {:02}:{:02}:{:02}",
        date_text(i64::from(days) + DAYS_0001_TO_1900),
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    );
    push_fraction(&mut out, ms % 1000, 3);
    out
}

/// datetimeoffset as SQL Server prints it, `2024-01-02 03:04:05.5 +01:00`:
/// the local time and its offset. The wire holds UTC and the offset in
/// minutes.
fn datetimeoffset(utc_days: i64, increments: u64, scale: u8, offset: i16) -> String {
    let unit = 10u64.pow(u32::from(scale));
    let per_day = 86_400 * unit;
    let shift = i128::from(offset) * 60 * i128::from(unit);
    let local = i128::from(utc_days) * i128::from(per_day) + i128::from(increments) + shift;
    let days = local.div_euclid(i128::from(per_day)) as i64;
    let within = local.rem_euclid(i128::from(per_day)) as u64;
    let sign = if offset < 0 { '-' } else { '+' };
    let abs = offset.unsigned_abs();
    format!(
        "{} {} {sign}{:02}:{:02}",
        date_text(days),
        time_text(within, scale),
        abs / 60,
        abs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_are_exact() {
        assert_eq!(decimal_text(-5, 1), "-0.5");
        assert_eq!(decimal_text(1250, 2), "12.50");
        assert_eq!(decimal_text(0, 2), "0.00");
        assert_eq!(decimal_text(1, 10), "0.0000000001");
        assert_eq!(decimal_text(-100, 0), "-100");
        assert_eq!(
            decimal_text(99_999_999_999_999_999_999_999_999_999_999_999_999, 10),
            "9999999999999999999999999999.9999999999"
        );
        assert_eq!(
            decimal_text(-99_999_999_999_999_999_999_999_999_999_999_999_999, 0),
            "-99999999999999999999999999999999999999"
        );
    }

    /// tiberius's own arithmetic, from the stored count of 1/10000 units.
    fn tiberius_money(units: i64) -> f64 {
        let high = units >> 32;
        let low = (units & 0xffff_ffff) as u32;
        ((high << 32) as f64 + f64::from(low)) / 1e4
    }

    #[test]
    fn money_recovers_the_stored_units() {
        for units in [
            0i64,
            1,
            -1,
            125_000,
            -5_000,
            2_147_483_647,  // smallmoney max
            -2_147_483_648, // smallmoney min
            123_456_789_012_345,
            (1 << 39) * 10_000 - 1, // just under 2^39
            -(1 << 39) * 10_000 + 1,
        ] {
            assert_eq!(
                money(tiberius_money(units)),
                decimal_text(units.into(), 4),
                "{units}"
            );
        }
        assert_eq!(money(12.5), "12.5000");
        assert_eq!(money(-0.0001), "-0.0001");
        // Beyond 2^39 only the nearest value.
        let max = money(tiberius_money(i64::MAX));
        assert!(max.starts_with("922337203685477."), "{max}");
    }

    #[test]
    fn dates() {
        assert_eq!(date_text(0), "0001-01-01");
        assert_eq!(date_text(DAYS_0001_TO_1900), "1900-01-01");
        assert_eq!(date_text(738_886), "2024-01-02");
        assert_eq!(date_text(3_652_058), "9999-12-31");
        assert_eq!(date_text(DAYS_0001_TO_1900 - 53_690), "1753-01-01");
        assert_eq!(date_text(730_178), "2000-02-29");
    }

    #[test]
    fn times() {
        assert_eq!(time_text(0, 7), "00:00:00");
        assert_eq!(time_text(110_451_234_567, 7), "03:04:05.1234567");
        assert_eq!(time_text(110_455_000_000, 7), "03:04:05.5");
        assert_eq!(time_text(86_399, 0), "23:59:59");
        assert_eq!(time_text(863_999_999_999, 7), "23:59:59.9999999");
        assert_eq!(time_text(11_184_567, 3), "03:06:24.567");
    }

    #[test]
    fn legacy_datetime() {
        // 03:04:05.123 is 11 045.123 s, stored as the nearest 1/300 s tick: 3 313 537.
        assert_eq!(datetime(45_291, 3_313_537), "2024-01-02 03:04:05.123");
        assert_eq!(datetime(45_291, 3_313_538), "2024-01-02 03:04:05.127");
        assert_eq!(datetime(45_291, 3_313_539), "2024-01-02 03:04:05.13");
        assert_eq!(datetime(45_291, 3_313_500), "2024-01-02 03:04:05");
        assert_eq!(datetime(45_291, 25_919_999), "2024-01-02 23:59:59.997");
        assert_eq!(datetime(-53_690, 0), "1753-01-01 00:00:00");
    }

    #[test]
    fn offsets_shift_to_local_time() {
        let day = 738_886; // 2024-01-02
        let at = |h: u64| h * 3600 * 10_000_000;
        assert_eq!(
            datetimeoffset(day, at(2) + 5_000_000, 7, 60),
            "2024-01-02 03:00:00.5 +01:00"
        );
        assert_eq!(
            datetimeoffset(day, at(1), 7, -150),
            "2024-01-01 22:30:00 -02:30"
        );
        assert_eq!(
            datetimeoffset(day, 23 * 3600, 0, 14 * 60),
            "2024-01-03 13:00:00 +14:00"
        );
        assert_eq!(datetimeoffset(day, 0, 3, 0), "2024-01-02 00:00:00 +00:00");
    }
}
