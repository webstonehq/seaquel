//! The import readers' pure mapping (`seaquel_workspace::imports`, phase 5e
//! Task 4a): every TablePlus and DBeaver case in `tests/fixtures/imports`
//! replayed through `tableplus_candidates`/`dbeaver_candidates` and
//! `mark_duplicates`, compared with `changes.json` exactly. Core's replay
//! (`seaquel-core/tests/imports.rs`) reads the same inputs from a temp home.

use std::collections::BTreeSet;

use seaquel_workspace::imports::*;
use serde_json::{json, Value};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/imports");

fn load(file: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{FIXTURES}/{file}")).unwrap()).unwrap()
}

fn existing(case: &Value) -> Vec<ConnectionIdentity> {
    serde_json::from_value(case["existing"].clone()).unwrap()
}

/// What the replay makes of a case's input: `null` is no file, `{"error"}`
/// a read that failed (Core's message, never the path), anything else the
/// reader's input.
fn answer(source: ImportSource, case: &Value) -> ImportCandidates {
    let input = &case["input"];
    if input.is_null() {
        return ImportCandidates::not_found();
    }
    if input
        .as_object()
        .is_some_and(|o| o.len() == 1 && o.contains_key("error"))
    {
        return ImportCandidates::unreadable("The file couldn't be read.");
    }
    let mut result = match source {
        ImportSource::Tableplus => tableplus_candidates(input),
        ImportSource::Dbeaver => dbeaver_candidates(input.as_str().unwrap().as_bytes()),
    };
    if let Ok(c) = &mut result {
        mark_duplicates(c, &existing(case));
    }
    ImportCandidates::from_result(result)
}

/// `<message>` stands for any non-empty text naming no path.
fn normalise(mut actual: Value, expected: &Value) -> Value {
    if expected.get("unreadable").and_then(Value::as_str) == Some("<message>") {
        if let Some(m) = actual.get("unreadable").and_then(Value::as_str) {
            assert!(!m.trim().is_empty(), "an empty message");
            assert!(
                !m.contains('/') && !m.contains('\\'),
                "the message names a path: {m}"
            );
            actual["unreadable"] = json!("<message>");
        }
    }
    actual
}

fn replay(file: &str, prefix: &str, source: ImportSource, changes: &Value) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let cases = load(file);
    for case in cases.as_array().unwrap() {
        let name = format!("{prefix}/{}", case["name"].as_str().unwrap());
        let expected = &changes[&name]["expected"]["output"];
        assert!(!expected.is_null(), "{name}: no entry in changes.json");
        let actual = serde_json::to_value(answer(source, case)).unwrap();
        assert_eq!(&normalise(actual, expected), expected, "{name}");
        seen.insert(name);
    }
    seen
}

#[test]
fn replays_every_import_case() {
    let changes = load("changes.json");
    let mut seen = replay(
        "tableplus.json",
        "tableplus",
        ImportSource::Tableplus,
        &changes,
    );
    seen.extend(replay(
        "dbeaver.json",
        "dbeaver",
        ImportSource::Dbeaver,
        &changes,
    ));
    let listed: BTreeSet<String> = changes
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| *k != "*")
        .cloned()
        .collect();
    assert_eq!(seen, listed, "changes.json and the cases differ");
    assert_eq!(seen.len(), 28);
}

/// The step that leaves `src-tauri`: the `plist` crate decodes each XML
/// plist to exactly the JSON the case recorded, and the candidates made from
/// the decoded value are the recorded ones. `unreadable.plist` doesn't decode.
#[test]
fn plists_decode_to_the_recorded_json() {
    let changes = load("changes.json");
    let mut decoded = 0;
    for case in load("tableplus.json").as_array().unwrap() {
        let Some(file) = case["plist"].as_str() else {
            continue;
        };
        let name = case["name"].as_str().unwrap();
        let bytes = std::fs::read(format!("{FIXTURES}/{file}")).unwrap();
        match plist::from_bytes::<plist::Value>(&bytes) {
            Ok(v) => {
                // As `read_tableplus_config` handed it to the page.
                let text = serde_json::to_string(&v).unwrap();
                let value: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(value, case["input"], "{name}");
                // And as Core will hand it over, without the text step.
                let direct = serde_json::to_value(&v).unwrap();
                let mut c = tableplus_candidates(&direct);
                if let Ok(c) = &mut c {
                    mark_duplicates(c, &existing(case));
                }
                let actual = serde_json::to_value(ImportCandidates::from_result(c)).unwrap();
                let expected = &changes[&format!("tableplus/{name}")]["expected"]["output"];
                assert_eq!(&normalise(actual, expected), expected, "{name}");
                decoded += 1;
            }
            Err(_) => assert_eq!(name, "unreadable", "{name} didn't decode"),
        }
    }
    assert_eq!(decoded, 13);
}

