//! What the in-crate decoding tests compare against.
//! It used to be the native driver's own
//! decoding, which went through the same `decode.rs` as the paths it
//! checked, so a mistake there showed on both sides and passed. Now it is:
//!
//! - the frozen typed-cell fixture, `tests/fixtures/cells.json`, recorded
//!   from the native driver while it existed (its README says when): each
//!   case's first cell, and one row;
//! - a few literal results (several chunks, an empty result, an ENUM, a
//!   DECIMAL(38, 0) that isn't a HUGEINT), written out by hand.

use seaquel_engine::Value;
use seaquel_engine_testkit::same_value;
use serde_json::Value as Json;

const CELLS: &str = include_str!("../tests/fixtures/cells.json");

/// What a case's result must be.
pub(crate) enum Expect {
    /// One row, whose first cell is this (the fixture's cases).
    FirstCell(Value),
    /// Exactly these columns and rows.
    Whole {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
    },
}

pub(crate) struct Case {
    pub name: String,
    pub setup: Vec<String>,
    pub teardown: Vec<String>,
    pub select: String,
    pub expect: Expect,
}

fn strings(j: &Json) -> Vec<String> {
    j.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

/// The fixture's typed-cell cases, as frozen.
pub(crate) fn cells() -> Vec<Case> {
    let fixture: Json = serde_json::from_str(CELLS).unwrap();
    fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Case {
            name: c["name"].as_str().unwrap().to_string(),
            setup: strings(&c["setup"]),
            teardown: strings(&c["teardown"]),
            select: c["select"].as_str().unwrap().to_string(),
            expect: Expect::FirstCell(Value::from_wire(c["expected"].clone()).unwrap()),
        })
        .collect()
}

fn whole(select: &str, columns: &[&str], rows: Vec<Vec<Value>>) -> Case {
    Case {
        name: select.to_string(),
        setup: vec![],
        teardown: vec![],
        select: select.to_string(),
        expect: Expect::Whole {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            rows,
        },
    }
}

/// Results the fixture doesn't cover, with their rows written out.
pub(crate) fn literal() -> Vec<Case> {
    vec![
        // Several of DuckDB's 2,048-row chunks.
        whole(
            "SELECT range AS i, 'x' || range AS s, range::DOUBLE / 7 AS f FROM range(5000)",
            &["i", "s", "f"],
            (0..5000i64)
                .map(|r| {
                    vec![
                        Value::Int(r),
                        Value::Text(format!("x{r}")),
                        Value::Float(r as f64 / 7.0),
                    ]
                })
                .collect(),
        ),
        // Empty, its columns still named.
        whole("SELECT 1 AS a, 'b' AS b WHERE false", &["a", "b"], vec![]),
        // A dictionary column across chunks.
        whole(
            "SELECT ['sad', 'ok'][1 + (range % 2)]::ENUM('sad', 'ok') AS e FROM range(3000)",
            &["e"],
            (0..3000)
                .map(|r| vec![Value::Text(if r % 2 == 0 { "sad" } else { "ok" }.into())])
                .collect(),
        ),
        // DECIMAL(38, 0) shares HUGEINT's Arrow carrier and stays a decimal.
        whole(
            "SELECT '99999999999999999999999999999999999999'::DECIMAL(38,0) AS d, NULL AS n",
            &["d", "n"],
            vec![vec![
                Value::Decimal("99999999999999999999999999999999999999".into()),
                Value::Null,
            ]],
        ),
        // BIT inside a LIST (Checkpoint H-1's lossy-Arrow cases).
        whole(
            "SELECT ['1'::BIT, '0101'::BIT, NULL] AS l",
            &["l"],
            vec![vec![Value::Array(vec![
                Value::Text("1".into()),
                Value::Text("0101".into()),
                Value::Null,
            ])]],
        ),
    ]
}

/// Every case: the fixture's, then the literal ones.
pub(crate) fn all() -> Vec<Case> {
    let mut cases = cells();
    cases.extend(literal());
    cases
}

pub(crate) fn same_rows(a: &[Vec<Value>], b: &[Vec<Value>]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_value(p, q)))
}

impl Case {
    /// Whether `columns` and `rows` are what this case expects; the
    /// difference otherwise.
    pub(crate) fn check(&self, columns: &[String], rows: &[Vec<Value>]) -> Result<(), String> {
        match &self.expect {
            Expect::FirstCell(want) => match rows {
                [row] if !columns.is_empty() && row.len() == columns.len() => {
                    if same_value(&row[0], want) {
                        Ok(())
                    } else {
                        Err(format!("{}: {:?}, not {want:?}", self.name, row[0]))
                    }
                }
                _ => Err(format!(
                    "{}: one row expected, got {columns:?} {:?}",
                    self.name,
                    rows.iter().take(2).collect::<Vec<_>>()
                )),
            },
            Expect::Whole {
                columns: want_columns,
                rows: want_rows,
            } => {
                if columns == want_columns.as_slice() && same_rows(rows, want_rows) {
                    Ok(())
                } else {
                    Err(format!(
                        "{}: {columns:?} {:?}, not {want_columns:?} {:?}",
                        self.name,
                        rows.iter().take(2).collect::<Vec<_>>(),
                        want_rows.iter().take(2).collect::<Vec<_>>()
                    ))
                }
            }
        }
    }
}

#[test]
fn the_fixture_reads_back() {
    let cells = cells();
    assert_eq!(cells.len(), 119);
    // The lossy-Arrow cases are in it.
    for name in ["UHUGEINT", "::BIT ", "BIGNUM"] {
        assert!(
            cells.iter().any(|c| c.select.contains(name)),
            "no {name} case"
        );
    }
}
