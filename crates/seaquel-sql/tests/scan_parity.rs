//! Parity of the scanner and the statement checks with the frozen TS
//! fixtures (`tests/fixtures/{split,statement-at,row-limit,count-query,
//! statements,read-only}.json`), which already hold the bug-fixed output
//! (fixes 10, 11, 12, 14 and 18; see the fixtures' README). `bugfixes.json`
//! has no hand-written scanner cases to apply (its `cases` are all AST,
//! builder and parse-error kinds), which `no_hand_written_scanner_cases`
//! checks.
//!
//! Also the properties every scanner function must have on every prefix of
//! every scanner-corpus input: no panic, ranges inside the input and on char
//! boundaries, and statements that rejoin to the input.
//!
//! Fixture offsets are UTF-16 code units; this crate works in UTF-8 bytes.

use std::collections::BTreeMap;
use std::path::PathBuf;

use seaquel_sql::read_only::read_only_error;
use seaquel_sql::scan::{
    count_query, has_row_limit, scan, split_statements, statement_at, strip_trailing_order_by,
    tokens, ScanOptions, Statement, TokenKind,
};
use seaquel_sql::statements::{destructive_reason, query_type, table_from_select};
use seaquel_sql::SqlEngine;
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn cases(file: &Value) -> &Vec<Value> {
    file["cases"].as_array().unwrap()
}

fn sql_of(case: &Value) -> &str {
    case["input"]["sql"].as_str().unwrap()
}

/// The UTF-16 offset of byte offset `b` (rounded down to a char boundary).
fn utf16_at(sql: &str, b: usize) -> usize {
    let mut b = b.min(sql.len());
    while !sql.is_char_boundary(b) {
        b -= 1;
    }
    sql[..b].encode_utf16().count()
}

/// A statement in the TS shape: UTF-16 offsets, `endOffset` of an
/// unterminated last statement is the UTF-16 length minus one.
fn ts_statement(sql: &str, s: &Statement) -> Value {
    let end = if s.end == sql.len() {
        sql.encode_utf16().count() as i64 - 1
    } else {
        utf16_at(sql, s.end) as i64
    };
    json!({
        "sql": &sql[s.text.clone()],
        "index": s.index,
        "startOffset": utf16_at(sql, s.start),
        "endOffset": end,
    })
}

/// Collects mismatches per fixture file and fails with all of them.
#[derive(Default)]
struct Report {
    passed: BTreeMap<&'static str, usize>,
    failed: BTreeMap<&'static str, Vec<String>>,
}

impl Report {
    fn check(&mut self, file: &'static str, what: String, got: &Value, want: &Value) {
        if got == want {
            *self.passed.entry(file).or_default() += 1;
        } else {
            self.failed
                .entry(file)
                .or_default()
                .push(format!("{what}\n    got:  {got}\n    want: {want}"));
        }
    }

