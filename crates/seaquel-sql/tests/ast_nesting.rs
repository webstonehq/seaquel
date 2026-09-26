//! Deeply nested input to the AST helpers (Task 6): parse errors, never a
//! panic or a stack overflow. In the module a panic is a trap. The module
//! links a 2 MB stack (`crates/seaquel-wasm/build.rs`); every call here runs
//! on a thread with half that, 1 MB, to keep a 2× margin.
//!
//! sqlparser's recursion limit stops nested parentheses and subqueries.
//! Operator chains (`a AND b AND …`, `a + b + …`, `x::int::int …`, `UNION`)
//! aren't parsed recursively, and dropping one recurses once per link, so
//! `ast::util::parse` refuses input whose chains could nest past 2,000. These
//! tests check both sides of that cap.
//!
//! Measured on a 1 MB native release stack with the cap switched off, the
//! first overflow is a WHERE or HAVING `OR`/`AND` chain of about 5,080 terms
//! (about 10,160 by the cap's count: the Visual tab's filter tree nests once
//! per term); `+`, `::`, `LIKE` and `AT TIME ZONE` chains go past 16,500,
//! `UNION` past 22,000 and `IS NULL` past 33,000. In a wasm32 build every
//! chain at the cap also parses on a 512 KB stack.
//!
//! The 1 MB stack is what release code gets (`cargo test --release`). Debug
//! frames are many times larger (sqlparser's own 50 levels of recursion
//! overflow 1 MB in debug), so a debug run uses 16 MB.

use std::collections::HashMap;

use seaquel_sql::ast::{column_refs, parse_builder_query, parse_error, parse_visual};
use seaquel_sql::SqlEngine;

const STACK: usize = if cfg!(debug_assertions) {
    16 << 20
} else {
    1 << 20
};

/// Run every AST entry point on `sql` for every engine, on a 1 MB stack.
/// Returns whether each engine's parse succeeded.
fn run_all(sql: String) -> Vec<(SqlEngine, bool)> {
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            let schema: HashMap<String, Vec<String>> = HashMap::new();
            SqlEngine::ALL
                .into_iter()
                .map(|e| {
                    let visual = parse_visual(&sql, e);
                    let err = parse_error(&sql, e);
                    assert_eq!(visual.is_err(), err.is_some(), "{e}");
                    let _ = column_refs(&sql, e);
                    let _ = parse_builder_query(&sql, e, &schema, None);
                    // The JSON the wasm export would send.
                    if let Ok(Some(v)) = &visual {
                        serde_json::to_string(v).expect("serializes");
                    }
                    (e, err.is_none())
                })
                .collect()
        })
        .expect("spawn")
        .join()
        .expect("no panic or overflow")
}

fn all_fail(sql: String) {
    let r = run_all(sql);
    assert!(r.iter().all(|(_, ok)| !ok), "{r:?}");
}

fn ok_in(sql: String, engine: SqlEngine) {
    let r = run_all(sql);
    assert!(
        r.iter().any(|(e, ok)| *e == engine && *ok),
        "{engine} should parse: {r:?}"
    );
}

#[test]
fn ten_thousand_nested_parentheses_are_a_parse_error() {
    all_fail(format!(
        "SELECT {}1{}",
        "(".repeat(10_000),
        ")".repeat(10_000)
    ));
    // Unbalanced, as while typing.
    all_fail(format!("SELECT {}1", "(".repeat(10_000)));
    all_fail("(".repeat(5_000));
    all_fail(format!(
        "SELECT * FROM t WHERE {}a = 1{}",
        "(".repeat(5_000),
        ")".repeat(5_000)
    ));
}

#[test]
fn deeply_nested_subqueries_are_a_parse_error() {
    all_fail(format!(
        "SELECT * FROM {}SELECT 1{}",
        "(SELECT * FROM ".repeat(2_000),
        ") x".repeat(2_000)
    ));
    all_fail(format!(
        "SELECT * FROM t WHERE a IN {}",
        "(SELECT a FROM t WHERE a IN ".repeat(2_000)
    ));
}

