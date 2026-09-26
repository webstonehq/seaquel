//! `parse_create_table` against the frozen `create-table.json` (Task 5). The
//! fixture already holds the output of fixes 16 and 17 (from the recorder's
//! `create-table-fixed.ts`, kept as
//! `docs/plans/artifacts/2026-09-27-sql-recorder-create-table-fixed.ts.txt`);
//! a `bugfixes.json` case with `replaces: "create-table.json: <name>"` would
//! supersede a case, and there are none today. Ids are compared without their values: the TS made random
//! UUIDs, the port gives placeholders the TS wrapper replaces.

use std::collections::HashSet;

use seaquel_sql::create_table::parse_create_table;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/create-table.json");
const BUGFIXES: &str = include_str!("fixtures/bugfixes.json");

fn cases() -> Vec<(String, String, Value)> {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("create-table.json");
    let bugfixes: Value = serde_json::from_str(BUGFIXES).expect("bugfixes.json");
    let replaced: Vec<(String, Value)> = bugfixes["cases"]
        .as_array()
        .expect("bugfixes cases")
        .iter()
        .filter_map(|c| {
            let name = c["replaces"].as_str()?;
            let name = name
                .strip_prefix("create-table.json: ")
                .or_else(|| name.strip_prefix("create-table: "))?;
            Some((name.to_string(), c["output"].clone()))
        })
        .collect();
    let cases: Vec<(String, String, Value)> = fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|c| {
            let name = c["name"].as_str().expect("name").to_string();
            let sql = c["input"]["sql"].as_str().expect("sql").to_string();
            let output = replaced
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, o)| o.clone())
                .unwrap_or_else(|| c["output"].clone());
            (name, sql, output)
        })
        .collect();
    // A `replaces` naming no case would silently test nothing.
    for (name, _) in &replaced {
        assert!(
            cases.iter().any(|(n, _, _)| n == name),
            "bugfixes.json replaces a create-table case that doesn't exist: {name}"
        );
    }
    cases
}

/// The result as the fixture writes it: ids removed, after checking each
/// list's ids are present and distinct (the table editor keys its lists on
/// them).
fn to_fixture_shape(sql: &str) -> Value {
    let Some(def) = parse_create_table(sql) else {
        return Value::Null;
    };
    let mut value = serde_json::to_value(def).expect("serialize");
    for list in ["columns", "indexes", "foreignKeys"] {
        let items = value[list].as_array_mut().expect("list");
        let mut seen = HashSet::new();
        for item in items {
            let id = item
                .as_object_mut()
                .expect("object")
                .remove("id")
                .expect("id");
            let id = id.as_str().expect("id string").to_string();
            assert!(!id.is_empty(), "{list}: empty id for {sql:?}");
            assert!(seen.insert(id), "{list}: duplicate id for {sql:?}");
        }
    }
    value
}

