//! A result's column kinds ([`Kind`]), read from DuckDB's logical types.
//! The helper sends them in each schema frame (`wire::schema_payload`), and
//! its client decodes by them, so a session setting that changes the Arrow
//! carriers (`arrow_lossless_conversion`) can't change the cells.

use std::panic::{catch_unwind, AssertUnwindSafe};

use duckdb::core::{LogicalTypeHandle, LogicalTypeId};
use duckdb::Statement;

use crate::decode::Kind;

#[cfg(test)]
use serde_json::Value as Json;

#[cfg(test)]
use crate::test_reference::{self, Case, Expect};

/// The kinds of an executed statement's columns. duckdb-rs panics on
/// logical types it doesn't know (reading or walking them); such a column
/// is [`Kind::Plain`], so its cells decode by their Arrow type alone.
pub(crate) fn of(stmt: &Statement<'_>) -> Vec<Kind> {
    (0..stmt.column_count())
        .map(|i| {
            catch_unwind(AssertUnwindSafe(|| kind_of(&stmt.column_logical_type(i))))
                .unwrap_or(Kind::Plain)
        })
        .collect()
}

/// The [`Kind`] of a result column's DuckDB type. duckdb-rs panics on type
/// ids it doesn't know; [`of`] catches that. (The browser driver and the
/// helper's client have no logical types and read the Arrow field instead:
/// [`Kind::of_field`], or the kinds the helper sent.)
fn kind_of(t: &LogicalTypeHandle) -> Kind {
    let children = || {
        (0..t.num_children())
            .map(|i| kind_of(&t.child(i)))
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
        LogicalTypeId::List | LogicalTypeId::Array => Kind::List(Box::new(kind_of(&t.child(0)))),
        LogicalTypeId::Struct => Kind::Struct(children()),
        LogicalTypeId::Union => Kind::Union(children()),
        LogicalTypeId::Map => Kind::Map(
            Box::new(kind_of(&t.child(0))),
            Box::new(kind_of(&t.child(1))),
        ),
        _ => Kind::Plain,
    }
}

/// The fixtures' one-case-per-line layout, shared with `tests/cells_fixture.rs`.
#[cfg(test)]
#[path = "../tests/common/fixture_format.rs"]
mod fixture_format;

/// The column kinds snapshot (`schema_frames_carry_the_recorded_kinds`).
#[cfg(test)]
const SNAPSHOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/kinds.json");

/// The cases the kinds snapshot covers: every reference case, and the
/// lossy-Arrow types side by side.
#[cfg(test)]
pub(crate) fn snapshot_cases() -> Vec<Case> {
    let mut cases = test_reference::all();
    let lossy = "SELECT '340282366920938463463374607431768211455'::UHUGEINT AS u, \
                 '101'::BIT AS b, ['1'::BIT] AS l, {'h': 1::HUGEINT, 'j': '{}'::JSON} AS s, \
                 MAP {'k': '1'::BIT} AS m";
    cases.push(Case {
        name: lossy.into(),
        setup: vec![],
        teardown: vec![],
        select: lossy.into(),
        expect: Expect::Whole {
            columns: vec![],
            rows: vec![],
        },
    });
    cases
}

/// Compares `got` (`{select, kinds}` per case) with the snapshot, or
/// rewrites it under `SEAQUEL_RECORD_KINDS=1` (only with a reason in
/// `tests/fixtures/README.md`: it was recorded from the native driver).
#[cfg(test)]
pub(crate) fn check_snapshot(got: Vec<Json>) {
    let text = fixture_format::one_case_per_line(&serde_json::json!({
        "about": "The column kinds the DuckDB helper sends for crates/seaquel-engine-duckdb's reference cases (src/kinds.rs). Recorded from the native driver's Decoder::of before it was deleted (Task 12 of the desktop DuckDB helper plan); frozen.",
        "cases": got,
    }));
    if std::env::var_os("SEAQUEL_RECORD_KINDS").is_some() {
        std::fs::write(SNAPSHOT, &text).unwrap();
        return;
    }
    // A Windows checkout can turn the fixture's line ends into `\r\n`.
    let stored = std::fs::read_to_string(SNAPSHOT)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    if stored != text {
        let stored: Json = serde_json::from_str(&stored).unwrap_or(Json::Null);
        let stored = stored["cases"].as_array().cloned().unwrap_or_default();
        let differ: Vec<String> = got
            .iter()
            .enumerate()
            .filter(|(i, g)| stored.get(*i) != Some(g))
            .map(|(i, g)| format!("{g}\n  recorded: {:?}", stored.get(i)))
            .collect();
        panic!(
            "column kinds differ from tests/fixtures/kinds.json ({} cases):\n{}",
            differ.len(),
            differ.join("\n")
        );
    }
}

#[cfg(test)]
mod tests {
    use duckdb::arrow::array::StructArray;
    use seaquel_engine::DbError;

    use super::*;
    use crate::blocking;
    use crate::session::{self, ChunkSink, Execution, Flow};

    /// Keeps the kinds [`of`] reads, as the helper's sink does.
    #[derive(Default)]
    struct KindsSink(Option<Vec<Kind>>);

    impl ChunkSink for KindsSink {
        fn columns(&mut self, stmt: &Statement<'_>) -> Result<(), DbError> {
            self.0 = Some(of(stmt));
            Ok(())
        }
        fn chunk(&mut self, _: StructArray) -> Result<Flow, DbError> {
            Ok(Flow::Continue)
        }
        fn finish(&mut self) -> Result<(), DbError> {
            Ok(())
        }
    }

    /// [`of`] reads the kinds the native driver's `Decoder::of` read for
    /// every reference case and the lossy-Arrow ones, as recorded before it
    /// moved here (`tests/fixtures/kinds.json`), with
    /// `arrow_lossless_conversion` on and off (the kinds come from the
    /// logical types, not the Arrow carriers).
    #[test]
    fn the_kinds_are_the_recorded_ones() {
        for lossless in [true, false] {
            let config = serde_json::from_value(serde_json::json!({
                "driver": "duckdb",
                "path": ":memory:"
            }))
            .unwrap();
            let (conn, _) = session::open_sessions(&config).unwrap();
            if !lossless {
                conn.execute_batch("RESET arrow_lossless_conversion")
                    .unwrap();
            }
            let mut got = Vec::new();
            for case in snapshot_cases() {
                for sql in &case.setup {
                    conn.execute_batch(sql).unwrap();
                }
                let (_call, worker) = blocking::call(conn.interrupt_handle());
                let mut sink = KindsSink::default();
                session::rows(
                    &conn,
                    &worker,
                    &case.select,
                    &[],
                    Execution::Materialized,
                    &mut sink,
                )
                .unwrap();
                got.push(serde_json::json!({
                    "select": case.select,
                    "kinds": serde_json::to_value(sink.0.expect("no columns")).unwrap(),
                }));
                for sql in &case.teardown {
                    conn.execute_batch(sql).unwrap();
                }
            }
            check_snapshot(got);
        }
    }
}