    fn finish(self) {
        for (file, n) in &self.passed {
            let bad = self.failed.get(file).map_or(0, Vec::len);
            eprintln!("{file}: {n} passed, {bad} failed");
        }
        let failures: Vec<String> = self.failed.into_values().flatten().collect();
        if !failures.is_empty() {
            let shown: Vec<&String> = failures.iter().take(40).collect();
            panic!(
                "{} mismatches (first {}):\n{}",
                failures.len(),
                shown.len(),
                shown
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }
}

fn engine(name: &str) -> SqlEngine {
    name.parse().unwrap()
}

#[test]
fn split_parity() {
    let file = fixture("split.json");
    let mut report = Report::default();
    for case in cases(&file) {
        let sql = sql_of(case);
        for (e, want) in case["output"].as_object().unwrap() {
            let got: Vec<Value> = split_statements(sql, engine(e))
                .iter()
                .map(|s| ts_statement(sql, s))
                .collect();
            report.check(
                "split.json",
                format!("{} [{e}] {sql:?}", case["name"]),
                &Value::Array(got),
                want,
            );
        }
    }
    report.finish();
}

#[test]
fn statement_at_parity() {
    let file = fixture("statement-at.json");
    let mut report = Report::default();
    for case in cases(&file) {
        let sql = sql_of(case);
        for (e, runs) in case["output"].as_object().unwrap() {
            // Expand the runs to one expected index per UTF-16 offset.
            let mut want_at: Vec<Value> = Vec::new();
            for run in runs.as_array().unwrap() {
                let (from, to) = (run[0].as_u64().unwrap(), run[1].as_u64().unwrap());
                for _ in from..=to {
                    want_at.push(run[2].clone());
                }
            }
            // Every byte offset 0..=len answers as the UTF-16 offset of its char.
            let mut got_runs: Vec<String> = Vec::new();
            let mut ok = true;
            for b in 0..=sql.len() {
                let got = statement_at(sql, b, engine(e)).map_or(Value::Null, |s| json!(s.index));
                let want = &want_at[utf16_at(sql, b)];
                if &got != want {
                    ok = false;
                    got_runs.push(format!("byte {b}: got {got}, want {want}"));
                }
            }
            // And past the end.
            let past = statement_at(sql, sql.len() + 10, engine(e))
                .map_or(Value::Null, |s| json!(s.index));
            if &past != want_at.last().unwrap() {
                ok = false;
                got_runs.push(format!("past the end: got {past}"));
            }
            let what = format!("{} [{e}] {sql:?}", case["name"]);
            if ok {
                report.check(
                    "statement-at.json",
                    what,
                    &Value::Bool(true),
                    &Value::Bool(true),
                );
            } else {
                got_runs.truncate(5);
                report.check(
                    "statement-at.json",
                    what,
                    &json!(got_runs),
                    &Value::Bool(true),
                );
            }
        }
    }
    report.finish();
}

#[test]
fn row_limit_and_count_query_parity() {
    let mut report = Report::default();
    let file = fixture("row-limit.json");
    for case in cases(&file) {
        let sql = sql_of(case);
        for (e, want) in case["output"].as_object().unwrap() {
            let got = json!(has_row_limit(sql, engine(e)));
            report.check(
                "row-limit.json",
                format!("{} [{e}] {sql:?}", case["name"]),
                &got,
                want,
            );
        }
    }
    let file = fixture("count-query.json");
    for case in cases(&file) {
        let sql = sql_of(case);
        for (e, want) in case["output"].as_object().unwrap() {
            let got = json!(count_query(sql, engine(e)));
            report.check(
                "count-query.json",
                format!("{} [{e}] {sql:?}", case["name"]),
                &got,
                want,
            );
        }
    }
    report.finish();
}

#[test]
fn statements_parity() {
    let file = fixture("statements.json");
    let mut report = Report::default();
    for case in cases(&file) {
        let sql = sql_of(case);
        for (e, want) in case["output"].as_object().unwrap() {
            let e = engine(e);
            let got = json!({
                "queryType": query_type(sql, e),
                "destructive": destructive_reason(sql, e),
                "table": table_from_select(sql, e),
            });
            report.check(
                "statements.json",
                format!("{} [{e}] {sql:?}", case["name"]),
                &got,
                want,
            );
        }
    }
    report.finish();
}

#[test]
fn read_only_parity() {
    let file = fixture("read-only.json");
    let mut report = Report::default();
    for case in cases(&file) {
        let sql = sql_of(case);
        for (e, want) in case["output"].as_object().unwrap() {
            let got = json!(read_only_error(sql, engine(e)));
            report.check(
                "read-only.json",
                format!("{} [{e}] {sql:?}", case["name"]),
                &got,
                want,
            );
        }
    }
    report.finish();
}

#[test]
fn no_hand_written_scanner_cases() {
    let bugfixes = fixture("bugfixes.json");
    let scanner_files = [
        "split.json",
        "statement-at.json",
        "row-limit.json",
        "count-query.json",
        "statements.json",
        "read-only.json",
    ];
    for case in bugfixes["cases"].as_array().unwrap() {
        let replaces = case["replaces"].as_str().unwrap_or("");
        assert!(
            !scanner_files.iter().any(|f| replaces.starts_with(f)),
            "a hand-written scanner case now exists and must be applied: {}",
            case["name"]
        );
        let kind = case["kind"].as_str().unwrap_or("");
        assert!(
            ["visual", "column-sources", "parse-error", "builder"].contains(&kind),
            "unhandled bugfixes.json case kind {kind:?}: {}",
            case["name"]
        );
    }
}

// ── Properties, on every prefix of every scanner-corpus input ──

fn corpus() -> Vec<String> {
    let file = fixture("split.json");
    cases(&file).iter().map(|c| sql_of(c).to_string()).collect()
}

/// Every prefix of `sql` that ends on a char boundary, the input itself included.
fn prefixes(sql: &str) -> impl Iterator<Item = &str> {
    (0..=sql.len())
        .filter(|&b| sql.is_char_boundary(b))
        .map(move |b| &sql[..b])
}

fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2028}'
    ) || matches!(
        c,
        '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    ) || ('\u{2000}'..='\u{200A}').contains(&c)
}

