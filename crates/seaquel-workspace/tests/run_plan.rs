//! `seaquel_workspace::run::plan` against the run fixtures
//! (`tests/fixtures/run`, recorded from today's TypeScript runner; see their
//! README) and phase 5b's rules on top of them.
//!
//! This file checks the planning fields: which statements run, their
//! substituted SQL and binds, query type, kind, table and column refs, the
//! destructive list, what is deferred and what history would record. Core's
//! `tests/run.rs` replays the same cases through `Workspace::run`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use seaquel_sql::statements::{DestructiveReason, QueryType};
use seaquel_sql::SqlEngine;
use seaquel_types::Value;
use seaquel_workspace::run::{
    plan as plan_with_cap, PageSource, PlanOptions, Planned, RunLimits, RunPlan, RunTarget,
    StatementKind, Step, INVALID_ARGUMENT, INVALID_PARAMETERS,
};
use serde_json::{json, Value as Json};

/// Core's page cap: `max_query_rows() - 1` at the default 100,000.
const MAX: u32 = 99_999;

/// The web server's run limits (`seaquel_server::WEB_RUN_LIMITS`).
const WEB: RunLimits = RunLimits {
    max_text_bytes: Some(2 * 1024 * 1024),
    max_statements: Some(10_000),
    max_param_values: Some(1_000),
    max_param_bytes: Some(1024 * 1024),
};

/// `plan` under the web's run limits.
fn plan_web(text: &str, target: &RunTarget) -> Result<RunPlan, seaquel_workspace::run::PlanError> {
    plan_with_cap(
        text,
        target,
        None,
        SqlEngine::Postgres,
        PlanOptions {
            page_size: 100,
            defer_writes: false,
            max_page_size: MAX,
            limits: WEB,
        },
    )
}

/// `plan` under Core's default cap and no run limits, as on the desktop.
fn plan(
    text: &str,
    target: &RunTarget,
    params: Option<&[(String, Value)]>,
    engine: SqlEngine,
    page_size: u32,
    defer_writes: bool,
) -> Result<RunPlan, seaquel_workspace::run::PlanError> {
    plan_with_cap(
        text,
        target,
        params,
        engine,
        PlanOptions {
            page_size,
            defer_writes,
            max_page_size: MAX,
            limits: RunLimits::default(),
        },
    )
}

const FILES: [&str; 10] = [
    "plan-postgres",
    "plan-mysql",
    "plan-mariadb",
    "plan-sqlite",
    "plan-mssql",
    "plan-duckdb",
    "cursor",
    "execute",
    "history",
    "pending",
];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/run")
}