#[test]
fn parse_int_matches_javascripts() {
    let nan: Option<f64> = None;
    for (text, want) in [
        ("5432", Some(5432.0)),
        ("5432abc", Some(5432.0)),
        (" 15432", Some(15432.0)),
        ("\t\n\u{a0}\u{feff}\u{3000}7", Some(7.0)),
        ("1e3", Some(1.0)),
        ("12.9", Some(12.0)),
        ("-1", Some(-1.0)),
        ("+7", Some(7.0)),
        ("0x10", Some(0.0)),
        ("007", Some(7.0)),
        ("99999999999999999999", Some(1e20)),
        ("", nan),
        ("   ", nan),
        ("abc", nan),
        ("-", nan),
        ("+", nan),
        ("--1", nan),
        ("${port}", nan),
        ("\u{200b}5", nan),
        ("１２", nan),
        ("Infinity", nan),
    ] {
        assert_eq!(parse_int(text), want, "{text:?}");
    }
    let minus_zero = parse_int("-0").unwrap();
    assert!(minus_zero == 0.0 && minus_zero.is_sign_negative());
    assert_eq!(parse_int(&"9".repeat(400)), Some(f64::INFINITY));
}

fn one_tableplus(port: Value) -> ImportCandidate {
    let entry = json!([{"ID": "x", "ConnectionName": "", "Driver": "PostgreSQL",
        "DatabaseHost": "db", "DatabasePort": port, "DatabaseName": "app", "DatabaseUser": "me"}]);
    tableplus_candidates(&entry).unwrap().remove(0)
}

fn one_dbeaver(port: Value) -> ImportCandidate {
    let file = json!({"connections": {"k": {"provider": "postgresql", "name": "N",
        "configuration": {"host": "db", "port": port, "database": "app", "user": "me"}}}});
    dbeaver_candidates(file.to_string().as_bytes())
        .unwrap()
        .remove(0)
}

#[test]
fn a_port_that_isnt_a_number_is_a_problem() {
    for port in [
        json!("abc"),
        json!("${port}"),
        json!("-1"),
        json!("65536"),
        json!(70000),
        json!("1".repeat(400)),
        json!(true),
    ] {
        for c in [one_tableplus(port.clone()), one_dbeaver(port.clone())] {
            assert_eq!(c.problem, Some(ImportProblem::InvalidPort), "{port}");
            assert_eq!(c.port, 0, "{port}");
        }
    }
    for (port, want) in [
        (json!("65535"), 65535),
        (json!("0"), 0),
        (json!("-0"), 0),
        (json!(" 15432"), 15432),
        (json!("1e3"), 1),
        (json!(6544), 6544),
        (json!(5432.9), 5432),
        (json!([5433]), 5433),
        (json!(""), 5432),
    ] {
        let d = one_dbeaver(port.clone());
        assert_eq!((d.port, d.problem), (want, None), "dbeaver {port}");
    }
    // TablePlus takes a number or a boolean as its text, and nothing else.
    for (port, want, problem) in [
        (json!(5433), 5433, None),
        (json!(""), 5432, None),
        (json!(null), 5432, None),
        (json!([5433]), 5432, None),
        (json!({"a": 1}), 5432, None),
        (json!(false), 0, Some(ImportProblem::InvalidPort)),
    ] {
        let t = one_tableplus(port.clone());
        assert_eq!((t.port, t.problem), (want, problem), "tableplus {port}");
    }
    // The name falls back to today's `host:port` text, `NaN` included.
    assert_eq!(one_tableplus(json!("abc")).name, "db:NaN");
    assert_eq!(one_tableplus(json!("-1")).name, "db:-1");
    assert_eq!(one_tableplus(json!("-0")).name, "db:0");
    // A port problem never hides an id problem.
    let both = json!([
        {"ID": "d", "Driver": "PostgreSQL", "DatabasePort": "x"},
        {"ID": "d", "Driver": "PostgreSQL", "DatabasePort": "x"},
        {"Driver": "PostgreSQL", "DatabasePort": "x"},
    ]);
    let c = tableplus_candidates(&both).unwrap();
    let got: Vec<_> = c
        .iter()
        .map(|c| (c.key.as_str(), c.problem, c.port))
        .collect();
    assert_eq!(
        got,
        [
            ("pos:0", Some(ImportProblem::DuplicateId), 0),
            ("pos:1", Some(ImportProblem::DuplicateId), 0),
            ("pos:2", Some(ImportProblem::NoId), 0),
        ]
    );
}

fn identity(id: &str, ty: &str, host: &str, port: f64, db: &str, user: &str) -> ConnectionIdentity {
    serde_json::from_value(json!({"id": id, "type": ty, "host": host, "port": port,
        "databaseName": db, "username": user}))
    .unwrap()
}