fn assert_range(sql: &str, start: usize, end: usize, what: &str) {
    assert!(
        start <= end && end <= sql.len(),
        "{what}: {start}..{end} outside 0..{} in {sql:?}",
        sql.len()
    );
    assert!(
        sql.is_char_boundary(start) && sql.is_char_boundary(end),
        "{what}: {start}..{end} not on char boundaries in {sql:?}"
    );
}

/// The scanner's tokens (comments included) are in order, don't overlap, sit
/// on char boundaries, and only whitespace lies between them.
fn check_scan(sql: &str, e: SqlEngine, options: ScanOptions) {
    let toks = scan(sql, e, options);
    let mut at = 0;
    for t in &toks {
        assert_range(sql, t.start, t.end, "token");
        assert!(
            t.start < t.end,
            "empty token at {} in {sql:?} [{e}]",
            t.start
        );
        assert!(
            t.start >= at,
            "overlapping tokens at {} in {sql:?} [{e}]",
            t.start
        );
        assert!(
            sql[at..t.start].chars().all(is_js_space),
            "non-space {:?} between tokens in {sql:?} [{e}] {options:?}",
            &sql[at..t.start]
        );
        at = t.end;
    }
    assert!(
        sql[at..].chars().all(is_js_space),
        "non-space tail {:?} in {sql:?} [{e}]",
        &sql[at..]
    );
    // `tokens` is `scan` without comments and without executable comments.
    let significant: Vec<_> = scan(sql, e, ScanOptions::default())
        .into_iter()
        .filter(|t| t.kind != TokenKind::Comment)
        .collect();
    assert_eq!(tokens(sql, e), significant);
}

/// Statements are in order, inside the input, on char boundaries; each
/// statement's trimmed text lies within it with only whitespace around it;
/// and between statements lie only `;`, whitespace and comments, so the
/// statements and what's between them rejoin to the input.
fn check_split(sql: &str, e: SqlEngine) {
    let statements = split_statements(sql, e);
    let mut prev_end = 0;
    for (k, s) in statements.iter().enumerate() {
        assert_eq!(s.index, k);
        assert_range(sql, s.start, s.end, "statement");
        assert_range(sql, s.text.start, s.text.end, "statement text");
        assert!(s.start <= s.text.start && s.text.end <= s.end);
        assert!(!s.text.is_empty(), "empty statement in {sql:?} [{e}]");
        assert!(sql[s.start..s.text.start].chars().all(is_js_space));
        assert!(sql[s.text.end..s.end].chars().all(is_js_space));
        assert!(s.start >= prev_end, "statements overlap in {sql:?} [{e}]");
        assert!(s.end == sql.len() || sql.as_bytes()[s.end] == b';');
        prev_end = s.end;
    }
    // What lies between statements holds no code.
    let significant = scan(
        sql,
        e,
        ScanOptions {
            exec_comments: true,
            ..ScanOptions::default()
        },
    );
    for t in significant.iter().filter(|t| t.kind != TokenKind::Comment) {
        let is_semicolon = t.kind == TokenKind::Punct && t.text(sql) == ";";
        let inside = statements
            .iter()
            .any(|s| t.start >= s.start && t.end <= s.end);
        assert!(
            is_semicolon || inside,
            "token {:?} outside every statement in {sql:?} [{e}]",
            t.text(sql)
        );
    }
    // With the statements in order and inside the input, they and the gaps
    // between them rejoin to the input by construction; what makes that mean
    // something is the check above, that no code lies in a gap.
    // statement_at never panics, and past the end is the last statement.
    for offset in [sql.len() / 2, usize::MAX] {
        let got = statement_at(sql, offset, e);
        assert_eq!(got.is_some(), !statements.is_empty());
    }
}