fn read(file: &str) -> Json {
    let path = fixtures().join(format!("{file}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Every case, as recorded and with its `changes.json` entry applied.
fn cases() -> Vec<(Json, Json, bool)> {
    let changes = read("changes");
    let mut out = Vec::new();
    for file in FILES {
        for case in read(file).as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let mut changed = case.clone();
            let listed = match changes.get(name) {
                Some(change) => {
                    for (k, v) in change["expected"].as_object().unwrap() {
                        changed[k] = v.clone();
                    }
                    true
                }
                None => false,
            };
            out.push((case.clone(), changed, listed));
        }
    }
    out
}

fn engine(case: &Json) -> SqlEngine {
    case["engine"].as_str().unwrap().parse().unwrap()
}

fn target(input: &Json) -> RunTarget {
    serde_json::from_value(input["target"].clone()).unwrap()
}

fn params(input: &Json) -> Option<Vec<(String, Value)>> {
    input["params"].as_array().map(|ps| {
        ps.iter()
            .map(|p| {
                let value = Value::from_wire(p.get("value").cloned().unwrap_or(Json::Null));
                (p["name"].as_str().unwrap().to_string(), value.unwrap())
            })
            .collect()
    })
}

fn plan_case(case: &Json) -> Result<RunPlan, seaquel_workspace::run::PlanError> {
    let input = &case["input"];
    let ps = params(input);
    plan(
        input["text"].as_str().unwrap(),
        &target(input),
        ps.as_deref(),
        engine(case),
        u32::try_from(input["pageSize"].as_u64().unwrap()).unwrap(),
        input["deferWrites"].as_bool().unwrap(),
    )
}

fn kind_name(kind: StatementKind) -> &'static str {
    match kind {
        StatementKind::Page => "page",
        StatementKind::Stream => "stream",
        StatementKind::Write => "write",
        StatementKind::Utility => "utility",
    }
}

fn to_json<T: serde::Serialize>(t: &T) -> Json {
    serde_json::to_value(t).unwrap()
}

/// The planning fields of `case` against `p`: `Err` with what differs.
fn compare(
    case: &Json,
    planned: &Result<RunPlan, seaquel_workspace::run::PlanError>,
) -> Result<(), String> {
    let p = match planned {
        Ok(p) => p,
        Err(e) => {
            // A run-level failure: nothing ran, and the TS toasted it.
            if e.code != INVALID_PARAMETERS {
                return Err(format!("unexpected plan error {e}"));
            }
            let toasted = case["toasts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["kind"] == "error" && t["message"] == e.message.as_str());
            if !toasted || !case["results"].as_array().unwrap().is_empty() {
                return Err(format!("plan failed with {e}, the case didn't"));
            }
            return Ok(());
        }
    };

    // The statements the run takes, as the text numbers them.
    let statements: Vec<Json> = p
        .statements
        .iter()
        .map(|s| json!({"index": s.text_index, "sql": s.sql}))
        .collect();
    if Json::Array(statements.clone()) != case["statements"] {
        return Err(format!(
            "statements: {} vs {}",
            Json::Array(statements),
            case["statements"]
        ));
    }

    let destructive = to_json(&p.destructive);
    if destructive != case["destructive"] {
        return Err(format!(
            "destructive: {destructive} vs {}",
            case["destructive"]
        ));
    }

    let deferred: Vec<Json> = p
        .statements
        .iter()
        .filter_map(|s| match &s.step {
            Step::Defer { source, query_type } => Some(json!({
                "index": s.index, "sql": s.sql, "source": to_json(source),
                "queryType": to_json(query_type),
            })),
            _ => None,
        })
        .collect();
    if Json::Array(deferred.clone()) != case["deferred"] {
        return Err(format!(
            "deferred: {} vs {}",
            Json::Array(deferred),
            case["deferred"]
        ));
    }

    let results = case["results"].as_array().unwrap();
    let run: Vec<&Planned> = p
        .statements
        .iter()
        .filter(|s| !matches!(s.step, Step::Defer { .. }))
        .collect();
    if run.len() != results.len() {
        return Err(format!(
            "{} statements run, {} results",
            run.len(),
            results.len()
        ));
    }
    for (s, r) in run.iter().zip(results) {
        let at = |field: &str| format!("result {}: {field}", s.index);
        if r["index"] != s.index || r["sql"] != s.sql.as_str() {
            return Err(at("index/sql"));
        }
        match &s.step {
            Step::Run {
                source,
                query_type,
                kind,
                table,
                column_refs,
            } => {
                if r["kind"] != kind_name(*kind) {
                    return Err(format!(
                        "{}: {} vs {}",
                        at("kind"),
                        kind_name(*kind),
                        r["kind"]
                    ));
                }
                // Error results that didn't stream have no source or type
                // (Replay rules).
                if !r["source"].is_null() && r["source"] != to_json(source) {
                    return Err(format!(
                        "{}: {} vs {}",
                        at("source"),
                        to_json(source),
                        r["source"]
                    ));
                }
                if !r["queryType"].is_null() && r["queryType"] != to_json(query_type) {
                    return Err(at("queryType"));
                }
                if let Some(t) = r.get("table") {
                    if *t != to_json(table) {
                        return Err(format!("{}: {} vs {t}", at("table"), to_json(table)));
                    }
                }
                if let Some(c) = r.get("columnRefs") {
                    if *c != to_json(column_refs) {
                        return Err(format!(
                            "{}: {} vs {c}",
                            at("columnRefs"),
                            to_json(column_refs)
                        ));
                    }
                }
            }
            Step::Fail { code, message } => {
                // A planned failure: the bare message, no kind or source.
                if !r["kind"].is_null()
                    || r["error"] != message.as_str()
                    || code != INVALID_PARAMETERS
                {
                    return Err(format!(
                        "{}: {message} vs {}",
                        at("planned failure"),
                        r["error"]
                    ));
                }
            }
            Step::Defer { .. } => unreachable!(),
        }
    }

    if !case["history"].is_null() && case["history"]["query"] != p.history_query.as_str() {
        return Err(format!(
            "history query: {:?} vs {}",
            p.history_query, case["history"]["query"]
        ));
    }
    Ok(())
}

#[test]
fn replays_the_planning_fields_of_every_fixture() {
    let mut failures = Vec::new();
    let mut differ_from_recording = HashSet::new();
    let all = cases();
    assert!(all.len() >= 95, "{} cases", all.len());
    for (recorded, changed, listed) in &all {
        let name = recorded["name"].as_str().unwrap();
        let planned = plan_case(recorded);
        if let Err(e) = compare(changed, &planned) {
            failures.push(format!("{name}: {e}"));
        }
        if compare(recorded, &planned).is_err() {
            differ_from_recording.insert(name.to_string());
            if !listed {
                failures.push(format!(
                    "{name}: differs from the recording without a changes.json entry"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // The one listed change the planning fields show: a comment-only
    // buffer at the cursor runs nothing. The others are
    // execution and history changes, replayed by Core's tests/run.rs.
    assert_eq!(
        differ_from_recording,
        HashSet::from(["cursor/comment-only".to_string()])
    );
}

fn run(text: &str, target: RunTarget, engine: SqlEngine) -> RunPlan {
    plan(text, &target, None, engine, 100, false).unwrap()
}

fn only(p: &RunPlan) -> &Planned {
    assert_eq!(p.statements.len(), 1, "{p:?}");
    &p.statements[0]
}

fn source(p: &Planned) -> &PageSource {
    match &p.step {
        Step::Run { source, .. } | Step::Defer { source, .. } => source,
        Step::Fail { .. } => panic!("failed: {p:?}"),
    }
}

fn kind(p: &Planned) -> StatementKind {
    match &p.step {
        Step::Run { kind, .. } => *kind,
        other => panic!("not run: {other:?}"),
    }
}

fn current(cursor: u64) -> RunTarget {
    RunTarget::Current { cursor }
}

#[test]
fn current_takes_a_utf16_cursor() {
    let pg = SqlEngine::Postgres;
    // `東京` is two UTF-16 units and six bytes: offset 24 is in the second
    // statement in UTF-16 but still in the first as a byte offset.
    let text = "SELECT '東京東京東京東京' AS a;\nSELECT 2 AS b;";
    assert_eq!(only(&run(text, current(24), pg)).sql, "SELECT 2 AS b");
    assert_eq!(
        only(&run(text, current(5), pg)).sql,
        "SELECT '東京東京東京東京' AS a"
    );
    // `😀` is two units: the second statement starts at UTF-16 20.
    let text = "SELECT '😀' AS a;\nSELECT 2 AS b;";
    assert_eq!(only(&run(text, current(18), pg)).text_index, 1);
    // A cursor between the halves of a surrogate pair rounds down.
    let text = "SELECT 1 AS a;\nSELECT '😀' AS b;";
    let at = text
        .encode_utf16()
        .position(|u| (0xD800..0xDC00).contains(&u))
        .unwrap();
    let p = run(text, current(at as u64 + 1), pg);
    assert_eq!(only(&p).sql, "SELECT '😀' AS b");
    assert_eq!(only(&p).index, 0);
    // Past the end, and far past it: the last statement.
    assert_eq!(only(&run(text, current(10_000), pg)).text_index, 1);
    assert_eq!(only(&run(text, current(u64::MAX), pg)).text_index, 1);
    // History records the statement as typed.
    assert_eq!(run(text, current(0), pg).history_query, "SELECT 1 AS a");
}

#[test]
fn a_replaced_lone_surrogate_keeps_offsets() {
    // The client sends `text.toWellFormed()`: a lone surrogate is U+FFFD,
    // one UTF-16 unit, as the surrogate was, so the editor's offsets hold.
    let original: Vec<u16> = "SELECT 'x' AS a;\nSELECT 2 AS b;".encode_utf16().collect();
    let mut units = original.clone();
    units[8] = 0xD83D; // the `x`, now a lone high surrogate
    let well_formed = String::from_utf16_lossy(&units);
    assert_eq!(well_formed.encode_utf16().count(), original.len());
    let second = u64::try_from(original.len() - 3).unwrap();
    let p = run(&well_formed, current(second), SqlEngine::Postgres);
    assert_eq!(only(&p).sql, "SELECT 2 AS b");
    let p = run(&well_formed, current(3), SqlEngine::Postgres);
    assert_eq!(only(&p).sql, "SELECT '\u{FFFD}' AS a");
}

#[test]
fn destructive_lists_every_statement_before_substitution() {
    let text = "DROP TABLE t;\nSELECT 1;\nDELETE FROM u WHERE {{w}};\nUPDATE v SET a = {{a}};";
    let values = vec![
        ("w".to_string(), Value::Bool(true)),
        ("a".to_string(), Value::Int(1)),
    ];
    // Deferred statements count too.
    for defer in [false, true] {
        let p = plan(
            text,
            &RunTarget::All,
            Some(&values),
            SqlEngine::Postgres,
            100,
            defer,
        )
        .unwrap();
        let listed: Vec<(u32, &str, DestructiveReason)> = p
            .destructive
            .iter()
            .map(|d| (d.index, d.sql.as_str(), d.reason))
            .collect();
        assert_eq!(
            listed,
            [
                (0, "DROP TABLE t", DestructiveReason::DropTable),
                (
                    3,
                    "UPDATE v SET a = {{a}}",
                    DestructiveReason::UpdateNoWhere
                ),
            ]
        );
    }
    // At the cursor, only its statement, numbered as in the text.
    let p = run(text, current(u64::MAX), SqlEngine::Postgres);
    assert_eq!(p.destructive.len(), 1);
    assert_eq!(p.destructive[0].index, 3);
    // A value can't add a destructive keyword: it's bound or quoted.
    let text = "SELECT {{v}}";
    let values = vec![("v".to_string(), Value::Text("1; DROP TABLE t".into()))];
    for engine in SqlEngine::ALL {
        let p = plan(text, &RunTarget::All, Some(&values), engine, 100, false).unwrap();
        assert!(p.destructive.is_empty(), "{engine}");
        assert_eq!(p.statements.len(), 1, "{engine}");
    }
}

#[test]
fn defer_writes_marks_every_non_select() {
    let text = "SELECT 1;\nINSERT INTO t VALUES (1);\nUPDATE t SET a = 1 WHERE b;\n\
                DELETE FROM t WHERE a;\nCREATE TABLE u (a int);\nSELECT 2 LIMIT 1";
    let p = plan(text, &RunTarget::All, None, SqlEngine::Sqlite, 100, true).unwrap();
    let steps: Vec<(u32, Option<QueryType>)> = p
        .statements
        .iter()
        .map(|s| match &s.step {
            Step::Defer { query_type, .. } => (s.index, Some(*query_type)),
            _ => (s.index, None),
        })
        .collect();
    assert_eq!(
        steps,
        [
            (0, None),
            (1, Some(QueryType::Insert)),
            (2, Some(QueryType::Update)),
            (3, Some(QueryType::Delete)),
            (4, Some(QueryType::Other)),
            (5, None),
        ]
    );
    // Without it they run.
    let p = plan(text, &RunTarget::All, None, SqlEngine::Sqlite, 100, false).unwrap();
    let kinds: Vec<StatementKind> = p.statements.iter().map(kind).collect();
    use StatementKind::*;
    assert_eq!(kinds, [Page, Write, Write, Write, Utility, Stream]);
}

#[test]
fn substitution_fails_the_run_at_the_cursor_and_the_statement_in_run_all() {
    let text = "SELECT 1;\nSELECT {{a}};\nSELECT 2;";
    let values = vec![("a".to_string(), Value::Bytes(vec![1]))];
    let err = plan(
        text,
        &current(12),
        Some(&values),
        SqlEngine::Postgres,
        100,
        false,
    )
    .unwrap_err();
    assert_eq!(err.code, INVALID_PARAMETERS);
    assert!(err.message.contains("{{a}}"), "{}", err.message);

    let p = plan(
        text,
        &RunTarget::All,
        Some(&values),
        SqlEngine::Postgres,
        100,
        false,
    )
    .unwrap();
    assert_eq!(p.statements.len(), 3);
    assert!(matches!(p.statements[0].step, Step::Run { .. }));
    match &p.statements[1].step {
        Step::Fail { code, message } => {
            assert_eq!(code, INVALID_PARAMETERS);
            assert_eq!(message, &err.message);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(p.statements[2].step, Step::Run { .. }));
    // Also with pending changes on: substitution comes first.
    let p = plan(
        text,
        &RunTarget::All,
        Some(&values),
        SqlEngine::Postgres,
        100,
        true,
    )
    .unwrap();
    assert!(matches!(p.statements[1].step, Step::Fail { .. }));
    // Without values nothing is substituted.
    let p = run(text, RunTarget::All, SqlEngine::Postgres);
    assert_eq!(source(&p.statements[1]).sql, "SELECT {{a}}");
}

#[test]
fn mariadb_executable_comment_is_code() {
    let text = "SELECT 1 AS a /*M! ; DELETE FROM t */";
    let p = run(text, RunTarget::All, SqlEngine::Mariadb);
    assert_eq!(p.statements.len(), 2);
    assert_eq!(p.destructive.len(), 1);
    assert_eq!(p.destructive[0].reason, DestructiveReason::DeleteNoWhere);
    // On MySQL the same text is one statement with a comment.
    let p = run(text, RunTarget::All, SqlEngine::Mysql);
    assert_eq!(p.statements.len(), 1);
    assert!(p.destructive.is_empty());
}

#[test]
fn page_size_zero_streams_every_select() {
    let text = "SELECT 1;\nSELECT a FROM t;\nINSERT INTO t VALUES (1)";
    let p = plan(text, &RunTarget::All, None, SqlEngine::Postgres, 0, false).unwrap();
    let kinds: Vec<StatementKind> = p.statements.iter().map(kind).collect();
    assert_eq!(
        kinds,
        [
            StatementKind::Stream,
            StatementKind::Stream,
            StatementKind::Write
        ]
    );
}

#[test]
fn a_row_limited_select_streams() {
    for (engine, sql) in [
        (SqlEngine::Postgres, "SELECT a FROM t LIMIT 5"),
        (SqlEngine::Postgres, "SELECT a FROM t OFFSET 5"),
        (
            SqlEngine::Postgres,
            "SELECT a FROM t FETCH FIRST 5 ROWS ONLY",
        ),
        (SqlEngine::Mssql, "SELECT TOP 5 a FROM t"),
        (SqlEngine::Duckdb, "SELECT a FROM t LIMIT 5"),
    ] {
        assert_eq!(
            kind(only(&run(sql, RunTarget::All, engine))),
            StatementKind::Stream,
            "{sql}"
        );
    }
    // A limit in a subquery doesn't count.
    let sql = "SELECT a FROM (SELECT a FROM t LIMIT 5) s";
    assert_eq!(
        kind(only(&run(sql, RunTarget::All, SqlEngine::Postgres))),
        StatementKind::Page
    );
}

#[test]
fn page_size_past_the_cap_is_invalid() {
    let max = MAX;
    assert!(plan(
        "SELECT 1",
        &RunTarget::All,
        None,
        SqlEngine::Postgres,
        max,
        false
    )
    .is_ok());
    for size in [max + 1, u32::MAX] {
        let err = plan(
            "SELECT 1",
            &RunTarget::All,
            None,
            SqlEngine::Postgres,
            size,
            false,
        )
        .unwrap_err();
        assert_eq!(err.code, INVALID_ARGUMENT);
    }
    use seaquel_workspace::run::page_offset;
    assert_eq!(page_offset(1, 100, max).unwrap(), 0);
    assert_eq!(page_offset(3, 100, max).unwrap(), 200);
    assert_eq!(
        page_offset(u32::MAX, max, max).unwrap(),
        u64::from(u32::MAX - 1) * u64::from(max)
    );
    assert_eq!(page_offset(0, 100, max).unwrap_err().code, INVALID_ARGUMENT);
    assert_eq!(
        page_offset(2, max + 1, max).unwrap_err().code,
        INVALID_ARGUMENT
    );
}

/// Phase 5b probe (I1): under the web's limits a run's text and statement
/// count are bounded, so planning an 8 MiB frame can't take gigabytes and
/// seconds. The text is checked before anything scans it. Without limits
/// (the desktop, owner 2026-10-02) the same scripts plan.
#[test]
#[allow(clippy::disallowed_types, clippy::disallowed_methods)] // Instant, in a native-only test
fn runs_are_bounded_by_the_interfaces_limits() {
    use std::time::{Duration, Instant};
    const MIB: usize = 1024 * 1024;

    let over = "1;".repeat(4 * MIB);
    for target in [RunTarget::All, current(0)] {
        let start = Instant::now();
        let err = plan_web(&over, &target).unwrap_err();
        assert_eq!(err.code, INVALID_ARGUMENT, "{target:?}");
        assert!(err.message.contains("2 MiB"), "{}", err.message);
        assert!(start.elapsed() < Duration::from_millis(20), "{target:?}");
    }
    let at_limit = format!("SELECT 1 FROM t{}", " ".repeat(2 * MIB - 15));
    assert_eq!(at_limit.len(), 2 * MIB);
    assert_eq!(
        plan_web(&at_limit, &RunTarget::All)
            .unwrap()
            .statements
            .len(),
        1
    );
    let past = format!("{at_limit} ");
    assert_eq!(
        plan_web(&past, &RunTarget::All).unwrap_err().code,
        INVALID_ARGUMENT
    );

    let many = "SELECT 1;".repeat(10_000);
    assert_eq!(
        plan_web(&many, &RunTarget::All).unwrap().statements.len(),
        10_000
    );
    let too_many = format!("{many}SELECT 2");
    let err = plan_web(&too_many, &RunTarget::All).unwrap_err();
    assert_eq!(err.code, INVALID_ARGUMENT);
    assert!(err.message.contains("at most 10,000"), "{}", err.message);
    assert!(err.message.contains("10,001"), "{}", err.message);
    assert!(!err.message.contains("SELECT"), "{}", err.message);
    // At the cursor only one statement runs, however many the text holds.
    assert_eq!(
        plan_web(&too_many, &current(3)).unwrap().statements.len(),
        1
    );

    // No limits: a 3 MiB, 20,000-statement script plans.
    let big = format!("SELECT 1 FROM t WHERE a = '{}';", "x".repeat(130)).repeat(20_000);
    assert!(big.len() > 3 * MIB);
    let p = plan(&big, &RunTarget::All, None, SqlEngine::Postgres, 100, false).unwrap();
    assert_eq!(p.statements.len(), 20_000);
}

/// The substitution budget counts only what substituting adds, so a
/// script of any size runs with a few parameters (owner, 2026-10-02: a
/// 50 MB dump must not be refused). Unit-tested on the budget function, so
/// a 40 MiB script isn't planned (its scan would take over a gigabyte).
#[test]
fn the_substitution_budget_counts_only_growth() {
    use seaquel_sql::params::Values;
    use seaquel_workspace::run::{check_substitution_budget, MAX_RUN_SUBSTITUTED_BYTES};
    const MIB: usize = 1024 * 1024;

    let dump = format!(
        "INSERT INTO t VALUES ({{{{id}}}}, '{{{{name}}}}');\n{}",
        "INSERT INTO t VALUES (1, 'abcdefghij');\n".repeat(40 * MIB / 40)
    );
    assert!(dump.len() >= 40 * MIB);
    let values = [
        ("id".to_string(), Value::Int(7)),
        ("name".to_string(), Value::Text("x".repeat(1024))),
    ];
    assert!(check_substitution_budget([dump.as_str()], &Values::new(&values)).is_ok());
    // Many statements, each small: only their growth adds up.
    let statements = vec!["SELECT 1"; 100_000];
    assert!(check_substitution_budget(statements.iter().copied(), &Values::new(&values)).is_ok());

    // Growth past the budget is refused, whatever the text's size.
    let big = [("p".to_string(), Value::Text("x".repeat(MIB)))];
    let uses = MAX_RUN_SUBSTITUTED_BYTES / (2 * MIB) + 1;
    let sql = "SELECT {{p}}".to_string() + &",{{p}}".repeat(uses - 1);
    let err = check_substitution_budget([sql.as_str()], &Values::new(&big)).unwrap_err();
    assert_eq!(err.code, INVALID_PARAMETERS);
    assert!(err.message.contains("32 MiB"), "{}", err.message);
    let sql = "SELECT {{p}}".to_string() + &",{{p}}".repeat(uses - 3);
    assert!(check_substitution_budget([sql.as_str()], &Values::new(&big)).is_ok());
}

/// Phase 5b probe (N3): a value used many times can't multiply into a
/// run too large to hold. Inlining engines copy it per use (SQL Server,
/// DuckDB), MySQL binds it per use; the bound is checked before anything
/// is substituted, over the whole run, and refuses it outright.
#[test]
#[allow(clippy::disallowed_types, clippy::disallowed_methods)] // Instant, in a native-only test
fn substitution_cannot_amplify_a_run() {
    use seaquel_workspace::run::MAX_RUN_SUBSTITUTED_BYTES;
    use std::time::{Duration, Instant};

    let text = format!("SELECT {{{{p}}}}{}", ",{{p}}".repeat(300_000));
    let values = [("p".to_string(), Value::Text("x".repeat(1024 * 1024)))];
    for engine in SqlEngine::ALL {
        for target in [RunTarget::All, current(0)] {
            let start = Instant::now();
            let err = plan(&text, &target, Some(&values), engine, 100, false).unwrap_err();
            assert_eq!(err.code, INVALID_PARAMETERS, "{engine} {target:?}");
            assert!(err.message.contains("32 MiB"), "{}", err.message);
            assert!(!err.message.contains("xxx"), "{}", err.message);
            // Refused before substituting (which would take terabytes); a
            // debug build's scan of 1.8 MiB is the time here.
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "{engine} {target:?}: {:?}",
                start.elapsed()
            );
        }
    }
    // Across statements too: each is small, the run isn't.
    let text = "SELECT {{p}};".repeat(64);
    let err = plan(
        &text,
        &RunTarget::All,
        Some(&values),
        SqlEngine::Mssql,
        100,
        false,
    )
    .unwrap_err();
    assert_eq!(err.code, INVALID_PARAMETERS);
    // At the cursor only that statement counts.
    let p = plan(
        &text,
        &current(0),
        Some(&values),
        SqlEngine::Mssql,
        100,
        false,
    )
    .unwrap();
    assert_eq!(p.statements.len(), 1);

    // A large value used a few times is fine.
    let text = "SELECT {{p}}, {{p}}; SELECT {{p}}";
    let p = plan(
        text,
        &RunTarget::All,
        Some(&values),
        SqlEngine::Duckdb,
        100,
        false,
    )
    .unwrap();
    assert_eq!(p.statements.len(), 2);
    const { assert!(MAX_RUN_SUBSTITUTED_BYTES >= 3 * (2 * 1024 * 1024 + 32)) };
    // Without values nothing is substituted, so nothing is counted.
    let text = format!("SELECT {{{{p}}}}{}", ",{{p}}".repeat(300_000));
    assert!(plan(&text, &RunTarget::All, None, SqlEngine::Mssql, 100, false).is_ok());
}

/// Phase 5b probe (N4): a `{{name}}` the call has no value for binds NULL,
/// as today's TypeScript did (`substitute`'s `Map` rule, pinned by
/// seaquel-sql's frozen `query-params-test:6 values 0`). The dialog sends a
/// value for every parameter it finds, so only an API caller leaves one out.
#[test]
fn a_missing_value_binds_null() {
    let values = [("other".to_string(), Value::Int(1))];
    let p = plan(
        "SELECT {{p}}",
        &current(0),
        Some(&values),
        SqlEngine::Postgres,
        100,
        false,
    )
    .unwrap();
    let Step::Run { source, .. } = &p.statements[0].step else {
        panic!("{:?}", p.statements[0].step)
    };
    assert_eq!(source.sql, "SELECT $1");
    assert_eq!(source.params, vec![Value::Null]);
}

/// Phase 5b review (C1): the values are looked up and costed once per run,
/// not once per statement, and a decimal's written-out exponent is costed
/// without building it. Before, 1,000 statements × 100 `1e1048575`s took
/// 4.1 s and 10,000 × 1,000 about 400 s, all inside the run's stream.
#[test]
#[allow(clippy::disallowed_types, clippy::disallowed_methods)] // Instant, in a native-only test
fn planning_with_many_values_is_linear() {
    use std::time::{Duration, Instant};

    let values: Vec<(String, Value)> = (0..10_000)
        .map(|i| (format!("p{i}"), Value::Text(format!("value {i}"))))
        .collect();
    let text = (0..10_000)
        .map(|i| format!("SELECT {{{{p{i}}}}};"))
        .collect::<String>();
    let start = Instant::now();
    let p = plan(
        &text,
        &RunTarget::All,
        Some(&values),
        SqlEngine::Mssql,
        100,
        false,
    )
    .unwrap();
    assert_eq!(p.statements.len(), 10_000);
    // ~100 ms in a debug build; the old code took minutes.
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );

    let decimals: Vec<(String, Value)> = (0..100)
        .map(|i| (format!("d{i}"), Value::Decimal("1e1048575".into())))
        .collect();
    let uses = (0..100)
        .map(|i| format!("{{{{d{i}}}}}"))
        .collect::<Vec<_>>()
        .join(", ");
    let text = format!("SELECT {uses};").repeat(1_000);
    let start = Instant::now();
    let err = plan(
        &text,
        &RunTarget::All,
        Some(&decimals),
        SqlEngine::Duckdb,
        100,
        false,
    )
    .unwrap_err();
    // 100,000 uses of a megabyte each: refused, and fast.
    assert_eq!(err.code, INVALID_PARAMETERS);
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );
}

/// Phase 5b review (C1): the web caps the parameter values a run sends;
/// the desktop doesn't.
#[test]
fn the_web_caps_parameter_values() {
    let many: Vec<(String, Value)> = (0..1_001)
        .map(|i| (format!("p{i}"), Value::Int(i)))
        .collect();
    let web = |values: &[(String, Value)]| {
        plan_with_cap(
            "SELECT {{p0}}",
            &RunTarget::All,
            Some(values),
            SqlEngine::Postgres,
            PlanOptions {
                page_size: 100,
                defer_writes: false,
                max_page_size: MAX,
                limits: WEB,
            },
        )
    };
    let err = web(&many).unwrap_err();
    assert_eq!(err.code, INVALID_PARAMETERS);
    assert!(err.message.contains("at most 1,000"), "{}", err.message);
    assert!(web(&many[..1_000]).is_ok());
    let big = [("p0".to_string(), Value::Text("x".repeat(1024 * 1024 + 1)))];
    let err = web(&big).unwrap_err();
    assert_eq!(err.code, INVALID_PARAMETERS);
    assert!(err.message.contains("1 MiB"), "{}", err.message);
    assert!(!err.message.contains("xxx"));
    // No limits on the desktop.
    assert!(plan(
        "SELECT {{p0}}",
        &RunTarget::All,
        Some(&many),
        SqlEngine::Postgres,
        100,
        false
    )
    .is_ok());
    assert!(plan(
        "SELECT {{p0}}",
        &RunTarget::All,
        Some(&big),
        SqlEngine::Postgres,
        100,
        false
    )
    .is_ok());
}

#[test]
fn nothing_to_run_is_empty() {
    for text in ["", "   \n\t", "-- only a comment", "/* ; */ -- ;\n;;", ";"] {
        for target in [RunTarget::All, current(0), current(3), current(u64::MAX)] {
            for engine in SqlEngine::ALL {
                let p = plan(text, &target, None, engine, 100, false).unwrap();
                assert!(p.statements.is_empty(), "{text:?} {target:?} {engine}");
                assert!(p.destructive.is_empty());
            }
        }
    }
}

/// Every string in `value`, recursively.
fn strings(value: &Json, out: &mut Vec<String>) {
    match value {
        Json::String(s) => out.push(s.clone()),
        Json::Array(items) => items.iter().for_each(|v| strings(v, out)),
        Json::Object(map) => map.values().for_each(|v| strings(v, out)),
        _ => {}
    }
}

#[test]
fn plan_never_panics() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../seaquel-sql/tests/fixtures");
    let mut corpus = Vec::new();
    for file in ["split", "statement-at", "params"] {
        let text = std::fs::read_to_string(dir.join(format!("{file}.json"))).unwrap();
        strings(&serde_json::from_str(&text).unwrap(), &mut corpus);
    }
    corpus.sort();
    corpus.dedup();
    assert!(corpus.len() > 1000, "{}", corpus.len());
    let values = |text: &str| -> Vec<(String, Value)> {
        seaquel_sql::params::extract_parameters(text)
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let value = match i % 4 {
                    0 => Value::Text("it's \\ $$ 😀".into()),
                    1 => Value::Int(-5),
                    2 => Value::Null,
                    _ => Value::Bytes(vec![1]),
                };
                (name, value)
            })
            .collect()
    };
    for (n, text) in corpus.iter().enumerate() {
        let engine = SqlEngine::ALL[n % SqlEngine::ALL.len()];
        let len16 = text.encode_utf16().count() as u64;
        let vals = values(text);
        let _ = plan(text, &RunTarget::All, Some(&vals), engine, 100, n % 2 == 0);
        let _ = plan(text, &current(len16 / 2), None, engine, 0, false);
        let _ = plan(text, &current(len16 + 1), Some(&vals), engine, 1, true);
    }
}
