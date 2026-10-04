//! Native values (phase 2 Task 16): each DuckDB type decodes to its own
//! `Value` kind; temporal text is exactly what DuckDB prints; binding a
//! decoded value back compares equal; and rows keyed by any of these types
//! are found again by `WHERE k = ?`. No server needed: in-memory databases.

#[path = "common/engine.rs"]
mod engine_switch;

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use seaquel_engine::{ConnectConfig, Driver, Value};
use seaquel_engine_testkit::run_typed_cells;
use serde_json::json;

#[path = "common/cast.rs"]
mod cast;
#[path = "common/cells.rs"]
mod cells;

use cells::{cases, dec, text};

fn config() -> ConnectConfig {
    serde_json::from_value(json!({ "driver": "duckdb", "path": ":memory:" })).unwrap()
}

async fn open() -> Arc<dyn Driver> {
    engine_switch::engine().open(&config()).await.unwrap()
}

#[tokio::test]
async fn text_matches_what_duckdb_prints() {
    cast::compare_with_cast(&*open().await, false).await;
}

/// The same with `arrow_lossless_conversion` reset, as a session may do:
/// HUGEINT, UHUGEINT and UUID arrive in their lossy Arrow forms and still
/// decode exactly. TIMETZ loses its offset there, so it's left out (see
/// `lossy_timetz_has_no_offset`).
#[tokio::test]
async fn text_matches_what_duckdb_prints_without_lossless_arrow() {
    cast::compare_with_cast(&*open().await, true).await;
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
//
// The cases are in `common/cells.rs`; `read_only.rs` runs them through
// `query_read_only` too.

#[tokio::test]
async fn typed_cells() {
    run_typed_cells(&*engine_switch::engine(), &config(), &cases()).await;
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