fn check_engine(sql: &str, e: SqlEngine) {
    for ts_words in [false, true] {
        let options = ScanOptions {
            exec_comments: true,
            ansi_mysql: true,
            pg_backslash: true,
            ts_words,
        };
        check_scan(sql, e, options);
    }
    check_split(sql, e);
    let _ = has_row_limit(sql, e);
    let stripped = strip_trailing_order_by(sql, e);
    assert!(sql.starts_with(stripped));
    assert!(count_query(sql, e).contains(stripped));
    let _ = query_type(sql, e);
    let _ = destructive_reason(sql, e);
    let _ = table_from_select(sql, e);
    let _ = read_only_error(sql, e);
}

fn check_all(sql: &str) {
    for e in SqlEngine::ALL {
        check_engine(sql, e);
    }
}

/// Every prefix of every corpus input, one test per engine so they run in
/// parallel.
fn every_prefix_of_the_corpus(e: SqlEngine) {
    for sql in corpus() {
        for prefix in prefixes(&sql) {
            check_engine(prefix, e);
        }
    }
}

#[test]
fn every_prefix_of_the_corpus_postgres() {
    every_prefix_of_the_corpus(SqlEngine::Postgres);
}

#[test]
fn every_prefix_of_the_corpus_mysql() {
    every_prefix_of_the_corpus(SqlEngine::Mysql);
}

#[test]
fn every_prefix_of_the_corpus_mariadb() {
    every_prefix_of_the_corpus(SqlEngine::Mariadb);
}

#[test]
fn every_prefix_of_the_corpus_sqlite() {
    every_prefix_of_the_corpus(SqlEngine::Sqlite);
}

#[test]
fn every_prefix_of_the_corpus_mssql() {
    every_prefix_of_the_corpus(SqlEngine::Mssql);
}

#[test]
fn every_prefix_of_the_corpus_duckdb() {
    every_prefix_of_the_corpus(SqlEngine::Duckdb);
}

/// Inputs that aren't in the corpus: odd bytes, lone quote characters, deep
/// nesting, the characters the scanner decides on in every combination.
#[test]
fn every_prefix_of_odd_inputs() {
    let inputs = [
        "",
        "\u{FEFF}SELECT 1;\u{FEFF}",
        "\u{85}SELECT 1",
        "$",
        "$$",
        "$a",
        "$a$",
        "$1$",
        "E",
        "E'",
        "e'\\",
        "'\\",
        "\"\\",
        "`",
        "[",
        "]]",
        "/*",
        "/*!",
        "/*M!",
        "/*!5",
        "/*!50700 SELECT 1; */ */",
        "*/",
        "--",
        "-- ",
        "--\r",
        "#",
        "0x",
        "0X1g",
        "$.",
        "$.5",
        ".5e",
        "1e+",
        "1e-5x",
        "1.5.5",
        "@@",
        "a.2b",
        "`a`.5",
        "((((((((((",
        "))))))))))",
        "😀😀;東京;\u{1F600}",
        "SELECT 'é\\😀'",
        "SELECT E'\\😀'",
        "SELECT $東京$ ; $東京$",
        "SELECT $😀$ x $😀$",
        "\\",
        "\u{2028};\u{3000};\u{A0}",
        "U&\"x\"(",
        "SELECT 1 /*! ; DELETE FROM t */",
        "SELECT '/*!' FROM t; SELECT 1",
        "DELETE FROM t WHERE (",
        "WITH d AS (DELETE FROM t) SELECT (",
        "EXPLAIN (",
        ")DELETE",
        "ALTER TABLE t DROP",
        "MERGE INTO t USING s ON 1=1 WHEN MATCHED THEN DELETE",
    ];
    for sql in inputs {
        for prefix in prefixes(sql) {
            check_all(prefix);
        }
    }
    // Every combination of the characters the scanner branches on, up to
    // three long.
    let alphabet = [
        "'", "\"", "`", "[", "]", "\\", "$", "e", "E", "-", "#", "/", "*", "!", "M", "(", ")", ";",
        ".", "0", "x", "@", "\r", "\n", " ", "東", "😀", "\u{A0}",
    ];
    let mut s = String::new();
    for a in alphabet {
        for b in alphabet {
            for c in alphabet {
                s.clear();
                s.push_str(a);
                s.push_str(b);
                s.push_str(c);
                check_all(&s);
            }
        }
    }
}

