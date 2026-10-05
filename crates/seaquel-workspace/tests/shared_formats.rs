//! Replays `fixtures/shared/formats.json` (with `changes.json` applied)
//! through `seaquel_workspace::shared`'s readers, writers and file names,
//! and checks the formats' own properties: every value the writer writes
//! reads back unchanged, older releases' reader (ported below) reads what
//! Core writes, and the canonical hash ignores formatting.

use std::collections::HashMap;

use seaquel_types::storage::PersistedQueryParameter;
use seaquel_workspace::shared::format::{
    dashboard_content, parse_dashboard, parse_labels, parse_project, parse_query, parse_template,
    query_content, template_content, write_dashboard, write_labels, write_project, write_query,
    write_template, DashboardFile, LabelFile, ProjectFile, QueryFile, TemplateFile, TemplateSsh,
};
use seaquel_workspace::shared::names::{file_stem, free_path, path_key, TakenPaths};
use seaquel_workspace::shared::{content_hash, MAX_FILE_NAME_BYTES};
use serde_json::{json, Map, Value};

const QUERIES_DIR: &str = ".seaquel/projects/team/queries";

/// The id a write case's reparse gives the file, as Core always writes one.
const CORE_ID: &str = "00000000-0000-4000-8000-0000000000c0";

fn raw_cases() -> Vec<Box<serde_json::value::RawValue>> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/shared");
    serde_json::from_str(&std::fs::read_to_string(format!("{dir}/formats.json")).unwrap()).unwrap()
}

fn fixtures() -> (Vec<Value>, Map<String, Value>) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/shared");
    let formats: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}/formats.json")).unwrap())
            .unwrap();
    let changes: Map<String, Value> =
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}/changes.json")).unwrap())
            .unwrap();
    (formats, changes)
}