#[test]
fn long_operator_chains_past_the_cap_are_a_parse_error() {
    for sql in [
        format!(
            "SELECT * FROM t WHERE {}",
            vec!["a = 1"; 30_000].join(" AND ")
        ),
        format!(
            "SELECT * FROM t WHERE {}",
            vec!["a = 1"; 1_001].join(" OR ")
        ),
        format!("SELECT {} FROM t", vec!["a"; 30_000].join(" + ")),
        format!("SELECT a{} FROM t", "::int".repeat(30_000)),
        vec!["SELECT 1"; 30_000].join(" UNION "),
        format!("SELECT * FROM t WHERE a{}", " IS NULL".repeat(30_000)),
        format!("SELECT a{} FROM t", "[1]".repeat(30_000)),
        // A group at the start of a chain is its deepest leaf, so chains in
        // nested groups add up: 12 levels of 200 operators.
        format!("SELECT * FROM t WHERE {}", {
            let chain = vec!["a = 1"; 100].join(" AND ");
            (0..12).fold("a = 1".to_string(), |s, _| format!("({s}) AND {chain}"))
        }),
    ] {
        for (e, ok) in run_all(sql) {
            assert!(!ok, "{e}");
        }
    }
    let msg = parse_error(
        &format!("SELECT {}", vec!["1"; 2_002].join(" + ")),
        SqlEngine::Postgres,
    );
    assert_eq!(msg.as_deref(), Some("query is nested too deeply to parse"));
}

#[test]
fn chains_up_to_the_cap_still_parse() {
    // The count is conservative: `a = 1` is two operators with its AND, and
    // `SELECT *` counts one. 1,000 terms is exactly the cap.
    let and = |n: usize| format!("SELECT * FROM t WHERE {}", vec!["a = 1"; n].join(" AND "));
    ok_in(and(1_000), SqlEngine::Postgres);
    all_fail(and(1_001));
    // A tool-generated `id = 1 OR id = 2 …` of 1,000 terms keeps inline
    // editing (column_refs) and the Visual tab.
    let or = |n: usize| {
        format!(
            "SELECT id, name FROM t WHERE {}",
            (0..n)
                .map(|i| format!("id = {i}"))
                .collect::<Vec<_>>()
                .join(" OR ")
        )
    };
    ok_in(or(1_000), SqlEngine::Postgres);
    assert!(column_refs(&or(1_000), SqlEngine::Postgres).is_some());
    all_fail(or(1_001));
    // HAVING at exactly the cap: 1,000 `>` and 999 AND, plus `*`.
    let having = |n: usize| {
        format!(
            "SELECT * FROM t GROUP BY a HAVING {}",
            vec!["SUM(a) > 1"; n].join(" AND ")
        )
    };
    ok_in(having(1_000), SqlEngine::Postgres);
    all_fail(having(1_001));
    ok_in(
        format!("SELECT {} FROM t", vec!["a"; 2_001].join(" + ")),
        SqlEngine::Postgres,
    );
    ok_in(
        format!("SELECT a{} FROM t", "::int".repeat(2_000)),
        SqlEngine::Postgres,
    );
    all_fail(format!("SELECT a{} FROM t", "::int".repeat(2_001)));
    ok_in(
        format!("SELECT a{} FROM t", " AT TIME ZONE 'UTC'".repeat(2_000)),
        SqlEngine::Postgres,
    );
    ok_in(
        format!("SELECT a FROM t WHERE a{}", " IS NULL".repeat(2_000)),
        SqlEngine::Postgres,
    );
    ok_in(vec!["SELECT 1"; 2_001].join(" UNION "), SqlEngine::Postgres);
    // A chain on the right of each link: `a AND (b AND (c …))` is bounded by
    // the recursion limit, but each group can hold its own chain.
    ok_in(
        format!(
            "SELECT * FROM t WHERE {}a = 1{}",
            "a = 1 AND (".repeat(20),
            ")".repeat(20)
        ),
        SqlEngine::Postgres,
    );
}

#[test]
fn wide_queries_are_not_deep() {
    // Commas separate subtrees: a wide select list, a long IN list or many
    // rows don't count as depth.
    ok_in(
        format!("SELECT {} FROM t", vec!["a + 1"; 20_000].join(", ")),
        SqlEngine::Postgres,
    );
    ok_in(
        format!(
            "SELECT * FROM t WHERE id IN ({})",
            (0..50_000)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        SqlEngine::Postgres,
    );
    ok_in(
        format!(
            "INSERT INTO t VALUES {}",
            vec!["(1, 'a' || 'b')"; 20_000].join(", ")
        ),
        SqlEngine::Postgres,
    );
    // Statements are separate too.
    ok_in(
        vec!["SELECT 1 + 1 + 1"; 5_000].join(";\n"),
        SqlEngine::Postgres,
    );
}