/// A 1 MB script splits in well under the 60 ms sqlparser's tokenizer took
/// natively. Timed only in an optimized build (`cargo test --release`); a
/// debug build checks that it finishes.
#[test]
#[allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "a native-only timing test; the crate itself never reads the clock"
)]
fn a_megabyte_script_splits_fast() {
    let mut sql = String::new();
    // The six e2e schema files, whole: well-formed scripts with comments,
    // strings and quoted names.
    let file = fixture("split.json");
    let schemas: Vec<&str> = cases(&file)
        .iter()
        .filter(|c| c["name"].as_str().unwrap().starts_with("schema:"))
        .map(sql_of)
        .collect();
    assert_eq!(schemas.len(), 6);
    while sql.len() < 1_000_000 {
        for input in &schemas {
            sql.push_str(input);
            sql.push('\n');
        }
    }
    for e in SqlEngine::ALL {
        let started = std::time::Instant::now();
        let statements = split_statements(&sql, e);
        let split = started.elapsed();
        let started = std::time::Instant::now();
        let _ = destructive_reason(&sql, e);
        let _ = read_only_error(&sql, e);
        let _ = has_row_limit(&sql, e);
        let checks = started.elapsed();
        eprintln!(
            "{e}: {} bytes, {} statements, split {split:?}, destructive + read-only + row limit {checks:?}",
            sql.len(),
            statements.len()
        );
        if !cfg!(debug_assertions) {
            assert!(split.as_millis() < 30, "{e}: split took {split:?}");
        }
    }
}

/// Inputs built to make the statement checks rescan: every one is a single
/// statement of about 200 KB. Checks that none of them is quadratic enough to
/// stall the editor.
#[test]
#[allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "a native-only timing test; the crate itself never reads the clock"
)]
fn pathological_statements_stay_fast() {
    let cases = [
        ("(SELECT 1) DELETE ", SqlEngine::Postgres),
        ("(SELECT 1) UPDATE t SET a = 1 ", SqlEngine::Postgres),
        ("ALTER TABLE t ", SqlEngine::Mssql),
        ("x UPDATE t SET a = 1 ", SqlEngine::Mssql),
        ("((", SqlEngine::Postgres),
        ("$a$ ", SqlEngine::Postgres),
        ("/*! ", SqlEngine::Mysql),
    ];
    for (unit, e) in cases {
        let mut sql = unit.repeat(200_000 / unit.len());
        sql.push_str("WHERE 1 = 1");
        let started = std::time::Instant::now();
        let _ = split_statements(&sql, e);
        let _ = destructive_reason(&sql, e);
        let _ = read_only_error(&sql, e);
        let _ = table_from_select(&sql, e);
        let took = started.elapsed();
        eprintln!("{unit:?} [{e}]: {took:?}");
        if !cfg!(debug_assertions) {
            assert!(took.as_millis() < 200, "{unit:?} [{e}] took {took:?}");
        }
    }
}