#[test]
fn duplicates_compare_the_five_fields() {
    // Text, not `json!`, which would sort the keys.
    let file = r#"{"connections": {
        "same": {"provider": "postgresql", "name": "A", "configuration": {"host": "h", "port": "5432", "database": "d", "user": "u"}},
        "type": {"provider": "mysql", "name": "A", "configuration": {"host": "h", "port": "5432", "database": "d", "user": "u"}},
        "host": {"provider": "postgresql", "name": "A", "configuration": {"host": "H", "port": "5432", "database": "d", "user": "u"}},
        "port": {"provider": "postgresql", "name": "A", "configuration": {"host": "h", "port": "5433", "database": "d", "user": "u"}},
        "db": {"provider": "postgresql", "name": "A", "configuration": {"host": "h", "port": "5432", "database": "D", "user": "u"}},
        "user": {"provider": "postgresql", "name": "A", "configuration": {"host": "h", "port": "5432", "database": "d", "user": "U"}},
        "name": {"provider": "postgresql", "name": "Other name", "configuration": {"host": "h", "port": "5432", "database": "d", "user": "u"}},
        "bad-port": {"provider": "postgresql", "name": "A", "configuration": {"host": "h", "port": "x", "database": "d", "user": "u"}}
    }}"#;
    let mut c = dbeaver_candidates(file.as_bytes()).unwrap();
    let existing = [
        identity("zero", "postgres", "h", 0.0, "d", "u"),
        identity("first", "postgres", "h", 5432.0, "d", "u"),
        identity("second", "postgres", "h", 5432.0, "d", "u"),
    ];
    mark_duplicates(&mut c, &existing);
    let got: Vec<_> = c
        .iter()
        .map(|c| (c.key.as_str(), c.duplicate_of.as_deref()))
        .collect();
    assert_eq!(
        got,
        [
            ("same", Some("first")),
            ("type", None),
            ("host", None),
            ("port", None),
            ("db", None),
            ("user", None),
            ("name", Some("first")),
            // An invalid port is 0 on the wire, but matches nothing, as
            // today's `NaN` matched nothing.
            ("bad-port", None),
        ]
    );
    // Marking again with nothing saved clears the marks.
    mark_duplicates(&mut c, &[]);
    assert!(c.iter().all(|c| c.duplicate_of.is_none()));
    // A candidate's own identity matches it, so Core can check one import
    // against the ones made before it in the same call.
    let mut again = c.clone();
    let made = [ConnectionIdentity::of_candidate("new-1", &c[0])];
    mark_duplicates(&mut again, &made);
    assert_eq!(again[0].duplicate_of.as_deref(), Some("new-1"));
    assert_eq!(again[6].duplicate_of.as_deref(), Some("new-1"));
}

#[test]
fn refused_keys_are_the_ones_with_problems() {
    let entries = json!([
        {"ID": "ok", "Driver": "PostgreSQL"},
        {"ID": "bad", "Driver": "PostgreSQL", "DatabasePort": "x"},
        {"Driver": "PostgreSQL"},
    ]);
    let c = tableplus_candidates(&entries).unwrap();
    let keys = |k: &[&str]| k.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(refused_key(&c, &keys(&["id:ok"])), None);
    assert_eq!(refused_key(&c, &keys(&["id:gone"])), None);
    assert_eq!(
        refused_key(&c, &keys(&["id:ok", "id:bad", "pos:2"])),
        Some(("id:bad", ImportProblem::InvalidPort))
    );
    assert_eq!(
        refused_key(&c, &keys(&["pos:2"])),
        Some(("pos:2", ImportProblem::NoId))
    );
    assert_eq!(ImportProblem::InvalidPort.as_str(), "invalidPort");
    assert_eq!(ImportProblem::NoId.as_str(), "noId");
    assert_eq!(ImportProblem::DuplicateId.as_str(), "duplicateId");
}