/// A number as `JSON.stringify` would hand it over: whole numbers as
/// integers.
fn num(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

fn param_json(p: &PersistedQueryParameter) -> Value {
    let mut o = Map::new();
    o.insert("name".into(), json!(p.name));
    o.insert("type".into(), json!(p.ty));
    if let Some(d) = &p.default_value {
        o.insert("defaultValue".into(), json!(d));
    }
    if let Some(d) = &p.description {
        o.insert("description".into(), json!(d));
    }
    Value::Object(o)
}

/// A parsed query in today's shape, without `id`, `repoId` (the scan's)
/// and `filePath` (the caller's own input).
fn query_json(q: &QueryFile) -> Value {
    let mut o = Map::new();
    o.insert("name".into(), json!(q.name));
    if let Some(d) = &q.description {
        o.insert("description".into(), json!(d));
    }
    o.insert("query".into(), json!(q.query));
    if !q.parameters.is_empty() {
        o.insert(
            "parameters".into(),
            Value::Array(q.parameters.iter().map(param_json).collect()),
        );
    }
    if let Some(d) = &q.database {
        o.insert("databaseType".into(), json!(d));
    }
    o.insert("tags".into(), json!(q.tags));
    o.insert("folder".into(), json!(q.folder));
    if let Some(id) = &q.file_id {
        o.insert("fileId".into(), json!(id));
    }
    Value::Object(o)
}

fn dashboard_json(d: &DashboardFile) -> Value {
    let mut o = Map::new();
    o.insert("name".into(), json!(d.name));
    if let Some(desc) = &d.description {
        o.insert("description".into(), json!(desc));
    }
    o.insert(
        "widgets".into(),
        serde_json::from_str(&d.widgets).expect("widgets are JSON"),
    );
    if let Some(v) = &d.viewport {
        o.insert(
            "viewport".into(),
            serde_json::from_str(v).expect("viewport is JSON"),
        );
    }
    o.insert(
        "dateFilter".into(),
        d.date_filter
            .as_deref()
            .map_or(Value::Null, |f| serde_json::from_str(f).unwrap()),
    );
    if let Some(id) = &d.file_id {
        o.insert("fileId".into(), json!(id));
    }
    Value::Object(o)
}

fn template_json(t: &TemplateFile) -> Value {
    let mut o = Map::new();
    o.insert("name".into(), json!(t.name));
    o.insert("type".into(), json!(t.ty));
    o.insert("host".into(), json!(t.host));
    o.insert("port".into(), num(t.port));
    o.insert("databaseName".into(), json!(t.database_name));
    if let Some(s) = &t.ssl_mode {
        o.insert("sslMode".into(), json!(s));
    }
    if let Some(ssh) = &t.ssh_tunnel {
        let mut s = Map::new();
        s.insert("enabled".into(), json!(true));
        if let Some(h) = &ssh.host {
            s.insert("host".into(), json!(h));
        }
        if let Some(p) = ssh.port {
            s.insert("port".into(), num(p));
        }
        o.insert("sshTunnel".into(), Value::Object(s));
    }
    o.insert("labels".into(), json!(t.labels));
    if let Some(id) = &t.file_id {
        o.insert("fileId".into(), json!(id));
    }
    Value::Object(o)
}

fn project_json(p: &ProjectFile) -> Value {
    let mut o = Map::new();
    o.insert("name".into(), json!(p.name));
    if let Some(d) = &p.description {
        o.insert("description".into(), json!(d));
    }
    o.insert("dirName".into(), json!(p.dir));
    o.insert("connections".into(), json!([]));
    Value::Object(o)
}

fn labels_json(labels: &[LabelFile]) -> Value {
    json!({ "labels": labels.iter().map(|l| json!({
        "id": l.id, "name": l.name, "isPredefined": false, "color": l.color,
    })).collect::<Vec<_>>() })
}

/// Today's output without the fields Core's types don't hold: the scan's
/// `id`, `repoId` and `projectId`, and `filePath`.
fn strip_scan_fields(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut o = o.clone();
            for k in ["id", "repoId", "projectId", "filePath"] {
                o.remove(k);
            }
            Value::Object(o)
        }
        other => other.clone(),
    }
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn query_from_args(q: &Value) -> QueryFile {
    QueryFile {
        name: str_of(q, "name").unwrap_or_default(),
        description: str_of(q, "description"),
        database: str_of(q, "databaseType"),
        tags: q
            .get("tags")
            .and_then(Value::as_array)
            .map(|t| {
                t.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        parameters: q
            .get("parameters")
            .map(|p| serde_json::from_value(p.clone()).unwrap())
            .unwrap_or_default(),
        query: str_of(q, "query").unwrap_or_default(),
        folder: String::new(),
        file_id: None,
    }
}

/// A dashboard case's arguments as raw JSON, so the widgets keep their key
/// order (`serde_json::Value` sorts keys).
#[derive(serde::Deserialize)]
struct RawDashboardCase {
    args: RawDashboardArgs,
}

#[derive(serde::Deserialize)]
struct RawDashboardArgs {
    dashboard: RawDashboard,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDashboard {
    name: String,
    description: Option<String>,
    widgets: Box<serde_json::value::RawValue>,
    viewport: Option<Box<serde_json::value::RawValue>>,
    date_filter: Option<Box<serde_json::value::RawValue>>,
}

fn dashboard_from_args(raw_case: &str) -> DashboardFile {
    let c: RawDashboardCase = serde_json::from_str(raw_case).unwrap();
    let d = c.args.dashboard;
    DashboardFile {
        name: d.name,
        description: d.description,
        widgets: d.widgets.get().to_string(),
        viewport: d.viewport.map(|v| v.get().to_string()),
        date_filter: d
            .date_filter
            .map(|f| f.get().to_string())
            .filter(|f| f != "null"),
        file_id: None,
    }
}

fn template_from_args(t: &Value) -> TemplateFile {
    TemplateFile {
        name: str_of(t, "name").unwrap_or_default(),
        ty: str_of(t, "type").unwrap_or_default(),
        host: str_of(t, "host").unwrap_or_default(),
        port: t["port"].as_f64().unwrap(),
        database_name: str_of(t, "databaseName").unwrap_or_default(),
        ssl_mode: str_of(t, "sslMode"),
        ssh_tunnel: t
            .get("sshTunnel")
            .filter(|s| s["enabled"].as_bool() == Some(true))
            .map(|s| TemplateSsh {
                host: str_of(s, "host"),
                port: s.get("port").and_then(Value::as_f64),
            }),
        labels: t
            .get("labels")
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        file_id: None,
    }
}

/// What a case must give: the recorded output, or `changes.json`'s.
fn expected(case: &Value, changes: &Map<String, Value>) -> (Value, Option<Value>) {
    let name = case["name"].as_str().unwrap();
    match changes.get(name).map(|c| &c["expected"]) {
        Some(e) => (
            e["output"].clone(),
            e.get("reparsed")
                .cloned()
                .or_else(|| case.get("reparsed").cloned()),
        ),
        None => (case["output"].clone(), case.get("reparsed").cloned()),
    }
}

#[test]
fn replays_every_format_case() {
    let (formats, changes) = fixtures();
    assert_eq!(formats.len(), 153);
    let mut failures = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let raw = raw_cases();
    for (case, raw_case) in formats.iter().zip(&raw) {
        let name = case["name"].as_str().unwrap();
        let func = case["fn"].as_str().unwrap();
        *counts.entry(func.to_string()).or_default() += 1;
        let args = &case["args"];
        let (want, want_reparsed) = expected(case, &changes);
        let (got, got_reparsed): (Value, Option<Value>) = match func {
            "parseQuery" => {
                let q = parse_query(
                    args["text"].as_str().unwrap(),
                    args["path"].as_str().unwrap(),
                    args["queriesDir"].as_str().unwrap(),
                );
                (query_json(&q), None)
            }
            "writeQuery" => {
                let q = query_from_args(&args["query"]);
                let text = write_query(&q);
                // Core writes its files with their id, and reads its
                // own escapes only in such a file: the reparse
                // reads the file as Core writes it.
                let with_id = write_query(&QueryFile {
                    file_id: Some(CORE_ID.into()),
                    ..q
                });
                let mut back = parse_query(&with_id, &format!("{QUERIES_DIR}/x.sql"), QUERIES_DIR);
                back.file_id = None;
                (json!(text), Some(query_json(&back)))
            }
            "parseDashboard" => {
                let d = parse_dashboard(
                    args["text"].as_str().unwrap(),
                    args["path"].as_str().unwrap(),
                );
                (d.as_ref().map_or(Value::Null, dashboard_json), None)
            }
            "writeDashboard" => {
                let text = write_dashboard(&dashboard_from_args(raw_case.get()));
                let back = parse_dashboard(&text, ".seaquel/projects/team/dashboards/x.json")
                    .expect("what Core writes parses");
                (json!(text), Some(dashboard_json(&back)))
            }
            "parseTemplate" => {
                let t = parse_template(args["text"].as_str().unwrap());
                (t.as_ref().map_or(Value::Null, template_json), None)
            }
            "writeTemplate" => {
                let t = template_from_args(&args["template"]);
                let text = write_template(&t);
                // As for queries: reread as Core writes it, with its id.
                let with_id = write_template(&TemplateFile {
                    file_id: Some(CORE_ID.into()),
                    ..t
                });
                let mut back = parse_template(&with_id).expect("what Core writes parses");
                back.file_id = None;
                (json!(text), Some(template_json(&back)))
            }
            "parseProject" => {
                let p = parse_project(
                    args["text"].as_str().unwrap(),
                    args["dir"].as_str().unwrap(),
                );
                (project_json(&p), None)
            }
            "writeProject" => {
                let p = &args["project"];
                let text = write_project(&ProjectFile {
                    name: str_of(p, "name").unwrap(),
                    description: str_of(p, "description"),
                    dir: "team".into(),
                });
                (
                    json!(text),
                    Some(project_json(&parse_project(&text, "team"))),
                )
            }
            "parseLabels" => (
                labels_json(&parse_labels(args["text"].as_str().unwrap())),
                None,
            ),
            "writeLabels" => {
                let labels: Vec<LabelFile> = args["labels"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| LabelFile {
                        id: str_of(l, "id").unwrap(),
                        name: str_of(l, "name").unwrap(),
                        color: str_of(l, "color").unwrap(),
                    })
                    .collect();
                let text = write_labels(&labels);
                (json!(text), Some(labels_json(&parse_labels(&text))))
            }
            "fileName" => {
                let stem = file_stem(args["name"].as_str().unwrap());
                (
                    json!({
                        "stem": stem,
                        "query": format!("{stem}.sql"),
                        "dashboard": format!("{stem}.json"),
                    }),
                    None,
                )
            }
            other => panic!("unknown fn {other}"),
        };
        let want = if func.starts_with("parse") {
            strip_scan_fields(&want)
        } else {
            want
        };
        if got != want {
            failures.push(format!("{name}: output\n  got  {got}\n  want {want}"));
        }
        if let (Some(got), Some(want)) = (got_reparsed, want_reparsed) {
            let want = strip_scan_fields(&want);
            if got != want {
                failures.push(format!("{name}: reparsed\n  got  {got}\n  want {want}"));
            }
        }
    }
    assert_eq!(counts["fileName"], 49);
    assert!(
        failures.is_empty(),
        "{} of 153 cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn file_stem_avoids_reserved_names() {
    for (name, stem) in [
        ("CON", "con-file"),
        ("prn", "prn-file"),
        ("Aux", "aux-file"),
        ("NUL", "nul-file"),
        ("com0", "com0-file"),
        ("COM9", "com9-file"),
        ("lpt0", "lpt0-file"),
        ("LPT9", "lpt9-file"),
        ("COM¹", "com¹-file"),
        ("lpt²", "lpt²-file"),
        ("lpt³", "lpt³-file"),
        // Not reserved: other words, longer names, a reserved word with more.
        ("console", "console"),
        ("com10", "com10"),
        ("con.sql", "con-sql"),
        ("aux file", "aux-file"),
        // Trailing dots and spaces never survive.
        ("report. ", "report"),
        ("...", "untitled"),
    ] {
        assert_eq!(file_stem(name), stem, "{name:?}");
    }
    // A cut never ends in `-` and stays within the byte cap.
    let long = format!("{}-b", "a".repeat(249));
    let s = file_stem(&long);
    assert!(s.len() <= 250 && !s.ends_with('-'), "{s}");
}

#[test]
fn free_path_is_case_insensitive() {
    let dir = ".seaquel/projects/team/queries";
    let taken = TakenPaths::from_paths([
        format!("{dir}/Sales.sql"),
        format!("{dir}/sales-2.SQL"),
        // NFD, as macOS writes it.
        format!("{dir}/a\u{308}rger.sql"),
    ]);
    let is_taken = |p: &str| taken.contains(p);
    assert_eq!(
        free_path(dir, "sales", ".sql", &is_taken),
        format!("{dir}/sales-3.sql")
    );
    assert_eq!(
        free_path(dir, "\u{e4}rger", ".sql", &is_taken),
        format!("{dir}/\u{e4}rger-2.sql")
    );
    assert_eq!(
        free_path(dir, "orders", ".sql", &is_taken),
        format!("{dir}/orders.sql")
    );
    assert_eq!(path_key("A/\u{c4}.SQL"), path_key("a/a\u{308}.sql"));
    // A `-n` suffix past 255 bytes cuts the stem again, on a character
    // boundary.
    let stem = file_stem(&"売".repeat(100));
    let first = free_path(dir, &stem, ".json", &|_| false);
    let name = first.rsplit('/').next().unwrap();
    assert!(name.len() <= MAX_FILE_NAME_BYTES);
    let second = free_path(dir, &stem, ".json", &|p| p == first);
    let name = second.rsplit('/').next().unwrap();
    assert!(name.len() <= MAX_FILE_NAME_BYTES, "{}", name.len());
    assert!(name.ends_with("-2.json"));
    assert_eq!(name.chars().filter(|c| *c == '売').count(), 82);
}

#[test]
fn canonical_hash_ignores_formatting() {
    // An older release's query file (no id, CRLF, a BOM, a trailing space
    // after the body, comments and unknown keys) hashes as Core's own.
    let core = write_query(&QueryFile {
        name: "Orders".into(),
        description: Some("All of them".into()),
        database: None,
        tags: vec!["sales".into()],
        parameters: vec![],
        query: "SELECT 1".into(),
        folder: String::new(),
        file_id: Some("1b4e28ba-2fa1-41d2-883f-0016dc9b6b30".into()),
    });
    let older = "\u{feff}---\r\n# by hand\r\nauthor: Dana\r\nname: Orders\r\ndescription: \"All of them\"\r\ntags: [sales]\r\n---\r\n\r\nSELECT 1  \r\n";
    let path = format!("{QUERIES_DIR}/orders.sql");
    let h = |t: &str| content_hash(&query_content(&parse_query(t, &path, QUERIES_DIR)));
    assert_eq!(h(&core), h(older));
    assert_ne!(h(&core), h(&core.replace("SELECT 1", "SELECT 2")));

    // A dashboard: key spacing and the viewport don't count; run state on a
    // widget is stripped.
    let a = "{\"name\":\"Sales\",\"widgets\":[{\"id\":\"w1\",\"x\":1}],\"viewport\":{\"x\":0,\"y\":0,\"zoom\":1}}";
    let b = "{\n  \"id\": \"1b4e28ba-2fa1-41d2-883f-0016dc9b6b30\",\n  \"name\": \"Sales\",\n  \"widgets\": [ { \"id\": \"w1\", \"x\": 1.0, \"result\": [1] } ],\n  \"viewport\": { \"x\": 50, \"y\": 9, \"zoom\": 2 }\n}\n";
    let path = ".seaquel/projects/team/dashboards/sales.json";
    let h = |t: &str| content_hash(&dashboard_content(&parse_dashboard(t, path).unwrap()));
    assert_eq!(h(a), h(b));

    // A template: labels, credentials and comments don't count.
    let a = "name: Warehouse\ntype: postgres\nhost: db\nport: 5432\ndatabaseName: w\n";
    let b = "# shared\nid: 1b4e28ba-2fa1-41d2-883f-0016dc9b6b30\nname: Warehouse\nhost: db\nusername: me\npassword: x\nport: 5432\ndatabaseName: w\nlabels: [Prod]\n";
    let h = |t: &str| content_hash(&template_content(&parse_template(t).unwrap()));
    assert_eq!(h(a), h(b));
}

#[test]
fn the_template_drops_credentials_on_read() {
    let t = parse_template(
        "name: W\nusername: me\npassword: hunter2\nconnectionString: postgres://me:pw@h/db\nsshPassword: x\nsshKeyPassphrase: y\nsshUsername: z\nhost: h\nsshTunnel:\n  enabled: true\n  host: b\n  sshPassword: p\n  password: q\n  username: u\n  port: 2222\n",
    )
    .unwrap();
    let text = write_template(&t);
    for secret in [
        "me",
        "hunter2",
        "postgres://",
        "x\n",
        "y\n",
        "z\n",
        "p\n",
        "q\n",
        "u\n",
    ] {
        assert!(
            !text.contains(&format!(": {secret}")),
            "{secret:?} leaked into {text:?}"
        );
    }
    assert_eq!(t.host, "h");
    assert_eq!(
        t.ssh_tunnel,
        Some(TemplateSsh {
            host: Some("b".into()),
            port: Some(2222.0)
        })
    );
}

#[test]
fn a_query_file_whose_name_is_empty_takes_its_file_name() {
    let path = format!("{QUERIES_DIR}/monthly_sales.sql");
    for text in [
        "---\nname:\n---\nSELECT 1\n",
        "---\nname: \"  \"\n---\nSELECT 1\n",
        "---\ndescription: none\n---\nSELECT 1\n",
    ] {
        assert_eq!(parse_query(text, &path, QUERIES_DIR).name, "monthly sales");
    }
    let dash = ".seaquel/projects/team/dashboards/team_kpis.json";
    assert_eq!(
        parse_dashboard("{\"name\": \" \", \"widgets\": []}", dash)
            .unwrap()
            .name,
        "team kpis"
    );
    // A template without a name isn't a template.
    assert!(parse_template("name:\nhost: h\n").is_none());
}

// ── Older releases' reader, ported (yaml-utils.ts, query-file-parser.ts,
// config-file-parser.ts at 5d876af), to check what they read in Core's
// files. JavaScript's regex classes are spelled out: `\w` is ASCII, `\s`
// and `trim` are JavaScript's whitespace, `.` excludes line terminators. ──

mod old {
    use seaquel_types::names::{is_js_space, js_trim};

    fn is_word(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_'
    }

    fn is_line_terminator(c: char) -> bool {
        matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
    }

    /// `parseYamlValue`.
    pub fn yaml_value(value: &str) -> String {
        if value.is_empty() {
            return String::new();
        }
        let quoted = (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''));
        if quoted {
            // `slice(1, -1)` on UTF-16; both ends are one unit here.
            return if value.len() < 2 {
                String::new()
            } else {
                value[1..value.len() - 1].to_string()
            };
        }
        value.to_string()
    }

    /// `parseYamlArray`.
    pub fn yaml_array(value: &str) -> Vec<String> {
        if value.is_empty() {
            return vec![];
        }
        if value.starts_with('[') && value.ends_with(']') {
            let inner = &value[1..value.len() - 1];
            return inner
                .split(',')
                .map(|s| yaml_value(js_trim(s)))
                .filter(|s| !s.is_empty())
                .collect();
        }
        vec![yaml_value(value)]
    }

    /// `/^(\w+):\s*(.*)$/` on one line: key and value (untrimmed).
    pub fn key_line(line: &str) -> Option<(&str, &str)> {
        let key_end = line.find(|c: char| !is_word(c)).unwrap_or(line.len());
        if key_end == 0 || !line[key_end..].starts_with(':') {
            return None;
        }
        let rest = line[key_end + 1..].trim_start_matches(is_js_space);
        if line.contains(is_line_terminator) {
            return None;
        }
        Some((&line[..key_end], rest))
    }

    /// `/^\s+-\s*(\w+):\s*(.*)$/` (`dash`) or `/^\s+(\w+):\s*(.*)$/`.
    pub fn indented(line: &str, dash: bool) -> Option<(&str, &str)> {
        let rest = line.trim_start_matches(is_js_space);
        if rest.len() == line.len() {
            return None;
        }
        let rest = if dash {
            rest.strip_prefix('-')?.trim_start_matches(is_js_space)
        } else {
            rest
        };
        key_line(rest)
    }

    #[derive(Debug, Default, PartialEq)]
    pub struct Param {
        pub name: String,
        pub ty: String,
        pub default_value: Option<String>,
        pub description: Option<String>,
    }

    #[derive(Debug, Default, PartialEq)]
    pub struct Query {
        pub name: String,
        pub description: Option<String>,
        pub database: Option<String>,
        pub tags: Vec<String>,
        pub parameters: Vec<Param>,
        pub query: String,
    }

    fn set_param(p: &mut Param, key: &str, value: &str, has_name: &mut bool) {
        let v = yaml_value(value);
        match key {
            "name" => {
                p.name = v;
                *has_name = true;
            }
            "type" => p.ty = v,
            "default" | "defaultValue" => p.default_value = Some(v),
            "description" => p.description = Some(v),
            _ => {}
        }
    }

    /// `parseQueryFile` with `parseYamlFrontmatter`.
    pub fn parse_query(content: &str, file_name: &str) -> Query {
        let mut out = Query::default();
        let fm = content.strip_prefix("---\n").and_then(|rest| {
            let at = rest.find("\n---\n")?;
            Some((&rest[..at], &rest[at + 5..]))
        });
        let Some((yaml, body)) = fm else {
            out.query = js_trim(content).to_string();
            out.name = file_name.trim_end_matches(".sql").replace(['-', '_'], " ");
            return out;
        };
        out.query = js_trim(body).to_string();
        let mut in_params = false;
        let mut current: Option<(Param, bool)> = None;
        let mut params = Vec::new();
        for line in yaml.split('\n') {
            let trimmed = js_trim(line);
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = key_line(line) {
                let value = js_trim(value);
                if key == "parameters" {
                    in_params = true;
                    continue;
                }
                in_params = false;
                match key {
                    "name" => out.name = yaml_value(value),
                    "description" => out.description = Some(yaml_value(value)),
                    "database" => out.database = Some(yaml_value(value)),
                    "tags" => out.tags = yaml_array(value),
                    _ => {}
                }
                continue;
            }
            if in_params {
                if let Some((key, value)) = indented(line, true) {
                    if let Some((p, true)) = current.take() {
                        if !p.name.is_empty() {
                            params.push(p);
                        }
                    }
                    let mut p = Param::default();
                    let mut has_name = false;
                    set_param(&mut p, key, js_trim(value), &mut has_name);
                    current = Some((p, has_name));
                    continue;
                }
                if let (Some((key, value)), Some((p, has_name))) =
                    (indented(line, false), current.as_mut())
                {
                    set_param(p, key, js_trim(value), has_name);
                }
            }
        }
        if let Some((p, true)) = current {
            if !p.name.is_empty() {
                params.push(p);
            }
        }
        for p in &mut params {
            if p.ty.is_empty() {
                p.ty = "text".into();
            }
        }
        out.parameters = params;
        out
    }

    #[derive(Debug, Default, PartialEq)]
    pub struct Template {
        pub name: String,
        pub host: String,
        pub database_name: String,
        pub labels: Vec<String>,
        pub ssh_host: Option<String>,
    }

    /// `parseConnectionFile`'s string fields.
    pub fn parse_template(content: &str) -> Option<Template> {
        let mut fields = std::collections::HashMap::new();
        let mut out = Template::default();
        let mut in_ssh = false;
        for line in content.split('\n') {
            let trimmed = js_trim(line);
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = key_line(line) {
                let value = js_trim(value);
                in_ssh = key == "sshTunnel";
                if key == "labels" {
                    out.labels = yaml_array(value);
                } else if !in_ssh {
                    fields.insert(key.to_string(), yaml_value(value));
                }
                continue;
            }
            if in_ssh {
                if let Some(("host", value)) = indented(line, false) {
                    out.ssh_host = Some(yaml_value(js_trim(value)));
                }
            }
        }
        out.name = fields.get("name").cloned().filter(|n| !n.is_empty())?;
        out.host = fields.get("host").cloned().unwrap_or("localhost".into());
        out.database_name = fields.get("databaseName").cloned().unwrap_or_default();
        Some(out)
    }

    /// `parseProjectFile`'s name and description.
    pub fn parse_project(content: &str, dir: &str) -> (String, Option<String>) {
        let mut name = dir.to_string();
        let mut description = None;
        for line in content.split('\n') {
            let trimmed = js_trim(line);
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = key_line(line) {
                let v = yaml_value(js_trim(value));
                match key {
                    "name" => name = v,
                    "description" => description = Some(v).filter(|d| !d.is_empty()),
                    _ => {}
                }
            }
        }
        (name, description)
    }
}

/// A small deterministic generator, so the property tests need no
/// dependency and fail the same way every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    /// A value made of the pieces that matter to the quoting: quotes,
    /// backslashes, `:`, `#`, brackets, commas, edge and inner spaces,
    /// tabs, newlines and Unicode.
    fn value(&mut self, newlines: bool) -> String {
        const PIECES: &[&str] = &[
            "a",
            "Sales",
            " ",
            "  ",
            ":",
            "#",
            "\"",
            "'",
            "''",
            "\\",
            "\\n",
            "\\\"",
            "[",
            "]",
            ",",
            "{",
            "}",
            "-",
            "- ",
            "Отчёт",
            "売上",
            "ß",
            "🚀",
            "e\u{301}",
            "\t",
            "x: y",
            "'q'",
            "\"q\"",
            "---",
            "null",
            "true",
            "5",
            "&",
            "*",
            "|",
            ">",
            "%",
            "@",
            "`",
            "\u{a0}",
            "\u{3000}",
        ];
        let n = 1 + self.below(6);
        let mut s: String = (0..n).map(|_| *self.pick(PIECES)).collect();
        if newlines && self.below(4) == 0 {
            let bounds: Vec<usize> = s.char_indices().map(|(i, _)| i).chain([s.len()]).collect();
            let at = bounds[self.below(bounds.len())];
            s.insert(at, '\n');
        }
        s
    }
}

fn name_like(rng: &mut Rng) -> String {
    // Names are stored trimmed and non-empty (Core's check).
    loop {
        let v = rng.value(false);
        let t = seaquel_types::names::js_trim(&v).to_string();
        if !t.is_empty() {
            return t;
        }
    }
}

fn random_query(rng: &mut Rng) -> QueryFile {
    let n_tags = rng.below(4);
    let n_params = rng.below(3);
    QueryFile {
        name: name_like(rng),
        description: (rng.below(2) == 0).then(|| name_like(rng) + &rng.value(true)),
        database: (rng.below(2) == 0).then(|| name_like(rng)),
        tags: (0..n_tags).map(|_| name_like(rng)).collect(),
        parameters: (0..n_params)
            .map(|i| PersistedQueryParameter {
                name: if rng.below(2) == 0 {
                    format!("p{i}")
                } else {
                    name_like(rng)
                },
                ty: name_like(rng),
                default_value: (rng.below(2) == 0).then(|| rng.value(false)),
                description: (rng.below(2) == 0).then(|| name_like(rng)),
            })
            .collect(),
        query: "SELECT 1".into(),
        folder: String::new(),
        // Core writes every file with its id.
        file_id: Some("00000000-0000-4000-8000-000000000001".into()),
    }
}

#[test]
fn parse_write_round_trips_every_value() {
    let mut rng = Rng(0x5eed_0e5f_2026_1001);
    let path = format!("{QUERIES_DIR}/x.sql");
    for _ in 0..20_000 {
        let q = random_query(&mut rng);
        let text = write_query(&q);
        let back = parse_query(&text, &path, QUERIES_DIR);
        assert_eq!(back.name, q.name, "{text:?}");
        assert_eq!(back.description, q.description, "{text:?}");
        assert_eq!(back.tags, q.tags, "{text:?}");
        assert_eq!(back.database, q.database, "{text:?}");
        assert_eq!(back.parameters, q.parameters, "{text:?}");
        assert_eq!(back.query, q.query);
        // The canonical form is stable.
        assert_eq!(write_query(&back), text);

        let t = TemplateFile {
            name: name_like(&mut rng),
            ty: name_like(&mut rng),
            host: rng.value(false),
            port: (1 + rng.below(65535)) as f64,
            database_name: rng.value(false),
            ssl_mode: (rng.below(2) == 0).then(|| name_like(&mut rng)),
            ssh_tunnel: (rng.below(2) == 0).then(|| TemplateSsh {
                host: Some(name_like(&mut rng)),
                port: Some(22.0),
            }),
            labels: (0..rng.below(3)).map(|_| name_like(&mut rng)).collect(),
            file_id: Some("00000000-0000-4000-8000-000000000002".into()),
        };
        let text = write_template(&t);
        let back = parse_template(&text).unwrap();
        // An empty host or database reads as the default, as today.
        if !t.host.is_empty() {
            assert_eq!(back.host, t.host, "{text:?}");
        }
        assert_eq!(back.name, t.name, "{text:?}");
        assert_eq!(back.ty, t.ty, "{text:?}");
        assert_eq!(back.port, t.port, "{text:?}");
        assert_eq!(back.ssl_mode, t.ssl_mode, "{text:?}");
        assert_eq!(back.database_name, t.database_name, "{text:?}");
        assert_eq!(back.labels, t.labels, "{text:?}");
        assert_eq!(back.ssh_tunnel, t.ssh_tunnel, "{text:?}");

        let p = ProjectFile {
            name: name_like(&mut rng),
            description: (rng.below(2) == 0).then(|| name_like(&mut rng) + &rng.value(true)),
            dir: "team".into(),
        };
        let back = parse_project(&write_project(&p), "team");
        assert_eq!((back.name, back.description), (p.name, p.description));

        let d = DashboardFile {
            name: name_like(&mut rng) + &rng.value(true),
            description: Some(rng.value(true)).filter(|d| !d.is_empty()),
            widgets: json!([{"id": rng.value(true), "n": 1.5, "big": 1e21}]).to_string(),
            viewport: Some("{\"x\":0,\"y\":0,\"zoom\":1}".into()),
            date_filter: None,
            file_id: None,
        };
        let text = write_dashboard(&d);
        let back = parse_dashboard(&text, ".seaquel/projects/team/dashboards/x.json").unwrap();
        assert_eq!(back.name, d.name);
        assert_eq!(back.description, d.description);
        assert_eq!(
            serde_json::from_str::<Value>(&back.widgets).unwrap(),
            serde_json::from_str::<Value>(&d.widgets).unwrap()
        );
    }
}

/// Values older readers can't hold as written: a value Core puts in double
/// quotes (it holds a newline, or `'` together with `"` or `\`) and that has
/// something escaped there: a `"`, or a `\` Core's reader would misread (one
/// before `"`, `\`, `n` or a newline, or at the end; today's writer already
/// escaped `"` that way); a line terminator JavaScript's `.` refuses; a list
/// item holding `,` (older readers split every comma).
fn old_readers_can_hold(v: &str, in_list: bool) -> bool {
    let needs = needs_quotes_hint(v, in_list);
    let double = needs && (v.contains('\n') || (v.contains('\'') && v.contains(['"', '\\'])));
    let escaped = double && (v.contains('"') || has_misread_backslash(v));
    let terminator = v.contains(['\r', '\u{2028}', '\u{2029}']);
    let split = in_list && v.contains(',');
    !escaped && !terminator && !split
}

/// A `\` that Core's reader would take as an escape if written as it is.
fn has_misread_backslash(v: &str) -> bool {
    let chars: Vec<char> = v.chars().collect();
    chars.iter().enumerate().any(|(i, c)| {
        *c == '\\' && matches!(chars.get(i + 1), None | Some('"' | '\\' | 'n' | '\n'))
    })
}

/// Whether Core quotes `v` at all (the quoting triggers, as the writer
/// documents them).
fn needs_quotes_hint(v: &str, in_list: bool) -> bool {
    v.contains([':', '#', '"', '[', ']', '\n', '\r'])
        || (in_list && v.contains(','))
        || v.starts_with(seaquel_types::names::is_js_space)
        || v.ends_with(seaquel_types::names::is_js_space)
        || v.starts_with('\'')
}

#[test]
fn older_readers_read_what_we_write() {
    let mut rng = Rng(0x01de_12ea_d0e5_2026);
    let mut checked = 0;
    for _ in 0..20_000 {
        let q = random_query(&mut rng);
        let text = write_query(&q);
        let old = old::parse_query(&text, "x.sql");
        if old_readers_can_hold(&q.name, false) {
            assert_eq!(old.name, q.name, "name in {text:?}");
            checked += 1;
        }
        if let Some(d) = &q.description {
            if old_readers_can_hold(d, false) {
                // The one accepted difference: a newline reads as `\n`.
                assert_eq!(
                    old.description.as_deref(),
                    Some(&*d.replace('\n', "\\n")),
                    "{text:?}"
                );
            }
        }
        if q.tags.iter().all(|t| old_readers_can_hold(t, true)) {
            assert_eq!(old.tags, q.tags, "tags in {text:?}");
        }
        for (o, p) in old.parameters.iter().zip(&q.parameters) {
            if let Some(d) = &p.default_value {
                if old_readers_can_hold(d, false) {
                    assert_eq!(o.default_value.as_deref(), Some(d.as_str()), "{text:?}");
                }
            }
        }
        assert_eq!(old.query, q.query);
        if let Some(db) = &q.database {
            if old_readers_can_hold(db, false) {
                assert_eq!(old.database.as_ref(), Some(db), "{text:?}");
            }
        }
        for (o, p) in old.parameters.iter().zip(&q.parameters) {
            if old_readers_can_hold(&p.ty, false) && old_readers_can_hold(&p.name, false) {
                assert_eq!((&o.name, &o.ty), (&p.name, &p.ty), "{text:?}");
            }
        }

        let t = TemplateFile {
            name: name_like(&mut rng),
            ty: "postgres".into(),
            host: name_like(&mut rng),
            port: 5432.0,
            database_name: name_like(&mut rng),
            ssl_mode: None,
            ssh_tunnel: Some(TemplateSsh {
                host: Some(name_like(&mut rng)),
                port: Some(22.0),
            }),
            labels: (0..rng.below(3)).map(|_| name_like(&mut rng)).collect(),
            file_id: Some("1b4e28ba-2fa1-41d2-883f-0016dc9b6b30".into()),
        };
        let text = write_template(&t);
        let old = old::parse_template(&text).expect("older readers find the name");
        for (got, want) in [
            (&old.name, &t.name),
            (&old.host, &t.host),
            (&old.database_name, &t.database_name),
        ] {
            if old_readers_can_hold(want, false) {
                assert_eq!(got, want, "{text:?}");
            }
        }
        let ssh = t.ssh_tunnel.as_ref().unwrap().host.as_ref().unwrap();
        if old_readers_can_hold(ssh, false) {
            assert_eq!(old.ssh_host.as_ref(), Some(ssh), "{text:?}");
        }
        if t.labels.iter().all(|l| old_readers_can_hold(l, true)) {
            assert_eq!(old.labels, t.labels, "{text:?}");
        }

        let p = ProjectFile {
            name: name_like(&mut rng),
            description: None,
            dir: "team".into(),
        };
        if old_readers_can_hold(&p.name, false) {
            assert_eq!(old::parse_project(&write_project(&p), "team").0, p.name);
        }
    }
    assert!(checked > 10_000, "only {checked} names were checked");
}

/// R1: nesting 100,000 levels deep (arrays, and objects inside `widgets`)
/// is refused, not followed: on a 2 MiB stack, nothing overflows, and the
/// writers and the canonical text never see more than 128 levels.
#[test]
fn deep_nesting_is_refused_on_a_small_stack() {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(|| {
            const DEPTH: usize = 100_000;
            for (open, close) in [("[", "]"), ("{\"a\":", "}")] {
                let deep = format!("{}1{}", open.repeat(DEPTH), close.repeat(DEPTH));
                let text = format!("{{\"name\":\"Deep\",\"widgets\":[{deep}]}}");
                let path = ".seaquel/projects/t/dashboards/deep.json";
                assert!(parse_dashboard(&text, path).is_none());
                let d = DashboardFile {
                    name: "Deep".into(),
                    widgets: format!("[{deep}]"),
                    viewport: Some(text.clone()),
                    date_filter: Some(deep.clone()),
                    ..Default::default()
                };
                let _ = write_dashboard(&d);
                let _ = dashboard_content(&d);
                assert!(seaquel_workspace::shared::file_hash(path, &text).is_none());
                // 128 levels still read.
                let shallow = format!("{}1{}", open.repeat(100), close.repeat(100));
                let text = format!("{{\"name\":\"Shallow\",\"widgets\":[{shallow}]}}");
                assert!(parse_dashboard(&text, path).is_some(), "{open}");
            }
            // The line readers have nothing to recurse into.
            let yaml = format!("name: [{}\n", "[".repeat(DEPTH));
            let _ = parse_template(&yaml);
            let _ = parse_query(&format!("---\n{yaml}---\nSELECT 1\n"), "x.sql", "");
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn the_parsers_never_panic() {
    let mut rng = Rng(0x0bad_1e55_c0ff_ee00);
    const BITS: &[&str] = &[
        "---\n",
        "---",
        "\n",
        "\r\n",
        "\r",
        "\u{feff}",
        "name:",
        "name: ",
        "id: ",
        "tags: [",
        "]",
        ",",
        "'",
        "\"",
        "\\",
        "parameters:\n",
        "  - name: x\n",
        "    type: y\n",
        "  ",
        "\t",
        "sshTunnel:\n",
        "  enabled: true\n",
        "  port: 9x\n",
        "port: -1e400\n",
        "{",
        "}",
        "[",
        "\"name\":",
        "\"widgets\":",
        "null",
        "1e400",
        "\u{d7ff}",
        "\u{10ffff}",
        "🚀",
        ":",
        "#",
        "labels:\n",
        "  - name: a\n",
        "    color: b\n",
        "\"\\u0000\"",
        "[1,",
        "{\"a\":",
        "\"widgets\":[null]",
        "\"widgets\":[\"ab🚀\"]",
        "\"widgets\":{}",
        "\"name\":5",
    ];
    for _ in 0..50_000 {
        let n = rng.below(12);
        let text: String = (0..n).map(|_| *rng.pick(BITS)).collect();
        let q = parse_query(&text, &format!("{QUERIES_DIR}/a/b.sql"), QUERIES_DIR);
        let _ = write_query(&q);
        let _ = query_content(&q);
        if let Some(d) = parse_dashboard(&text, ".seaquel/projects/t/dashboards/x.json") {
            let _ = write_dashboard(&d);
        }
        if let Some(t) = parse_template(&text) {
            let _ = write_template(&t);
        }
        let _ = write_project(&parse_project(&text, "d"));
        let _ = write_labels(&parse_labels(&text));
        let _ = file_stem(&text);
        let _ = path_key(&text);
    }
}

#[test]
fn a_backslash_is_escaped_only_where_the_reader_would_misread_it() {
    // Both `\` and `'` need double quotes; only the `\` before `n` (and
    // one at the end) is escaped, so older readers keep the rest as typed.
    for (value, line) in [
        ("C:\\temp: it's", "name: \"C:\\temp: it's\""),
        ("C:\\new: it's", "name: \"C:\\\\new: it's\""),
        ("it's: a\\", "name: \"it's: a\\\\\""),
        ("it's \\\"q\\\\", "name: \"it's \\\\\\\"q\\\\\\\\\""),
    ] {
        // Core writes the file with its id; its escapes are read
        // only in such a file.
        let q = QueryFile {
            name: value.into(),
            query: "SELECT 1".into(),
            file_id: Some("00000000-0000-4000-8000-000000000001".into()),
            ..Default::default()
        };
        let text = write_query(&q);
        assert!(text.contains(&format!("{line}\n")), "{value:?}: {text:?}");
        let path = format!("{QUERIES_DIR}/x.sql");
        assert_eq!(parse_query(&text, &path, QUERIES_DIR).name, value);
    }
}

#[test]
fn engine_names_types_and_ports_are_written_as_values() {
    let q = QueryFile {
        name: "Odd".into(),
        database: Some("pg:15".into()),
        parameters: vec![PersistedQueryParameter {
            name: "a: b".into(),
            ty: "text # x".into(),
            default_value: None,
            description: None,
        }],
        query: "SELECT 1".into(),
        ..Default::default()
    };
    let text = write_query(&q);
    assert!(text.contains("database: \"pg:15\"\n"), "{text}");
    assert!(text.contains("    type: \"text # x\"\n"), "{text}");
    let back = parse_query(&text, &format!("{QUERIES_DIR}/x.sql"), QUERIES_DIR);
    assert_eq!((back.database, back.parameters), (q.database, q.parameters));
    let t = TemplateFile {
        name: "W".into(),
        ty: "post: gres".into(),
        host: "h".into(),
        port: 5432.0,
        database_name: "d".into(),
        ssl_mode: Some("require # x".into()),
        ..Default::default()
    };
    let text = write_template(&t);
    assert!(
        text.contains("port: 5432\n") && text.contains("type: \"post: gres\"\n"),
        "{text}"
    );
    let back = parse_template(&text).unwrap();
    assert_eq!((back.ty, back.ssl_mode), (t.ty, t.ssl_mode));
}

#[test]
#[allow(clippy::disallowed_types, clippy::disallowed_methods)] // Instant, in a native-only test
fn a_million_keys_parse_quickly() {
    // A repeated key is looked up through an index, not a scan.
    let mut text = String::from("{\"name\":\"Big\",\"widgets\":[]");
    for i in 0..1_000_000 {
        text.push_str(&format!(",\"k{i}\":{i}"));
    }
    text.push_str(",\"k5\":\"last\"}");
    let started = std::time::Instant::now();
    let d = parse_dashboard(&text, ".seaquel/projects/t/dashboards/big.json").unwrap();
    assert_eq!(d.name, "Big");
    let mut yaml = String::from("name: W\n");
    for i in 0..200_000 {
        yaml.push_str(&format!("k{i}: {i}\nhost: h{i}\n"));
    }
    let t = parse_template(&yaml).unwrap();
    assert_eq!(t.host, "h199999");
    let took = started.elapsed();
    assert!(took.as_secs() < 30, "took {took:?}");
}

#[test]
fn numbers_parse_as_javascript_reads_them() {
    // `JSON.parse` rounds a decimal correctly; serde_json's default fast
    // path can be one ulp off. Core parses numbers itself (M7).
    let d = parse_dashboard(
        "{\"name\":\"N\",\"widgets\":[{\"x\":0.30000000000000004,\"y\":2.2250738585072011e-308,\"z\":9007199254740993,\"w\":1e400}]}",
        "x.json",
    )
    .unwrap();
    assert_eq!(
        d.widgets,
        "[{\"x\":0.30000000000000004,\"y\":2.225073858507201e-308,\"z\":9007199254740992,\"w\":null}]"
    );
}

// ── Probe fix 2: double quotes as 2026.9.2 wrote them ──

/// A file with no `id:` line was written by 2026.9.x or by hand, whose
/// writer double-quotes a value holding `\` without escaping it: the value
/// reads literally, as 2026.9.2's reader reads it.
#[test]
fn a_920_double_quoted_value_reads_literally() {
    let q = parse_query(
        "---\nname: \"C:\\new: path\"\ndescription: \"a \\\"b\\\"\"\ntags: [\"x\\n,y\"]\n---\nSELECT 1\n",
        ".seaquel/projects/team/queries/x.sql",
        ".seaquel/projects/team/queries",
    );
    assert_eq!(q.name, "C:\\new: path");
    assert_eq!(q.description.as_deref(), Some("a \\\"b\\\""));
    assert_eq!(q.tags, ["x\\n,y"]);
    let t = parse_template("name: Warehouse\nhost: \"C:\\new: host\"\ntype: postgres\n").unwrap();
    assert_eq!(t.host, "C:\\new: host");
    // `project.yaml` has no id and Core rewrites it, so it keeps Core's
    // escapes (the documented limit): a `\n` there is a newline.
    let p = parse_project("name: \"C:\\new: team\"\n", "team");
    assert_eq!(p.name, "C:\new: team");
}

/// The same value written by Core, with its id, round-trips: Core's
/// escapes are undone in a file that carries Core's id.
#[test]
fn a_core_written_value_with_an_id_round_trips() {
    for v in [
        "C:\\new: path",
        "line one\nline two",
        "it's \"q\" \\n",
        "a\\",
    ] {
        let q = QueryFile {
            name: v.to_string(),
            description: Some(v.to_string()),
            tags: vec![v.to_string(), "plain".to_string()],
            query: "SELECT 1".into(),
            file_id: Some("00000000-0000-4000-8000-000000000001".into()),
            ..QueryFile::default()
        };
        let back = parse_query(
            &write_query(&q),
            ".seaquel/projects/team/queries/x.sql",
            ".seaquel/projects/team/queries",
        );
        assert_eq!(back.name, v, "{v:?}");
        assert_eq!(back.description.as_deref(), Some(v));
        assert_eq!(back.tags, q.tags);
        let t = TemplateFile {
            name: "W".into(),
            ty: "postgres".into(),
            host: v.to_string(),
            port: 5432.0,
            database_name: v.to_string(),
            ssl_mode: None,
            ssh_tunnel: None,
            labels: vec![],
            file_id: Some("00000000-0000-4000-8000-000000000002".into()),
        };
        let back = parse_template(&write_template(&t)).unwrap();
        assert_eq!((back.host.as_str(), back.database_name.as_str()), (v, v));
    }
}

/// A file's hash and a row's agree for both kinds of file: Core's (with
/// its id, escapes undone) and 9.2's (without one, read literally).
#[test]
fn hashes_agree_for_core_and_legacy_files() {
    use seaquel_types::storage::PersistedSavedQuery;
    use seaquel_workspace::shared::plan::row_hash;
    use seaquel_workspace::shared::{file_hash, Link, RowChange};
    let rel = ".seaquel/projects/team/queries/x.sql";
    let row_of = |q: &QueryFile| PersistedSavedQuery {
        id: "q1".into(),
        name: q.name.clone(),
        query: q.query.clone(),
        project_id: "p1".into(),
        created_at: String::new(),
        updated_at: String::new(),
        parameters: None,
        starred: false,
        shared: true,
        description: q.description.clone(),
        database_type: None,
        tags: None,
        folder: None,
        shared_path: None,
    };
    let none = Link::default();
    for text in [
        "---\nname: \"C:\\new: path\"\ndescription: \"x\\ny\"\n---\nSELECT 1\n".to_string(),
        "---\nid: 00000000-0000-4000-8000-000000000001\nname: \"C:\\new: path\"\ndescription: \"x\\ny\"\n---\nSELECT 1\n".to_string(),
    ] {
        let q = parse_query(&text, rel, ".seaquel/projects/team/queries");
        let row = row_of(&q);
        let rh = row_hash(&RowChange::Query {
            row: Some(&row),
            link: &none,
            renamed: false,
        });
        assert_eq!(rh, file_hash(rel, &text).map(|(h, _)| h), "{text:?}");
        // And the file Core writes for that row hashes the same.
        let mut again = q.clone();
        again.file_id = Some("00000000-0000-4000-8000-000000000003".into());
        assert_eq!(rh, file_hash(rel, &write_query(&again)).map(|(h, _)| h));
    }
}