/// The Task 3 review's adversarial inputs, ten per engine, written against
/// the branches of `sql-scan.ts` before reading the port. Every one gave the
/// models' output for every engine (compared with the TS models then); here
/// they join the prefix and property checks.
const ADVERSARIAL: &[&str] = &[
    // postgres
    "SELECT E'\\\\'; DELETE FROM t; SELECT 'x'",
    "SELECT e'a\\'b'; UPDATE t SET a=1",
    "SELECT xE'a;b'; SELECT 2",
    "SELECT $1; SELECT $a$ ; $a$; DELETE FROM t",
    "SELECT $_$;$_$, $$;$$, $1$;$2",
    "/* a /* b */ ; DELETE FROM t */ SELECT 1; SELECT 2",
    "SELECT t.limit, t.offset FROM t; SELECT (1) LIMIT 1",
    "SELECT 1 -- x\rDELETE FROM t",
    "SELECT U&\"d\\0061ta\"(1); SELECT 1.e5INTO",
    "SELECT \"a\"\"b\".limit FROM \"s\".\"t\"\"x\" WHERE 1=1OFFSET 1",
    // mysql
    "SELECT a#b;\nDELETE FROM t",
    "SELECT 'a\\';b'; UPDATE t SET a=1",
    "SELECT \"a\\\";b\"; DELETE FROM t",
    "SELECT `a``;b` FROM `s`.`t`; SELECT 2",
    "SELECT 1--1; DELETE FROM t",
    "SELECT 1 --\tx\nDELETE FROM t",
    "SELECT 1 /*!50700 ; DELETE FROM t */",
    "SELECT 1 /*M! ; DELETE FROM t */",
    "SELECT a FROM db.2fa_codes WHERE x=1LIMIT 1",
    "SELECT @a:=1, @@version, 1.5INTO @x, .5e1FROM t",
    // mariadb
    "SELECT 1 /*M!100000 ; DELETE FROM t */",
    "SELECT 1 /*!; UPDATE t SET a = 1*/",
    "SELECT '/*!' FROM t; SELECT 1",
    "SELECT `x`.5 FROM t",
    "SELECT \"a\" \"b\" FROM t#;\n; DROP TABLE t",
    "INSERT INTO t VALUES (1) ON DUPLICATE KEY UPDATE a = 1",
    "SELECT * FROM t FOR UPDATE",
    "SELECT 1 # /*!\n; DELETE FROM x",
    "SELECT 'a''b\\\\' ; TRUNCATE t",
    "SELECT a FROM `we``ird`.`ta ble`",
    // sqlite
    "SELECT [a]]b] FROM t; SELECT 2",
    "SELECT [a;b] FROM [s].[t]",
    "SELECT `a;b`, \"c;d\" FROM t; DELETE FROM t",
    "SELECT 'a\\'; DELETE FROM t; SELECT '",
    "/* /* */ DELETE FROM t */",
    "SELECT $a$;$a$",
    "SELECT #a; SELECT 1",
    "SELECT E'a\\';b'",
    "SELECT 1 FROM t LIMIT 1 OFFSET 2",
    "SELECT a FROM [we]]ird]",
    // mssql
    "SELECT [a]]b;] FROM t; SELECT 2",
    "SELECT #temp.a FROM #temp",
    "SELECT 1 DELETE FROM t",
    "SET NOCOUNT ON UPDATE t SET a = 1",
    "SELECT a=0x1INTO #q",
    "SELECT a=$1.5INTO #m",
    "/* /* */ ; DELETE FROM t */ SELECT 1",
    "SELECT TOP 5 * FROM t ORDER BY a",
    "IF UPDATE(a) SELECT 1; UPDATE STATISTICS t",
    "SELECT 1 AS k END CONVERSATION @h",
    // duckdb
    "SELECT $x$ ; $x$; SELECT 2",
    "SELECT E'\\';DELETE FROM t'",
    "SELECT * FROM read_csv('a;b.csv'); DROP TABLE t",
    "SELECT 1 -- c\rDELETE FROM t",
    "/* /* */ ; */ SELECT 1",
    "SELECT a.limit FROM a",
    "SELECT * FROM t QUALIFY x LIMIT 5",
    "SELECT \"a\"\"\" FROM \"s\".\"t\"",
    "SELECT 1.5e-3INTO",
    "PRAGMA version; SELECT 1",
];