/// Plist values are typed: numbers and booleans where the TypeScript took
/// text become their JavaScript text (`toTablePlusConnection`'s `str`), and
/// `tLSMode` goes through `Number()`.
#[test]
fn tableplus_values_read_like_the_typescripts() {
    let entries = json!([
        {"ID": 1.5, "ConnectionName": 1e21, "Driver": "PostgreSQL", "DatabaseName": false,
         "DatabaseUser": -0.0, "DatabasePort": 5433.0},
        {"ID": "t-bool", "Driver": "MySQL", "tLSMode": true},
        {"ID": "t-false", "Driver": "MySQL", "tLSMode": false},
        {"ID": "t-space", "Driver": "MySQL", "tLSMode": " 2 "},
        {"ID": "t-hex", "Driver": "MySQL", "tLSMode": "0x1"},
        {"ID": "t-empty", "Driver": "MySQL", "tLSMode": ""},
        {"ID": "t-array", "Driver": "MySQL", "tLSMode": [[2]]},
        {"ID": "t-empty-array", "Driver": "MySQL", "tLSMode": []},
        {"ID": "t-two", "Driver": "MySQL", "tLSMode": [1, 2]},
        {"ID": "t-object", "Driver": "MySQL", "tLSMode": {}},
        {"ID": "t-null", "Driver": "MySQL", "tLSMode": null},
        {"ID": "t-inf", "Driver": "MySQL", "tLSMode": "inf"},
        {"ID": "t-float", "Driver": "MariaDB", "tLSMode": 2.0},
        {"ID": "ssh-neg", "Driver": "PostgreSQL", "isOverSSH": true, "ServerAddress": "b",
         "ServerPort": "-5", "ServerUser": 7, "isUsePrivateKey": "true"},
        {"ID": "ssh-zero", "Driver": "PostgreSQL", "isOverSSH": true, "ServerAddress": "b", "ServerPort": "0"},
        {"ID": "ssh-text-flag", "Driver": "PostgreSQL", "isOverSSH": 1, "ServerAddress": "b"},
        ["an", "array"],
        {"ID": "", "Driver": "PostgreSQL"},
        {"ID": ["x"], "Driver": "PostgreSQL"},
        {"ID": "redis", "Driver": "Redis"},
        {"ID": "proto", "Driver": "toString"},
    ]);
    let c = tableplus_candidates(&entries).unwrap();
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v[0]["key"], "id:1.5");
    assert_eq!(v[0]["name"], "1e+21");
    assert_eq!(v[0]["databaseName"], "false");
    assert_eq!(v[0]["username"], "0");
    assert_eq!(v[0]["port"], 5433);
    let ssl: Vec<_> = c[1..13].iter().map(|c| c.ssl_mode.as_deref()).collect();
    assert_eq!(
        ssl,
        [
            Some("disable"),
            Some("prefer"),
            Some("require"),
            Some("disable"),
            Some("prefer"),
            Some("require"),
            Some("prefer"),
            None,
            None,
            None,
            None,
            Some("require"),
        ]
    );
    assert_eq!(
        v[13]["sshTunnel"],
        json!({"enabled": true, "host": "b", "port": 0, "username": "7", "authMethod": "password"})
    );
    // `parseInt` keeps `-5`, which `connectionCreate` refuses.
    assert_eq!(v[13]["problem"], "invalidSshPort");
    assert_eq!(v[14]["sshTunnel"]["port"], 22);
    assert!(v[15].get("sshTunnel").is_none());
    // The array is skipped; an empty or non-text `ID` is no ID.
    assert_eq!(v[16]["key"], "pos:17");
    assert_eq!(v[16]["problem"], "noId");
    assert_eq!(v[17]["key"], "pos:18");
    assert_eq!(v[17]["problem"], "noId");
    assert_eq!(c.len(), 18, "unsupported drivers are left out");
}

