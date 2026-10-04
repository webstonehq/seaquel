//! DuckDB's own text against the decoded cells: `values.rs` runs it on
//! the driver its suite tests, `remote.rs` on the remote driver (the DuckDB
//! helper plan's Checkpoint H-1: a session that resets
//! `arrow_lossless_conversion` must not change what the remote driver
//! decodes).

use seaquel_engine::{Driver, Value};

/// Queries whose rows are `(v, CAST(v AS VARCHAR))`. `v` must decode to
/// that exact text (numbers: to digits of it), under any session time zone
/// for everything but TIMESTAMPTZ, which prints in UTC.
pub const CAST_CASES: &[&str] = &[
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

pub async fn compare_with_cast(driver: &dyn Driver, lossy: bool) {
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