#[test]
fn every_prefix_of_the_adversarial_inputs() {
    for sql in ADVERSARIAL {
        for prefix in prefixes(sql) {
            check_all(prefix);
        }
    }
}

/// Word characters follow each engine's identifier rule (fix 19), and a
/// `$tag$` takes any non-ASCII char at any length (fixes 19 and 10). Checked
/// against `scan-model.ts`.
#[test]
fn word_characters_per_engine() {
    use SqlEngine::*;
    let table = |sql, e| table_from_select(sql, e).map(|t| t.table);
    for e in SqlEngine::ALL {
        // A Thai name keeps its vowel signs and tone marks everywhere.
        assert_eq!(
            table("SELECT * FROM ตารางที่", e).as_deref(),
            Some("ตารางที่"),
            "{e}"
        );
    }
    // A circled letter is a symbol: a word char except on SQL Server.
    assert_eq!(table("SELECT * FROM Ⓐb", Postgres).as_deref(), Some("Ⓐb"));
    assert_eq!(table("SELECT * FROM Ⓐb", Mysql).as_deref(), Some("Ⓐb"));
    assert_eq!(table("SELECT * FROM Ⓐb", Mssql), None);
    // A letter outside the BMP: not on MySQL.
    assert_eq!(
        table("SELECT * FROM \u{20000}x", Sqlite).as_deref(),
        Some("\u{20000}x")
    );
    assert_eq!(
        table("SELECT * FROM \u{20000}x", Mssql).as_deref(),
        Some("\u{20000}x")
    );
    assert_eq!(table("SELECT * FROM \u{20000}x", Mariadb), None);
    // An emoji is a word char on Postgres only of these two.
    let texts = |sql: &'static str, e| -> Vec<&'static str> {
        tokens(sql, e).iter().map(|t| t.text(sql)).collect()
    };
    assert_eq!(texts("a😀b", Duckdb), ["a😀b"]);
    assert_eq!(texts("a😀b", Mssql), ["a", "😀", "b"]);
    assert_eq!(texts("ſelect x", Postgres), ["ſelect", "x"]);
    assert_eq!(
        query_type("ſelect 1", Postgres),
        seaquel_sql::statements::QueryType::Select
    );
    // Tags: any non-ASCII char, any length.
    assert_eq!(texts("$٣$;$٣$", Postgres), ["$٣$;$٣$"]);
    let long = format!("SELECT ${0}$;${0}$; SELECT 2", "a".repeat(200));
    assert_eq!(split_statements(&long, Postgres).len(), 2);
    // MySQL's `--` needs ASCII space or a control char after it.
    assert_eq!(texts("1--\u{A0}x", Mysql), ["1", "-", "-", "x"]);
    assert_eq!(texts("1--\u{7F}x", Mysql), ["1"]);
    assert!(texts("1--\tx", Mysql) == ["1"]);
}

/// The two shapes the Task 3 review found quadratic in `destructive_reason`,
/// each about 460 KB: nested groups that each have their own WHERE, and a
/// deep group followed by many `) UPDATE … WHERE` (the EXPLAIN back-scan).
#[test]
#[allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "a native-only timing test; the crate itself never reads the clock"
)]
fn nested_statements_stay_fast() {
    let n = 30_000;
    let nested = format!("{}{}", "(DELETE FROM t ".repeat(n), "WHERE 1)".repeat(n));
    let back_scan = format!(
        "{}x{}",
        "(".repeat(n),
        ") UPDATE t SET a=1 WHERE 1 ".repeat(n / 2)
    );
    for (name, sql) in [("nested", nested), ("back-scan", back_scan)] {
        for e in [SqlEngine::Postgres, SqlEngine::Mssql] {
            let started = std::time::Instant::now();
            let reason = destructive_reason(&sql, e);
            let took = started.elapsed();
            eprintln!("{name} [{e}] {} bytes: {took:?} ({reason:?})", sql.len());
            if !cfg!(debug_assertions) {
                assert!(took.as_millis() < 200, "{name} [{e}] took {took:?}");
            }
        }
    }
}