/// DBeaver's connections come in `Object.entries` order: array-index keys
/// first, ascending, then the rest as written; a repeated key keeps its
/// first place and its last value (`JSON.parse`). The provider is compared
/// in lower case.
#[test]
fn dbeaver_reads_like_the_typescript() {
    let text = r#"{"connections": {
        "b": {"provider": "postgresql", "name": "B1"},
        "10": {"provider": "PostgreSQL", "name": "Ten"},
        "a": {"provider": "MSSQL", "name": "A"},
        "2": {"provider": "DuckDB", "name": "Two", "configuration": {"database": "/x.duckdb"}},
        "02": {"provider": "sqlite", "name": "Zero two"},
        "4294967295": {"provider": "mysql", "name": "Big"},
        "b": {"provider": "mariadb", "name": "B2"},
        "skip-me": {"provider": "oracle", "name": "O"},
        "no-provider": {"name": "N"},
        "not-text": {"provider": 5, "name": "N"},
        "a-string": "postgresql",
        "kelvin": {"provider": "ducKdb", "name": "K"},
        "odd": {"provider": "postgres", "name": 12, "id": "ignored",
                "configuration": {"host": 0, "user": true, "database": ["a", null, 1]}},
        "config-text": {"provider": "postgres", "name": "C", "configuration": "x"}
    }, "connections2": 1}"#;
    let c = dbeaver_candidates(text.as_bytes()).unwrap();
    let keys: Vec<_> = c.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "2",
            "10",
            "b",
            "a",
            "02",
            "4294967295",
            "kelvin",
            "odd",
            "config-text"
        ]
    );
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v[1]["type"], "postgres");
    assert_eq!(v[2]["name"], "B2");
    assert_eq!(v[2]["type"], "mariadb");
    assert_eq!(v[3]["type"], "mssql");
    assert_eq!(v[3]["port"], 1433);
    assert_eq!(v[0]["type"], "duckdb");
    assert_eq!(v[0]["port"], 0);
    assert_eq!(v[6]["type"], "duckdb");
    assert_eq!(v[7]["name"], "12");
    assert_eq!(v[7]["host"], "localhost");
    assert_eq!(v[7]["username"], "true");
    assert_eq!(v[7]["databaseName"], "a,,1");
    assert_eq!(v[8]["host"], "localhost");
    assert!(c
        .iter()
        .all(|c| c.ssl_mode.is_none() && c.ssh_tunnel.is_none()));

    // `connections` as an array is read by index, as `Object.entries` does.
    let arr = r#"{"connections": [{"provider": "postgres", "name": "Zero"}, "x", {"provider": "mysql", "name": "Two"}]}"#;
    let keys: Vec<_> = dbeaver_candidates(arr.as_bytes())
        .unwrap()
        .into_iter()
        .map(|c| c.key)
        .collect();
    assert_eq!(keys, ["0", "2"]);

    for (text, want) in [
        ("{}", Ok(0)),
        (r#"{"connections": null}"#, Ok(0)),
        (r#"{"connections": 5}"#, Ok(0)),
        (r#"{"connections": "ab"}"#, Ok(0)),
        (
            r#"{"connections": {}, "connections": {"k": {"provider": "postgres"}}}"#,
            Ok(1),
        ),
        ("", Err(ImportError::NotJson)),
        ("{", Err(ImportError::NotJson)),
        ("{} x", Err(ImportError::NotJson)),
        ("\u{feff}{}", Err(ImportError::NotJson)),
        ("null", Err(ImportError::NotAnObject)),
        ("[]", Err(ImportError::NotAnObject)),
        ("\"s\"", Err(ImportError::NotAnObject)),
    ] {
        assert_eq!(
            dbeaver_candidates(text.as_bytes()).map(|c| c.len()),
            want,
            "{text:?}"
        );
    }
    assert_eq!(
        dbeaver_candidates(b"{\"connections\": \xff}"),
        Err(ImportError::NotJson)
    );
    assert_eq!(
        tableplus_candidates(&json!({"ID": "x"})).map(|c| c.len()),
        Err(ImportError::NotAList)
    );
    for e in [
        ImportError::NotJson,
        ImportError::NotAList,
        ImportError::NotAnObject,
    ] {
        let m = e.to_string();
        assert!(m.len() > 10 && !m.contains('/'), "{m}");
    }
}

#[test]
fn default_paths_are_todays() {
    assert_eq!(
        default_path(ImportSource::Tableplus, "macos"),
        Some("Library/Application Support/com.tinyapp.TablePlus/Data/Connections.plist")
    );
    assert_eq!(default_path(ImportSource::Tableplus, "linux"), None);
    assert_eq!(default_path(ImportSource::Tableplus, "windows"), None);
    assert_eq!(
        default_path(ImportSource::Dbeaver, "macos"),
        Some("Library/DBeaverData/workspace6/General/.dbeaver/data-sources.json")
    );
    assert_eq!(
        default_path(ImportSource::Dbeaver, "windows"),
        Some("AppData/Roaming/DBeaverData/workspace6/General/.dbeaver/data-sources.json")
    );
    assert_eq!(
        default_path(ImportSource::Dbeaver, "linux"),
        Some(".local/share/DBeaverData/workspace6/General/.dbeaver/data-sources.json")
    );
    assert_eq!(default_path(ImportSource::Dbeaver, "freebsd"), None);
}

/// A small deterministic generator (xorshift), so the fuzzing needs no new
/// dependency and fails the same way every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

const KEYS: [&str; 18] = [
    "ID",
    "ConnectionName",
    "Driver",
    "DatabaseHost",
    "DatabasePort",
    "DatabaseName",
    "DatabaseUser",
    "DatabasePath",
    "tLSMode",
    "isOverSSH",
    "ServerAddress",
    "ServerPort",
    "ServerUser",
    "isUsePrivateKey",
    "provider",
    "name",
    "configuration",
    "connections",
];
const TEXTS: [&str; 22] = [
    "",
    " ",
    "0",
    "-0",
    "2",
    "1e3",
    "65535",
    "65536",
    "-1",
    "abc",
    "${port}",
    "0x2",
    "PostgreSQL",
    "MySQL",
    "SQLite",
    "SQL Server",
    "postgresql",
    "DUCKDB",
    "\u{0}",
    "🦀",
    "\u{feff}7",
    "99999999999999999999999999",
];

fn value(r: &mut Rng, depth: u32) -> Value {
    let kind = if depth == 0 { r.below(5) } else { r.below(7) };
    match kind {
        0 => Value::Null,
        1 => json!(r.below(2) == 0),
        2 => match r.below(5) {
            0 => json!(r.next() as i64),
            1 => json!(r.below(70000)),
            2 => json!(f64::from_bits(r.next()))
                .as_f64()
                .map_or(json!(1.5), |f| json!(f)),
            3 => json!(-0.0),
            _ => json!(u64::MAX),
        },
        3 | 4 => json!(*r.pick(&TEXTS)),
        5 => Value::Array((0..r.below(4)).map(|_| value(r, depth - 1)).collect()),
        _ => Value::Object(
            (0..r.below(8))
                .map(|_| (r.pick(&KEYS).to_string(), value(r, depth - 1)))
                .collect(),
        ),
    }
}

#[test]
fn never_panics() {
    let mut r = Rng(0x5eed_1a4a);
    let existing = [identity("c", "postgres", "localhost", 5432.0, "", "")];
    for _ in 0..20_000 {
        let v = value(&mut r, 4);
        let entries = Value::Array((0..r.below(5)).map(|_| value(&mut r, 3)).collect());
        for input in [&v, &entries] {
            if let Ok(mut c) = tableplus_candidates(input) {
                mark_duplicates(&mut c, &existing);
                let _ = refused_key(&c, &[String::new(), "pos:0".into(), "id:".into()]);
                let _ = serde_json::to_string(&ImportCandidates::from_result(Ok(c))).unwrap();
            }
        }
        let doc = json!({"connections": v});
        for bytes in [doc.to_string().into_bytes(), v.to_string().into_bytes()] {
            if let Ok(mut c) = dbeaver_candidates(&bytes) {
                mark_duplicates(&mut c, &existing);
                let _ = serde_json::to_string(&c).unwrap();
            }
            // Cut short and with a byte flipped.
            let mut cut = bytes.clone();
            cut.truncate(r.below(bytes.len() as u64 + 1) as usize);
            let _ = dbeaver_candidates(&cut);
            let mut flipped = bytes.clone();
            if !flipped.is_empty() {
                let i = r.below(flipped.len() as u64) as usize;
                flipped[i] ^= 1 << r.below(8);
            }
            let _ = dbeaver_candidates(&flipped);
        }
        let _ = parse_int(r.pick(&TEXTS));
        // Escapes in keys and values, lone surrogates and numbers past
        // f64's range, spliced into a document.
        let piece = *r.pick(&[
            r#""\ud800""#,
            r#""\udfff""#,
            r#""\ud83e\udd80""#,
            r#""\u0000""#,
            r#""\"""#,
            "1e400",
            "-1e999",
            "1e-400",
        ]);
        let doc = match r.below(4) {
            0 => format!(r#"{{"connections": {{{piece}: {{"provider": "postgres"}}}}}}"#),
            1 => format!(
                r#"{{"connections": {{"k": {{"provider": "postgres", {piece}: 1, "name": {piece}}}}}}}"#
            ),
            2 => format!(
                r#"{{"connections": {{"k": {{"provider": {piece}, "configuration": {{"host": {piece}, "port": {piece}, {piece}: 2}}}}}}}}"#
            ),
            _ => format!(
                r#"{{{piece}: 1, "connections": [{piece}, {{"name": {piece}, "provider": "mysql"}}]}}"#
            ),
        };
        if let Ok(mut c) = dbeaver_candidates(doc.as_bytes()) {
            mark_duplicates(&mut c, &existing);
            let _ = serde_json::to_string(&c).unwrap();
        }
    }
    // Deep nesting and large inputs.
    let mut deep = json!("2");
    for _ in 0..1_000 {
        deep = Value::Array(vec![deep]);
    }
    let mut entry = serde_json::Map::new();
    entry.insert("ID".into(), json!("d"));
    entry.insert("Driver".into(), json!("MySQL"));
    entry.insert("ConnectionName".into(), deep.clone());
    entry.insert("tLSMode".into(), deep);
    let deep_entry = Value::Array(vec![Value::Object(entry)]);
    assert_eq!(
        tableplus_candidates(&deep_entry).unwrap()[0]
            .ssl_mode
            .as_deref(),
        Some("require")
    );
    let nested = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert_eq!(
        dbeaver_candidates(nested.as_bytes()),
        Err(ImportError::NotAnObject)
    );
    for doc in [
        format!(r#"{{"connections": {{"k": {nested}}}}}"#),
        format!(r#"{{"connections": {nested}}}"#),
    ] {
        assert_eq!(dbeaver_candidates(doc.as_bytes()), Ok(Vec::new()));
    }
    // A configuration too deep to decode has no fields; the connection
    // stays (review M2), and has no name.
    let deep_config = format!(
        r#"{{"connections": {{"k": {{"provider": "postgres", "configuration": {nested}}}}}}}"#
    );
    let c = dbeaver_candidates(deep_config.as_bytes()).unwrap();
    assert_eq!(
        (c.len(), c[0].host.as_str(), c[0].problem),
        (1, "localhost", Some(ImportProblem::NoName))
    );
    let _ = parse_int(&"9".repeat(1_000_000));
    let _ = dbeaver_candidates(&[0xff; 64]);
}

#[test]
fn debug_shows_no_host_name_or_user() {
    let c = "CANARY-4a1f";
    let entries = json!([{"ID": c, "ConnectionName": c, "Driver": "PostgreSQL", "DatabaseHost": c,
        "DatabaseName": c, "DatabaseUser": c, "isOverSSH": true, "ServerAddress": c, "ServerUser": c}]);
    let mut list = tableplus_candidates(&entries).unwrap();
    mark_duplicates(&mut list, &[identity("c1", "postgres", c, 5432.0, c, c)]);
    let file = json!({"connections": {c: {"provider": "postgres", "name": c,
        "configuration": {"host": c, "database": c, "user": c}}}});
    let d = dbeaver_candidates(file.to_string().as_bytes()).unwrap();
    let out = format!(
        "{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
        list,
        d,
        ImportCandidates::from_result(Ok(list.clone())),
        ImportCandidates::unreadable(c),
        identity("c1", "postgres", c, 1.0, c, c),
        ImportError::NotJson,
        ImportSource::Tableplus,
    );
    assert!(!out.contains("CANARY"), "{out}");
    // It still says something useful.
    assert!(
        out.contains("postgres") && out.contains("ImportCandidate"),
        "{out}"
    );
}

/// Coordinator's decision (Decision 47): a TablePlus SSH port that isn't a
/// whole number from 0 to 65535 is `invalidSshPort`, with the tunnel's port
/// 0, since `connectionCreate` would refuse it. `parseInt(…) || 22` still
/// makes `NaN` and 0 into 22. Id problems and `invalidPort` come first. No
/// recorded case has such a port.
#[test]
fn an_ssh_port_out_of_range_is_a_problem() {
    let tunnel = |id: &str, port: Value, db_port: &str| {
        json!({"ID": id, "ConnectionName": "A", "Driver": "PostgreSQL", "DatabasePort": db_port,
            "isOverSSH": true, "ServerAddress": "b", "ServerUser": "u", "ServerPort": port})
    };
    let entries = json!([
        tunnel("neg", json!("-5"), "5432"),
        tunnel("big", json!("65536"), "5432"),
        tunnel("far", json!(70000), "5432"),
        tunnel("inf", json!("9".repeat(400)), "5432"),
        tunnel("max", json!("65535"), "5432"),
        tunnel("nan", json!("ssh"), "5432"),
        tunnel("zero", json!("0"), "5432"),
        tunnel("both", json!("-5"), "x"),
        tunnel("dup", json!("-5"), "5432"),
        tunnel("dup", json!("-5"), "5432"),
    ]);
    let c = tableplus_candidates(&entries).unwrap();
    let got: Vec<_> = c
        .iter()
        .map(|c| {
            (
                c.key.as_str(),
                c.problem.map(ImportProblem::as_str),
                c.ssh_tunnel.as_ref().map(|t| t.port),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("id:neg", Some("invalidSshPort"), Some(0.0)),
            ("id:big", Some("invalidSshPort"), Some(0.0)),
            ("id:far", Some("invalidSshPort"), Some(0.0)),
            ("id:inf", Some("invalidSshPort"), Some(0.0)),
            ("id:max", None, Some(65535.0)),
            ("id:nan", None, Some(22.0)),
            ("id:zero", None, Some(22.0)),
            ("id:both", Some("invalidPort"), Some(0.0)),
            ("pos:8", Some("duplicateId"), Some(0.0)),
            ("pos:9", Some("duplicateId"), Some(0.0)),
        ]
    );
    // On the wire, and refused by the create.
    let v = serde_json::to_value(&c[0]).unwrap();
    assert_eq!(v["problem"], "invalidSshPort");
    assert_eq!(v["sshTunnel"]["port"], 0);
    assert_eq!(
        refused_key(&c, &["id:neg".to_string()]),
        Some(("id:neg", ImportProblem::InvalidSshPort))
    );
}

/// Coordinator's decision (Decision 47): a DBeaver connection whose name is
/// missing, `null` or blank after JavaScript's trim is `noName`, since
/// `connectionCreate` refuses an empty name. `invalidPort` comes first. No
/// recorded case has such a connection.
#[test]
fn a_dbeaver_connection_without_a_name_is_a_problem() {
    let text = r#"{"connections": {
        "missing": {"provider": "postgres"},
        "null": {"provider": "postgres", "name": null},
        "empty": {"provider": "postgres", "name": ""},
        "blank": {"provider": "postgres", "name": "  　 "},
        "false": {"provider": "postgres", "name": false},
        "zero": {"provider": "postgres", "name": 0},
        "named": {"provider": "postgres", "name": "N"},
        "both": {"provider": "postgres", "configuration": {"port": "x"}}
    }}"#;
    let c = dbeaver_candidates(text.as_bytes()).unwrap();
    let got: Vec<_> = c
        .iter()
        .map(|c| {
            (
                c.key.as_str(),
                c.name.as_str(),
                c.problem.map(ImportProblem::as_str),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("missing", "", Some("noName")),
            ("null", "", Some("noName")),
            ("empty", "", Some("noName")),
            ("blank", " \u{a0}\u{3000} ", Some("noName")),
            ("false", "false", None),
            ("zero", "0", None),
            ("named", "N", None),
            ("both", "", Some("invalidPort")),
        ]
    );
    assert_eq!(
        refused_key(&c, &["missing".to_string()]),
        Some(("missing", ImportProblem::NoName))
    );
}

/// Review M1: a `connections` object or array that doesn't decode (a key
/// with a lone surrogate escape, which `serde_json` can't hold) is
/// unreadable, never an empty list.
#[test]
fn undecodable_connections_are_unreadable() {
    for text in [
        r#"{"connections": {"\ud800": {"provider": "postgres", "name": "A"}, "ok": {"provider": "postgres", "name": "B"}}}"#,
        r#"{"connections": {"ok": {"provider": "postgres", "name": "B"}, "\udfff": 1}}"#,
    ] {
        assert_eq!(
            dbeaver_candidates(text.as_bytes()),
            Err(ImportError::NotJson),
            "{text}"
        );
    }
    // An escaped key that does decode is that key.
    let ok = r#"{"connections": {"A🦀": {"provider": "postgres", "name": "A"}}}"#;
    assert_eq!(dbeaver_candidates(ok.as_bytes()).unwrap()[0].key, "A🦀");
}

/// Review M2: only `provider`, `name` and `configuration` (and in it
/// `host`, `port`, `database` and `user`) are read, so a number past f64's
/// range or deep nesting anywhere else keeps the connection, as in
/// JavaScript. Such a number in a field that is read is `Infinity`.
#[test]
fn fields_that_arent_read_never_drop_a_connection() {
    let deep = format!("{}{}", "[".repeat(5_000), "]".repeat(5_000));
    let text = format!(
        r#"{{"connections": {{
        "big": {{"provider": "postgres", "name": "Big", "save-password": 1e400,
                 "configuration": {{"host": "h", "port": "5432", "handlers": {{"x": {deep}}}, "z": -1e999}}}},
        "deep": {{"provider": "postgres", "name": "Deep", "folder": {deep}}},
        "inf-port": {{"provider": "postgres", "name": "P", "configuration": {{"port": 1e400}}}},
        "inf-name": {{"provider": "postgres", "name": 1e400}},
        "neg-host": {{"provider": "postgres", "name": "H", "configuration": {{"host": -1e400}}}},
        "repeated": {{"provider": "oracle", "name": "Old", "provider": "postgres", "name": "New"}},
        "bad-provider": {{"provider": "\ud800", "name": "S"}},
        "bad-config": {{"provider": "postgres", "name": "C", "configuration": {{"host": "\ud800", "user": "u"}}}}
    }}}}"#
    );
    let c = dbeaver_candidates(text.as_bytes()).unwrap();
    let got: Vec<_> = c
        .iter()
        .map(|c| {
            (
                c.key.as_str(),
                c.name.as_str(),
                c.host.as_str(),
                c.port,
                c.username.as_str(),
                c.problem.map(ImportProblem::as_str),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("big", "Big", "h", 5432, "", None),
            ("deep", "Deep", "localhost", 5432, "", None),
            ("inf-port", "P", "localhost", 0, "", Some("invalidPort")),
            ("inf-name", "Infinity", "localhost", 5432, "", None),
            ("neg-host", "H", "-Infinity", 5432, "", None),
            ("repeated", "New", "localhost", 5432, "", None),
            // A field that can't be decoded at all (a lone surrogate) is
            // read as absent.
            ("bad-config", "C", "localhost", 5432, "u", None),
        ]
    );
}

/// Review M3: a driver or provider named like an `Object.prototype` member
/// is unsupported (the TypeScript's map lookup returned the inherited
/// function).
#[test]
fn prototype_names_are_unsupported() {
    for name in [
        "toString",
        "constructor",
        "__proto__",
        "hasOwnProperty",
        "valueOf",
    ] {
        let tp = json!([{"ID": "x", "Driver": name}]);
        assert_eq!(tableplus_candidates(&tp).unwrap().len(), 0, "{name}");
        let lower = name.to_lowercase();
        let db = format!(
            r#"{{"connections": {{"k": {{"provider": "{name}", "name": "N"}}, "l": {{"provider": "{lower}", "name": "N"}}}}}}"#
        );
        assert_eq!(
            dbeaver_candidates(db.as_bytes()).unwrap().len(),
            0,
            "{name}"
        );
    }
}