#[test]
fn matches_the_frozen_fixture() {
    let cases = cases();
    assert_eq!(cases.len(), 124);
    let mut failures = Vec::new();
    for (name, sql, expected) in &cases {
        let actual = to_fixture_shape(sql);
        if &actual != expected {
            failures.push(format!(
                "{name}\n  sql: {sql:?}\n  expected: {expected}\n  actual:   {actual}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

/// The table editor parses on every edit of the SQL pane, so every prefix of
/// every input (on char boundaries) must parse or give `None`, never panic.
#[test]
fn every_prefix_parses_without_panicking() {
    let mut runs = 0usize;
    for (_, sql, _) in cases() {
        for (i, _) in sql.char_indices().chain([(sql.len(), ' ')]) {
            let _ = parse_create_table(&sql[..i]);
            // And the rest, so the parser also starts mid-statement.
            let _ = parse_create_table(&sql[i..]);
            runs += 2;
        }
    }
    assert!(runs > 10_000, "{runs}");
}

#[test]
fn odd_inputs_do_not_panic() {
    let inputs = [
        "",
        "(",
        ")",
        "\"",
        "[",
        "]",
        "`",
        "CREATE TABLE",
        "CREATE TABLE (",
        "CREATE TABLE t (",
        "CREATE TABLE t ()",
        "CREATE TABLE \"\" (a int)",
        "CREATE TABLE \" (a int)",
        "CREATE TABLE t (\"",
        "CREATE TABLE t (a int DEFAULT",
        "CREATE TABLE t (a int DEFAULT ",
        "CREATE TABLE t (a int DEFAULT '",
        "CREATE TABLE t (a int) CREATE INDEX",
        "CREATE TABLE t (a int) CREATE INDEX i ON t (",
        "CREATE TABLE t (a int) CREATE INDEX i ON t ()",
        "CREATE TABLE t (a int REFERENCES",
        "CREATE TABLE t (a int, PRIMARY KEY (",
        "CREATE TABLE t (a int, CONSTRAINT \"x\" FOREIGN KEY (a) REFERENCES",
        "CREATE TABLE t (a varchar(), b numeric(,))",
        "CREATE TABLE t (a int COLLATE)",
        "CREATE TABLE t (ŉot null int, 東京 text, \"😀\" int)",
        "CREATE TABLE t (a\u{2028}int\u{feff}NOT\u{a0}NULL)",
        "CREATE TABLE a.b.c.d (x int)",
        "CREATE TABLE [a]]].[b]]] ([c]]] int)",
        "/* CREATE TABLE t (a int) */",
        "-- CREATE TABLE t (a int)",
        "CREATE TABLE t (a int /* unterminated",
    ];
    for sql in inputs {
        let _ = parse_create_table(sql);
        for (i, _) in sql.char_indices() {
            let _ = parse_create_table(&sql[..i]);
        }
    }
}

/// Long inputs don't overflow the stack (the module's is small): the matcher
/// loops over a run of repeated characters instead of recursing per
/// character. They also stay inside the step budget, so they parse.
#[test]
fn long_inputs() {
    let mut sql = String::from("CREATE TABLE t (\n");
    for i in 0..2_000 {
        sql.push_str(&format!(
            "  c{i} varchar(10) NOT NULL DEFAULT 'x' COLLATE \"C\" REFERENCES u (id),\n"
        ));
    }
    sql.push_str("  PRIMARY KEY (c0)\n);\nCREATE INDEX i ON t (c1, c2);");
    let def = parse_create_table(&sql).expect("parses");
    assert_eq!(def.columns.len(), 2_000);
    assert_eq!(def.foreign_keys.len(), 2_000);
    assert_eq!(def.indexes.len(), 1);

    let long_name = "a".repeat(200_000);
    let def =
        parse_create_table(&format!("CREATE TABLE {long_name} ({long_name} int)")).expect("parses");
    assert_eq!(def.columns[0].name.len(), 200_000);

    let long_default = format!("CREATE TABLE t (a text DEFAULT {})", "x ".repeat(50_000));
    assert!(parse_create_table(&long_default).is_some());

    let unterminated = format!("CREATE TABLE \"{}", "\"\"".repeat(100_000));
    assert!(parse_create_table(&unterminated).is_none());
}

/// Inputs that made the matcher quadratic before its possessive `\s+`, run
/// cache and step budget, with what `create-table-fixed.ts` gives for each
/// (V8 took 0.2–1.3 s on the first three). The table editor parses on every
/// keystroke, so each must stay fast; the bound is loose for debug builds.
#[test]
#[allow(clippy::disallowed_types)] // Instant, in a native-only test
fn pathological_inputs_stay_fast() {
    use std::time::{Duration, Instant};

    fn timed(sql: &str) -> Option<seaquel_types::CreateTableDefinition> {
        let start = Instant::now();
        let def = parse_create_table(sql);
        let took = start.elapsed();
        assert!(
            took < Duration::from_millis(500),
            "{took:?} for {}…",
            sql.chars().take(40).collect::<String>()
        );
        def
    }

    // A whitespace run in DEFAULT not followed by a keyword.
    let spaces = " ".repeat(50_000);
    let def = timed(&format!("CREATE TABLE t (a int DEFAULT x{spaces}y)")).expect("parses");
    assert_eq!(def.columns[0].default_value, format!("x{spaces}y"));
    assert_eq!(def.columns[0].ty, "int");

    // The same, ending in a newline, which `.` doesn't cross: no default.
    let spaces = " ".repeat(20_000);
    let def = timed(&format!("CREATE TABLE t (a int DEFAULT x{spaces}\ny)")).expect("parses");
    assert_eq!(def.columns[0].default_value, "");
    assert_eq!(def.columns[0].ty, "int");

    // Unclosed index column lists: no index.
    let indexes = "CREATE INDEX i ON t (a ".repeat(5_000);
    let def = timed(&format!("CREATE TABLE t (a int) {indexes}")).expect("parses");
    assert_eq!(def.columns.len(), 1);
    assert!(def.indexes.is_empty());

    let def = timed(&format!(
        "CREATE TABLE t (a int DEFAULT {})",
        "x  ".repeat(5_000)
    ))
    .expect("parses");
    assert_eq!(def.columns[0].default_value.len(), 3 * 5_000 - 2);

    assert!(timed(&format!("CREATE TABLE {} x", "a".repeat(100_000))).is_none());
    assert!(timed(&format!("CREATE TABLE{}", " ".repeat(50_000))).is_none());

    let def = timed(&format!("CREATE TABLE t ({})", ",".repeat(1_000_000))).expect("parses");
    assert!(def.columns.is_empty());
}
